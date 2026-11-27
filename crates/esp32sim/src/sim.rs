//! Simulator runtime: runs the emulated ESP32 + circuit in a dedicated thread.

use crate::debug::DebugInfo;
use devices::{Board, ProjectConfig};
use esp32_soc::flash::{build_flash, Flash};
use esp32_soc::{Machine, MachineConfig, RunState};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub enum Cmd {
    Serial(Vec<u8>),
    Input { id: String, action: Value, reply: Option<Sender<Result<(), String>>> },
    Pause,
    Resume,
    Reset,
    /// Reload project (rebuild board + reload firmware)
    Reload(Box<ProjectConfig>, Option<Sender<Result<(), String>>>),
    Speed(f64),
    Step { core: usize, count: u64, reply: Sender<Value> },
    /// Step one source line / over calls (uses debug info)
    StepLine { core: usize, over: bool, reply: Sender<Value> },
    /// Source-level breakpoints ({"file","line"} or {"address"}); kept and re-resolved
    /// whenever new firmware / debug info is loaded.  Replies with the resolution.
    SetBreakpointSpecs { specs: Vec<Value>, root: PathBuf, reply: Option<Sender<Value>> },
    ReadMem { addr: u32, len: u32, reply: Sender<Vec<u8>> },
    Regs { core: usize, reply: Sender<Value> },
    Framebuffer { id: String, reply: Sender<Option<(usize, usize, Vec<u8>)>> },
    Exec { f: Box<dyn FnOnce(&mut Machine, &mut Board) -> Value + Send>, reply: Sender<Value> },
    /// Erase the data partitions (like `pio run -t erase` for NVS / file systems) and reset.
    EraseFlashData,
    /// Run until emulated time `t` (ns), then pause and publish (lock-step scripting).
    RunUntil(u64),
    Quit,
}

#[derive(Default)]
pub struct SerialBuf {
    pub data: Vec<u8>,
    /// absolute offset of data[0]
    pub base: u64,
}

impl SerialBuf {
    pub fn push(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
        if self.data.len() > 1 << 20 {
            let cut = self.data.len() - (1 << 19);
            self.data.drain(..cut);
            self.base += cut as u64;
        }
    }
    pub fn total(&self) -> u64 {
        self.base + self.data.len() as u64
    }
    /// Bytes after absolute offset `from`.
    pub fn since(&self, from: u64) -> (&[u8], u64) {
        let start = from.saturating_sub(self.base).min(self.data.len() as u64) as usize;
        (&self.data[start..], self.total())
    }
}

pub struct Shared {
    pub status: Value,
    pub board: Value,
    pub serial: SerialBuf,
    pub messages: Vec<(u64, String)>,
    pub msg_seq: u64,
    pub version: u64,
    pub project: ProjectConfig,
    pub debug: Option<Arc<DebugInfo>>,
}

impl Shared {
    pub fn message(&mut self, m: String) {
        self.msg_seq += 1;
        self.messages.push((self.msg_seq, m));
        if self.messages.len() > 500 {
            self.messages.remove(0);
        }
    }
}

pub struct SimHandle {
    pub tx: Sender<Cmd>,
    pub shared: Arc<Mutex<Shared>>,
    pub thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SimHandle {
    /// Stop the simulator thread and wait for it (it saves flash data on the way out).
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl SimHandle {
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
    pub fn input(&self, id: &str, action: Value) -> Result<(), String> {
        let (tx, rx) = channel();
        self.send(Cmd::Input { id: id.to_string(), action, reply: Some(tx) });
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "simulator not responding".to_string())?
    }
    pub fn exec<F: FnOnce(&mut Machine, &mut Board) -> Value + Send + 'static>(&self, f: F) -> Value {
        let (tx, rx) = channel();
        self.send(Cmd::Exec { f: Box::new(f), reply: tx });
        rx.recv_timeout(Duration::from_secs(10)).unwrap_or(Value::Null)
    }
}

/// Locate firmware for a project: explicit path, or PlatformIO build output.
pub fn find_firmware(cfg: &ProjectConfig) -> Result<(PathBuf, Option<PathBuf>), String> {
    if let Some(f) = &cfg.firmware {
        let p = cfg.resolve(f);
        if !p.exists() {
            return Err(format!("firmware '{}' not found (build the project first)", p.display()));
        }
        let is_elf = |f: &std::path::Path| {
            let mut h = [0u8; 4];
            std::fs::File::open(f).and_then(|mut x| std::io::Read::read_exact(&mut x, &mut h)).is_ok() && h == *b"\x7fELF"
        };
        // symbols: explicit `elf:`, the firmware itself if it is an ELF, or a sibling
        // ELF (firmware.elf next to firmware.bin; cargo's extension-less ELF next to app.bin)
        let elf = cfg.elf.as_ref().map(|e| cfg.resolve(e)).or_else(|| {
            [p.clone(), p.with_extension("elf"), p.with_extension("")].into_iter().find(|g| g.is_file() && is_elf(g))
        });
        return Ok((p, elf));
    }
    // PlatformIO layout: <source>/.pio/build/<env>/firmware.bin
    let src = cfg.source.as_ref().map(|s| cfg.resolve(s)).unwrap_or_else(|| cfg.base_dir.clone());
    let build = src.join(".pio").join("build");
    if let Ok(rd) = std::fs::read_dir(&build) {
        let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
        for e in rd.flatten() {
            let fw = e.path().join("firmware.bin");
            if let Ok(m) = fw.metadata().and_then(|m| m.modified()) {
                if best.as_ref().map(|b| m > b.0).unwrap_or(true) {
                    best = Some((m, fw));
                }
            }
        }
        if let Some((_, fw)) = best {
            let elf = fw.with_extension("elf");
            return Ok((fw.clone(), if elf.exists() { Some(elf) } else { None }));
        }
    }
    Err("no firmware configured and no PlatformIO build output found (.pio/build/*/firmware.bin)".into())
}

fn load_flash(cfg: &ProjectConfig) -> Result<(Flash, Option<PathBuf>, PathBuf), String> {
    let (fw, elf) = find_firmware(cfg)?;
    let data = std::fs::read(&fw).map_err(|e| format!("{}: {}", fw.display(), e))?;
    let parts = cfg.partitions.as_ref().map(|p| std::fs::read(cfg.resolve(p))).transpose().map_err(|e| e.to_string())?;
    let boot = cfg.bootloader.as_ref().map(|p| std::fs::read(cfg.resolve(p))).transpose().map_err(|e| e.to_string())?;
    // PlatformIO puts partitions.bin next to firmware.bin
    let parts = parts.or_else(|| std::fs::read(fw.with_file_name("partitions.bin")).ok());
    let flash = build_flash(&data, cfg.flash_bytes(), parts.as_deref(), boot.as_deref())?;
    Ok((flash, elf, fw))
}

pub fn board_of(m: &mut Machine) -> &mut Board {
    m.sys.io.as_any().and_then(|a| a.downcast_mut::<Board>()).expect("board")
}

pub struct Sim {
    pub machine: Option<Machine>,
    pub cfg: ProjectConfig,
    shared: Arc<Mutex<Shared>>,
    rx: Receiver<Cmd>,
    running: bool,
    speed: f64,
    pace_wall: Instant,
    pace_emu: u64,
    last_publish: Instant,
    last_inst: u64,
    last_inst_t: Instant,
    mips: f64,
    breakpoints: Vec<u32>,
    debug: Option<Arc<DebugInfo>>,
    firmware_path: Option<PathBuf>,
    /// pause when emulated time reaches this (Cmd::RunUntil)
    run_limit: Option<u64>,
    /// breakpoints as the user set them (file/line or address) + project root
    bp_specs: (Vec<Value>, PathBuf),
    /// keep data partitions (NVS, file systems) in .esp32sim/ between runs
    pub persist_flash: bool,
    last_flash_save: Instant,
}

impl Sim {
    pub fn start(cfg: ProjectConfig, autostart: bool) -> SimHandle {
        Self::start_with(cfg, autostart, true)
    }

    /// `persist`: keep data partitions in the project's .esp32sim/ folder between runs.
    pub fn start_with(cfg: ProjectConfig, autostart: bool, persist: bool) -> SimHandle {
        let autostart_persist = persist;
        let (tx, rx) = channel();
        let shared = Arc::new(Mutex::new(Shared {
            status: json!({"state": "loading"}),
            board: Value::Null,
            serial: SerialBuf::default(),
            messages: Vec::new(),
            msg_seq: 0,
            version: 0,
            project: cfg.clone(),
            debug: None,
        }));
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("esp32-sim".into())
            .stack_size(64 << 20)
            .spawn(move || {
                let mut sim = Sim {
                    machine: None,
                    cfg: cfg.clone(),
                    shared: sh,
                    rx,
                    running: autostart,
                    speed: cfg.speed.unwrap_or(1.0),
                    pace_wall: Instant::now(),
                    pace_emu: 0,
                    last_publish: Instant::now(),
                    last_inst: 0,
                    last_inst_t: Instant::now(),
                    mips: 0.0,
                    breakpoints: Vec::new(),
                    debug: None,
                    firmware_path: None,
                    run_limit: None,
                    bp_specs: (Vec::new(), PathBuf::new()),
                    persist_flash: autostart_persist,
                    last_flash_save: Instant::now(),
                };
                let shared2 = sim.shared.clone();
                // an internal emulator bug must not leave the UI hanging silently
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    if let Err(e) = sim.load() {
                        if e.starts_with("no firmware configured") {
                            sim.msg("note: no firmware yet: press ⚙ Build & Run (PlatformIO) or set `firmware:` in esp32sim.yaml. The circuit can be edited meanwhile.".into());
                        } else {
                            sim.msg(format!("error: {}", e));
                        }
                        sim.running = false;
                    }
                    sim.run();
                }));
                if let Err(p) = r {
                    let what = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                    if let Ok(mut s) = shared2.lock() {
                        s.message(format!("error: internal emulator error ({}). Please report it with the firmware if possible; use Reload simulation to restart.", what));
                        s.status["state"] = json!(format!("halted: internal emulator error: {}", what));
                        s.version += 1;
                    }
                }
            })
            .expect("spawn simulator");
        SimHandle { tx, shared, thread: Some(thread) }
    }

    fn flash_store(&self) -> Option<PathBuf> {
        if !self.persist_flash || self.cfg.base_dir.as_os_str().is_empty() {
            return None;
        }
        Some(self.cfg.base_dir.join(".esp32sim").join("flash-data.bin"))
    }

    /// Save the data partitions (NVS, SPIFFS/LittleFS, FAT, coredump ...) so that
    /// they survive restarts and new builds, like on a real board.
    fn save_flash_data(&mut self) {
        let Some(path) = self.flash_store() else { return };
        let Some(m) = self.machine.as_mut() else { return };
        if !m.sys.flash.dirty {
            return;
        }
        m.sys.flash.dirty = false;
        let data = m.sys.flash.data.clone();
        let parts = m.sys.flash.partitions();
        // file: [u32 count] then per partition [offset u32, size u32] + bytes
        let mut out = Vec::new();
        let dp: Vec<_> = parts.iter().filter(|p| p.ptype == 0x01 && (p.offset + p.size) as usize <= data.len()).collect();
        out.extend((dp.len() as u32).to_le_bytes());
        for p in dp {
            out.extend(p.offset.to_le_bytes());
            out.extend(p.size.to_le_bytes());
            out.extend(&data[p.offset as usize..(p.offset + p.size) as usize]);
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&path, out);
    }

    /// Restore saved data partitions whose offset and size still match.
    fn restore_flash_data(&self, m: &mut Machine) -> usize {
        let Some(path) = self.flash_store() else { return 0 };
        let Ok(d) = std::fs::read(&path) else { return 0 };
        let parts = m.sys.flash.partitions();
        let mut n = 0;
        let rd = |o: usize| -> Option<u32> { d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) };
        let Some(count) = rd(0) else { return 0 };
        let mut pos = 4usize;
        for _ in 0..count {
            let (Some(off), Some(size)) = (rd(pos), rd(pos + 4)) else { break };
            pos += 8;
            let end = pos + size as usize;
            if end > d.len() {
                break;
            }
            if parts.iter().any(|p| p.ptype == 0x01 && p.offset == off && p.size == size) && (off + size) as usize <= m.sys.flash.data.len() {
                m.sys.flash.data[off as usize..(off + size) as usize].copy_from_slice(&d[pos..end]);
                n += 1;
            }
            pos = end;
        }
        m.sys.flash.dirty = false;
        n
    }

    /// Resolve the stored breakpoint specs against the current debug info.
    fn resolve_breakpoints(&self) -> (Value, Vec<u32>) {
        let (specs, root) = &self.bp_specs;
        let mut addrs = Vec::new();
        let mut resolved = Vec::new();
        for b in specs {
            if let Some(a) = b.get("address").and_then(|x| x.as_str()) {
                if let Ok(x) = u32::from_str_radix(a.trim_start_matches("0x"), 16) {
                    addrs.push(x);
                    resolved.push(json!({"address": a, "verified": true}));
                }
                continue;
            }
            let file = b.get("file").and_then(|x| x.as_str()).unwrap_or("");
            let line = b.get("line").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
            let abs = root.join(file).display().to_string();
            let found = self.debug.as_ref().map(|d| {
                let mut v = d.addresses(&abs, line);
                if v.is_empty() {
                    v = d.addresses(file, line);
                }
                v
            });
            match found {
                Some(a) if !a.is_empty() => {
                    resolved.push(json!({"file": file, "line": line, "verified": true, "addresses": a.iter().map(|x| format!("0x{:08x}", x)).collect::<Vec<_>>()}));
                    // every location of the line (loops, inlined copies)
                    addrs.extend(a);
                }
                _ => resolved.push(json!({"file": file, "line": line, "verified": false})),
            }
        }
        addrs.sort();
        addrs.dedup();
        (json!(resolved), addrs)
    }

    fn msg(&self, m: String) {
        self.shared.lock().unwrap().message(m);
    }

    fn load(&mut self) -> Result<(), String> {
        self.save_flash_data();
        let board = self.cfg.build_board()?;
        let (flash, elf, fw) = load_flash(&self.cfg)?;
        self.firmware_path = Some(fw.clone());
        let mut m = Machine::new(flash, MachineConfig::default());
        let restored = self.restore_flash_data(&mut m);
        if restored > 0 {
            self.msg(format!("restored {} data partition(s) (NVS / file systems) from the previous run", restored));
        }
        m.set_io(Box::new(board));
        self.debug = None;
        if let Some(e) = &elf {
            match std::fs::read(e) {
                Ok(bytes) => {
                    if let Err(err) = m.load_symbols(&bytes) {
                        self.msg(format!("warning: could not read symbols from {}: {}", e.display(), err));
                    }
                    match DebugInfo::load(&bytes) {
                        Ok(d) => self.debug = Some(Arc::new(d)),
                        Err(err) => self.msg(format!("note: no line info ({})", err)),
                    }
                }
                Err(err) => self.msg(format!("warning: {}: {}", e.display(), err)),
            }
        }
        m.boot()?;
        if !self.bp_specs.0.is_empty() {
            // line numbers map to different addresses in a new build
            let (_, addrs) = self.resolve_breakpoints();
            self.breakpoints = addrs;
        }
        for c in 0..2 {
            m.cpus[c].breakpoints = self.breakpoints.clone();
        }
        let msgs: Vec<String> = m.messages.drain(..).collect();
        for x in msgs {
            self.msg(x);
        }
        self.msg(format!("loaded {}{}", fw.display(), elf.map(|e| format!(" (symbols: {})", e.display())).unwrap_or_default()));
        {
            let mut s = self.shared.lock().unwrap();
            s.debug = self.debug.clone();
            s.project = self.cfg.clone();
        }
        self.machine = Some(m);
        self.reset_pacing();
        self.publish(true);
        Ok(())
    }

    fn reset_pacing(&mut self) {
        self.pace_wall = Instant::now();
        self.pace_emu = self.machine.as_ref().map(|m| m.now_ns()).unwrap_or(0);
    }

    fn state_str(&self) -> String {
        match &self.machine {
            None => "no firmware".into(),
            Some(m) => match &m.state {
                RunState::Running => {
                    if self.running {
                        "running".into()
                    } else {
                        "paused".into()
                    }
                }
                RunState::Paused => "paused".into(),
                RunState::Breakpoint(c, pc) => format!("breakpoint core {} at 0x{:08x}", c, pc),
                RunState::Halted(e) => format!("halted: {}", e),
            },
        }
    }

    fn publish(&mut self, force: bool) {
        if !force && self.last_publish.elapsed() < Duration::from_millis(40) {
            return;
        }
        self.last_publish = Instant::now();
        let state = self.state_str();
        let Some(m) = self.machine.as_mut() else {
            let mut s = self.shared.lock().unwrap();
            s.status = json!({"state": state});
            s.version += 1;
            return;
        };
        let dt = self.last_inst_t.elapsed().as_secs_f64();
        if dt > 0.5 {
            self.mips = (m.total_instructions - self.last_inst) as f64 / dt / 1e6;
            self.last_inst = m.total_instructions;
            self.last_inst_t = Instant::now();
        }
        let msgs: Vec<String> = m.messages.drain(..).collect();
        let time_ns = m.now_ns();
        let pcs = [m.cpus[0].pc, m.cpus[1].pc];
        let cpu_mhz = m.sys.cpu_hz / 1_000_000;
        let (insts, resets) = (m.total_instructions, m.resets);
        let bp = match &m.state {
            RunState::Breakpoint(c, pc) => Some((*c, *pc)),
            _ => None,
        };
        let flash_dirty = m.sys.flash.dirty;
        let board = board_of(m);
        let out: Vec<u8> = board.console_out.drain(..).collect();
        let events: Vec<String> = board.events.drain(..).map(|e| format!("[{}] {}", e.source, e.text)).collect();
        let snap = board.snapshot();
        let mut loc = Value::Null;
        if let (Some((c, pc)), Some(d)) = (bp, &self.debug) {
            if let Some(l) = d.location(pc) {
                loc = json!({"core": c, "pc": pc, "file": l.file, "line": l.line, "function": l.function});
            }
        }
        let mut s = self.shared.lock().unwrap();
        for x in msgs.into_iter().chain(events) {
            s.message(x);
        }
        s.serial.push(&out);
        s.board = snap;
        s.status = json!({
            "state": state, "running": self.running, "time_ns": time_ns, "instructions": insts,
            "mips": self.mips, "cpu_mhz": cpu_mhz, "speed": self.speed, "resets": resets,
            "pc": [format!("0x{:08x}", pcs[0]), format!("0x{:08x}", pcs[1])],
            "location": loc, "flash_dirty": flash_dirty,
            "firmware": self.firmware_path.as_ref().map(|p| p.display().to_string()),
        });
        s.version += 1;
    }

    fn handle(&mut self, c: Cmd) -> bool {
        match c {
            Cmd::Quit => {
                self.save_flash_data();
                return false;
            }
            Cmd::Serial(bytes) => {
                if let Some(m) = self.machine.as_mut() {
                    board_of(m).console_in.extend(bytes);
                }
            }
            Cmd::Input { id, action, reply } => {
                let r = match self.machine.as_mut() {
                    Some(m) => {
                        let now = m.now_ns();
                        let b = board_of(m);
                        b.now = b.now.max(now);
                        b.input(&id, &action)
                    }
                    None => Err("simulation not loaded".into()),
                };
                if let Some(tx) = reply {
                    let _ = tx.send(r);
                }
                self.publish(true);
            }
            Cmd::Pause => {
                self.running = false;
                self.publish(true);
            }
            Cmd::RunUntil(t) => {
                self.run_limit = Some(t);
                if let Some(m) = self.machine.as_mut() {
                    if m.state == RunState::Paused {
                        m.state = RunState::Running;
                    }
                    if m.now_ns() >= t {
                        self.running = false;
                        self.publish(true);
                        return true;
                    }
                }
                self.running = true;
            }
            Cmd::Resume => {
                if let Some(m) = self.machine.as_mut() {
                    if let RunState::Breakpoint(..) = m.state {
                        m.state = RunState::Running;
                    }
                    if m.state == RunState::Paused {
                        m.state = RunState::Running;
                    }
                }
                self.running = true;
                self.reset_pacing();
            }
            Cmd::EraseFlashData => {
                if let Some(p) = self.flash_store() {
                    let _ = std::fs::remove_file(p);
                }
                if let Some(m) = self.machine.as_mut() {
                    for p in m.sys.flash.partitions() {
                        if p.ptype == 0x01 && (p.offset + p.size) as usize <= m.sys.flash.data.len() {
                            m.sys.flash.data[p.offset as usize..(p.offset + p.size) as usize].fill(0xFF);
                        }
                    }
                    m.sys.flash.dirty = false;
                    m.state = RunState::Running;
                    let _ = m.reset(1);
                }
                self.msg("flash data partitions erased (NVS, file systems); the board was reset".into());
                self.running = true;
                self.publish(true);
            }
            Cmd::Reset => {
                // EN button: the chip restarts, flash contents and the circuit stay
                let r = match self.machine.as_mut() {
                    Some(m) => {
                        m.state = RunState::Running;
                        m.reset(1)
                    }
                    None => self.load(),
                };
                if let Err(e) = r {
                    self.msg(format!("error: {}", e));
                }
                self.running = true;
                self.publish(true);
            }
            Cmd::Reload(cfg, reply) => {
                self.cfg = *cfg;
                self.speed = self.cfg.speed.unwrap_or(self.speed);
                let r = self.load();
                if let Err(e) = &r {
                    self.msg(format!("error: {}", e));
                    self.machine = None;
                    self.publish(true);
                } else if self.run_limit.is_none() {
                    // a reloaded firmware boots and runs, like after Reset (even if
                    // the previous one was stopped at a breakpoint); in lock-step
                    // mode (`esp32sim test`) RunUntil decides
                    self.running = true;
                    self.reset_pacing();
                    self.publish(true);
                }
                if let Some(tx) = reply {
                    let _ = tx.send(r);
                }
            }
            Cmd::Speed(s) => {
                self.speed = s.max(0.0);
                self.reset_pacing();
            }
            Cmd::Step { core, count, reply } => {
                let v = match self.machine.as_mut() {
                    Some(m) => {
                        self.running = false;
                        crate::debug::step_instructions(m, core, count);
                        self.publish(true);
                        crate::debug::regs_json(self.machine.as_mut().unwrap(), core, self.debug.as_deref())
                    }
                    None => Value::Null,
                };
                let _ = reply.send(v);
            }
            Cmd::StepLine { core, over, reply } => {
                let v = match (self.machine.as_mut(), self.debug.clone()) {
                    (Some(m), Some(d)) => {
                        self.running = false;
                        crate::debug::step_line(m, core, over, &d);
                        self.publish(true);
                        crate::debug::regs_json(self.machine.as_mut().unwrap(), core, Some(&d))
                    }
                    (Some(m), None) => {
                        crate::debug::step_instructions(m, core, 1);
                        crate::debug::regs_json(m, core, None)
                    }
                    _ => Value::Null,
                };
                let _ = reply.send(v);
            }
            Cmd::SetBreakpointSpecs { specs, root, reply } => {
                self.bp_specs = (specs, root);
                let (resolved, addrs) = self.resolve_breakpoints();
                self.breakpoints = addrs.clone();
                if let Some(m) = self.machine.as_mut() {
                    for c in 0..2 {
                        m.cpus[c].breakpoints = addrs.clone();
                    }
                }
                if let Some(r) = reply {
                    let _ = r.send(resolved);
                }
            }
            Cmd::ReadMem { addr, len, reply } => {
                let v = match self.machine.as_mut() {
                    Some(m) => m.sys.read_bytes(addr, len.min(65536) as usize),
                    None => vec![],
                };
                let _ = reply.send(v);
            }
            Cmd::Regs { core, reply } => {
                let v = match self.machine.as_mut() {
                    Some(m) => crate::debug::regs_json(m, core, self.debug.as_deref()),
                    None => Value::Null,
                };
                let _ = reply.send(v);
            }
            Cmd::Framebuffer { id, reply } => {
                let v = self.machine.as_mut().and_then(|m| {
                    let b = board_of(m);
                    b.find(&id).and_then(|i| b.comps[i].dev.framebuffer())
                });
                let _ = reply.send(v);
            }
            Cmd::Exec { f, reply } => {
                let v = match self.machine.as_mut() {
                    Some(m) => {
                        let mp: *mut Machine = m;
                        // SAFETY: the board lives inside the machine's io box; the closure gets
                        // both, but must not replace sys.io.
                        let b = board_of(unsafe { &mut *mp });
                        f(unsafe { &mut *mp }, b)
                    }
                    None => Value::Null,
                };
                let _ = reply.send(v);
            }
        }
        true
    }

    fn run(&mut self) {
        loop {
            // commands
            loop {
                let c = if self.running && self.machine.as_ref().map(|m| m.state == RunState::Running).unwrap_or(false) {
                    match self.rx.try_recv() {
                        Ok(c) => c,
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(_) => return,
                    }
                } else {
                    match self.rx.recv_timeout(Duration::from_millis(30)) {
                        Ok(c) => c,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            self.publish(false);
                            break;
                        }
                        Err(_) => return,
                    }
                };
                if !self.handle(c) {
                    return;
                }
            }
            let Some(m) = self.machine.as_mut() else { continue };
            if !self.running || m.state != RunState::Running {
                continue;
            }
            // run a slice of emulated time
            let mut slice = 2_000_000u64; // 2 ms
            if let Some(lim) = self.run_limit {
                slice = slice.min(lim.saturating_sub(m.now_ns()));
                if slice == 0 {
                    self.running = false;
                    self.publish(true);
                    continue;
                }
            }
            let wall0 = Instant::now();
            let st = m.run_for(slice);
            if st != RunState::Running {
                self.running = false;
                let msg = match &st {
                    RunState::Breakpoint(c, pc) => format!("breakpoint hit on core {} at {}", c, m.describe(*pc)),
                    RunState::Halted(e) => format!("emulation halted: {}", e),
                    _ => String::new(),
                };
                if !msg.is_empty() {
                    self.msg(msg);
                }
                self.publish(true);
                continue;
            }
            let emu_now = m.now_ns();
            self.publish(false);
            if self.last_flash_save.elapsed() > Duration::from_secs(5) {
                self.last_flash_save = Instant::now();
                self.save_flash_data();
            }
            // real-time pacing
            if self.speed > 0.0 {
                let emu = emu_now.saturating_sub(self.pace_emu) as f64 / self.speed;
                let wall = self.pace_wall.elapsed().as_nanos() as f64;
                if emu > wall + 1_000_000.0 {
                    let d = ((emu - wall) as u64).min(20_000_000);
                    std::thread::sleep(Duration::from_nanos(d));
                } else if wall - emu > 200_000_000.0 {
                    // too slow to keep up: don't accumulate debt
                    self.reset_pacing();
                }
            } else if wall0.elapsed() > Duration::from_millis(50) {
                std::thread::yield_now();
            }
        }
    }
}

//! The complete ESP32 machine: two Xtensa cores + system bus + boot logic + scheduler.

use crate::elf::Symbols;
use crate::flash::{parse_app_image, AppImage, Flash};
use crate::io::IoBridge;
use crate::rom::{self, HleOutcome, RomTable};
use crate::sys::Sys;
use xtensa_core::cpu::{PS, PS_CALLINC_SHIFT, PS_WOE, VECBASE, WINDOWBASE, WINDOWSTART};
use xtensa_core::{Cpu, StepResult};

/// Light / deep sleep in progress (RTC_CNTL_STATE0.SLEEP_EN was set).
#[derive(Debug, Clone)]
pub struct SleepState {
    pub deep: bool,
    /// wake-up time (ns) when the RTC timer wake-up is enabled
    pub wake_at: Option<u64>,
    /// EXT0: (gpio, wake level)
    pub ext0: Option<(u8, bool)>,
    /// EXT1: (gpios, wake when any is high (true) / all low (false))
    pub ext1: Option<(Vec<u8>, bool)>,
    pub since: u64,
}

/// Trivial hasher for PC-keyed maps (keys are already well distributed).
#[derive(Default)]
pub struct PcHasher(u64);
impl std::hash::Hasher for PcHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 << 8) | *b as u64;
        }
    }
    fn write_u32(&mut self, v: u32) {
        self.0 = (v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// Instructions per core per scheduling quantum.
pub const QUANTUM: u64 = 1024;
/// Slice length while the cores are interleaved finely (cross-core handshakes).
const FINE_SLICE: u64 = 24;
/// How long fine interleaving lasts after a handshake was detected.
const FINE_SCHED_NS: u64 = 50_000;

/// Magic return address used as the "caller" of the app entry points.
const EXIT_ADDR: u32 = 0x4000_0010;
pub use crate::hle::{TRAMP_ADDR, TRAMP_RET};

#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    Running,
    /// Stopped by the user / debugger
    Paused,
    /// Breakpoint hit on core n at pc
    Breakpoint(usize, u32),
    /// Emulation cannot continue
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct MachineConfig {
    pub mac: [u8; 6],
    pub trace: bool,
    pub log_unknown_regs: bool,
}

impl Default for MachineConfig {
    fn default() -> Self {
        MachineConfig { mac: [0x24, 0x0A, 0xC4, 0x12, 0x34, 0x56], trace: false, log_unknown_regs: false }
    }
}

/// A ROM function (coroutine) waiting for a firmware callback to return.
struct PendingHle {
    core: usize,
    sp: u32,
    callinc: u32,
    /// original return address register a(4*callinc) (clobbered by trampoline calls)
    ra: u32,
    co: crate::hle::HleCoroutine,
}

pub struct Machine {
    pub cpus: [Cpu; 2],
    pub sys: Sys,
    pub rom: RomTable,
    pub symbols: Symbols,
    pub app: Option<AppImage>,
    pub state: RunState,
    pub config: MachineConfig,
    busy_until: [u64; 2],
    appcpu_started: bool,
    pub total_instructions: u64,
    pub resets: u32,
    /// ROM HLE call counts keyed by entry address (see `hle_call_counts`).
    pub hle_counts: std::collections::HashMap<u32, u64, std::hash::BuildHasherDefault<PcHasher>>,
    /// Messages for the user (boot info, warnings, crashes)
    pub messages: Vec<String>,
    last_exc_report: [u64; 2],
    pending: Vec<PendingHle>,
    /// Stop (as if at a breakpoint) when a CPU exception is raised.
    pub break_on_exception: bool,
    /// Fine-grained core interleaving is active until this time (ns).
    fine_until: u64,
    /// Cross-core handshakes detected (each switches to fine interleaving briefly).
    pub handshakes: u64,
    radio_reported: bool,
    /// Sleep in progress, if any.
    pub sleeping: Option<SleepState>,
    radio_first: Option<u64>,
}

impl Machine {
    pub fn new(flash: Flash, config: MachineConfig) -> Machine {
        let mut sys = Sys::new(flash, config.mac);
        sys.log_unknown = config.log_unknown_regs;
        let rom = RomTable::load();
        let mut symbols = Symbols::default();
        for (a, n) in rom.by_addr.iter() {
            if (0x4000_0000..0x4007_0000).contains(a) {
                symbols.add(*a, &format!("rom:{}", n), 0);
            }
        }
        let mut m = Machine {
            cpus: [Cpu::new(0), Cpu::new(1)],
            sys,
            rom,
            symbols,
            app: None,
            state: RunState::Halted("not booted".into()),
            config,
            busy_until: [0; 2],
            appcpu_started: false,
            total_instructions: 0,
            resets: 0,
            hle_counts: Default::default(),
            break_on_exception: false,
            fine_until: 0,
            handshakes: 0,
            radio_reported: false,
            sleeping: None,
            radio_first: None,
            messages: Vec::new(),
            last_exc_report: [0; 2],
            pending: Vec::new(),
        };
        m.cpus[0].trace_enabled = m.config.trace;
        m.cpus[1].trace_enabled = m.config.trace;
        m
    }

    pub fn set_io(&mut self, io: Box<dyn IoBridge>) {
        self.sys.io = io;
    }

    /// Attach symbols from the firmware ELF (for backtraces / debugging).
    pub fn load_symbols(&mut self, elf: &[u8]) -> Result<(), String> {
        let s = Symbols::from_elf(elf)?;
        for (a, (n, sz)) in s.funcs {
            self.symbols.funcs.insert(a, (n, sz));
        }
        for (n, a) in s.by_name {
            self.symbols.by_name.insert(n, a);
        }
        Ok(())
    }

    /// Perform what the ROM + 2nd stage bootloader do: select the app, map flash, load RAM segments.
    pub fn boot(&mut self) -> Result<(), String> {
        let part = self.sys.flash.boot_partition().ok_or("no bootable app partition found in flash")?;
        let img = parse_app_image(&self.sys.flash, part.offset)?;
        // MMU mappings for flash segments
        for c in 0..2 {
            for e in self.sys.dport.mmu[c].iter_mut() {
                *e = 0x100;
            }
        }
        for seg in &img.segments {
            let la = seg.load_addr;
            let is_drom = (0x3F40_0000..0x3F80_0000).contains(&la);
            let is_irom = (0x400C_2000..0x40C0_0000).contains(&la);
            if is_drom || is_irom {
                let vstart = la & !0xFFFF;
                let pstart = seg.flash_off & !0xFFFF;
                let pages = ((la & 0xFFFF) + seg.len).div_ceil(0x10000);
                for i in 0..pages {
                    let v = vstart + i * 0x10000;
                    let idx = if is_drom { (v - 0x3F40_0000) >> 16 } else { 64 + ((v - 0x4000_0000) >> 16) };
                    for c in 0..2 {
                        self.sys.dport.mmu[c][idx as usize] = (pstart >> 16) + i;
                    }
                }
            } else if la == 0 {
                continue; // padding
            } else {
                let mut buf = vec![0u8; seg.len as usize];
                self.sys.flash.read(seg.flash_off, &mut buf);
                if let Err(e) = self.sys.load(la, &buf) {
                    self.messages.push(format!("warning: segment at 0x{:08x} ({} bytes) not loaded: {}", la, seg.len, e));
                }
            }
        }
        self.sys.dport.mmu_dirty = true;
        // caches enabled as left by the bootloader
        let v = self.sys.dport.regs.read(0x40) | (1 << 3);
        self.sys.dport.regs.write(0x40, v);
        self.sys.dport.regs.write(0x44, 0);
        self.sys.dport.regs.write(0x5C, 0);
        // ROM data the app may rely on
        // g_rom_flashchip (esp_rom_spiflash_chip_t) as configured by the bootloader
        let id = self.sys.chip.jedec_id;
        let dev_id = ((id >> 16) & 0xFF) | (id & 0xFF00) | ((id & 0xFF) << 16);
        let fsize = self.sys.flash.size();
        for (i, v) in [dev_id, fsize, 0x10000, 0x1000, 0x100, 0xFFFF].iter().enumerate() {
            self.sys.poke32(0x3FFA_E270 + 4 * i as u32, *v);
        }
        // UART0 as the ROM's Uart_Init left it: RX timeout interrupt enabled
        // (console drivers of older IDF versions rely on it and never set it)
        self.sys.uart[0].regs.write(crate::periph::uart::CONF1, (1 << 31) | (2 << 24) | 0x6060);
        self.sys.poke32(0x3FFE_01E0, 80); // g_ticks_per_us_pro
        self.sys.poke32(0x3FFE_40F0, 80); // g_ticks_per_us_app
        // guest trampoline + call stub for ROM functions calling back into firmware
        let (tramp, stub) = crate::hle::trampoline_code();
        let _ = self.sys.load(TRAMP_ADDR, &tramp);
        let _ = self.sys.load(crate::hle::STUB_ADDR, &stub);
        let _ = self.sys.load(crate::hle::SPILL_ADDR, &crate::hle::spill_code());
        self.pending.clear();
        crate::hle::FILES.lock().unwrap().clear();
        // PRO CPU initial state: as if called via CALL4 from the ROM
        let entry = img.entry;
        Self::prepare_core(&mut self.cpus[0], entry, 0x3FFE_3F20);
        self.cpus[0].stalled = false;
        self.cpus[1].reset();
        self.cpus[1].stalled = true;
        self.appcpu_started = false;
        self.busy_until = [0; 2];
        self.sys.cpu_hz = self.sys.compute_cpu_hz();
        self.messages.push(format!(
            "boot: app '{}' {} (IDF {}) from partition '{}' @0x{:x}, entry 0x{:08x}",
            img.project_name, img.version, img.idf_version, part.label, part.offset, entry
        ));
        self.app = Some(img);
        self.state = RunState::Running;
        Ok(())
    }

    fn prepare_core(cpu: &mut Cpu, entry: u32, sp: u32) {
        cpu.reset();
        cpu.sr[WINDOWBASE] = 0;
        cpu.sr[WINDOWSTART] = 1;
        cpu.sr[PS] = PS_WOE | (1 << PS_CALLINC_SHIFT);
        cpu.sr[VECBASE] = 0x4000_0000;
        cpu.set_ar(1, sp);
        cpu.set_ar(4, (1 << 30) | (EXIT_ADDR & 0x3FFF_FFFF));
        cpu.set_ar(5, sp);
        cpu.pc = entry;
        cpu.stalled = false;
        cpu.waiting = false;
    }

    /// Full system reset (software reset): reload RAM from flash and restart.
    pub fn reset(&mut self, reason: u32) -> Result<(), String> {
        self.resets += 1;
        let io = std::mem::replace(&mut self.sys.io, Box::new(crate::io::NullIo::default()));
        self.sys.reset_peripherals();
        self.sys.io = io;
        self.sys.rtc.reset_reason = reason;
        self.messages.push(format!("reset (reason {})", reason));
        self.boot()
    }

    pub fn now_ns(&self) -> u64 {
        self.sys.now
    }

    fn update_irq_lines(&mut self) {
        if self.sys.irq.dirty {
            self.sys.irq.dirty = false;
            let l0 = self.sys.irq.lines(0);
            let l1 = self.sys.irq.lines(1);
            self.cpus[0].set_irq_lines(l0);
            self.cpus[1].set_irq_lines(l1);
        }
    }

    fn check_appcpu(&mut self) {
        if !self.appcpu_started && self.sys.dport.appcpu_running() && self.sys.dport.appcpu_boot_addr != 0 {
            let addr = self.sys.dport.appcpu_boot_addr;
            Self::prepare_core(&mut self.cpus[1], addr, 0x3FFE_7E30);
            self.cpus[1].cycles = self.cpus[0].cycles;
            let cc = self.cpus[0].ccount();
            self.cpus[1].write_sr(234, cc);
            self.appcpu_started = true;
            self.messages.push(format!("APP CPU started at 0x{:08x} ({})", addr, self.symbols.describe(addr)));
        }
        if self.appcpu_started {
            let stalled = self.sys.rtc.cpu_stalled(1) || self.sys.dport.regs.read(crate::periph::dport::APPCPU_CTRL_C) & 1 != 0;
            self.cpus[1].stalled = stalled;
        }
    }

    /// Run one core for up to `n` instructions. Returns false if the machine stopped.
    /// SLEEP_EN was set: stop both CPUs until a wake-up source fires.
    fn enter_sleep(&mut self) {
        use crate::periph::rtc::*;
        let r = &self.sys.rtc.regs;
        let ena = (r.read(WAKEUP_STATE) >> 11) & 0x7FF;
        let deep = r.read(DIG_PWC) & (1 << 31) != 0;
        let mut wake_at = None;
        if ena & (1 << 3) != 0 {
            let ticks = ((r.read(SLP_TIMER1) as u64 & 0xFFFF) << 32) | r.read(SLP_TIMER0) as u64;
            let ns = (ticks as u128 * 1_000_000_000u128 / self.sys.rtc.slow_hz as u128) as u64;
            wake_at = Some(ns.max(self.sys.now));
        }
        let conf = r.read(EXT_WAKEUP_CONF);
        let ext0 = if ena & 1 != 0 {
            let sel = (self.sys.gpio.rtc.read(0xBC) >> 27) as usize;
            RTC_GPIO.get(sel).map(|&g| (g, conf & (1 << 30) != 0))
        } else {
            None
        };
        let ext1 = if ena & 2 != 0 {
            let mask = r.read(EXT_WAKEUP1) & 0x3FFFF;
            let pins: Vec<u8> = (0..18).filter(|i| mask & (1 << i) != 0).map(|i| RTC_GPIO[i]).collect();
            Some((pins, conf & (1 << 31) != 0))
        } else {
            None
        };
        if wake_at.is_none() && ext0.is_none() && ext1.is_none() && ena & !0xB != 0 {
            self.messages.push(format!("note: sleep wake-up sources 0x{:x} are not emulated (timer, EXT0 and EXT1 are)", ena));
        }
        let what = if deep { "deep sleep" } else { "light sleep" };
        let until = match wake_at {
            Some(w) => format!(" for {:.3} s", (w - self.sys.now) as f64 / 1e9),
            None => String::new(),
        };
        self.messages.push(format!("{}{}{}", what, until, if ext0.is_some() || ext1.is_some() { " (or until the EXT wake-up pin)" } else { "" }));
        for c in 0..2 {
            self.cpus[c].stalled = true;
        }
        self.sleeping = Some(SleepState { deep, wake_at, ext0, ext1, since: self.sys.now });
    }

    fn check_wake(&mut self) {
        use crate::periph::rtc::*;
        let Some(s) = self.sleeping.clone() else { return };
        let now = self.sys.now;
        let mut cause = 0u32;
        let mut ext1_status = 0u32;
        if s.wake_at.map(|w| now >= w).unwrap_or(false) {
            cause |= 1 << 3;
        }
        if let Some((g, lv)) = s.ext0 {
            if self.sys.io.gpio_read(now, g) == Some(lv) {
                cause |= 1;
            }
        }
        if let Some((pins, any_high)) = &s.ext1 {
            let levels: Vec<bool> = pins.iter().map(|&g| self.sys.io.gpio_read(now, g).unwrap_or(false)).collect();
            let hit = if *any_high { levels.iter().any(|&l| l) } else { levels.iter().all(|&l| !l) };
            if hit {
                cause |= 2;
                for (i, &g) in pins.iter().enumerate() {
                    if levels[i] == *any_high {
                        if let Some(n) = RTC_GPIO.iter().position(|&x| x == g) {
                            ext1_status |= 1 << n;
                        }
                    }
                }
            }
        }
        if cause == 0 {
            return;
        }
        self.sleeping = None;
        if s.deep {
            // the chip resets; RTC memory survives, the wake-up cause is latched
            self.messages.push(format!("woke from deep sleep after {:.3} s", (now - s.since) as f64 / 1e9));
            let _ = self.reset(5);
            let ws = self.sys.rtc.regs.read(WAKEUP_STATE);
            self.sys.rtc.regs.write(WAKEUP_STATE, (ws & !0x7FF) | cause);
            self.sys.rtc.regs.write(EXT_WAKEUP1_STATUS, ext1_status);
            let rs = self.sys.rtc.regs.read(RESET_STATE);
            self.sys.rtc.regs.write(RESET_STATE, (rs & !0xFFF) | 5 | (5 << 6));
        } else {
            let r = &mut self.sys.rtc.regs;
            let ws = r.read(WAKEUP_STATE);
            r.write(WAKEUP_STATE, (ws & !0x7FF) | cause);
            r.write(EXT_WAKEUP1_STATUS, ext1_status);
            let st = r.read(STATE0);
            r.write(STATE0, st & !(1 << 31));
            let raw = r.read(INT_RAW);
            r.write(INT_RAW, raw | 1);
            for c in 0..2 {
                self.cpus[c].stalled = false;
            }
        }
    }

    /// ROM HLE call counts by function name, most frequent first.
    pub fn hle_call_counts(&self) -> Vec<(String, u64)> {
        let mut by: std::collections::HashMap<String, u64> = Default::default();
        for (pc, n) in &self.hle_counts {
            *by.entry(self.rom.name(*pc).unwrap_or("?").to_string()).or_insert(0) += n;
        }
        let mut v: Vec<_> = by.into_iter().collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        v
    }

    fn run_core(&mut self, c: usize, n: u64) -> bool {
        let q0 = self.cpus[c].cycles;
        self.run_core_to(c, q0 + n, q0)
    }

    /// Run core `c` until its cycle counter reaches `target`.  `q0` is its cycle
    /// count at the start of the current quantum (for sub-quantum timestamps).
    /// Returns early (true) when the core wrote a word the other core is polling.
    fn run_core_to(&mut self, c: usize, target: u64, q0: u64) -> bool {
        self.sys.cur_core = c;
        self.sys.q_cyc0 = q0;
        if self.cpus[c].stalled {
            // a stalled CPU's clock still runs
            let d = target - self.cpus[c].cycles;
            self.cpus[c].advance_cycles(d as u32);
            return true;
        }
        if self.busy_until[c] > self.sys.now {
            let d = target - self.cpus[c].cycles;
            self.cpus[c].advance_cycles(d as u32);
            return true;
        }
        self.sys.yield_core = false;
        while self.cpus[c].cycles < target {
            let r = self.cpus[c].run_until(&mut self.sys, target);
            if self.sys.yield_core {
                // This core just wrote the word the other core is polling (a cross-core
                // handshake).  Interleave the cores finely for a while so that short
                // pulses on such flags are seen, as they would be on real hardware.
                self.sys.yield_core = false;
                if self.appcpu_started && r == StepResult::Ok {
                    self.fine_until = self.sys.now + FINE_SCHED_NS;
                    self.handshakes += 1;
                    return true;
                }
            }
            match r {
                StepResult::Ok => {}
                StepResult::Waiting => {
                    let d = target - self.cpus[c].cycles;
                    self.cpus[c].advance_cycles(d as u32);
                    break;
                }
                StepResult::Hle if crate::hle::is_tramp_ret(self.cpus[c].pc) => {
                    self.continue_hle(c);
                    if self.state != RunState::Running {
                        return false;
                    }
                }
                StepResult::Hle => {
                    if self.cpus[c].pc == EXIT_ADDR {
                        self.state = RunState::Halted(format!("core {} returned from its entry function", c));
                        return false;
                    }
                    *self.hle_counts.entry(self.cpus[c].pc).or_insert(0) += 1;
                    let out = rom::call(&self.rom, &mut self.cpus[c], &mut self.sys);
                    self.cpus[c].advance_cycles(4);
                    match out {
                        HleOutcome::Done => {}
                        HleOutcome::Delay(ns) => {
                            let now = self.sys.precise_now(self.cpus[c].cycles);
                            self.busy_until[c] = now + ns;
                            // consume the rest of this quantum
                            let d = target.saturating_sub(self.cpus[c].cycles);
                            self.cpus[c].advance_cycles(d as u32);
                            break;
                        }
                        HleOutcome::Coroutine(name) => {
                            let callinc = (self.cpus[c].sr[PS] >> PS_CALLINC_SHIFT) & 3;
                            let sp = self.cpus[c].ar(1);
                            let ra = self.cpus[c].ar((callinc * 4) as u8);
                            let co = crate::hle::start(&name, &mut self.cpus[c], &mut self.sys);
                            self.drive_hle(PendingHle { core: c, sp, callinc, ra, co }, 0);
                        }
                        HleOutcome::Reset => {
                            let _ = self.reset(0x0C);
                            return false;
                        }
                        HleOutcome::Unsupported(msg) => {
                            let bt = self.backtrace(c);
                            self.state = RunState::Halted(format!("{} (core {}, called from {})", msg, c, bt.get(0).cloned().unwrap_or_default()));
                            self.messages.push(format!("backtrace: {}", bt.join(" <- ")));
                            return false;
                        }
                    }
                }
                StepResult::Breakpoint => {
                    self.state = RunState::Breakpoint(c, self.cpus[c].pc);
                    return false;
                }
                StepResult::DebugBreak => {
                    self.state = RunState::Breakpoint(c, self.cpus[c].pc);
                    return false;
                }
                StepResult::Stalled => break,
            }
            if self.sys.irq.dirty {
                self.update_irq_lines();
            }
        }
        true
    }

    /// Resume a ROM coroutine: either it requests a firmware call or it finishes.
    fn drive_hle(&mut self, mut p: PendingHle, result: u32) {
        use corosensei::CoroutineResult;
        let c = p.core;
        let input = crate::hle::ResumeIn { result, cpu: &mut self.cpus[c] as *mut Cpu, sys: &mut self.sys as *mut Sys };
        match p.co.resume(input) {
            CoroutineResult::Yield(call) => {
                let block = crate::hle::ARGBLOCK[c];
                self.sys.poke32(block, call.func);
                for i in 0..6 {
                    let v = call.args.get(i).copied().unwrap_or(0);
                    self.sys.poke32(block + 4 + 4 * i as u32, v);
                }
                let n = p.callinc.clamp(1, 3);
                let cpu = &mut self.cpus[c];
                cpu.set_ar((4 * n + 3) as u8, crate::hle::STUB_ADDR);
                cpu.set_ar((4 * n + 2) as u8, block);
                cpu.pc = TRAMP_ADDR + 8 * (n - 1);
                self.pending.push(p);
            }
            CoroutineResult::Return(r) => {
                let cpu = &mut self.cpus[c];
                let ps = cpu.sr[PS];
                cpu.sr[PS] = (ps & !(3 << PS_CALLINC_SHIFT)) | (p.callinc << PS_CALLINC_SHIFT);
                cpu.set_ar((p.callinc * 4) as u8, p.ra);
                cpu.hle_return(r.ret, r.ret_hi);
            }
        }
    }

    /// A firmware callback returned to the trampoline: resume its ROM coroutine.
    fn continue_hle(&mut self, c: usize) {
        let sp = self.cpus[c].ar(1);
        let n = (self.cpus[c].pc - TRAMP_RET) / 8 + 1;
        let result = self.cpus[c].ar((4 * n + 2) as u8);
        match self.pending.iter().rposition(|p| p.core == c && p.sp == sp) {
            Some(i) => {
                let p = self.pending.remove(i);
                self.drive_hle(p, result);
            }
            None => {
                let list: Vec<String> = self.pending.iter().map(|p| format!("core{} sp={:08x}", p.core, p.sp)).collect();
                self.state = RunState::Halted(format!(
                    "stray return to HLE trampoline on core {} (sp={:08x} pc={:08x}); pending: {:?}",
                    c, sp, self.cpus[c].pc, list
                ));
            }
        }
    }

    /// Execute exactly one instruction on core `c` (debugger single step).
    /// ROM functions (HLE) count as one step. Returns false if the machine stopped.
    pub fn step_core(&mut self, c: usize) -> bool {
        if self.state != RunState::Running {
            if let RunState::Breakpoint(..) = self.state {
                self.state = RunState::Running;
            } else if self.state == RunState::Paused {
                self.state = RunState::Running;
            } else {
                return false;
            }
        }
        self.check_appcpu();
        self.sys.cur_core = c;
        self.sys.q_cyc0 = self.cpus[c].cycles;
        let before = self.cpus[c].cycles;
        let bps = std::mem::take(&mut self.cpus[c].breakpoints);
        let r = self.cpus[c].step(&mut self.sys);
        self.cpus[c].breakpoints = bps;
        let ok = match r {
            StepResult::Hle => {
                // run the ROM function (and any firmware callbacks it makes) to completion
                let sp = self.cpus[c].ar(1);
                let mut guard = 0;
                loop {
                    let pc = self.cpus[c].pc;
                    if (0x4000_0000..0x4007_0000).contains(&pc) || crate::hle::is_tramp_ret(pc) {
                        if !self.run_core(c, 1) {
                            break;
                        }
                    } else if self.pending.iter().any(|p| p.core == c && p.sp == sp) {
                        if !self.run_core(c, 1) {
                            break;
                        }
                    } else {
                        break;
                    }
                    guard += 1;
                    if guard > 5_000_000 {
                        break;
                    }
                }
                self.state == RunState::Running
            }
            _ => true,
        };
        let dt = self.cycles_to_ns(self.cpus[c].cycles - before);
        self.sys.now += dt;
        self.sys.tick();
        self.update_irq_lines();
        ok
    }

    fn cycles_to_ns(&self, cycles: u64) -> u64 {
        ((cycles as u128 * 1_000_000_000u128) / self.sys.cpu_hz as u128) as u64
    }

    /// Run the machine for `ns` nanoseconds of emulated time (or until it stops).
    pub fn run_for(&mut self, ns: u64) -> RunState {
        if self.state != RunState::Running {
            return self.state.clone();
        }
        let end = self.sys.now + ns;
        while self.sys.now < end {
            if !self.step_quantum(end) {
                break;
            }
        }
        self.state.clone()
    }

    /// Execute one scheduling quantum; may fast-forward when both cores are idle.
    pub fn step_quantum(&mut self, limit: u64) -> bool {
        self.check_appcpu();
        let i0 = self.cpus[0].icount + self.cpus[1].icount;
        // keep both cores' cycle counters aligned with the global clock
        let start_cycles = self.cpus[0].cycles;
        // Each core runs QUANTUM cycles per round.  Normally that is one slice per
        // core; during cross-core handshakes they alternate in FINE_SLICE steps.
        let q0 = [self.cpus[0].cycles, self.cpus[1].cycles];
        let tgt = [q0[0] + QUANTUM, q0[1] + QUANTUM];
        loop {
            let slice = if self.fine_until > self.sys.now { FINE_SLICE } else { QUANTUM };
            let mut progressed = false;
            for c in 0..2 {
                if c == 1 && !self.appcpu_started {
                    continue;
                }
                if self.cpus[c].cycles < tgt[c] {
                    let t = (self.cpus[c].cycles + slice).min(tgt[c]);
                    if !self.run_core_to(c, t, q0[c]) {
                        self.account(i0);
                        return false;
                    }
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        let _ = start_cycles;
        if !self.appcpu_started {
            let cyc = self.cpus[0].cycles;
            self.cpus[1].cycles = cyc;
        }
        let dt = self.cycles_to_ns(QUANTUM);
        self.sys.now += dt;
        if let Some(r) = self.sys.rtc.reset_request.take() {
            let _ = self.reset(r);
            return self.state == RunState::Running;
        }
        let next = self.sys.tick();
        self.update_irq_lines();
        self.report_exceptions();
        if self.sys.rtc.sleep_request {
            self.sys.rtc.sleep_request = false;
            self.enter_sleep();
            // powering the radio down before sleeping is not "using Wi-Fi"
            self.radio_first = None;
            self.sys.radio_touched = false;
        }
        if self.sleeping.is_some() {
            self.check_wake();
        }
        if self.sys.radio_touched && !self.radio_reported {
            let first = *self.radio_first.get_or_insert(self.sys.now);
            if self.sys.now > first + 300_000_000 {
                self.radio_reported = true;
                self.messages.push("note: the firmware is starting the Wi-Fi/Bluetooth radio. The radio is not emulated, so connections never come up (WiFi.status() stays disconnected); the rest of the firmware keeps running.".into());
            }
        }
        // idle fast-forward
        let idle = |m: &Machine, c: usize| -> bool {
            let cpu = &m.cpus[c];
            if c == 1 && !m.appcpu_started {
                return true;
            }
            cpu.stalled || (cpu.waiting && !cpu.has_wakeup_interrupt()) || m.busy_until[c] > m.sys.now
        };
        if idle(self, 0) && idle(self, 1) {
            let mut t = limit;
            if let Some(n) = next {
                t = t.min(n);
            }
            if let Some(w) = self.sleeping.as_ref().and_then(|s| s.wake_at) {
                t = t.min(w.max(self.sys.now + 1));
            }
            for c in 0..2 {
                if c == 1 && !self.appcpu_started {
                    continue;
                }
                if self.busy_until[c] > self.sys.now {
                    t = t.min(self.busy_until[c]);
                }
                if !self.cpus[c].stalled {
                    let cyc = self.cpus[c].cycles_to_timer() as u64;
                    t = t.min(self.sys.now + self.cycles_to_ns(cyc));
                }
            }
            if t > self.sys.now + dt {
                let skip = t - self.sys.now;
                let cycles = ((skip as u128 * self.sys.cpu_hz as u128) / 1_000_000_000u128) as u64;
                for c in 0..2 {
                    let mut left = cycles;
                    while left > 0 {
                        let s = left.min(u32::MAX as u64 / 2) as u32;
                        self.cpus[c].advance_cycles(s);
                        left -= s as u64;
                    }
                }
                self.sys.now = t;
                self.sys.tick();
                self.update_irq_lines();
            }
        }
        self.account(i0);
        true
    }

    fn account(&mut self, i0: u64) {
        let i1 = self.cpus[0].icount + self.cpus[1].icount;
        self.total_instructions += i1 - i0;
    }

    fn report_exceptions(&mut self) {
        for c in 0..2 {
            if let Some(e) = self.cpus[c].last_exception.take() {
                // Only report each distinct exception occasionally
                if self.last_exc_report[c] != self.sys.now / 1_000_000_000 || true {
                    self.last_exc_report[c] = self.sys.now / 1_000_000_000;
                    let cause = match e.cause {
                        0 => "IllegalInstruction",
                        1 => "Syscall",
                        2 => "InstructionFetchError",
                        3 => "LoadStoreError",
                        6 => "IntegerDivideByZero",
                        9 => "LoadStoreAlignment",
                        20 => "InstFetchProhibited",
                        28 => "LoadProhibited",
                        29 => "StoreProhibited",
                        32 => "Coprocessor0Disabled",
                        _ => "Exception",
                    };
                    if e.cause == 32 {
                        continue; // lazy FPU context switching: normal
                    }
                    let mut msg = format!(
                        "core {}: {} (cause {}) at {} [pc=0x{:08x}] vaddr=0x{:08x}",
                        c,
                        cause,
                        e.cause,
                        self.symbols.describe(e.pc),
                        e.pc,
                        e.vaddr
                    );
                    if let Some(f) = self.sys.fault_log.pop() {
                        msg.push_str(&format!(" ({})", f));
                    }
                    if let Some((pc, raw)) = self.cpus[c].last_illegal.take() {
                        msg.push_str(&format!(" illegal insn 0x{:06x} at 0x{:08x}", raw, pc));
                    }
                    self.messages.push(msg);
                    if self.break_on_exception {
                        self.state = RunState::Breakpoint(c, self.cpus[c].pc);
                    }
                }
            }
        }
        self.sys.fault_log.clear();
    }

    /// Best-effort backtrace for a core using the windowed-ABI frame layout.
    pub fn backtrace(&mut self, c: usize) -> Vec<String> {
        let mut out = vec![self.symbols.describe(self.cpus[c].pc)];
        let cpu = &self.cpus[c];
        let mut pc = cpu.ar(0);
        if (cpu.hle_lo..cpu.hle_hi).contains(&cpu.pc) {
            // inside an emulated ROM function: ENTRY has not run, the return
            // address is in a(4*CALLINC) of the caller's window
            let callinc = (cpu.sr[PS] >> PS_CALLINC_SHIFT) & 3;
            let ra = cpu.ar((callinc * 4) as u8);
            let real = (ra & 0x3FFF_FFFF) | 0x4000_0000;
            out.push(format!("{} (call site)", self.symbols.describe(real.wrapping_sub(3))));
        }
        let mut sp = cpu.ar(1);
        let base_pc = cpu.pc;
        for _ in 0..16 {
            if pc == 0 || sp == 0 {
                break;
            }
            let real = (pc & 0x3FFF_FFFF) | (base_pc & 0xC000_0000);
            out.push(self.symbols.describe(real.wrapping_sub(3)));
            // caller's a0/a1 are saved in the base save area below sp
            let npc = self.sys.peek32(sp.wrapping_sub(16));
            let nsp = self.sys.peek32(sp.wrapping_sub(12));
            if nsp <= sp || nsp.wrapping_sub(sp) > 0x10000 {
                break;
            }
            pc = npc;
            sp = nsp;
        }
        out
    }

    pub fn describe(&self, addr: u32) -> String {
        self.symbols.describe(addr)
    }
}

//! Debugging support: DWARF line tables, stepping, register / backtrace views.

use esp32_soc::Machine;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use xtensa_core::Bus;

#[derive(Clone, Debug)]
pub struct Loc {
    pub file: String,
    pub line: u32,
    pub function: Option<String>,
}

type Slice = addr2line::gimli::EndianSlice<'static, addr2line::gimli::RunTimeEndian>;

pub struct DebugInfo {
    ctx: addr2line::Context<Slice>,
    /// owns the ELF bytes referenced by `ctx`
    _data: Box<[u8]>,
    /// (file, line) -> addresses where the line starts
    lines: BTreeMap<(String, u32), Vec<u32>>,
    /// sorted list of (start address, file index, line)
    files: Vec<String>,
    /// cache: does a source file exist on this machine?
    exists: std::sync::Mutex<std::collections::HashMap<String, bool>>,
    /// local-variable evaluator
    locals: Option<crate::locals::Locals>,
}

// SAFETY: the addr2line context is only ever used behind a Mutex/Arc from one thread at a time.
unsafe impl Send for DebugInfo {}
unsafe impl Sync for DebugInfo {}

impl DebugInfo {
    pub fn load(elf: &[u8]) -> Result<DebugInfo, String> {
        use addr2line::gimli;
        use object::{Object, ObjectSection};
        let data: Box<[u8]> = elf.to_vec().into_boxed_slice();
        // SAFETY: `data` is a heap allocation owned by the returned DebugInfo and never moved/freed
        // while `ctx` (which borrows it) is alive.
        let sdata: &'static [u8] = unsafe { std::slice::from_raw_parts(data.as_ptr(), data.len()) };
        let obj = object::File::parse(sdata).map_err(|e| e.to_string())?;
        if obj.section_by_name(".debug_line").is_none() {
            return Err("ELF has no DWARF line information (build with -g)".into());
        }
        let endian = if obj.is_little_endian() { gimli::RunTimeEndian::Little } else { gimli::RunTimeEndian::Big };
        let load = |id: gimli::SectionId| -> Result<Slice, gimli::Error> {
            let bytes: &'static [u8] = match obj.section_by_name(id.name()) {
                Some(sec) => match sec.uncompressed_data() {
                    Ok(std::borrow::Cow::Borrowed(b)) => b,
                    Ok(std::borrow::Cow::Owned(v)) => Box::leak(v.into_boxed_slice()),
                    Err(_) => &[],
                },
                None => &[],
            };
            Ok(gimli::EndianSlice::new(bytes, endian))
        };
        let dwarf = gimli::Dwarf::load(load).map_err(|e| e.to_string())?;
        let ctx = addr2line::Context::from_dwarf(dwarf).map_err(|e| e.to_string())?;
        let locals = gimli::Dwarf::load(load).ok().map(crate::locals::Locals::new);
        let mut lines: BTreeMap<(String, u32), Vec<u32>> = BTreeMap::new();
        let mut files: Vec<String> = Vec::new();
        // enumerate code addresses from executable sections
        for sec in obj.sections() {
            if sec.kind() != object::SectionKind::Text {
                continue;
            }
            let (start, size) = (sec.address(), sec.size());
            if let Ok(iter) = ctx.find_location_range(start, start + size) {
                let mut last: Option<(String, u32)> = None;
                for (addr, _len, loc) in iter {
                    if let (Some(f), Some(l)) = (loc.file, loc.line) {
                        let key = (f.to_string(), l);
                        if last.as_ref() != Some(&key) {
                            lines.entry(key.clone()).or_default().push(addr as u32);
                            last = Some(key);
                        }
                    }
                }
            }
        }
        // Lines whose code was entirely inlined from other functions (e.g.
        // `delay.delay_ms(500)`) never appear in the line table themselves; their
        // breakpoint location is the entry of the inlined instance (call site).
        if let Ok(inl) = inlined_call_sites(load) {
            for (key, pc) in inl {
                let v = lines.entry(key).or_default();
                if !v.contains(&pc) {
                    v.push(pc);
                }
            }
        }
        for (f, _) in lines.keys() {
            if files.last() != Some(f) {
                files.push(f.clone());
            }
        }
        files.sort();
        files.dedup();
        Ok(DebugInfo { ctx, _data: data, lines, files, exists: Default::default(), locals })
    }

    /// Source locations for `pc`, innermost first: the line being executed and,
    /// when it was inlined, the call sites in the functions it was inlined into.
    pub fn frames(&self, pc: u32) -> Vec<Loc> {
        let mut out = Vec::new();
        if let Ok(mut frames) = self.ctx.find_frames(pc as u64).skip_all_loads() {
            while let Ok(Some(fr)) = frames.next() {
                let function = fr.function.as_ref().and_then(|n| n.demangle().ok().map(|s| s.to_string()));
                if let Some(l) = fr.location {
                    if let (Some(f), Some(line)) = (l.file, l.line) {
                        out.push(Loc { file: f.to_string(), line, function });
                    }
                }
            }
        }
        if out.is_empty() {
            if let Ok(Some(l)) = self.ctx.find_location(pc as u64) {
                if let (Some(f), Some(line)) = (l.file, l.line) {
                    out.push(Loc { file: f.to_string(), line, function: None });
                }
            }
        }
        out
    }

    /// Where execution is, as the user sees it: the innermost frame whose source
    /// file exists on this machine ("just my code": library code inlined into a
    /// sketch is shown at the sketch line that called it), else the innermost.
    pub fn location(&self, pc: u32) -> Option<Loc> {
        let frames = self.frames(pc);
        let first = frames.first().cloned()?;
        Some(frames.into_iter().find(|f| self.file_exists(&f.file)).unwrap_or(first))
    }

    fn file_exists(&self, f: &str) -> bool {
        let mut cache = self.exists.lock().unwrap();
        *cache.entry(f.to_string()).or_insert_with(|| std::path::Path::new(f).is_file())
    }

    /// Local variables at `pc` (innermost scope first).
    pub fn locals(&self, pc: u32, t: &mut crate::locals::Target) -> Vec<Value> {
        self.locals.as_ref().map(|l| l.at(pc, t)).unwrap_or_default()
    }

    /// Source files known to the debug info.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Resolve a breakpoint "file:line" to addresses (matches by path suffix).
    pub fn addresses(&self, file: &str, line: u32) -> Vec<u32> {
        let want = file.replace('\\', "/");
        let mut best: Vec<u32> = Vec::new();
        // find the nearest line >= requested that has code, within 20 lines
        for l in line..line + 20 {
            for ((f, fl), addrs) in self.lines.range((String::new(), 0)..) {
                if *fl != l {
                    continue;
                }
                let f2 = f.replace('\\', "/");
                if f2.ends_with(&want) || want.ends_with(&f2) || f2.rsplit('/').next() == want.rsplit('/').next() {
                    best.extend(addrs.iter().copied());
                }
            }
            if !best.is_empty() {
                break;
            }
        }
        best.sort();
        best.dedup();
        best
    }
}

/// (call_file, call_line) -> entry address of every DW_TAG_inlined_subroutine.
fn inlined_call_sites<F>(load: F) -> Result<Vec<((String, u32), u32)>, addr2line::gimli::Error>
where
    F: Fn(addr2line::gimli::SectionId) -> Result<Slice, addr2line::gimli::Error>,
{
    use addr2line::gimli;
    let dwarf = gimli::Dwarf::load(&load)?;
    let mut out = Vec::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = match dwarf.unit(header) {
            Ok(u) => u,
            Err(_) => continue,
        };
        let Some(lp) = unit.line_program.clone() else { continue };
        let lh = lp.header();
        let comp_dir = unit.comp_dir.map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        let file_path = |idx: u64| -> Option<String> {
            let f = lh.file(idx)?;
            let name = dwarf.attr_string(&unit, f.path_name()).ok()?.to_string_lossy().into_owned();
            if name.starts_with('/') {
                return Some(name);
            }
            let dir = f.directory(lh).and_then(|d| dwarf.attr_string(&unit, d).ok()).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
            let base = if dir.starts_with('/') { dir } else if dir.is_empty() { comp_dir.clone() } else { format!("{}/{}", comp_dir, dir) };
            Some(if base.is_empty() { name } else { format!("{}/{}", base.trim_end_matches('/'), name) })
        };
        let mut entries = unit.entries();
        while let Some((_, e)) = entries.next_dfs()? {
            if e.tag() != gimli::DW_TAG_inlined_subroutine {
                continue;
            }
            let file = match e.attr_value(gimli::DW_AT_call_file)? {
                Some(gimli::AttributeValue::FileIndex(i)) => i,
                Some(v) => match v.udata_value() {
                    Some(i) => i,
                    None => continue,
                },
                None => continue,
            };
            let line = match e.attr_value(gimli::DW_AT_call_line)?.and_then(|v| v.udata_value()) {
                Some(l) if l > 0 => l as u32,
                _ => continue,
            };
            let mut pc = None;
            for at in [gimli::DW_AT_entry_pc, gimli::DW_AT_low_pc] {
                if let Some(v) = e.attr_value(at)? {
                    pc = match v {
                        gimli::AttributeValue::Addr(a) => Some(a),
                        gimli::AttributeValue::DebugAddrIndex(i) => dwarf.address(&unit, i).ok(),
                        _ => None,
                    };
                    if pc.is_some() {
                        break;
                    }
                }
            }
            if pc.is_none() {
                if let Ok(mut r) = dwarf.die_ranges(&unit, e) {
                    let mut lo = u64::MAX;
                    while let Ok(Some(rg)) = r.next() {
                        if rg.begin < rg.end {
                            lo = lo.min(rg.begin);
                        }
                    }
                    if lo != u64::MAX {
                        pc = Some(lo);
                    }
                }
            }
            if let (Some(pc), Some(path)) = (pc, file_path(file)) {
                if pc != 0 {
                    out.push(((path, line), pc as u32));
                }
            }
        }
    }
    Ok(out)
}

pub fn step_instructions(m: &mut Machine, core: usize, n: u64) {
    for _ in 0..n {
        if !m.step_core(core) {
            break;
        }
    }
}

/// Step until the source line changes (over: do not stop inside called functions).
pub fn step_line(m: &mut Machine, core: usize, over: bool, d: &DebugInfo) {
    let start = d.location(m.cpus[core].pc);
    let sp0 = m.cpus[core].ar(1);
    for _ in 0..2_000_000 {
        if !m.step_core(core) {
            return;
        }
        let pc = m.cpus[core].pc;
        if m.cpus[core].breakpoints.contains(&pc) {
            return;
        }
        let loc = match d.location(pc) {
            Some(l) => l,
            None => continue,
        };
        let same = start.as_ref().map(|s| s.file == loc.file && s.line == loc.line).unwrap_or(false);
        if same {
            continue;
        }
        if over && m.cpus[core].ar(1) < sp0 {
            continue; // inside a callee
        }
        return;
    }
}

pub fn regs_json(m: &mut Machine, core: usize, d: Option<&DebugInfo>) -> Value {
    let bt = m.backtrace(core);
    let cpu = &m.cpus[core];
    let ar: Vec<String> = (0..16u8).map(|i| format!("0x{:08x}", cpu.ar(i))).collect();
    let sr = |n: usize| format!("0x{:08x}", cpu.sr[n]);
    let pc = cpu.pc;
    let loc = d.and_then(|d| d.location(pc));
    let mut dis = Vec::new();
    let mut a = pc;
    for _ in 0..12 {
        let w = m.sys.fetch(a).unwrap_or(0);
        let (t, l) = xtensa_core::disasm(a, w);
        dis.push(json!({"addr": format!("0x{:08x}", a), "text": t, "current": a == pc}));
        a += l as u32;
    }
    let frames: Vec<Value> = bt
        .iter()
        .map(|f| {
            let addr = u32::from_str_radix(f.split("0x").last().unwrap_or("").trim_end_matches(|c: char| !c.is_ascii_hexdigit()), 16).ok();
            let l = addr.and_then(|a| d.and_then(|d| d.location(a)));
            json!({"text": f, "file": l.as_ref().map(|x| x.file.clone()), "line": l.as_ref().map(|x| x.line)})
        })
        .collect();
    // local variables (needs DWARF): CFA = stack pointer before the function's ENTRY
    let locals = match d {
        Some(d) => {
            let cfa = m.symbols.lookup(pc).and_then(|(_, off)| {
                let start = pc.wrapping_sub(off);
                let w = m.sys.fetch(start).ok()?;
                // ENTRY as, imm: op0=6, n=3, m=0; frame size = imm12 * 8
                if w & 0xFF == 0x36 && off >= 3 {
                    Some(m.cpus[core].ar(1).wrapping_add(((w >> 12) & 0xFFF) * 8))
                } else {
                    None
                }
            });
            let regs: Vec<u32> = (0..16u8).map(|i| m.cpus[core].ar(i)).collect();
            let reg = move |n: u16| regs.get(n as usize).copied().unwrap_or(0);
            let mut mem = |a: u32, n: usize| m.sys.read_bytes(a, n);
            let mut t = crate::locals::Target { reg: &reg, mem: &mut mem, cfa };
            d.locals(pc, &mut t)
        }
        None => Vec::new(),
    };
    json!({
        "locals": locals,
        "core": core, "pc": format!("0x{:08x}", pc), "symbol": m.describe(pc),
        "location": loc.map(|l| json!({"file": l.file, "line": l.line, "function": l.function})),
        "a": ar, "ps": sr(230), "sar": sr(3), "windowbase": cpu.sr[72], "windowstart": format!("0x{:04x}", cpu.sr[73]),
        "lbeg": sr(0), "lend": sr(1), "lcount": sr(2), "exccause": cpu.sr[232], "excvaddr": sr(238),
        "ccount": sr(234), "intenable": sr(228), "interrupt": format!("0x{:08x}", cpu.interrupt()),
        "backtrace": frames, "disassembly": dis,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use esp32_soc::{Machine, MachineConfig, RunState};

    const ELF: &[u8] = include_bytes!("../tests/fixtures/debug-demo/blink.elf");

    fn machine() -> Machine {
        let flash = esp32_soc::flash::build_flash(ELF, 4 << 20, None, None).expect("flash image from ELF");
        let mut m = Machine::new(flash, MachineConfig::default());
        m.load_symbols(ELF).unwrap();
        m.boot().unwrap();
        m
    }

    #[test]
    fn line_table_and_locations() {
        let d = DebugInfo::load(ELF).unwrap();
        assert!(d.files().iter().any(|f| f.ends_with("blink.c")));
        assert_eq!(d.addresses("blink.c", 11), vec![0x4008_0432]);
        assert_eq!(d.addresses("src/blink.c", 5), vec![0x4008_0413], "matches by file name");
        // a line without code resolves to the next line that has some
        assert_eq!(d.addresses("blink.c", 7), vec![0x4008_0420]);
        let l = d.location(0x4008_0413).unwrap();
        assert_eq!((l.line, l.function.as_deref()), (5, Some("step")));
    }

    #[test]
    fn breakpoint_and_stepping() {
        let d = DebugInfo::load(ELF).unwrap();
        let mut m = machine();
        let bp = d.addresses("blink.c", 11)[0];
        for c in 0..2 {
            m.cpus[c].breakpoints = vec![bp];
        }
        let st = m.run_for(50_000_000);
        assert_eq!(st, RunState::Breakpoint(0, bp));
        let line = |m: &Machine| d.location(m.cpus[0].pc).map(|l| l.line);
        // 11 -> 9 (loop jump) -> 10 -> into step() at 4
        step_line(&mut m, 0, false, &d);
        assert_eq!(line(&m), Some(9));
        step_line(&mut m, 0, false, &d);
        assert_eq!(line(&m), Some(10));
        step_line(&mut m, 0, false, &d);
        assert_eq!(line(&m), Some(4), "step into the call");
        // the global `counter` advances by 3 per iteration
        let sym = *m.symbols.by_name.get("counter").expect("counter symbol");
        let before = m.sys.peek32(sym);
        for c in 0..2 {
            m.cpus[c].breakpoints.clear();
        }
        m.state = RunState::Running;
        for c in 0..2 {
            m.cpus[c].breakpoints = vec![bp];
        }
        assert_eq!(m.run_for(50_000_000), RunState::Breakpoint(0, bp));
        assert_eq!(m.sys.peek32(sym), before.wrapping_add(3));
    }
}

#[cfg(test)]
mod rust_dwarf_tests {
    //! DWARF produced by a real compiler (rustc / LLVM for Xtensa, esp-hal hello world).
    use super::*;
    use esp32_soc::{Machine, MachineConfig, RunState};

    const ELF: &[u8] = include_bytes!("../../../examples/rust-esp-hal/hello.elf");

    #[test]
    fn rustc_line_table() {
        let d = DebugInfo::load(ELF).unwrap();
        let l = d.location(0x400d_0574).unwrap();
        assert!(l.file.ends_with("src/main.rs"), "{}", l.file);
        assert_eq!(l.line, 8);
        assert_eq!(l.function.as_deref(), Some("main"));
        assert!(!d.addresses("main.rs", 19).is_empty());
    }

    #[test]
    fn breakpoints_in_rust_main() {
        let d = DebugInfo::load(ELF).unwrap();
        // `delay.delay_ms(500)` on line 19 is fully inlined: it resolves to the
        // entry of the inlined instance, found through DW_TAG_inlined_subroutine
        assert_eq!(d.addresses("main.rs", 19), vec![0x400d_0611]);
        let flash = esp32_soc::flash::build_flash(ELF, 4 << 20, None, None).unwrap();
        let mut m = Machine::new(flash, MachineConfig::default());
        m.load_symbols(ELF).unwrap();
        m.boot().unwrap();
        // line 14 (Delay::new) runs once
        let bps = d.addresses("main.rs", 14);
        for c in 0..2 {
            m.cpus[c].breakpoints = bps.clone();
        }
        match m.run_for(2_000_000_000) {
            RunState::Breakpoint(0, pc) => assert!(bps.contains(&pc)),
            other => panic!("expected a breakpoint, got {:?}", other),
        }
        let frames = d.frames(m.cpus[0].pc);
        assert!(frames[0].file.ends_with("delay.rs"), "innermost is the inlined library code");
        assert!(frames.iter().any(|f| f.file.ends_with("src/main.rs") && f.line == 14), "{:?}", frames);
        // the inlined delay loop hits line 19 again and again (like gdb)
        let bp19 = d.addresses("main.rs", 19);
        for c in 0..2 {
            m.cpus[c].breakpoints = bp19.clone();
        }
        m.state = RunState::Running;
        assert!(matches!(m.run_for(2_000_000_000), RunState::Breakpoint(0, 0x400d_0611)));
        // locals of the inlined delay_ms(500): the `ms` parameter is a constant 500
        let regs = regs_json(&mut m, 0, Some(&d));
        let locals = regs["locals"].as_array().cloned().unwrap_or_default();
        eprintln!("locals: {}", serde_json::to_string(&locals).unwrap());
        assert!(locals.iter().any(|l| l["name"] == "ms" && l["value"] == "500"), "{:?}", locals);
    }
}

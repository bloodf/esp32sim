use esp32_soc::flash::build_flash;
use esp32_soc::io::IoBridge;
use esp32_soc::{Machine, MachineConfig, RunState};
use std::io::Write;
use std::sync::{Arc, Mutex};

struct ConsoleIo {
    input: Arc<Mutex<std::collections::VecDeque<u8>>>,
}
impl IoBridge for ConsoleIo {
    fn uart_tx(&mut self, _now: u64, port: u8, b: u8) {
        if port == 0 {
            let mut o = std::io::stdout();
            let _ = o.write_all(&[b]);
            let _ = o.flush();
        }
    }
    fn uart_rx(&mut self, _now: u64, port: u8) -> Option<u8> {
        if port == 0 { self.input.lock().unwrap().pop_front() } else { None }
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        if self.input.lock().unwrap().is_empty() { None } else { Some(now) }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let fw = std::fs::read(&args[1]).expect("read firmware");
    let secs: f64 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(2.0);
    // optional: input script "t:text" pairs
    let inputs: Vec<(f64, String)> = args.iter().skip(3).filter_map(|a| {
        let (t, s) = a.split_once(':')?; Some((t.parse().ok()?, s.replace("\\n", "\n").replace("\\r", "\r")))
    }).collect();
    let flash = build_flash(&fw, 4 << 20, None, None).expect("flash");
    let mut cfg = MachineConfig::default();
    cfg.trace = std::env::var("TRACE").is_ok();
    cfg.log_unknown_regs = std::env::var("UNKNOWN").is_ok();
    let mut m = Machine::new(flash, cfg);
    let input = Arc::new(Mutex::new(std::collections::VecDeque::new()));
    m.set_io(Box::new(ConsoleIo { input: input.clone() }));
    m.boot().expect("boot");
    if let Ok(n) = std::env::var("TRACE_LEN") {
        let n: usize = n.parse().unwrap_or(4096);
        for c in 0..2 {
            m.cpus[c].trace = vec![0; n];
            m.cpus[c].trace_pos = 0;
        }
    }
    let stop_exc = std::env::var("STOP_EXC").is_ok();
    m.break_on_exception = stop_exc;
    for c in 0..2 {
        m.cpus[c].break_on_exception = stop_exc;
    }
    for msg in m.messages.drain(..) { eprintln!("[emu] {}", msg); }
    let t0 = std::time::Instant::now();
    let total = (secs * 1e9) as u64;
    let mut pending = inputs.clone();
    while m.now_ns() < total {
        let st = m.run_for(10_000_000);
        if let RunState::Breakpoint(c, pc) = st {
            if stop_exc {
                for msg in m.messages.drain(..) { eprintln!("[emu] {}", msg); }
                let pcs = m.cpus[c].recent_pcs();
                let srcs = m.sys.irq.sources;
                for i in 0..69 { if srcs >> i & 1 != 0 { eprintln!("[emu] source {} active -> pro int {}, app int {}", i, m.sys.irq.map[0][i], m.sys.irq.map[1][i]); } }
                let o = 1 - c;
                let opcs = m.cpus[o].recent_pcs();
                eprintln!("[emu] other core backtrace: {}", m.backtrace(o).join(" <- "));
                eprintln!("[emu] other core {} pc {:08x} trace: {}", o, m.cpus[o].pc, opcs.iter().map(|p| format!("{:08x}", p)).collect::<Vec<_>>().join(" "));
                eprintln!("[emu] core{} INTERRUPT={:08x} INTENABLE={:08x} PS={:08x}", c, m.cpus[c].interrupt(), m.cpus[c].intenable(), m.cpus[c].ps());
                eprintln!("[emu] stopped on exception, core {} now at {:08x}; trace ({}): {}", c, pc, pcs.len(), pcs.iter().map(|p| format!("{:08x}", p)).collect::<Vec<_>>().join(" "));
                break;
            }
        }
        let now_s = m.now_ns() as f64 / 1e9;
        pending.retain(|(t, s)| { if *t <= now_s { input.lock().unwrap().extend(s.bytes()); false } else { true } });
        for msg in m.messages.drain(..) { eprintln!("\n[emu {:.3}s] {}", now_s, msg); }
        if std::env::var("EVENTS").is_ok() { for e in m.sys.events.drain(..) { eprintln!("[evt] {}", e); } }
        if st != RunState::Running { eprintln!("\n[emu] stopped: {:?}", st); break; }
    }
    let el = t0.elapsed().as_secs_f64();
    eprintln!("[emu] cross-core handshakes: {}", m.handshakes);
    eprintln!("[emu] cpu clock {} MHz, apb {} MHz", m.sys.cpu_hz / 1_000_000, m.sys.apb_hz() / 1_000_000);
    eprintln!("\n[emu] emulated {:.3}s in {:.2}s wall, {} instructions ({:.1} MIPS), pc0={} pc1={}",
        m.now_ns() as f64/1e9, el, m.total_instructions, m.total_instructions as f64/el/1e6,
        m.describe(m.cpus[0].pc), m.describe(m.cpus[1].pc));
    if std::env::var("TRACE").is_ok() {
        for c in 0..2 { eprintln!("core{} recent: {}", c, m.cpus[c].recent_pcs().iter().map(|p| format!("{:08x}", p)).collect::<Vec<_>>().join(" ")); }
    }
    if let Ok(spec) = std::env::var("DIS") {
        for part in spec.split(',') {
            let (a, n) = part.split_once('+').unwrap_or((part, "24"));
            let mut pc = u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap();
            let n: u32 = n.parse().unwrap();
            eprintln!("--- disassembly at {:08x}", pc);
            for _ in 0..n {
                use xtensa_core::Bus;
                let w = m.sys.fetch(pc).unwrap_or(0);
                let (t, l) = xtensa_core::disasm(pc, w);
                eprintln!("  {:08x}: {:06x}  {}", pc, w & if l == 2 { 0xffff } else { 0xffffff }, t);
                pc += l as u32;
            }
        }
    }
    if let Ok(spec) = std::env::var("DISF") {
        use xtensa_core::Bus;
        for part in spec.split(',') {
            let target = u32::from_str_radix(part.trim_start_matches("0x"), 16).unwrap();
            // scan back for "entry a1, N" (bytes 0x36 0x?1)
            let mut start = target;
            for back in 0..4096u32 {
                let a = target - back;
                let w = m.sys.fetch(a).unwrap_or(0);
                if w & 0xFF == 0x36 && (w >> 8) & 0xF == 1 { start = a; break; }
            }
            eprintln!("--- function containing {:08x} starts at {:08x}", target, start);
            let mut pc = start;
            while pc <= target + 12 {
                let w = m.sys.fetch(pc).unwrap_or(0);
                let (t, l) = xtensa_core::disasm(pc, w);
                let mut extra = String::new();
                if t.starts_with("l32r") {
                    let addr = u32::from_str_radix(t.rsplit("0x").next().unwrap(), 16).unwrap();
                    let v = m.sys.peek32(addr);
                    extra = format!("  ; =0x{:08x}", v);
                    if (0x3F40_0000..0x3F80_0000).contains(&v) {
                        let s = m.sys.read_cstr(v, 60);
                        if s.len() > 2 && s.iter().all(|c| (0x20..0x7f).contains(c) || *c >= 0x80) { extra.push_str(&format!(" \"{}\"", String::from_utf8_lossy(&s))); }
                    }
                }
                eprintln!("{} {:08x}: {}{}", if pc == target {">"} else {" "}, pc, t, extra);
                pc += l as u32;
            }
        }
    }
    if std::env::var("SCANFP").is_ok() {
        use xtensa_core::Bus;
        let segs: Vec<(u32,u32)> = m.app.as_ref().unwrap().segments.iter().filter(|s| (0x4008_0000..0x40C0_0000).contains(&s.load_addr)).map(|s| (s.load_addr, s.len)).collect();
        let mut seen = std::collections::HashSet::new();
        for (base, len) in segs {
            // linear sweep from each entry instruction
            let mut a = base;
            while a < base + len {
                let w = m.sys.fetch(a).unwrap_or(0);
                if w & 0xFF == 0x36 && (w >> 8) & 0xF == 1 {
                    // sweep this function
                    let mut pc = a; let mut lines = Vec::new(); let mut has = false;
                    for _ in 0..400 {
                        let w = m.sys.fetch(pc).unwrap_or(0);
                        let (t, l) = xtensa_core::disasm(pc, w);
                        if t.starts_with("divn.s") || t.starts_with("sqrt0.s") || t.starts_with("rsqrt0.s") || t.starts_with("recip0.s") || t.starts_with("mksadj") { has = true; }
                        lines.push(format!("  {:08x}: {}", pc, t));
                        pc += l as u32;
                        if t.starts_with("retw") && has { break; }
                        if t.starts_with("retw") { break; }
                    }
                    if has {
                        let key: Vec<String> = lines.iter().filter(|l| l.contains(".s ") || l.contains("wfr") || l.contains("rfr")).map(|l| l.split(": ").nth(1).unwrap().to_string()).collect();
                        let k = key.join(";");
                        if seen.insert(k) { eprintln!("=== fn at {:08x}", a); for l in &lines { if l.contains(".s ") || l.contains("wfr") || l.contains("rfr") || l.contains("entry") {eprintln!("{}", l);} } }
                    }
                }
                a += 1;
            }
        }
    }
    if std::env::var("REGS").is_ok() {
        for c in 0..2 {
            let cpu = &m.cpus[c];
            eprintln!("core{} pc={:08x} ps={:08x} wb={} ws={:04x} intenable={:08x} interrupt={:08x} ccount={:08x} ccompare0={:08x} waiting={}", c, cpu.pc, cpu.sr[230], cpu.sr[72], cpu.sr[73], cpu.sr[228], cpu.interrupt(), cpu.sr[234], cpu.sr[240], cpu.waiting);
            for i in 0..16u8 { eprint!("a{}={:08x} ", i, cpu.ar(i)); } eprintln!();
        }
    }
    if std::env::var("UARTDBG").is_ok() {
        let u = &m.sys.uart[0];
        eprintln!("uart0: rx_fifo={} int_raw={:08x} int_ena={:08x} conf0={:08x} conf1={:08x} clkdiv={:08x} next_rx={} tout_fired={}",
            u.rx.len(), u.int_raw, u.regs.read(0x0C), u.regs.read(0x20), u.regs.read(0x24), u.regs.read(0x14), u.next_rx_ns, u.tout_fired);
        eprintln!("irq sources={:x} map pro uart0={} app uart0={} lines0={:08x} lines1={:08x} intenable0={:08x} intenable1={:08x}",
            m.sys.irq.sources, m.sys.irq.map[0][34], m.sys.irq.map[1][34], m.sys.irq.lines(0), m.sys.irq.lines(1), m.cpus[0].intenable(), m.cpus[1].intenable());
    }
    if let Some(t) = &m.sys.read_trace {
        let mut seen = std::collections::BTreeMap::new();
        for (a, v) in t { seen.insert(*a, *v); }
        eprintln!("recent peripheral reads: {}", seen.iter().map(|(a, v)| format!("{:08x}={:08x}", a, v)).collect::<Vec<_>>().join(" "));
    }
    if std::env::var("HLE").is_ok() { eprintln!("HLE: {:?}", m.hle_call_counts()); }
}

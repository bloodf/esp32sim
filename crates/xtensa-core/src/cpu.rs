//! Xtensa LX6 CPU interpreter, configured like the ESP32 cores.

use crate::decode::{decode, insn_len, Insn, Op, B4CONST, B4CONSTU};

// ---- special register numbers
pub const LBEG: usize = 0;
pub const LEND: usize = 1;
pub const LCOUNT: usize = 2;
pub const SAR: usize = 3;
pub const BR: usize = 4;
pub const LITBASE: usize = 5;
pub const SCOMPARE1: usize = 12;
pub const ACCLO: usize = 16;
pub const ACCHI: usize = 17;
pub const WINDOWBASE: usize = 72;
pub const WINDOWSTART: usize = 73;
pub const EPC1: usize = 177;
pub const DEPC: usize = 192;
pub const EPS2: usize = 194;
pub const EXCSAVE1: usize = 209;
pub const CPENABLE: usize = 224;
pub const INTERRUPT: usize = 226;
pub const INTCLEAR: usize = 227;
pub const INTENABLE: usize = 228;
pub const PS: usize = 230;
pub const VECBASE: usize = 231;
pub const EXCCAUSE: usize = 232;
pub const DEBUGCAUSE: usize = 233;
pub const CCOUNT: usize = 234;
pub const PRID: usize = 235;
pub const ICOUNT: usize = 236;
pub const ICOUNTLEVEL: usize = 237;
pub const EXCVADDR: usize = 238;
pub const CCOMPARE0: usize = 240;

// ---- PS fields
pub const PS_INTLEVEL: u32 = 0xF;
pub const PS_EXCM: u32 = 1 << 4;
pub const PS_UM: u32 = 1 << 5;
pub const PS_RING: u32 = 3 << 6;
pub const PS_OWB_SHIFT: u32 = 8;
pub const PS_OWB: u32 = 0xF << 8;
pub const PS_CALLINC_SHIFT: u32 = 16;
pub const PS_CALLINC: u32 = 3 << 16;
pub const PS_WOE: u32 = 1 << 18;

// ---- exception causes
pub const EXC_ILLEGAL: u32 = 0;
pub const EXC_SYSCALL: u32 = 1;
pub const EXC_IFETCH_ERROR: u32 = 2;
pub const EXC_LOADSTORE_ERROR: u32 = 3;
pub const EXC_LEVEL1_INT: u32 = 4;
pub const EXC_ALLOCA: u32 = 5;
pub const EXC_DIVIDE_BY_ZERO: u32 = 6;
pub const EXC_PRIVILEGED: u32 = 8;
pub const EXC_UNALIGNED: u32 = 9;
pub const EXC_INST_FETCH_PROHIBITED: u32 = 20;
pub const EXC_LOAD_PROHIBITED: u32 = 28;
pub const EXC_STORE_PROHIBITED: u32 = 29;
pub const EXC_COPROCESSOR0_DISABLED: u32 = 32;

// ---- vector offsets (relative to VECBASE)
pub const VEC_WINDOW_OVERFLOW4: u32 = 0x00;
pub const VEC_WINDOW_UNDERFLOW4: u32 = 0x40;
pub const VEC_WINDOW_OVERFLOW8: u32 = 0x80;
pub const VEC_WINDOW_UNDERFLOW8: u32 = 0xC0;
pub const VEC_WINDOW_OVERFLOW12: u32 = 0x100;
pub const VEC_WINDOW_UNDERFLOW12: u32 = 0x140;
pub const VEC_LEVEL: [u32; 8] = [0, 0, 0x180, 0x1C0, 0x200, 0x240, 0x280, 0x2C0];
pub const VEC_KERNEL: u32 = 0x300;
pub const VEC_USER: u32 = 0x340;
pub const VEC_DOUBLE: u32 = 0x3C0;

pub const EXCM_LEVEL: u32 = 3;
pub const DEBUG_LEVEL: u32 = 6;

/// Priority level of each of the 32 CPU interrupts (ESP32 configuration).
pub const INT_LEVEL: [u8; 32] = [
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 1, 1, 7, 3, 5, 1, 1, 2, 2, 2, 3, 3, 4, 4, 5, 3, 4, 3, 4, 5,
];
/// Edge-triggered external interrupts.
pub const INT_EDGE_MASK: u32 = (1 << 10) | (1 << 22) | (1 << 28) | (1 << 30);
/// Software interrupts (set via INTSET).
pub const INT_SOFT_MASK: u32 = (1 << 7) | (1 << 29);
/// CCOMPARE timer interrupts.
pub const INT_TIMER: [u32; 3] = [6, 15, 16];
pub const INT_TIMER_MASK: u32 = (1 << 6) | (1 << 15) | (1 << 16);
/// Interrupts that are not driven by the interrupt matrix.
pub const INT_INTERNAL_MASK: u32 = INT_SOFT_MASK | INT_TIMER_MASK | (1 << 11);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemFault;

/// Memory interface used by the CPU.
pub trait Bus {
    /// Read `size` (1, 2 or 4) bytes.  Alignment has already been checked.
    /// `cyc` is the accessing core's cycle counter (for sub-quantum timing).
    fn read(&mut self, addr: u32, size: u8, cyc: u64) -> Result<u32, MemFault>;
    fn write(&mut self, addr: u32, size: u8, val: u32, cyc: u64) -> Result<(), MemFault>;
    /// Fetch up to 4 instruction bytes starting at `addr` (little endian).
    fn fetch(&mut self, addr: u32) -> Result<u32, MemFault>;
    /// True when the system needs the scheduler to look at it before the next
    /// instruction (e.g. an interrupt line changed).  Ends `Cpu::run_until` early.
    #[inline(always)]
    fn wants_break(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepResult {
    /// Instruction executed (or exception/interrupt taken).
    Ok,
    /// PC is inside the HLE (ROM) region; the caller must emulate the function.
    Hle,
    /// CPU is waiting for an interrupt (WAITI).
    Waiting,
    /// A debugger breakpoint was hit (PC not advanced).
    Breakpoint,
    /// BREAK instruction executed while a debugger is attached.
    DebugBreak,
    /// The CPU is stalled / not running.
    Stalled,
}

const IRAM_LO: u32 = 0x4007_0000;
const IRAM_HI: u32 = 0x400C_2000;

#[derive(Clone, Copy)]
struct CacheEntry {
    tag: u32,
    insn: Insn,
}

const ICACHE_BITS: u32 = 16;
const ICACHE_SIZE: usize = 1 << ICACHE_BITS;

#[derive(Clone, Debug, Default)]
pub struct ExceptionRecord {
    pub cause: u32,
    pub pc: u32,
    pub vaddr: u32,
}

pub struct Cpu {
    pub core_id: u32,
    pub ar: [u32; 64],
    pub pc: u32,
    pub sr: [u32; 256],
    pub fr: [u32; 16],
    pub fcr: u32,
    pub fsr: u32,
    pub threadptr: u32,
    /// Pending internal interrupt bits (software, timer, latched edge interrupts).
    pub irq_int: u32,
    /// Level of the external interrupt lines coming from the interrupt matrix.
    pub irq_ext: u32,
    pub waiting: bool,
    pub stalled: bool,
    pub cycles: u64,
    /// cycles until the next CCOMPARE match
    timer_countdown: u32,
    icache: Vec<CacheEntry>,
    /// 256-byte IRAM pages that have had instructions decoded (stores there invalidate the cache).
    code_pages: Vec<bool>,
    /// [lo, hi) address range handled by high-level emulation.
    pub hle_lo: u32,
    pub hle_hi: u32,
    /// Base of guest-callback return traps (offsets 0, 8, 16 trap to HLE).
    pub hle_extra: u32,
    pub breakpoints: Vec<u32>,
    skip_bp: bool,
    pub debugger_attached: bool,
    pub last_exception: Option<ExceptionRecord>,
    /// Number of instructions executed.
    pub icount: u64,
    /// Set when an ILL / unknown instruction is hit; helps diagnostics.
    pub last_illegal: Option<(u32, u32)>,
    /// Ring buffer of recent PCs for diagnostics.
    pub trace: Vec<u32>,
    pub trace_pos: usize,
    pub trace_enabled: bool,
    /// Stop execution (StepResult::Breakpoint) right after an exception is raised.
    pub break_on_exception: bool,
    exc_break: bool,
}

impl Cpu {
    pub fn new(core_id: u32) -> Cpu {
        let mut c = Cpu {
            core_id,
            ar: [0; 64],
            pc: 0,
            sr: [0; 256],
            fr: [0; 16],
            fcr: 0,
            fsr: 0,
            threadptr: 0,
            irq_int: 0,
            irq_ext: 0,
            waiting: false,
            stalled: false,
            cycles: 0,
            timer_countdown: u32::MAX,
            icache: vec![
                CacheEntry { tag: 1, insn: decode(0) };
                ICACHE_SIZE
            ],
            code_pages: vec![false; ((IRAM_HI - IRAM_LO) >> 8) as usize + 1],
            hle_lo: 0x4000_0000,
            hle_hi: 0x4007_0000,
            hle_extra: 0x4007_FF03,
            breakpoints: Vec::new(),
            skip_bp: false,
            debugger_attached: false,
            last_exception: None,
            icount: 0,
            last_illegal: None,
            trace: vec![0; 64],
            trace_pos: 0,
            trace_enabled: false,
            break_on_exception: false,
            exc_break: false,
        };
        c.reset();
        c
    }

    pub fn reset(&mut self) {
        self.ar = [0; 64];
        self.sr = [0; 256];
        self.sr[PS] = 0x1F; // INTLEVEL=15, EXCM=1
        self.sr[WINDOWBASE] = 0;
        self.sr[WINDOWSTART] = 1;
        self.sr[VECBASE] = 0x4000_0000;
        self.sr[PRID] = if self.core_id == 0 { 0xCDCD } else { 0xABAB };
        self.sr[ATOMCTL_SR] = 0x28;
        self.pc = 0x4000_0400;
        self.irq_int = 0;
        self.irq_ext = 0;
        self.waiting = false;
        self.flush_icache();
        self.recompute_timer();
    }

    pub fn flush_icache(&mut self) {
        for e in self.icache.iter_mut() {
            e.tag = 1;
        }
    }

    /// Invalidate decoded instructions that may overlap `addr`.
    pub fn invalidate_code(&mut self, addr: u32, len: u32) {
        let start = addr.wrapping_sub(3);
        let end = addr.wrapping_add(len);
        let mut a = start;
        while a != end {
            let idx = Self::cache_index(a);
            if self.icache[idx].tag == a {
                self.icache[idx].tag = 1;
            }
            a = a.wrapping_add(1);
        }
    }

    #[inline(always)]
    fn cache_index(pc: u32) -> usize {
        ((pc ^ (pc >> ICACHE_BITS)) & (ICACHE_SIZE as u32 - 1)) as usize
    }

    // ---------------------------------------------------------------- registers
    #[inline(always)]
    pub fn wb(&self) -> u32 {
        self.sr[WINDOWBASE]
    }
    #[inline(always)]
    pub fn ar(&self, i: u8) -> u32 {
        self.ar[(((self.sr[WINDOWBASE] << 2) + i as u32) & 63) as usize]
    }
    #[inline(always)]
    pub fn set_ar(&mut self, i: u8, v: u32) {
        self.ar[(((self.sr[WINDOWBASE] << 2) + i as u32) & 63) as usize] = v;
    }
    #[inline(always)]
    pub fn ps(&self) -> u32 {
        self.sr[PS]
    }
    pub fn ccount(&self) -> u32 {
        self.sr[CCOUNT]
    }
    pub fn intenable(&self) -> u32 {
        self.sr[INTENABLE]
    }
    pub fn interrupt(&self) -> u32 {
        self.irq_int | (self.irq_ext & !INT_EDGE_MASK & !INT_INTERNAL_MASK)
    }

    /// Update external interrupt lines (from interrupt matrix).  Rising edges on
    /// edge-triggered interrupts are latched.
    pub fn set_irq_lines(&mut self, lines: u32) {
        let rising = lines & !self.irq_ext & INT_EDGE_MASK;
        self.irq_int |= rising;
        self.irq_ext = lines;
    }

    fn windowstart_bit(&self, wb: u32) -> bool {
        self.sr[WINDOWSTART] & (1 << (wb & 15)) != 0
    }

    fn rotate_window(&mut self, n: i32) {
        self.sr[WINDOWBASE] = ((self.sr[WINDOWBASE] as i32 + n) & 15) as u32;
    }

    #[inline(always)]
    fn cwoe(&self) -> bool {
        (self.sr[PS] & (PS_WOE | PS_EXCM)) == PS_WOE
    }

    /// Number of free register groups above the current window (0..3).
    #[inline(always)]
    fn free_groups(&self) -> u32 {
        let ws = self.sr[WINDOWSTART] & 0xFFFF;
        let rep = ws | (ws << 16);
        let sh = rep >> (self.sr[WINDOWBASE] + 1);
        (sh | 8).trailing_zeros()
    }

    /// Raise a window-overflow exception for the oldest live frame.
    fn window_overflow(&mut self, pc: u32) {
        let wb = self.sr[WINDOWBASE];
        let ws = self.sr[WINDOWSTART] & 0xFFFF;
        let rep = (ws | (ws << 16)) >> (wb + 1);
        let n = rep.trailing_zeros() + 1;
        self.rotate_window(n as i32);
        self.sr[PS] = (self.sr[PS] & !PS_OWB) | (wb << PS_OWB_SHIFT) | PS_EXCM;
        self.sr[EPC1] = pc;
        let vec = match (rep >> n).trailing_zeros() {
            0 => VEC_WINDOW_OVERFLOW4,
            1 => VEC_WINDOW_OVERFLOW8,
            _ => VEC_WINDOW_OVERFLOW12,
        };
        self.pc = self.sr[VECBASE].wrapping_add(vec);
    }

    // ---------------------------------------------------------------- timers
    fn recompute_timer(&mut self) {
        let cc = self.sr[CCOUNT];
        let mut m = u32::MAX;
        for i in 0..3 {
            let d = self.sr[CCOMPARE0 + i].wrapping_sub(cc);
            let d = if d == 0 { u32::MAX } else { d };
            m = m.min(d);
        }
        self.timer_countdown = m;
    }

    /// Advance the cycle counter by `n` cycles (used for instruction execution and idle skipping).
    #[inline(always)]
    pub fn advance_cycles(&mut self, n: u32) {
        self.cycles += n as u64;
        let old = self.sr[CCOUNT];
        self.sr[CCOUNT] = old.wrapping_add(n);
        if n >= self.timer_countdown {
            // one or more comparators matched in (old, old+n]
            for i in 0..3 {
                let d = self.sr[CCOMPARE0 + i].wrapping_sub(old);
                if d != 0 && d <= n {
                    self.irq_int |= 1 << INT_TIMER[i];
                }
            }
            self.recompute_timer();
        } else {
            self.timer_countdown -= n;
        }
    }

    /// Cycles until the next CCOMPARE match (for idle fast-forward).
    pub fn cycles_to_timer(&self) -> u32 {
        self.timer_countdown
    }

    // ---------------------------------------------------------------- special regs
    pub fn read_sr(&self, n: u32) -> u32 {
        let n = n as usize;
        match n {
            INTERRUPT => self.interrupt(),
            BR => self.sr[BR] & 0xFFFF,
            _ => self.sr[n],
        }
    }

    pub fn write_sr(&mut self, n: u32, v: u32) {
        let n = n as usize;
        match n {
            WINDOWBASE => self.sr[n] = v & 15,
            WINDOWSTART => self.sr[n] = v & 0xFFFF,
            PS => self.sr[n] = v & (PS_INTLEVEL | PS_EXCM | PS_UM | PS_RING | PS_OWB | PS_CALLINC | PS_WOE),
            INTERRUPT => {
                // INTSET: only software interrupts can be set
                self.irq_int |= v & INT_SOFT_MASK;
            }
            INTCLEAR => {
                self.irq_int &= !(v & (INT_SOFT_MASK | INT_EDGE_MASK));
            }
            CCOUNT => {
                self.sr[n] = v;
                self.recompute_timer();
            }
            PRID => {}
            240 | 241 | 242 => {
                self.sr[n] = v;
                self.irq_int &= !(1 << INT_TIMER[n - 240]);
                self.recompute_timer();
            }
            SAR => self.sr[n] = v & 0x3F,
            LITBASE => self.sr[n] = v & 0xFFFF_F001,
            EXCCAUSE => self.sr[n] = v & 0x3F,
            _ => self.sr[n] = v,
        }
    }

    // ---------------------------------------------------------------- exceptions
    /// Raise a general exception at the current PC.
    pub fn exception(&mut self, cause: u32, vaddr: Option<u32>) {
        self.last_exception = Some(ExceptionRecord { cause, pc: self.pc, vaddr: vaddr.unwrap_or(0) });
        if self.break_on_exception && cause != EXC_COPROCESSOR0_DISABLED {
            self.exc_break = true;
        }
        self.sr[EXCCAUSE] = cause;
        if let Some(v) = vaddr {
            self.sr[EXCVADDR] = v;
        }
        let vb = self.sr[VECBASE];
        if self.sr[PS] & PS_EXCM != 0 {
            self.sr[DEPC] = self.pc;
            self.pc = vb.wrapping_add(VEC_DOUBLE);
        } else {
            self.sr[EPC1] = self.pc;
            let user = self.sr[PS] & PS_UM != 0;
            self.sr[PS] |= PS_EXCM;
            self.pc = vb.wrapping_add(if user { VEC_USER } else { VEC_KERNEL });
        }
    }

    fn debug_exception(&mut self, cause_bits: u32) {
        let lvl = DEBUG_LEVEL as usize;
        if (self.sr[PS] & PS_INTLEVEL) >= DEBUG_LEVEL {
            return;
        }
        self.sr[EPC1 + lvl - 1] = self.pc;
        self.sr[EPS2 + lvl - 2] = self.sr[PS];
        self.sr[DEBUGCAUSE] = cause_bits;
        self.sr[PS] = (self.sr[PS] & !PS_INTLEVEL) | DEBUG_LEVEL | PS_EXCM;
        self.pc = self.sr[VECBASE].wrapping_add(VEC_LEVEL[lvl]);
    }

    /// Check for and take a pending interrupt.  Returns true if one was taken.
    #[inline]
    pub fn check_interrupts(&mut self) -> bool {
        let pending = self.interrupt() & self.sr[INTENABLE];
        if pending == 0 {
            return false;
        }
        let ps = self.sr[PS];
        let mut cur = ps & PS_INTLEVEL;
        if ps & PS_EXCM != 0 && cur < EXCM_LEVEL {
            cur = EXCM_LEVEL;
        }
        let mut best = 0u32;
        let mut p = pending;
        while p != 0 {
            let i = p.trailing_zeros();
            p &= p - 1;
            let l = INT_LEVEL[i as usize] as u32;
            if l > best {
                best = l;
            }
        }
        if best <= cur && best != 7 {
            return false;
        }
        self.waiting = false;
        let vb = self.sr[VECBASE];
        if best == 1 {
            self.sr[EXCCAUSE] = EXC_LEVEL1_INT;
            self.sr[EPC1] = self.pc;
            let user = ps & PS_UM != 0;
            self.sr[PS] |= PS_EXCM;
            self.pc = vb.wrapping_add(if user { VEC_USER } else { VEC_KERNEL });
        } else {
            let l = best as usize;
            self.sr[EPC1 + l - 1] = self.pc;
            self.sr[EPS2 + l - 2] = ps;
            self.sr[PS] = (ps & !PS_INTLEVEL) | best | PS_EXCM;
            self.pc = vb.wrapping_add(VEC_LEVEL[l]);
        }
        true
    }

    /// Whether a pending interrupt could wake the CPU from WAITI.
    pub fn has_wakeup_interrupt(&self) -> bool {
        let pending = self.interrupt() & self.sr[INTENABLE];
        if pending == 0 {
            return false;
        }
        let cur = self.sr[PS] & PS_INTLEVEL;
        let mut p = pending;
        while p != 0 {
            let i = p.trailing_zeros();
            p &= p - 1;
            if INT_LEVEL[i as usize] as u32 > cur {
                return true;
            }
        }
        false
    }

    // ---------------------------------------------------------------- memory helpers
    #[inline(always)]
    fn load<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u8) -> Option<u32> {
        if size > 1 && addr & (size as u32 - 1) != 0 {
            self.exception(EXC_UNALIGNED, Some(addr));
            return None;
        }
        match bus.read(addr, size, self.cycles) {
            Ok(v) => Some(v),
            Err(_) => {
                self.exception(EXC_LOAD_PROHIBITED, Some(addr));
                None
            }
        }
    }

    #[inline(always)]
    fn store<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u8, val: u32) -> bool {
        if size > 1 && addr & (size as u32 - 1) != 0 {
            self.exception(EXC_UNALIGNED, Some(addr));
            return false;
        }
        if addr.wrapping_sub(IRAM_LO) < IRAM_HI - IRAM_LO {
            let pg = ((addr - IRAM_LO) >> 8) as usize;
            // an instruction starting up to 2 bytes earlier (previous page) may overlap
            if self.code_pages[pg] || (pg > 0 && self.code_pages[pg - 1]) {
                self.invalidate_code(addr, size as u32);
            }
        }
        match bus.write(addr, size, val, self.cycles) {
            Ok(()) => true,
            Err(_) => {
                self.exception(EXC_STORE_PROHIBITED, Some(addr));
                false
            }
        }
    }

    #[inline(never)]
    fn fill_icache<B: Bus>(&mut self, bus: &mut B, pc: u32, idx: usize) -> bool {
        match bus.fetch(pc) {
            Ok(w) => {
                if pc.wrapping_sub(IRAM_LO) < IRAM_HI - IRAM_LO {
                    self.code_pages[((pc - IRAM_LO) >> 8) as usize] = true;
                }
                self.icache[idx] = CacheEntry { tag: pc, insn: decode(w) };
                true
            }
            Err(_) => {
                self.exception(EXC_INST_FETCH_PROHIBITED, Some(pc));
                false
            }
        }
    }

    #[inline(always)]
    fn cp0_enabled(&mut self) -> bool {
        if self.sr[CPENABLE] & 1 == 0 {
            self.exception(EXC_COPROCESSOR0_DISABLED, None);
            false
        } else {
            true
        }
    }

    #[inline(always)]
    fn f(&self, i: u8) -> f32 {
        f32::from_bits(self.fr[i as usize])
    }
    #[inline(always)]
    fn set_f(&mut self, i: u8, v: f32) {
        self.fr[i as usize] = v.to_bits();
    }
    #[inline(always)]
    fn bit_b(&self, i: u8) -> bool {
        self.sr[BR] & (1 << i) != 0
    }
    #[inline(always)]
    fn set_b(&mut self, i: u8, v: bool) {
        if v {
            self.sr[BR] |= 1 << i;
        } else {
            self.sr[BR] &= !(1 << i);
        }
    }

    /// Perform a windowed-ABI style return to the caller (used by HLE functions).
    /// `callinc` is the caller's CALLn increment (from PS.CALLINC at entry).
    pub fn hle_return(&mut self, ret: Option<u32>, ret_hi: Option<u32>) {
        let callinc = (self.sr[PS] & PS_CALLINC) >> PS_CALLINC_SHIFT;
        let base = (callinc * 4) as u8;
        if let Some(v) = ret {
            self.set_ar(base + 2, v);
        }
        if let Some(v) = ret_hi {
            self.set_ar(base + 3, v);
        }
        let ra = self.ar(base);
        if callinc == 0 {
            self.pc = ra;
        } else {
            self.pc = (self.pc & 0xC000_0000) | (ra & 0x3FFF_FFFF);
        }
    }

    /// Argument `n` (0-based) of a function being entered via CALLn (before ENTRY).
    pub fn hle_arg<B: Bus>(&mut self, bus: &mut B, n: u32) -> u32 {
        let callinc = (self.sr[PS] & PS_CALLINC) >> PS_CALLINC_SHIFT;
        if n < 6 {
            self.ar((callinc * 4 + 2 + n) as u8)
        } else {
            // extra arguments live at the caller's stack pointer
            let sp = self.ar(1);
            bus.read(sp.wrapping_add((n - 6) * 4), 4, self.cycles).unwrap_or(0)
        }
    }

    fn record_trace(&mut self, pc: u32) {
        self.trace[self.trace_pos] = pc;
        self.trace_pos = (self.trace_pos + 1) % self.trace.len();
    }

    pub fn recent_pcs(&self) -> Vec<u32> {
        let n = self.trace.len();
        (0..n).map(|i| self.trace[(self.trace_pos + i) % n]).filter(|&p| p != 0).collect()
    }

    // ---------------------------------------------------------------- execution
    /// Execute one instruction (or take one interrupt).
    #[inline(always)]
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> StepResult {
        if self.stalled {
            return StepResult::Stalled;
        }
        if self.irq_int != 0 || self.irq_ext != 0 {
            if self.check_interrupts() {
                self.advance_cycles(1);
                return StepResult::Ok;
            }
        }
        if self.waiting {
            return StepResult::Waiting;
        }
        let pc = self.pc;
        if !self.breakpoints.is_empty() {
            if self.skip_bp {
                self.skip_bp = false;
            } else if self.breakpoints.contains(&pc) {
                self.skip_bp = true;
                return StepResult::Breakpoint;
            }
        }
        if (pc >= self.hle_lo && pc < self.hle_hi) || (pc.wrapping_sub(self.hle_extra) & !0x18) == 0 {
            return StepResult::Hle;
        }
        if self.trace_enabled {
            self.record_trace(pc);
        }
        let idx = Self::cache_index(pc);
        if self.icache[idx].tag != pc && !self.fill_icache(bus, pc, idx) {
            return StepResult::Ok;
        }
        let insn = self.icache[idx].insn;
        // lazy window overflow check
        if insn.wgroup != 0 && self.cwoe() && insn.wgroup as u32 > self.free_groups() {
            self.window_overflow(pc);
            self.advance_cycles(1);
            return StepResult::Ok;
        }
        self.icount += 1;
        self.advance_cycles(1);
        self.exec(bus, insn, pc)
    }

    /// Execute instructions until `cycles >= target` or a step returns something other than `Ok`.
    pub fn run_until<B: Bus>(&mut self, bus: &mut B, target: u64) -> StepResult {
        while self.cycles < target {
            let r = self.step(bus);
            if self.exc_break {
                self.exc_break = false;
                return StepResult::Breakpoint;
            }
            if r != StepResult::Ok || bus.wants_break() {
                return r;
            }
        }
        StepResult::Ok
    }

    /// Single-step ignoring breakpoints at the current PC.
    pub fn step_over_bp<B: Bus>(&mut self, bus: &mut B) -> StepResult {
        self.skip_bp = true;
        self.step(bus)
    }

    #[inline(always)]
    fn exec<B: Bus>(&mut self, bus: &mut B, i: Insn, pc: u32) -> StepResult {
        use Op::*;
        let (r, s, t) = (i.r, i.s, i.t);
        let next = pc.wrapping_add(i.len as u32);
        let mut npc = next;
        let mut fallthrough = true;
        macro_rules! branch {
            ($cond:expr) => {
                if $cond {
                    npc = pc.wrapping_add(4).wrapping_add(i.imm as u32);
                    fallthrough = false;
                }
            };
        }
        macro_rules! ld {
            ($addr:expr, $size:expr) => {
                match self.load(bus, $addr, $size) {
                    Some(v) => v,
                    None => return StepResult::Ok,
                }
            };
        }
        macro_rules! st {
            ($addr:expr, $size:expr, $v:expr) => {
                if !self.store(bus, $addr, $size, $v) {
                    return StepResult::Ok;
                }
            };
        }
        macro_rules! cp0 {
            () => {
                if !self.cp0_enabled() {
                    return StepResult::Ok;
                }
            };
        }
        match i.op {
            Nop | Cache | TlbNop => {}
            Ill => {
                self.last_illegal = Some((pc, i.raw()));
                self.exception(EXC_ILLEGAL, None);
                return StepResult::Ok;
            }
            Mac16 => {
                if !self.exec_mac16(bus, &i) {
                    self.last_illegal = Some((pc, i.raw()));
                    self.exception(EXC_ILLEGAL, None);
                    return StepResult::Ok;
                }
            }
            TlbRead => self.set_ar(t, 0),
            // ------------------------------------------------ system
            Break | BreakN => {
                if self.debugger_attached {
                    return StepResult::DebugBreak;
                }
                self.debug_exception(if i.op == Break { 1 << 3 } else { 1 << 4 });
                if self.pc != pc {
                    return StepResult::Ok;
                }
            }
            Syscall => {
                self.exception(EXC_SYSCALL, None);
                return StepResult::Ok;
            }
            Simcall => {
                self.set_ar(2, (-1i32) as u32);
            }
            Rsil => {
                let ps = self.sr[PS];
                self.set_ar(t, ps);
                self.sr[PS] = (ps & !PS_INTLEVEL) | s as u32;
            }
            Waiti => {
                self.sr[PS] = (self.sr[PS] & !PS_INTLEVEL) | s as u32;
                self.pc = npc;
                self.waiting = true;
                return StepResult::Waiting;
            }
            Rfe | Rfue => {
                self.sr[PS] &= !PS_EXCM;
                npc = self.sr[EPC1];
                fallthrough = false;
            }
            Rfde => {
                self.sr[PS] &= !PS_EXCM;
                npc = self.sr[DEPC];
                fallthrough = false;
            }
            Rfwo | Rfwu => {
                let wb = self.sr[WINDOWBASE];
                if i.op == Rfwo {
                    self.sr[WINDOWSTART] &= !(1 << wb);
                } else {
                    self.sr[WINDOWSTART] |= 1 << wb;
                }
                self.sr[WINDOWBASE] = (self.sr[PS] & PS_OWB) >> PS_OWB_SHIFT;
                self.sr[PS] &= !PS_EXCM;
                npc = self.sr[EPC1];
                fallthrough = false;
            }
            Rfi => {
                let l = s as usize;
                if (2..=7).contains(&l) {
                    npc = self.sr[EPC1 + l - 1];
                    let eps = self.sr[EPS2 + l - 2];
                    self.write_sr(PS as u32, eps);
                    fallthrough = false;
                } else {
                    self.exception(EXC_ILLEGAL, None);
                    return StepResult::Ok;
                }
            }
            Rfme => {
                self.exception(EXC_ILLEGAL, None);
                return StepResult::Ok;
            }
            Ret | RetN => {
                npc = self.ar(0);
                fallthrough = false;
            }
            Retw | RetwN => {
                let a0 = self.ar(0);
                let n = a0 >> 30;
                let wb = self.sr[WINDOWBASE];
                let m = if self.windowstart_bit(wb.wrapping_sub(1)) {
                    1
                } else if self.windowstart_bit(wb.wrapping_sub(2)) {
                    2
                } else if self.windowstart_bit(wb.wrapping_sub(3)) {
                    3
                } else {
                    0
                };
                if n == 0 || (m != 0 && m != n) || !self.cwoe() {
                    self.last_illegal = Some((pc, i.raw()));
                    self.exception(EXC_ILLEGAL, None);
                    return StepResult::Ok;
                }
                let ret_pc = (pc & 0xC000_0000) | (a0 & 0x3FFF_FFFF);
                self.rotate_window(-(n as i32));
                let nwb = self.sr[WINDOWBASE];
                if self.windowstart_bit(nwb) {
                    self.sr[WINDOWSTART] &= !(1 << wb);
                    npc = ret_pc;
                    fallthrough = false;
                } else {
                    // window underflow
                    self.sr[PS] = (self.sr[PS] & !PS_OWB) | (wb << PS_OWB_SHIFT) | PS_EXCM;
                    self.sr[EPC1] = pc;
                    let vec = match n {
                        1 => VEC_WINDOW_UNDERFLOW4,
                        2 => VEC_WINDOW_UNDERFLOW8,
                        _ => VEC_WINDOW_UNDERFLOW12,
                    };
                    self.pc = self.sr[VECBASE].wrapping_add(vec);
                    return StepResult::Ok;
                }
            }
            Jx => {
                npc = self.ar(s);
                fallthrough = false;
            }
            Callx0 => {
                let tgt = self.ar(s);
                self.set_ar(0, next);
                npc = tgt;
                fallthrough = false;
            }
            Callx4 | Callx8 | Callx12 => {
                let n: u32 = match i.op {
                    Callx4 => 1,
                    Callx8 => 2,
                    _ => 3,
                };
                let tgt = self.ar(s);
                self.set_ar((n * 4) as u8, (n << 30) | (next & 0x3FFF_FFFF));
                self.sr[PS] = (self.sr[PS] & !PS_CALLINC) | (n << PS_CALLINC_SHIFT);
                npc = tgt;
                fallthrough = false;
            }
            Call0 => {
                self.set_ar(0, next);
                npc = (pc & !3).wrapping_add(i.imm as u32).wrapping_add(4);
                fallthrough = false;
            }
            Call4 | Call8 | Call12 => {
                let n: u32 = match i.op {
                    Call4 => 1,
                    Call8 => 2,
                    _ => 3,
                };
                self.set_ar((n * 4) as u8, (n << 30) | (next & 0x3FFF_FFFF));
                self.sr[PS] = (self.sr[PS] & !PS_CALLINC) | (n << PS_CALLINC_SHIFT);
                npc = (pc & !3).wrapping_add(i.imm as u32).wrapping_add(4);
                fallthrough = false;
            }
            Entry => {
                let callinc = (self.sr[PS] & PS_CALLINC) >> PS_CALLINC_SHIFT;
                if s > 3 || !self.cwoe() {
                    self.last_illegal = Some((pc, i.raw()));
                    self.exception(EXC_ILLEGAL, None);
                    return StepResult::Ok;
                }
                if callinc > 0 && self.free_groups() < callinc {
                    self.window_overflow(pc);
                    return StepResult::Ok;
                }
                let v = self.ar(s).wrapping_sub(i.imm as u32);
                self.set_ar(((callinc << 2) | (s as u32 & 3)) as u8, v);
                self.rotate_window(callinc as i32);
                let wb = self.sr[WINDOWBASE];
                self.sr[WINDOWSTART] |= 1 << wb;
            }
            Movsp => {
                let wb = self.sr[WINDOWBASE];
                let ws = self.sr[WINDOWSTART];
                let mask = (1u32 << ((wb + 15) & 15)) | (1 << ((wb + 14) & 15)) | (1 << ((wb + 13) & 15));
                if ws & mask == 0 && self.sr[PS] & PS_WOE != 0 {
                    self.exception(EXC_ALLOCA, None);
                    return StepResult::Ok;
                }
                let v = self.ar(s);
                self.set_ar(t, v);
            }
            Rotw => {
                self.rotate_window(i.imm);
            }
            // ------------------------------------------------ ALU
            And => {
                let v = self.ar(s) & self.ar(t);
                self.set_ar(r, v)
            }
            Or => {
                let v = self.ar(s) | self.ar(t);
                self.set_ar(r, v)
            }
            Xor => {
                let v = self.ar(s) ^ self.ar(t);
                self.set_ar(r, v)
            }
            Add => {
                let v = self.ar(s).wrapping_add(self.ar(t));
                self.set_ar(r, v)
            }
            Addx2 => {
                let v = (self.ar(s) << 1).wrapping_add(self.ar(t));
                self.set_ar(r, v)
            }
            Addx4 => {
                let v = (self.ar(s) << 2).wrapping_add(self.ar(t));
                self.set_ar(r, v)
            }
            Addx8 => {
                let v = (self.ar(s) << 3).wrapping_add(self.ar(t));
                self.set_ar(r, v)
            }
            Sub => {
                let v = self.ar(s).wrapping_sub(self.ar(t));
                self.set_ar(r, v)
            }
            Subx2 => {
                let v = (self.ar(s) << 1).wrapping_sub(self.ar(t));
                self.set_ar(r, v)
            }
            Subx4 => {
                let v = (self.ar(s) << 2).wrapping_sub(self.ar(t));
                self.set_ar(r, v)
            }
            Subx8 => {
                let v = (self.ar(s) << 3).wrapping_sub(self.ar(t));
                self.set_ar(r, v)
            }
            Neg => {
                let v = (self.ar(t) as i32).wrapping_neg() as u32;
                self.set_ar(r, v)
            }
            Abs => {
                let v = (self.ar(t) as i32).wrapping_abs() as u32;
                self.set_ar(r, v)
            }
            Ssr => {
                self.sr[SAR] = self.ar(s) & 31;
            }
            Ssl => {
                self.sr[SAR] = 32 - (self.ar(s) & 31);
            }
            Ssa8l => {
                self.sr[SAR] = (self.ar(s) & 3) << 3;
            }
            Ssa8b => {
                self.sr[SAR] = 32 - ((self.ar(s) & 3) << 3);
            }
            Ssai => {
                self.sr[SAR] = i.imm as u32;
            }
            Rer => self.set_ar(t, 0),
            Wer => {}
            Nsa => {
                let v = self.ar(s) as i32;
                let x = if v < 0 { !v } else { v } as u32;
                let res = if x == 0 { 31 } else { x.leading_zeros() - 1 };
                self.set_ar(t, res);
            }
            Nsau => {
                let v = self.ar(s);
                self.set_ar(t, v.leading_zeros());
            }
            Slli => {
                let v = ((self.ar(s) as u64) << (i.imm as u32)) as u32;
                self.set_ar(r, v)
            }
            Srai => {
                let v = ((self.ar(t) as i32) >> (i.imm as u32)) as u32;
                self.set_ar(r, v)
            }
            Srli => {
                let v = self.ar(t) >> (i.imm as u32);
                self.set_ar(r, v)
            }
            Src => {
                let v = ((((self.ar(s) as u64) << 32) | self.ar(t) as u64) >> self.sr[SAR]) as u32;
                self.set_ar(r, v)
            }
            Srl => {
                let v = ((self.ar(t) as u64) >> self.sr[SAR]) as u32;
                self.set_ar(r, v)
            }
            Sll => {
                let v = (((self.ar(s) as u64) << 32) >> self.sr[SAR]) as u32;
                self.set_ar(r, v)
            }
            Sra => {
                let v = ((self.ar(t) as i32 as i64) >> self.sr[SAR]) as u32;
                self.set_ar(r, v)
            }
            Mul16u => {
                let v = (self.ar(s) & 0xFFFF) * (self.ar(t) & 0xFFFF);
                self.set_ar(r, v)
            }
            Mul16s => {
                let v = (self.ar(s) as i16 as i32).wrapping_mul(self.ar(t) as i16 as i32) as u32;
                self.set_ar(r, v)
            }
            Andb => {
                let v = self.bit_b(s) && self.bit_b(t);
                self.set_b(r, v)
            }
            Andbc => {
                let v = self.bit_b(s) && !self.bit_b(t);
                self.set_b(r, v)
            }
            Orb => {
                let v = self.bit_b(s) || self.bit_b(t);
                self.set_b(r, v)
            }
            Orbc => {
                let v = self.bit_b(s) || !self.bit_b(t);
                self.set_b(r, v)
            }
            Xorb => {
                let v = self.bit_b(s) ^ self.bit_b(t);
                self.set_b(r, v)
            }
            Any4 | All4 | Any8 | All8 => {
                let n = if matches!(i.op, Any4 | All4) { 4 } else { 8 };
                let bits = (self.sr[BR] >> s) & ((1 << n) - 1);
                let v = if matches!(i.op, Any4 | Any8) { bits != 0 } else { bits == (1 << n) - 1 };
                self.set_b(t, v);
            }
            Mull => {
                let v = self.ar(s).wrapping_mul(self.ar(t));
                self.set_ar(r, v)
            }
            Muluh => {
                let v = ((self.ar(s) as u64 * self.ar(t) as u64) >> 32) as u32;
                self.set_ar(r, v)
            }
            Mulsh => {
                let v = ((self.ar(s) as i32 as i64 * self.ar(t) as i32 as i64) >> 32) as u32;
                self.set_ar(r, v)
            }
            Quou | Quos | Remu | Rems => {
                let a = self.ar(s);
                let b = self.ar(t);
                if b == 0 {
                    self.exception(EXC_DIVIDE_BY_ZERO, None);
                    return StepResult::Ok;
                }
                let v = match i.op {
                    Quou => a / b,
                    Remu => a % b,
                    Quos => (a as i32).wrapping_div(b as i32) as u32,
                    _ => (a as i32).wrapping_rem(b as i32) as u32,
                };
                self.set_ar(r, v)
            }
            Rsr => {
                let n = i.imm as u32;
                let v = if n == 234 { self.sr[CCOUNT] } else { self.read_sr(n) };
                self.set_ar(t, v);
            }
            Wsr => {
                let v = self.ar(t);
                self.write_sr(i.imm as u32, v);
            }
            Xsr => {
                let n = i.imm as u32;
                let old = self.read_sr(n);
                let v = self.ar(t);
                self.write_sr(n, v);
                self.set_ar(t, old);
            }
            Sext => {
                let bits = i.imm as u32 + 1;
                let sh = 32 - bits;
                let v = (((self.ar(s) << sh) as i32) >> sh) as u32;
                self.set_ar(r, v)
            }
            Clamps => {
                let b = i.imm as u32;
                let lo = -(1i64 << b);
                let hi = (1i64 << b) - 1;
                let v = (self.ar(s) as i32 as i64).clamp(lo, hi) as i32 as u32;
                self.set_ar(r, v)
            }
            Min => {
                let v = (self.ar(s) as i32).min(self.ar(t) as i32) as u32;
                self.set_ar(r, v)
            }
            Max => {
                let v = (self.ar(s) as i32).max(self.ar(t) as i32) as u32;
                self.set_ar(r, v)
            }
            Minu => {
                let v = self.ar(s).min(self.ar(t));
                self.set_ar(r, v)
            }
            Maxu => {
                let v = self.ar(s).max(self.ar(t));
                self.set_ar(r, v)
            }
            Moveqz => {
                if self.ar(t) == 0 {
                    let v = self.ar(s);
                    self.set_ar(r, v)
                }
            }
            Movnez => {
                if self.ar(t) != 0 {
                    let v = self.ar(s);
                    self.set_ar(r, v)
                }
            }
            Movltz => {
                if (self.ar(t) as i32) < 0 {
                    let v = self.ar(s);
                    self.set_ar(r, v)
                }
            }
            Movgez => {
                if (self.ar(t) as i32) >= 0 {
                    let v = self.ar(s);
                    self.set_ar(r, v)
                }
            }
            Movf => {
                if !self.bit_b(t) {
                    let v = self.ar(s);
                    self.set_ar(r, v)
                }
            }
            Movt => {
                if self.bit_b(t) {
                    let v = self.ar(s);
                    self.set_ar(r, v)
                }
            }
            Rur => {
                let n = i.imm as u32;
                let v = match n {
                    231 => self.threadptr,
                    232 => {
                        cp0!();
                        self.fcr
                    }
                    233 => {
                        cp0!();
                        self.fsr
                    }
                    _ => 0,
                };
                self.set_ar(r, v);
            }
            Wur => {
                let n = i.imm as u32;
                let v = self.ar(t);
                match n {
                    231 => self.threadptr = v,
                    232 => {
                        cp0!();
                        self.fcr = v & 0xFFF;
                    }
                    233 => {
                        cp0!();
                        self.fsr = v & 0xFFFF_FF80;
                    }
                    _ => {}
                }
            }
            Extui => {
                let shift = (i.imm & 0xFF) as u32;
                let bits = (i.imm >> 8) as u32;
                let v = (self.ar(t) >> shift) & ((1u32 << bits) - 1);
                self.set_ar(r, v)
            }
            L32e => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = ld!(a, 4);
                self.set_ar(t, v)
            }
            S32e => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = self.ar(t);
                st!(a, 4, v)
            }
            // ------------------------------------------------ floating point
            AddS | SubS | MulS | MaddS | MsubS => {
                cp0!();
                let (a, b) = (self.f(s), self.f(t));
                let v = match i.op {
                    AddS => a + b,
                    SubS => a - b,
                    MulS => a * b,
                    MaddS => a.mul_add(b, self.f(r)),
                    _ => (-a).mul_add(b, self.f(r)),
                };
                self.set_f(r, v)
            }
            MaddnS | DivnS | ConstS | Div0S | Recip0S | Sqrt0S | Rsqrt0S | Nexp01S | MksadjS | MkdadjS | AddexpS | AddexpmS => {
                cp0!();
                self.exec_fp_divsqrt(&i);
            }
            RoundS | TruncS | FloorS | CeilS => {
                cp0!();
                let x = self.f(s) as f64 * (1u64 << t) as f64;
                let y = match i.op {
                    RoundS => x.round_ties_even(),
                    TruncS => x.trunc(),
                    FloorS => x.floor(),
                    _ => x.ceil(),
                };
                let v = if y.is_nan() {
                    0x7FFF_FFFF
                } else if y >= 2147483647.0 {
                    0x7FFF_FFFF
                } else if y <= -2147483648.0 {
                    0x8000_0000
                } else {
                    y as i32 as u32
                };
                self.set_ar(r, v)
            }
            UtruncS => {
                cp0!();
                let x = (self.f(s) as f64 * (1u64 << t) as f64).trunc();
                let v = if x.is_nan() {
                    0xFFFF_FFFF
                } else if x < 0.0 {
                    if x <= -2147483648.0 {
                        0x8000_0000
                    } else {
                        x as i32 as u32
                    }
                } else if x >= 4294967295.0 {
                    0xFFFF_FFFF
                } else {
                    x as u32
                };
                self.set_ar(r, v)
            }
            FloatS => {
                cp0!();
                let v = (self.ar(s) as i32 as f64 / (1u64 << t) as f64) as f32;
                self.set_f(r, v)
            }
            UfloatS => {
                cp0!();
                let v = (self.ar(s) as f64 / (1u64 << t) as f64) as f32;
                self.set_f(r, v)
            }
            MovS => {
                cp0!();
                self.fr[r as usize] = self.fr[s as usize];
            }
            AbsS => {
                cp0!();
                self.fr[r as usize] = self.fr[s as usize] & 0x7FFF_FFFF;
            }
            NegS => {
                cp0!();
                self.fr[r as usize] = self.fr[s as usize] ^ 0x8000_0000;
            }
            Rfr => {
                cp0!();
                let v = self.fr[s as usize];
                self.set_ar(r, v)
            }
            Wfr => {
                cp0!();
                self.fr[r as usize] = self.ar(s);
            }
            UnS | OeqS | UeqS | OltS | UltS | OleS | UleS => {
                cp0!();
                let (a, b) = (self.f(s), self.f(t));
                let un = a.is_nan() || b.is_nan();
                let v = match i.op {
                    UnS => un,
                    OeqS => !un && a == b,
                    UeqS => un || a == b,
                    OltS => !un && a < b,
                    UltS => un || a < b,
                    OleS => !un && a <= b,
                    _ => un || a <= b,
                };
                self.set_b(r, v)
            }
            MoveqzS | MovnezS | MovltzS | MovgezS => {
                cp0!();
                let c = self.ar(t) as i32;
                let take = match i.op {
                    MoveqzS => c == 0,
                    MovnezS => c != 0,
                    MovltzS => c < 0,
                    _ => c >= 0,
                };
                if take {
                    self.fr[r as usize] = self.fr[s as usize];
                }
            }
            MovfS | MovtS => {
                cp0!();
                let b = self.bit_b(t);
                if (i.op == MovtS) == b {
                    self.fr[r as usize] = self.fr[s as usize];
                }
            }
            Lsi | Lsiu => {
                cp0!();
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = ld!(a, 4);
                self.fr[t as usize] = v;
                if i.op == Lsiu {
                    self.set_ar(s, a);
                }
            }
            Ssi | Ssiu => {
                cp0!();
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = self.fr[t as usize];
                st!(a, 4, v);
                if i.op == Ssiu {
                    self.set_ar(s, a);
                }
            }
            Lsx | Lsxu => {
                cp0!();
                let a = self.ar(s).wrapping_add(self.ar(t));
                let v = ld!(a, 4);
                self.fr[r as usize] = v;
                if i.op == Lsxu {
                    self.set_ar(s, a);
                }
            }
            Ssx | Ssxu => {
                cp0!();
                let a = self.ar(s).wrapping_add(self.ar(t));
                let v = self.fr[r as usize];
                st!(a, 4, v);
                if i.op == Ssxu {
                    self.set_ar(s, a);
                }
            }
            // ------------------------------------------------ loads / stores
            L32r => {
                let lb = self.sr[LITBASE];
                let a = if lb & 1 != 0 {
                    (lb & 0xFFFF_F000).wrapping_add(i.imm as u32)
                } else {
                    (pc.wrapping_add(3) & !3).wrapping_add(i.imm as u32)
                };
                let v = ld!(a, 4);
                self.set_ar(t, v)
            }
            L8ui => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = ld!(a, 1);
                self.set_ar(t, v)
            }
            L16ui => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = ld!(a, 2);
                self.set_ar(t, v)
            }
            L16si => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = ld!(a, 2);
                self.set_ar(t, v as u16 as i16 as i32 as u32)
            }
            L32i | L32ai => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = ld!(a, 4);
                self.set_ar(t, v)
            }
            S8i => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = self.ar(t);
                st!(a, 1, v & 0xFF)
            }
            S16i => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = self.ar(t);
                st!(a, 2, v & 0xFFFF)
            }
            S32i | S32ri => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let v = self.ar(t);
                st!(a, 4, v)
            }
            S32c1i => {
                let a = self.ar(s).wrapping_add(i.imm as u32);
                let old = ld!(a, 4);
                if old == self.sr[SCOMPARE1] {
                    let v = self.ar(t);
                    st!(a, 4, v);
                }
                self.set_ar(t, old);
            }
            Movi => self.set_ar(t, i.imm as u32),
            Addi => {
                let v = self.ar(s).wrapping_add(i.imm as u32);
                self.set_ar(t, v)
            }
            Addmi => {
                let v = self.ar(s).wrapping_add(i.imm as u32);
                self.set_ar(t, v)
            }
            // ------------------------------------------------ branches
            J => {
                npc = pc.wrapping_add(4).wrapping_add(i.imm as u32);
                fallthrough = false;
            }
            Beqz => branch!(self.ar(s) == 0),
            Bnez => branch!(self.ar(s) != 0),
            Bltz => branch!((self.ar(s) as i32) < 0),
            Bgez => branch!((self.ar(s) as i32) >= 0),
            Beqi => branch!(self.ar(s) as i32 == B4CONST[r as usize]),
            Bnei => branch!(self.ar(s) as i32 != B4CONST[r as usize]),
            Blti => branch!((self.ar(s) as i32) < B4CONST[r as usize]),
            Bgei => branch!((self.ar(s) as i32) >= B4CONST[r as usize]),
            Bltui => branch!(self.ar(s) < B4CONSTU[r as usize]),
            Bgeui => branch!(self.ar(s) >= B4CONSTU[r as usize]),
            Bf => branch!(!self.bit_b(s)),
            Bt => branch!(self.bit_b(s)),
            Bnone => branch!(self.ar(s) & self.ar(t) == 0),
            Beq => branch!(self.ar(s) == self.ar(t)),
            Blt => branch!((self.ar(s) as i32) < (self.ar(t) as i32)),
            Bltu => branch!(self.ar(s) < self.ar(t)),
            Ball => branch!(!self.ar(s) & self.ar(t) == 0),
            Bany => branch!(self.ar(s) & self.ar(t) != 0),
            Bne => branch!(self.ar(s) != self.ar(t)),
            Bge => branch!((self.ar(s) as i32) >= (self.ar(t) as i32)),
            Bgeu => branch!(self.ar(s) >= self.ar(t)),
            Bnall => branch!(!self.ar(s) & self.ar(t) != 0),
            Bbc => branch!(self.ar(s) & (1 << (self.ar(t) & 31)) == 0),
            Bbs => branch!(self.ar(s) & (1 << (self.ar(t) & 31)) != 0),
            Bbci => branch!(self.ar(s) & (1 << (((r & 1) << 4) | t)) == 0),
            Bbsi => branch!(self.ar(s) & (1 << (((r & 1) << 4) | t)) != 0),
            Loop | Loopnez | Loopgtz => {
                let v = self.ar(s);
                self.sr[LCOUNT] = v.wrapping_sub(1);
                self.sr[LBEG] = next;
                let lend = pc.wrapping_add(4).wrapping_add(i.imm as u32);
                self.sr[LEND] = lend;
                let skip = match i.op {
                    Loopnez => v == 0,
                    Loopgtz => (v as i32) <= 0,
                    _ => false,
                };
                if skip {
                    npc = lend;
                    fallthrough = false;
                }
            }
        }
        // zero-overhead loop
        if fallthrough && npc == self.sr[LEND] && self.sr[LCOUNT] != 0 && self.sr[PS] & PS_EXCM == 0 {
            self.sr[LCOUNT] = self.sr[LCOUNT].wrapping_sub(1);
            npc = self.sr[LBEG];
        }
        self.pc = npc;
        StepResult::Ok
    }

    /// ESP32 FPU division / square-root helper instructions.
    ///
    /// These are only meaningful as part of the compiler's canonical
    /// `__divsf3` / `sqrtf` sequences.  The exponent adjustment produced by
    /// MKDADJ/MKSADJ is carried in a private (signalling-NaN) encoding that
    /// ADDEXP/ADDEXPM understand, which yields correctly rounded results for
    /// all normal outcomes and IEEE special cases.
    fn exec_fp_divsqrt(&mut self, i: &Insn) {
        use Op::*;
        let (r, s, t) = (i.r, i.s, i.t);
        let fs = self.f(s);
        match i.op {
            MaddnS => {
                let v = self.f(s).mul_add(self.f(t), self.f(r));
                self.set_f(r, v)
            }
            DivnS => {
                let ft = self.f(t);
                let fr = self.f(r);
                // zero correction term (special operands) or an already overflowed
                // quotient: keep fr; otherwise fused final correction step.
                if ft != 0.0 && fr.is_finite() && ft.is_finite() {
                    let v = self.f(s).mul_add(ft, fr);
                    self.set_f(r, v)
                }
            }
            ConstS => {
                let v = match i.imm {
                    0 => 0.0f32,
                    1 => 1.0,
                    2 => 2.0,
                    3 => 0.5,
                    _ => 0.0,
                };
                self.set_f(r, v)
            }
            Div0S => {
                let n = fp_norm(fs);
                self.set_f(r, (1.0f64 / n as f64) as f32)
            }
            Recip0S => self.set_f(r, (1.0f64 / fs as f64) as f32),
            Sqrt0S => {
                let n = fp_norm(fs);
                self.set_f(r, (1.0f64 / (n as f64).sqrt()) as f32)
            }
            Rsqrt0S => self.set_f(r, (1.0f64 / (fs as f64).sqrt()) as f32),
            Nexp01S => {
                let n = fp_norm(fs);
                self.set_f(r, -n)
            }
            MkdadjS => {
                // fr = divisor (b), fs = dividend (a)
                let b = self.f(r);
                let a = fs;
                let neg = a.is_sign_negative() != b.is_sign_negative();
                let v = if a.is_nan() || b.is_nan() {
                    FP_TAG_SPECIAL | 4
                } else if a.is_infinite() {
                    if b.is_infinite() {
                        FP_TAG_SPECIAL | 4
                    } else {
                        FP_TAG_SPECIAL | if neg { 3 } else { 2 }
                    }
                } else if b.is_infinite() {
                    FP_TAG_SPECIAL | if neg { 1 } else { 0 }
                } else if b == 0.0 {
                    if a == 0.0 {
                        FP_TAG_SPECIAL | 4
                    } else {
                        FP_TAG_SPECIAL | if neg { 3 } else { 2 }
                    }
                } else if a == 0.0 {
                    FP_TAG_SPECIAL | if neg { 1 } else { 0 }
                } else {
                    let d = fp_norm_exp(a) - fp_norm_exp(b);
                    FP_TAG_SCALE | ((d + 0x8000) as u32 & 0xFFFF)
                };
                self.fr[r as usize] = v;
            }
            MksadjS => {
                let x = fs;
                let v = if x.is_nan() {
                    FP_TAG_SPECIAL | 4
                } else if x == 0.0 {
                    FP_TAG_SPECIAL | if x.is_sign_negative() { 1 } else { 0 }
                } else if x < 0.0 {
                    FP_TAG_SPECIAL | 4
                } else if x.is_infinite() {
                    FP_TAG_SPECIAL | 2
                } else {
                    let k = fp_norm_exp(x) / 2;
                    FP_TAG_SCALE | ((k + 0x8000) as u32 & 0xFFFF)
                };
                self.fr[r as usize] = v;
            }
            AddexpS | AddexpmS => {
                let bits = self.fr[s as usize];
                let cur = self.f(r);
                let v = if bits & 0xFFF0_0000 == FP_TAG_SCALE {
                    let d = (bits & 0xFFFF) as i32 - 0x8000;
                    fp_scale(cur, d)
                } else if bits & 0xFFF0_0000 == FP_TAG_SPECIAL {
                    if i.op == AddexpS {
                        0.0
                    } else {
                        match bits & 0xF {
                            0 => 0.0,
                            1 => -0.0,
                            2 => f32::INFINITY,
                            3 => f32::NEG_INFINITY,
                            _ => f32::NAN,
                        }
                    }
                } else {
                    // plain float operand: scale by its exponent
                    let (_, e) = fp_frexp(fs);
                    fp_scale(cur, e)
                };
                self.set_f(r, v)
            }
            _ => {}
        }
    }

    /// MAC16 option: implements the multiply/accumulate forms without loads.
    fn exec_mac16<B: Bus>(&mut self, bus: &mut B, i: &Insn) -> bool {
        let w = i.raw();
        let op2 = (w >> 20) & 0xF;
        let op1 = (w >> 16) & 0xF;
        let r = i.r;
        let s = i.s;
        let t = i.t;
        // op2: 0 MACID, 1 MACCD? 2 MACDD, 3 MACAD, 4 MACIA, 5 MACCA, 6 MACDA, 7 MACAA, 8 MACI, 9 MACC
        // op1[3:2] = operation: 0 MUL? (for MACxx): 01 = MUL, 10 = MULA, 11 = MULS; op1[1:0] = half selection
        let acc = ((self.sr[ACCHI] as u64 & 0xFF) << 32) | self.sr[ACCLO] as u64;
        let set_acc = |cpu: &mut Cpu, v: i64| {
            let v = v as u64 & 0xFF_FFFF_FFFF;
            cpu.sr[ACCLO] = v as u32;
            let hi = (v >> 32) as u32 & 0xFF;
            cpu.sr[ACCHI] = if hi & 0x80 != 0 { hi | 0xFFFF_FF00 } else { hi };
        };
        let acc_s = ((acc << 24) as i64) >> 24;
        match op2 {
            7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 => {
                // Identify operand sources
                let kind = op1 >> 2; // 0: (UMUL/LDINC etc), 1: MUL, 2: MULA, 3: MULS
                let halves = op1 & 3;
                let (x, y): (u32, u32) = match op2 {
                    7 => (self.ar(s), self.ar(t)), // MACAA: AR[s] * AR[t]
                    3 => (self.sr[32 + ((r as usize >> 2) & 1)], self.ar(t)), // MACAD? approximate
                    6 => (self.ar(s), self.sr[32 + 2 + ((t as usize >> 2) & 1)]),
                    2 => (self.sr[32 + ((r as usize >> 2) & 1)], self.sr[32 + 2 + ((t as usize >> 2) & 1)]),
                    _ => return false,
                };
                let xa = if halves & 1 != 0 { (x >> 16) as i16 } else { x as i16 } as i64;
                let yb = if halves & 2 != 0 { (y >> 16) as i16 } else { y as i16 } as i64;
                let p = xa * yb;
                match kind {
                    1 => set_acc(self, p),
                    2 => set_acc(self, acc_s + p),
                    3 => set_acc(self, acc_s - p),
                    0 if op2 == 7 => {
                        // UMUL.AA.*: unsigned 16x16 -> acc
                        let xu = if halves & 1 != 0 { x >> 16 } else { x & 0xFFFF } as i64;
                        let yu = if halves & 2 != 0 { y >> 16 } else { y & 0xFFFF } as i64;
                        set_acc(self, xu * yu)
                    }
                    _ => return false,
                }
                let _ = bus;
                true
            }
            9 => {
                // LDINC / LDDEC: op1 = 0 LDINC mw, as ; 4? LDDEC
                let a = if op1 == 0 { self.ar(s).wrapping_add(4) } else { self.ar(s).wrapping_sub(4) };
                match bus.read(a, 4, self.cycles) {
                    Ok(v) => {
                        self.sr[32 + ((r as usize >> 2) & 3)] = v;
                        self.set_ar(s, a);
                        true
                    }
                    Err(_) => false,
                }
            }
            _ => false,
        }
    }
}

pub const ATOMCTL_SR: usize = 99;

const FP_TAG_SCALE: u32 = 0x7FA0_0000;
const FP_TAG_SPECIAL: u32 = 0x7FB0_0000;

/// Split a float into mantissa in [1,2) and unbiased exponent (handles subnormals).
fn fp_frexp(x: f32) -> (f64, i32) {
    if x == 0.0 || !x.is_finite() {
        return (x as f64, 0);
    }
    let v = (x as f64).abs();
    let mut e = v.log2().floor() as i32;
    let mut m = v / 2f64.powi(e);
    if m >= 2.0 {
        m /= 2.0;
        e += 1;
    } else if m < 1.0 {
        m *= 2.0;
        e -= 1;
    }
    (if x < 0.0 { -m } else { m }, e)
}

/// Normalise to [1,4): the exponent is reduced to its parity (0 or 1).
fn fp_norm(x: f32) -> f32 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let (m, e) = fp_frexp(x);
    let p = e & 1;
    (m * if p == 1 { 2.0 } else { 1.0 }) as f32
}

/// Exponent removed by `fp_norm` (always even).
fn fp_norm_exp(x: f32) -> i32 {
    let (_, e) = fp_frexp(x);
    e - (e & 1)
}

fn fp_scale(v: f32, d: i32) -> f32 {
    ((v as f64) * 2f64.powi(d)) as f32
}

/// Helper for tests and tools: read the length of an instruction at `pc`.
pub fn length_at<B: Bus>(bus: &mut B, pc: u32) -> u8 {
    bus.fetch(pc).map(|w| insn_len(w as u8)).unwrap_or(3)
}

//! Small hand-assembled programs executed on the interpreter.  Encodings are
//! built with helpers and checked against the disassembler as well.
use xtensa_core::disasm::disasm;
use xtensa_core::{Bus, Cpu, MemFault, StepResult};

const BASE: u32 = 0x4008_0000;
const STACK: u32 = 0x3FFB_8000;

struct Mem {
    code: Vec<u8>,
    data: Vec<u8>,
}
impl Mem {
    fn slot(&mut self, a: u32) -> Result<(&mut Vec<u8>, usize), MemFault> {
        if (BASE..BASE + 0x1_0000).contains(&a) {
            Ok((&mut self.code, (a - BASE) as usize))
        } else if (STACK - 0x8000..STACK + 0x100).contains(&a) {
            Ok((&mut self.data, (a - (STACK - 0x8000)) as usize))
        } else {
            Err(MemFault)
        }
    }
}
impl Bus for Mem {
    fn read(&mut self, a: u32, size: u8, _c: u64) -> Result<u32, MemFault> {
        let (m, o) = self.slot(a)?;
        let mut v = 0u32;
        for i in 0..size as usize {
            v |= (m[o + i] as u32) << (8 * i);
        }
        Ok(v)
    }
    fn write(&mut self, a: u32, size: u8, val: u32, _c: u64) -> Result<(), MemFault> {
        let (m, o) = self.slot(a)?;
        for i in 0..size as usize {
            m[o + i] = (val >> (8 * i)) as u8;
        }
        Ok(())
    }
    fn fetch(&mut self, a: u32) -> Result<u32, MemFault> {
        self.read(a, 4, 0)
    }
}

/// Tiny assembler: pushes 24-bit words and records the disassembly for checks.
struct Asm {
    bytes: Vec<u8>,
}
impl Asm {
    fn pc(&self) -> u32 {
        BASE + self.bytes.len() as u32
    }
    fn w24(&mut self, w: u32) -> &mut Self {
        self.bytes.extend_from_slice(&w.to_le_bytes()[..3]);
        self
    }
    fn rrr(&mut self, op2: u32, op1: u32, r: u32, s: u32, t: u32) -> &mut Self {
        self.w24(op2 << 20 | op1 << 16 | r << 12 | s << 8 | t << 4)
    }
    fn movi(&mut self, t: u32, imm: i32) -> &mut Self {
        let i = imm as u32 & 0xFFF;
        self.w24((i & 0xFF) << 16 | 0xA << 12 | (i >> 8) << 8 | t << 4 | 2)
    }
    fn addi(&mut self, t: u32, s: u32, imm: i32) -> &mut Self {
        self.w24((imm as u32 & 0xFF) << 16 | 0xC << 12 | s << 8 | t << 4 | 2)
    }
    fn brk(&mut self) -> &mut Self {
        self.rrr(0, 0, 4, 1, 15)
    }
}

fn run(a: &Asm, max: usize) -> Cpu {
    let mut mem = Mem { code: vec![0; 0x1_0000], data: vec![0; 0x8100] };
    mem.code[..a.bytes.len()].copy_from_slice(&a.bytes);
    let mut cpu = Cpu::new(0);
    cpu.debugger_attached = true;
    cpu.pc = BASE;
    cpu.sr[230] = 0x0004_0000 | 0x20; // PS: WOE, UM
    cpu.set_ar(1, STACK);
    for _ in 0..max {
        match cpu.step(&mut mem) {
            StepResult::Ok => {}
            StepResult::DebugBreak => return cpu,
            r => panic!("unexpected {:?} at {:#x}", r, cpu.pc),
        }
    }
    panic!("program did not reach BREAK (pc {:#x})", cpu.pc);
}

fn dis(a: &Asm, off: usize) -> String {
    let b = &a.bytes[off..];
    let w = u32::from_le_bytes([b[0], b[1], b[2], *b.get(3).unwrap_or(&0)]);
    disasm(BASE + off as u32, w).0
}

#[test]
fn alu_ops() {
    let mut a = Asm { bytes: vec![] };
    a.movi(2, 100).movi(3, 23);
    a.rrr(8, 0, 4, 2, 3); // add a4, a2, a3
    a.rrr(0xC, 0, 5, 2, 3); // sub a5, a2, a3
    a.rrr(0xA, 0, 6, 3, 2); // addx4 a6, a3, a2
    a.rrr(8, 2, 7, 2, 3); // mull a7, a2, a3
    a.rrr(0xC, 2, 8, 2, 3); // quou a8, a2, a3
    a.rrr(0xE, 2, 9, 2, 3); // remu a9, a2, a3
    a.movi(11, -5);
    a.rrr(2, 1, 12, 1, 11); // srai a12, a11, 1
    a.rrr(4, 0, 0xF, 2, 14); // nsau a14, a2
    a.brk();
    assert_eq!(dis(&a, 6), "add a4, a2, a3");
    assert!(dis(&a, 15).starts_with("mull"), "{}", dis(&a, 15));
    let c = run(&a, 100);
    assert_eq!(c.ar(4), 123);
    assert_eq!(c.ar(5), 77);
    assert_eq!(c.ar(6), 23 * 4 + 100);
    assert_eq!(c.ar(7), 2300);
    assert_eq!(c.ar(8), 4);
    assert_eq!(c.ar(9), 8);
    assert_eq!(c.ar(12) as i32, -3);
    assert_eq!(c.ar(14), 25);
}

#[test]
fn zero_overhead_loop_and_branch() {
    let mut a = Asm { bytes: vec![] };
    a.movi(2, 0).movi(3, 10);
    // loop a3, end: LBEG = pc + 3, LEND = pc + 4 + imm8; the body is one 3-byte addi
    a.w24(2 << 16 | 8 << 12 | 3 << 8 | 7 << 4 | 6);
    a.addi(2, 2, 3);
    a.movi(4, 0).movi(5, 5);
    let top = a.pc();
    a.addi(4, 4, 2);
    a.addi(5, 5, -1);
    // bnez a5, top : BRI12 op0=6 n=1 m=1, target = pc + 4 + imm12
    let off = top.wrapping_sub(a.pc() + 4) & 0xFFF;
    a.w24(off << 12 | 5 << 8 | 1 << 6 | 1 << 4 | 6);
    a.brk();
    assert!(dis(&a, 6).starts_with("loop a3"), "{}", dis(&a, 6));
    let c = run(&a, 200);
    assert_eq!(c.ar(2), 30, "loop body should run 10 times");
    assert_eq!(c.ar(4), 10);
}

#[test]
fn windowed_call_entry_retw() {
    let mut a = Asm { bytes: vec![] };
    a.movi(6, 7); // callee's a2
    let call_pc = a.pc();
    // call4 to the function placed at BASE+16 (4-aligned)
    let target = BASE + 16;
    let off = (target - ((call_pc & !3) + 4)) >> 2;
    a.w24(off << 6 | 1 << 4 | 5);
    a.rrr(2, 0, 2, 6, 6); // mov a2, a6 (or a2, a6, a6)
    a.brk();
    while a.bytes.len() < 16 {
        a.bytes.push(0);
    }
    // entry a1, 32
    a.w24((32 / 8) << 12 | 1 << 8 | 3 << 4 | 6);
    a.addi(2, 2, 35);
    a.w24(0x000090); // retw
    assert!(dis(&a, 16).starts_with("entry a1"), "{}", dis(&a, 16));
    let c = run(&a, 100);
    assert_eq!(c.ar(2), 42);
    assert_eq!(c.ar(1), STACK, "stack pointer restored");
}

#[test]
fn unaligned_load_raises_exception() {
    let mut a = Asm { bytes: vec![] };
    a.movi(2, 1);
    // l32i through a misaligned base register
    a.addi(4, 1, 2); // a4 = sp + 2
    a.w24(0 << 16 | 2 << 12 | 4 << 8 | 3 << 4 | 2); // l32i a3, a4, 0
    a.brk();
    let mut mem = Mem { code: vec![0; 0x1_0000], data: vec![0; 0x8100] };
    mem.code[..a.bytes.len()].copy_from_slice(&a.bytes);
    let mut cpu = Cpu::new(0);
    cpu.pc = BASE;
    cpu.sr[230] = 0x0004_0000 | 0x20;
    cpu.set_ar(1, STACK);
    for _ in 0..3 {
        cpu.step(&mut mem);
    }
    let e = cpu.last_exception.clone().expect("exception");
    assert_eq!(e.cause, 9, "LoadStoreAlignment");
    assert_eq!(e.vaddr, STACK + 2);
}

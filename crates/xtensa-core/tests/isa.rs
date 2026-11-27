//! Instruction-set semantics: small assembly programs run on the interpreter
//! against a mock memory bus, with the expected register / memory results
//! taken from the Xtensa ISA reference.
use xtensa_core::cpu::*;
use xtensa_core::{assemble, Bus, Cpu, MemFault, StepResult};

const IRAM: u32 = 0x4008_0000;
const DRAM: u32 = 0x3FFB_0000;
const STACK_TOP: u32 = DRAM + 0x8000;

/// 64 KiB IRAM + 64 KiB DRAM; every other address faults.
struct Mem {
    iram: Vec<u8>,
    dram: Vec<u8>,
    writes: u32,
}

impl Mem {
    fn slot(&mut self, a: u32, n: u32) -> Result<(&mut Vec<u8>, usize), MemFault> {
        if a >= IRAM && a + n <= IRAM + 0x1_0000 {
            Ok((&mut self.iram, (a - IRAM) as usize))
        } else if a >= DRAM && a + n <= DRAM + 0x1_0000 {
            Ok((&mut self.dram, (a - DRAM) as usize))
        } else {
            Err(MemFault)
        }
    }
    fn w32(&self, a: u32) -> u32 {
        let o = (a - DRAM) as usize;
        u32::from_le_bytes(self.dram[o..o + 4].try_into().unwrap())
    }
}

impl Bus for Mem {
    fn read(&mut self, a: u32, size: u8, _c: u64) -> Result<u32, MemFault> {
        let (m, o) = self.slot(a, size as u32)?;
        Ok((0..size as usize).fold(0, |v, i| v | (m[o + i] as u32) << (8 * i)))
    }
    fn write(&mut self, a: u32, size: u8, val: u32, _c: u64) -> Result<(), MemFault> {
        self.writes += 1;
        let (m, o) = self.slot(a, size as u32)?;
        for i in 0..size as usize {
            m[o + i] = (val >> (8 * i)) as u8;
        }
        Ok(())
    }
    fn fetch(&mut self, a: u32) -> Result<u32, MemFault> {
        let (m, o) = self.slot(a, 1)?;
        Ok((0..4).fold(0, |v, i| v | (*m.get(o + i).unwrap_or(&0) as u32) << (8 * i)))
    }
}

/// Standard Xtensa window overflow/underflow handlers and a kernel exception
/// handler that records EXCCAUSE in a15-of-the-faulting-frame's memory slot
/// (DRAM+0xFF00), counts exceptions (DRAM+0xFF04) and skips the faulting
/// 3-byte instruction.  Level-1 interrupts (EXCCAUSE 4) set DRAM+0xFF08 and
/// clear all software interrupts.
const VECTORS: &str = "
        .org 0x40080000
_wof4:  s32e a0, a5, -16
        s32e a1, a5, -12
        s32e a2, a5, -8
        s32e a3, a5, -4
        rfwo
        .org 0x40080040
_wuf4:  l32e a0, a5, -16
        l32e a1, a5, -12
        l32e a2, a5, -8
        l32e a3, a5, -4
        rfwu
        .org 0x40080080
_wof8:  s32e a0, a9, -16
        l32e a0, a1, -12
        s32e a1, a9, -12
        s32e a2, a9, -8
        s32e a3, a9, -4
        s32e a4, a0, -32
        s32e a5, a0, -28
        s32e a6, a0, -24
        s32e a7, a0, -20
        rfwo
        .org 0x400800c0
_wuf8:  l32e a1, a9, -12
        l32e a0, a9, -16
        l32e a7, a1, -12
        l32e a2, a9, -8
        l32e a4, a7, -32
        l32e a3, a9, -4
        l32e a5, a7, -28
        l32e a6, a7, -24
        l32e a7, a7, -20
        rfwu
        .org 0x40080100
_wof12: s32e a0, a13, -16
        l32e a0, a1, -12
        s32e a1, a13, -12
        s32e a2, a13, -8
        s32e a3, a13, -4
        s32e a4, a0, -48
        s32e a5, a0, -44
        s32e a6, a0, -40
        s32e a7, a0, -36
        s32e a8, a0, -32
        s32e a9, a0, -28
        s32e a10, a0, -24
        s32e a11, a0, -20
        rfwo
        .org 0x40080140
_wuf12: l32e a1, a13, -12
        l32e a0, a13, -16
        l32e a11, a1, -12
        l32e a2, a13, -8
        l32e a4, a11, -48
        l32e a8, a11, -32
        l32e a3, a13, -4
        l32e a5, a11, -44
        l32e a6, a11, -40
        l32e a7, a11, -36
        l32e a9, a11, -28
        l32e a10, a11, -24
        l32e a11, a11, -20
        rfwu
        .org 0x400801c0
_level3: j level3_handler
        .org 0x40080300
_kernel: j kernel_handler
        .org 0x40080340
_user:  j kernel_handler
        .org 0x400803c0
_double: break 1, 14
        .org 0x40080400
kernel_handler:
        wsr a2, excsave1
        wsr a3, depc
        li a2, 0x3ffbff00
        rsr a3, exccause
        s32i a3, a2, 0
        l32i a3, a2, 4
        addi a3, a3, 1
        s32i a3, a2, 4
        rsr a3, exccause
        bnei a3, 4, L1_not_irq
        movi a3, 1
        s32i a3, a2, 8
        li a3, 0xffffffff
        wsr a3, intclear
        j L1_done
L1_not_irq:
        rsr a3, epc1
        addi a3, a3, 3
        wsr a3, epc1
L1_done:
        rsr a2, excsave1
        rsr a3, depc
        rfe
level3_handler:
        wsr a2, excsave1
        li a2, 0x3ffbff0c
        s32i a2, a2, 0          ; mark: level-3 interrupt seen
        movi a2, 0
        wsr a2, ccompare1       ; acknowledge timer 1
        rsr a2, excsave1
        rfi 3
        .org 0x40080600
start:
";

struct Run {
    cpu: Cpu,
    mem: Mem,
    steps: usize,
}

impl Run {
    fn a(&self, n: u8) -> u32 {
        self.cpu.ar(n)
    }
    fn word(&self, addr: u32) -> u32 {
        self.mem.w32(addr)
    }
    fn exc_cause(&self) -> u32 {
        self.word(0x3ffb_ff00)
    }
    fn exc_count(&self) -> u32 {
        self.word(0x3ffb_ff04)
    }
}

/// Assemble `body` after the vector table and run from `start` until
/// `break 1, 15`.  `setup` may adjust the CPU before execution.
fn run_with(body: &str, setup: impl FnOnce(&mut Cpu)) -> Run {
    let src = format!("{}\n{}\n        break 1, 15\n", VECTORS, body);
    let prog = assemble(&src, IRAM).unwrap_or_else(|e| panic!("assembly failed: {}\n", e));
    let mut mem = Mem { iram: vec![0; 0x1_0000], dram: vec![0; 0x1_0000], writes: 0 };
    mem.iram[..prog.bytes.len()].copy_from_slice(&prog.bytes);
    // the root frame's "caller stack pointer" slot (used when its extra
    // registers are spilled by an 8/12 overflow handler)
    let o = (STACK_TOP - 12 - DRAM) as usize;
    mem.dram[o..o + 4].copy_from_slice(&(STACK_TOP + 0x100).to_le_bytes());
    let mut cpu = Cpu::new(0);
    cpu.debugger_attached = true;
    cpu.pc = prog.symbol("start");
    cpu.sr[VECBASE] = IRAM;
    cpu.sr[PS] = PS_WOE | PS_UM; // windowed, user vector, level 0
    cpu.sr[WINDOWSTART] = 1;
    cpu.sr[CPENABLE] = 1;
    cpu.set_ar(1, STACK_TOP);
    setup(&mut cpu);
    let mut steps = 0;
    loop {
        steps += 1;
        assert!(steps < 200_000, "program did not finish, pc 0x{:08x}", cpu.pc);
        match cpu.step(&mut mem) {
            StepResult::Ok => {}
            StepResult::DebugBreak => {
                let w = mem.fetch(cpu.pc).unwrap();
                if (w >> 4) & 0xF == 14 {
                    panic!("double exception (cause {}) at 0x{:08x}", cpu.sr[EXCCAUSE], cpu.sr[DEPC]);
                }
                break;
            }
            StepResult::Waiting => {
                let skip = cpu.cycles_to_timer();
                assert!(skip != u32::MAX, "WAITI with no timer armed");
                cpu.advance_cycles(skip);
            }
            other => panic!("unexpected {:?} at 0x{:08x}", other, cpu.pc),
        }
    }
    Run { cpu, mem, steps }
}

fn run(body: &str) -> Run {
    run_with(body, |_| {})
}

/// Check `expect` = [(reg, value)] after running `body`.
fn check(body: &str, expect: &[(u8, u32)]) {
    let r = run(body);
    for &(reg, v) in expect {
        assert_eq!(r.a(reg), v, "a{} = 0x{:08x}, expected 0x{:08x}\nprogram:\n{}", reg, r.a(reg), v, body);
    }
    assert_eq!(r.exc_count(), 0, "unexpected exception, cause {}", r.exc_cause());
}

// ------------------------------------------------------------------- ALU

#[test]
fn add_sub_and_scaled_variants() {
    check(
        "movi a2, 100
         movi a3, -7
         add a4, a2, a3
         sub a5, a2, a3
         addx2 a6, a3, a2
         addx4 a7, a3, a2
         addx8 a8, a3, a2
         subx2 a9, a2, a3
         subx4 a10, a2, a3
         subx8 a11, a2, a3
         add.n a12, a2, a2
         addi a13, a2, -128
         addi.n a14, a2, -1
         addmi a15, a2, -256",
        &[
            (4, 93),
            (5, 107),
            (6, 86),
            (7, 72),
            (8, 44),
            (9, 207),
            (10, 407),
            (11, 807),
            (12, 200),
            (13, (-28i32) as u32),
            (14, 99),
            (15, (-156i32) as u32),
        ],
    );
}

#[test]
fn logic_and_moves() {
    check(
        "li a2, 0xf0f0ff00
         li a3, 0x0ff0f0f0
         and a4, a2, a3
         or a5, a2, a3
         xor a6, a2, a3
         mov a7, a2
         mov.n a8, a3
         movi a9, -2048
         movi.n a10, -32
         movi.n a11, 95",
        &[(4, 0x00F0F000), (5, 0xFFF0FFF0), (6, 0xFF000FF0), (7, 0xF0F0FF00), (8, 0x0FF0F0F0), (9, 0xFFFFF800), (10, 0xFFFFFFE0), (11, 95)],
    );
}

#[test]
fn neg_abs_min_max() {
    check(
        "movi a2, -5
         movi a3, 9
         li a15, 0x80000000
         neg a4, a2
         abs a5, a2
         abs a6, a15
         min a7, a2, a3
         max a8, a2, a3
         minu a9, a2, a3
         maxu a10, a2, a3
         neg a11, a15",
        &[(4, 5), (5, 5), (6, 0x8000_0000), (7, (-5i32) as u32), (8, 9), (9, 9), (10, (-5i32) as u32), (11, 0x8000_0000)],
    );
}

#[test]
fn conditional_moves() {
    check(
        "movi a2, 0
         movi a3, -1
         movi a4, 5
         movi a5, 111
         movi a6, 111
         movi a7, 111
         movi a8, 111
         movi a9, 111
         movi a10, 111
         movi a11, 111
         movi a12, 111
         moveqz a5, a4, a2      ; taken
         moveqz a6, a4, a3      ; not taken
         movnez a7, a4, a3      ; taken
         movnez a8, a4, a2      ; not taken
         movltz a9, a4, a3      ; taken
         movltz a10, a4, a2     ; not taken
         movgez a11, a4, a2     ; taken
         movgez a12, a4, a3     ; not taken",
        &[(5, 5), (6, 111), (7, 5), (8, 111), (9, 5), (10, 111), (11, 5), (12, 111)],
    );
}

#[test]
fn sign_extension_and_clamping() {
    check(
        "movi a2, 0x80
         movi a3, 300
         movi a4, -300
         li a5, 0x12348765
         sext a6, a2, 7
         sext a7, a5, 15
         clamps a8, a3, 7
         clamps a9, a4, 7
         clamps a10, a2, 8
         sext a11, a2, 8",
        &[(6, 0xFFFF_FF80), (7, 0xFFFF_8765), (8, 127), (9, (-128i32) as u32), (10, 128), (11, 128)],
    );
}

#[test]
fn normalization_shift_amounts() {
    check(
        "movi a2, 0
         movi a3, 1
         movi a4, -1
         li a5, 0x80000000
         li a6, 0x00010000
         nsa a7, a2
         nsa a8, a3
         nsa a9, a4
         nsau a10, a2
         nsau a11, a3
         nsau a12, a5
         nsau a13, a6
         nsa a14, a5",
        &[(7, 31), (8, 30), (9, 31), (10, 32), (11, 31), (12, 0), (13, 15), (14, 0)],
    );
}

#[test]
fn immediate_shifts_and_extract() {
    check(
        "li a2, 0x87654321
         slli a3, a2, 4
         slli a4, a2, 31
         srli a5, a2, 8
         srai a6, a2, 4
         srai a7, a2, 31
         extui a8, a2, 4, 8
         extui a9, a2, 28, 4
         extui a10, a2, 0, 16
         extui a11, a2, 16, 16",
        &[(3, 0x7654_3210), (4, 0x8000_0000), (5, 0x0087_6543), (6, 0xF876_5432), (7, 0xFFFF_FFFF), (8, 0x32), (9, 0x8), (10, 0x4321), (11, 0x8765)],
    );
}

#[test]
fn sar_based_shifts() {
    check(
        "li a2, 0x87654321
         li a3, 0x0fedcba9
         movi a4, 12
         ssl a4
         sll a5, a2              ; << 12
         ssr a4
         srl a6, a2              ; >> 12 logical
         sra a7, a2              ; >> 12 arithmetic
         ssai 8
         src a8, a3, a2          ; funnel: (a3:a2) >> 8
         movi a4, 1
         ssa8l a4                ; SAR = 8
         srl a9, a2
         ssa8b a4                ; SAR = 32 - 8 = 24
         sll a10, a2              ; << (32 - SAR) = 8
         rsr a11, sar",
        &[(5, 0x5432_1000), (6, 0x0008_7654), (7, 0xFFF8_7654), (8, 0xA987_6543), (9, 0x0087_6543), (10, 0x6543_2100), (11, 24)],
    );
}

#[test]
fn multiply_and_divide() {
    check(
        "li a2, 0x00012345
         li a3, 0xfffffff9       ; -7
         movi a4, 2
         li a5, 0x89abcdef
         li a6, 0x76543210
         mull a7, a2, a3
         muluh a8, a5, a6
         mulsh a9, a5, a6
         mul16u a10, a2, a3      ; 0x2345 * 0xfff9
         mul16s a11, a2, a3      ; 0x2345 * -7
         quos a12, a3, a4        ; -7 / 2 = -3
         rems a13, a3, a4        ; -7 % 2 = -1
         quou a14, a5, a6
         remu a15, a5, a6",
        &[
            (7, 0x0012_3450u32.wrapping_mul(0) | (0x0001_2345u32.wrapping_mul(0xFFFF_FFF9))),
            (8, ((0x89ab_cdefu64 * 0x7654_3210u64) >> 32) as u32),
            (9, ((0x89ab_cdefu32 as i32 as i64 * 0x7654_3210i64) >> 32) as u32),
            (10, 0x2345 * 0xFFF9),
            (11, (0x2345i32 * -7) as u32),
            (12, (-3i32) as u32),
            (13, (-1i32) as u32),
            (14, 0x89ab_cdef / 0x7654_3210),
            (15, 0x89ab_cdef % 0x7654_3210),
        ],
    );
}

#[test]
fn divide_by_zero_raises_exception() {
    let r = run(
        "movi a2, 10
         movi a3, 0
         movi a4, 77
         quou a4, a2, a3
         movi a5, 1",
    );
    assert_eq!(r.exc_cause(), EXC_DIVIDE_BY_ZERO);
    assert_eq!(r.exc_count(), 1);
    assert_eq!(r.a(4), 77, "destination unchanged");
    assert_eq!(r.a(5), 1, "execution resumed after the handler");
}

// ------------------------------------------------------------- memory

#[test]
fn loads_and_stores_of_every_width() {
    let r = run(
        "li a2, 0x3ffb1000
         li a3, 0x8899aabb
         s32i a3, a2, 0
         l8ui a4, a2, 0
         l8ui a5, a2, 3
         l16ui a6, a2, 2
         l16si a7, a2, 2
         l16si a8, a2, 0
         movi a9, 0x7f
         s8i a9, a2, 5
         li a10, 0xbeef
         s16i a10, a2, 6
         l32i a11, a2, 4
         s32i.n a3, a2, 8
         l32i.n a12, a2, 8
         l32ai a13, a2, 0
         s32ri a11, a2, 12",
    );
    assert_eq!(r.a(4), 0xBB);
    assert_eq!(r.a(5), 0x88);
    assert_eq!(r.a(6), 0x8899);
    assert_eq!(r.a(7), 0xFFFF_8899);
    assert_eq!(r.a(8), 0xFFFF_AABB);
    assert_eq!(r.a(11), 0xBEEF_7F00);
    assert_eq!(r.a(12), 0x8899_AABB);
    assert_eq!(r.a(13), 0x8899_AABB);
    assert_eq!(r.word(0x3FFB_100C), 0xBEEF_7F00);
}

#[test]
fn l32r_loads_literals() {
    check(
        "       j L1_code
                .align 4
         lit1:  .word 0xcafef00d
         lit2:  .word lit1
         L1_code:
                l32r a2, lit1
                l32r a3, lit2
                li a4, -1
                la a5, lit2",
        &[(2, 0xCAFE_F00D), (4, 0xFFFF_FFFF)],
    );
    let r = run(
        "       j L2_code
                .align 4
         lit:   .word 0x12345678
         L2_code:
                l32r a3, lit",
    );
    assert_eq!(r.a(3), 0x1234_5678);
}

#[test]
fn compare_and_swap() {
    let r = run(
        "li a2, 0x3ffb2000
         movi a3, 5
         s32i a3, a2, 0
         movi a4, 5
         wsr a4, scompare1
         movi a5, 9
         s32c1i a5, a2, 0        ; succeeds: mem 5 -> 9, a5 = old (5)
         l32i a6, a2, 0
         movi a7, 1
         s32c1i a7, a2, 0        ; fails: mem is 9 != 5, a7 = 9
         l32i a8, a2, 0",
    );
    assert_eq!(r.a(5), 5);
    assert_eq!(r.a(6), 9);
    assert_eq!(r.a(7), 9);
    assert_eq!(r.a(8), 9);
}

#[test]
fn memory_faults_and_alignment() {
    let r = run(
        "li a2, 0x60000000       ; unmapped
         l32i a3, a2, 0
         movi a4, 1",
    );
    assert_eq!(r.exc_cause(), EXC_LOAD_PROHIBITED);
    assert_eq!(r.cpu.sr[EXCVADDR], 0x6000_0000);
    assert_eq!(r.a(4), 1);
    let r = run(
        "li a2, 0x60000000
         s32i a2, a2, 0",
    );
    assert_eq!(r.exc_cause(), EXC_STORE_PROHIBITED);
    let r = run(
        "li a2, 0x3ffb1002
         l32i a3, a2, 0",
    );
    assert_eq!(r.exc_cause(), EXC_UNALIGNED);
    assert_eq!(r.cpu.sr[EXCVADDR], 0x3FFB_1002);
}

#[test]
fn self_modifying_code_is_seen() {
    // run `movi a2, 1`, overwrite it with `movi a2, 2`, run it again: the
    // decoded-instruction cache must notice the store
    let r = run(
        "       movi a3, 0
                movi a4, 0
         again:
         patch: movi a2, 1
                add a3, a3, a2
                bnez a4, L_out
                movi a4, 1
                la a5, patch
                la a6, newinsn
                l8ui a7, a6, 0
                s8i a7, a5, 0
                l8ui a7, a6, 1
                s8i a7, a5, 1
                l8ui a7, a6, 2
                s8i a7, a5, 2
                isync
                j again
         newinsn: movi a2, 2
         L_out:",
    );
    assert_eq!(r.a(3), 3, "1 from the original, 2 from the patched instruction");
}

// ------------------------------------------------------------ branches

#[test]
fn every_conditional_branch() {
    // a2 = 0, a3 = -3, a4 = 5, a5 = 12 (0b1100), a6 = 0x80000001
    let cases: &[(&str, bool)] = &[
        ("beqz a2", true),
        ("beqz a4", false),
        ("bnez a4", true),
        ("bnez a2", false),
        ("bltz a3", true),
        ("bltz a4", false),
        ("bgez a2", true),
        ("bgez a3", false),
        ("beqi a4, 5", true),
        ("beqi a4, 4", false),
        ("bnei a4, 4", true),
        ("blti a3, -1", true),
        ("blti a4, 5", false),
        ("bgei a5, 12", true),
        ("bltui a4, 6", true),
        ("bltui a3, 256", false),
        ("bgeui a3, 32768", true),
        ("beq a4, a4", true),
        ("beq a4, a5", false),
        ("bne a4, a5", true),
        ("blt a3, a4", true),
        ("blt a4, a3", false),
        ("bltu a4, a3", true),
        ("bge a4, a3", true),
        ("bgeu a3, a4", true),
        ("bgeu a4, a5", false),
        ("bany a4, a5", true),
        ("bany a2, a4", false),
        ("bnone a4, a2", true),
        ("ball a6, a6", true),
        ("ball a6, a4", false),
        ("bnall a6, a4", true),
        ("bbc a5, a4", true),   // bit 5 of 12 is clear
        ("bbs a5, a5", false),  // bit 12 of 12 is clear
        ("bbs a5, a3", true),   // bit (-3 & 31) = 29? no: 29 is clear -> see below
        ("bbci a6, 1", true),
        ("bbsi a6, 31", true),
        ("bbsi a6, 30", false),
        ("beqz.n a2", true),
        ("bnez.n a4", true),
    ];
    // bit 29 of 12 is clear, so `bbs a5, a3` is not taken
    let cases: Vec<(&str, bool)> = cases.iter().map(|&(c, t)| if c == "bbs a5, a3" { (c, false) } else { (c, t) }).collect();
    let mut src = String::from(
        "movi a2, 0
         movi a3, -3
         movi a4, 5
         movi a5, 12
         li a6, 0x80000001
         li a7, 0x3ffb3000\n",
    );
    for (i, (cond, _)) in cases.iter().enumerate() {
        src.push_str(&format!(
            "        movi a8, 0
                     {cond}, t{i}
                     j n{i}
             t{i}:   movi a8, 1
             n{i}:   s32i a8, a7, {off}\n",
            off = i * 4
        ));
    }
    let r = run(&src);
    for (i, (cond, taken)) in cases.iter().enumerate() {
        assert_eq!(r.word(0x3FFB_3000 + 4 * i as u32) == 1, *taken, "{} taken={}", cond, taken);
    }
}

#[test]
fn zero_overhead_loops() {
    check(
        "       movi a2, 0
                movi a3, 10
                loop a3, L1_end
                addi a2, a2, 3
         L1_end:
                movi a4, 0
                movi a5, 0
                loopnez a5, L2_end      ; count 0: body skipped
                addi a4, a4, 1
         L2_end:
                movi a6, 0
                movi a7, -4
                loopgtz a7, L3_end      ; count <= 0: body skipped
                addi a6, a6, 1
         L3_end:
                movi a8, 0
                movi a9, 4
                loopgtz a9, L4_end
                addi a8, a8, 5
                addi a8, a8, 1
         L4_end:
                rsr a10, lcount",
        &[(2, 30), (4, 0), (6, 0), (8, 24), (10, 0)],
    );
}

#[test]
fn jumps_calls_and_returns() {
    check(
        "       movi a2, 0
                j L1_skip
                movi a2, 99
         L1_skip:
                la a3, L2_tgt
                jx a3
                movi a2, 98
         L2_tgt:
                movi a4, 1
                call0 leaf
                la a3, leaf
                callx0 a3
                j L9_done
                .align 4
         leaf:  addi a4, a4, 10
                ret
         L9_done:",
        &[(2, 0), (4, 21)],
    );
}

// ---------------------------------------------------- register windows

#[test]
fn windowed_calls_pass_arguments() {
    check(
        "       movi a6, 7
                movi a10, 100
                movi a14, 1000
                call4 f4
                mov a3, a6             ; f4 returns in its a2 = caller's a6
                call8 f8
                mov a4, a10
                call12 f12
                mov a5, a14
                j L9_done
                .align 4
         f4:    entry a1, 32
                addi a2, a2, 1
                retw
                .align 4
         f8:    entry a1, 32
                addi a2, a2, 2
                retw.n
                .align 4
         f12:   entry a1, 48
                addi a2, a2, 3
                retw
         L9_done:
                mov a15, a1",
        &[(3, 8), (4, 102), (5, 1003), (15, STACK_TOP)],
    );
}

#[test]
fn deep_recursion_spills_register_windows() {
    // fib(15) with call8 recursion: forces window overflows and underflows
    let r = run(
        "       movi a10, 15
                call8 fib
                mov a2, a10
                j L9_done
                .align 4
         fib:   entry a1, 32
                bltui a2, 2, L1_base
                addi a10, a2, -1
                call8 fib
                mov a3, a10
                addi a10, a2, -2
                call8 fib
                add a2, a3, a10
         L1_base:
                retw
         L9_done:",
    );
    assert_eq!(r.a(2), 610);
    assert_eq!(r.a(1), STACK_TOP);
    assert!(r.mem.writes > 100, "spills wrote to the stack");
    assert_eq!(r.exc_count(), 0);
}

#[test]
fn rotw_and_movsp() {
    check(
        "       movi a2, 5
                movi a6, 11
                rotw 1                 ; a6 becomes a2
                mov a3, a2
                rotw -1
                mov a4, a7",
        &[(4, 11)],
    );
    // MOVSP with no live caller frames below raises the Alloca exception
    // (the OS handler would spill them); with a live frame it just moves.
    let r = run(
        "li a8, 0x3ffb7000
         movsp a9, a8
         movi a10, 1",
    );
    assert_eq!(r.exc_cause(), EXC_ALLOCA);
    assert_eq!(r.a(10), 1);
    let r = run_with(
        "li a8, 0x3ffb7000
         movsp a9, a8",
        |c| c.sr[WINDOWSTART] = 1 | 1 << 15,
    );
    assert_eq!(r.exc_count(), 0);
    assert_eq!(r.a(9), 0x3FFB_7000);
}

// ------------------------------------------------- exceptions / interrupts

#[test]
fn illegal_instruction_and_syscall() {
    let r = run(
        "ill
         movi a3, 1",
    );
    assert_eq!(r.exc_cause(), EXC_ILLEGAL);
    assert_eq!(r.a(3), 1);
    let r = run(
        "syscall
         movi a3, 2",
    );
    assert_eq!(r.exc_cause(), EXC_SYSCALL);
    assert_eq!(r.a(3), 2);
    assert_eq!(r.exc_count(), 1);
}

#[test]
fn software_interrupt_is_taken_when_enabled() {
    let r = run(
        "movi a2, 0x80            ; interrupt 7 = level-1 software interrupt
         wsr a2, intenable
         rsil a3, 0
         wsr a2, intset
         nop
         nop
         movi a4, 1",
    );
    assert_eq!(r.word(0x3FFB_FF08), 1, "handler ran");
    assert_eq!(r.exc_cause(), EXC_LEVEL1_INT);
    assert_eq!(r.a(4), 1);
    // masked by PS.INTLEVEL: never taken
    let r = run(
        "movi a2, 0x80
         wsr a2, intenable
         rsil a3, 1
         wsr a2, intset
         nop
         nop",
    );
    assert_eq!(r.word(0x3FFB_FF08), 0);
    assert_eq!(r.cpu.interrupt() & 0x80, 0x80, "still pending");
}

#[test]
fn ccompare_timer_wakes_waiti() {
    let r = run(
        "li a2, 0x8000            ; interrupt 15 = CCOMPARE1, level 3
         wsr a2, intenable
         rsr a3, ccount
         addi a3, a3, 100
         wsr a3, ccompare1
         waiti 0
         movi a5, 1",
    );
    assert_eq!(r.word(0x3FFB_FF0C), 0x3FFB_FF0C, "level-3 handler ran");
    assert_eq!(r.a(5), 1);
    assert!(r.cpu.cycles >= 100);
}

#[test]
fn rsil_and_special_registers() {
    check(
        "rsil a2, 5
         rsr a3, ps
         movi a4, 0x55
         wsr a4, excsave1
         xsr a5, excsave1      ; a5 <- 0x55, excsave1 <- old a5 (0)
         rsr a6, excsave1
         rsr a7, prid
         rsr a8, ccount
         nop
         rsr a9, ccount
         sub a9, a9, a8
         movi a10, 3
         wur a10, threadptr
         rur a11, threadptr",
        &[(2, PS_WOE | PS_UM), (3, PS_WOE | PS_UM | 5), (5, 0x55), (6, 0), (7, 0xCDCD), (9, 2), (11, 3)],
    );
}

#[test]
fn debugger_breakpoints_stop_before_the_instruction() {
    let src = format!("{}\n movi a2, 1\nbp: movi a2, 2\n movi a2, 3\n break 1, 15\n", VECTORS);
    let p = assemble(&src, IRAM).unwrap();
    let mut mem = Mem { iram: vec![0; 0x1_0000], dram: vec![0; 0x1_0000], writes: 0 };
    mem.iram[..p.bytes.len()].copy_from_slice(&p.bytes);
    let mut cpu = Cpu::new(0);
    cpu.debugger_attached = true;
    cpu.pc = p.symbol("start");
    cpu.sr[PS] = PS_WOE;
    cpu.sr[WINDOWSTART] = 1;
    cpu.breakpoints.push(p.symbol("bp"));
    let mut hit = false;
    for _ in 0..10 {
        if cpu.step(&mut mem) == StepResult::Breakpoint {
            hit = true;
            break;
        }
    }
    assert!(hit);
    assert_eq!(cpu.pc, p.symbol("bp"));
    assert_eq!(cpu.ar(2), 1);
    assert_eq!(cpu.step_over_bp(&mut mem), StepResult::Ok);
    assert_eq!(cpu.ar(2), 2);
}

// ------------------------------------------------------ floating point

#[test]
fn floating_point_arithmetic() {
    let f = |x: f32| x.to_bits();
    let r = run(
        "li a2, 0x40200000        ; 2.5
         li a3, 0xc0000000        ; -2.0
         wfr f0, a2
         wfr f1, a3
         add.s f2, f0, f1
         sub.s f3, f0, f1
         mul.s f4, f0, f1
         mov.s f5, f0
         madd.s f5, f0, f0        ; 2.5 + 2.5*2.5
         neg.s f6, f0
         abs.s f7, f1
         rfr a4, f2
         rfr a5, f3
         rfr a6, f4
         rfr a7, f5
         rfr a8, f6
         rfr a9, f7
         movi a10, -7
         float.s f8, a10, 0
         rfr a11, f8
         trunc.s a12, f4, 0
         li a13, 0x3fc00000       ; 1.5
         wfr f9, a13
         round.s a14, f9, 0       ; ties to even -> 2
         float.s f10, a10, 1      ; -7 / 2
         rfr a15, f10",
    );
    assert_eq!(r.a(4), f(0.5));
    assert_eq!(r.a(5), f(4.5));
    assert_eq!(r.a(6), f(-5.0));
    assert_eq!(r.a(7), f(8.75));
    assert_eq!(r.a(8), f(-2.5));
    assert_eq!(r.a(9), f(2.0));
    assert_eq!(r.a(11), f(-7.0));
    assert_eq!(r.a(12) as i32, -5);
    assert_eq!(r.a(14), 2);
    assert_eq!(r.a(15), f(-3.5));
}

#[test]
fn floating_point_compares_and_booleans() {
    check(
        "li a2, 0x3f800000         ; 1.0
         li a3, 0x40000000         ; 2.0
         wfr f0, a2
         wfr f1, a3
         movi a4, 0
         olt.s b0, f0, f1          ; true
         oeq.s b1, f0, f1          ; false
         ole.s b2, f0, f0          ; true
         bt b0, L1_a
         movi a4, 99
         L1_a:
         bf b1, L2_b
         movi a4, 98
         L2_b:
         orb b3, b0, b1            ; true
         andb b4, b0, b1           ; false
         xorb b5, b0, b2           ; false
         movi a5, 0
         movi a6, 7
         movt a5, a6, b3
         movi a7, 0
         movf a7, a6, b4
         rsr a8, br",
        &[(4, 0), (5, 7), (7, 7), (8, 0b1101)],
    );
}

#[test]
fn floating_point_requires_coprocessor_enable() {
    let r = run_with(
        "li a2, 0x3f800000
         wfr f0, a2",
        |c| c.sr[CPENABLE] = 0,
    );
    assert_eq!(r.exc_cause(), EXC_COPROCESSOR0_DISABLED);
}

#[test]
fn float_loads_and_stores() {
    let r = run(
        "li a2, 0x3ffb4000
         li a3, 0x41200000         ; 10.0
         s32i a3, a2, 0
         lsi f0, a2, 0
         add.s f1, f0, f0
         ssi f1, a2, 4
         l32i a4, a2, 4
         movi a5, 8
         ssx f0, a2, a5
         l32i a6, a2, 8
         lsiu f2, a2, 4            ; a2 += 4
         rfr a7, f2",
    );
    assert_eq!(r.a(4), 20.0f32.to_bits());
    assert_eq!(r.a(6), 10.0f32.to_bits());
    assert_eq!(r.a(2), 0x3FFB_4004);
    assert_eq!(r.a(7), 20.0f32.to_bits());
}

#[test]
fn instruction_counter_and_cycle_accounting() {
    let r = run(
        "movi a2, 100
         L1_l: addi a2, a2, -1
         bnez a2, L1_l",
    );
    assert_eq!(r.a(2), 0);
    assert!(r.cpu.icount >= 201, "icount {}", r.cpu.icount);
    assert!(r.steps as u64 >= r.cpu.icount);
}

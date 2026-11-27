//! Xtensa LX6 instruction decoder (ESP32 configuration).
//!
//! Instructions are decoded into a compact [`Insn`] record.  Field naming follows
//! the Xtensa ISA reference: `r`, `s`, `t` are the 4-bit register/sub-opcode
//! fields, `imm` carries the already sign-extended / scaled immediate.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    Ill,
    Nop,
    // ---- control / system
    Break,
    Syscall,
    Simcall,
    Rsil,
    Waiti,
    Rfe,
    Rfue,
    Rfde,
    Rfwo,
    Rfwu,
    Rfi,
    Rfme,
    Ret,
    Retw,
    Jx,
    Callx0,
    Callx4,
    Callx8,
    Callx12,
    Movsp,
    // ---- ALU
    And,
    Or,
    Xor,
    Add,
    Addx2,
    Addx4,
    Addx8,
    Sub,
    Subx2,
    Subx4,
    Subx8,
    Neg,
    Abs,
    Ssr,
    Ssl,
    Ssa8l,
    Ssa8b,
    Ssai,
    Rer,
    Wer,
    Rotw,
    Nsa,
    Nsau,
    TlbRead,
    TlbNop,
    Slli,
    Srai,
    Srli,
    Xsr,
    Src,
    Srl,
    Sll,
    Sra,
    Mul16u,
    Mul16s,
    Andb,
    Andbc,
    Orb,
    Orbc,
    Xorb,
    Any4,
    All4,
    Any8,
    All8,
    Mull,
    Muluh,
    Mulsh,
    Quou,
    Quos,
    Remu,
    Rems,
    Rsr,
    Wsr,
    Sext,
    Clamps,
    Min,
    Max,
    Minu,
    Maxu,
    Moveqz,
    Movnez,
    Movltz,
    Movgez,
    Movf,
    Movt,
    Rur,
    Wur,
    Extui,
    L32e,
    S32e,
    // ---- floating point
    AddS,
    SubS,
    MulS,
    MaddS,
    MsubS,
    MaddnS,
    DivnS,
    ConstS,
    Div0S,
    Recip0S,
    Sqrt0S,
    Rsqrt0S,
    Nexp01S,
    MksadjS,
    MkdadjS,
    AddexpS,
    AddexpmS,
    RoundS,
    TruncS,
    FloorS,
    CeilS,
    FloatS,
    UfloatS,
    UtruncS,
    MovS,
    AbsS,
    Rfr,
    Wfr,
    NegS,
    UnS,
    OeqS,
    UeqS,
    OltS,
    UltS,
    OleS,
    UleS,
    MoveqzS,
    MovnezS,
    MovltzS,
    MovgezS,
    MovfS,
    MovtS,
    Lsi,
    Ssi,
    Lsiu,
    Ssiu,
    Lsx,
    Lsxu,
    Ssx,
    Ssxu,
    // ---- loads / stores / immediates
    L32r,
    L8ui,
    L16ui,
    L16si,
    L32i,
    S8i,
    S16i,
    S32i,
    Cache,
    Movi,
    L32ai,
    Addi,
    Addmi,
    S32c1i,
    S32ri,
    // ---- calls / jumps / branches
    Call0,
    Call4,
    Call8,
    Call12,
    J,
    Beqz,
    Bnez,
    Bltz,
    Bgez,
    Beqi,
    Bnei,
    Blti,
    Bgei,
    Bltui,
    Bgeui,
    Entry,
    Bf,
    Bt,
    Loop,
    Loopnez,
    Loopgtz,
    Bnone,
    Beq,
    Blt,
    Bltu,
    Ball,
    Bbc,
    Bbci,
    Bany,
    Bne,
    Bge,
    Bgeu,
    Bnall,
    Bbs,
    Bbsi,
    // ---- MAC16 (decoded generically, executed by sub-fields)
    Mac16,
    // ---- density (narrow) — many map onto the wide ops above
    RetN,
    RetwN,
    BreakN,
}

/// A decoded instruction.
#[derive(Clone, Copy, Debug)]
pub struct Insn {
    pub op: Op,
    /// Length in bytes: 2 or 3.
    pub len: u8,
    pub r: u8,
    pub s: u8,
    pub t: u8,
    /// Highest AR register group (reg >> 2) referenced; used for lazy window-overflow checks.
    pub wgroup: u8,
    /// Immediate / offset, already decoded.  For PC-relative ops this is the
    /// offset relative to the value documented per-op in `exec`.
    pub imm: i32,
}

pub const B4CONST: [i32; 16] = [-1, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 32, 64, 128, 256];
pub const B4CONSTU: [u32; 16] = [32768, 65536, 2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 32, 64, 128, 256];

#[inline]
fn sext(v: u32, bits: u32) -> i32 {
    let sh = 32 - bits;
    ((v << sh) as i32) >> sh
}

#[inline]
fn g(reg: u8) -> u8 {
    reg >> 2
}

impl Insn {
    fn new(op: Op, len: u8, r: u8, s: u8, t: u8, imm: i32, raw: u32) -> Insn {
        // The raw word is only needed by ops that have no immediate; keep it in `imm`
        // so the decoded form stays 12 bytes (cache entries are 16 bytes).
        let imm = if matches!(op, Op::Mac16 | Op::Ill | Op::TlbRead | Op::TlbNop) { raw as i32 } else { imm };
        Insn { op, len, r, s, t, imm, wgroup: 0 }
    }
    /// Raw instruction word; only meaningful for MAC16, illegal and TLB/cache ops.
    #[inline]
    pub fn raw(&self) -> u32 {
        self.imm as u32
    }
    fn with_regs(mut self, regs: &[u8]) -> Insn {
        let mut m = 0;
        for &x in regs {
            m = m.max(g(x));
        }
        self.wgroup = m;
        self
    }
}

/// Returns instruction length from the first byte.
#[inline]
pub fn insn_len(b0: u8) -> u8 {
    let op0 = b0 & 0xF;
    if op0 >= 8 && op0 <= 0xD {
        2
    } else {
        3
    }
}

/// Decode an instruction from a little-endian word (low 24 bits used).
pub fn decode(w: u32) -> Insn {
    let op0 = (w & 0xF) as u8;
    let t = ((w >> 4) & 0xF) as u8;
    let s = ((w >> 8) & 0xF) as u8;
    let r = ((w >> 12) & 0xF) as u8;
    let op1 = ((w >> 16) & 0xF) as u8;
    let op2 = ((w >> 20) & 0xF) as u8;
    let w24 = w & 0xFF_FFFF;
    use Op::*;
    let ill = Insn::new(Ill, 3, r, s, t, 0, w24);
    match op0 {
        0 => decode_qrst(w24, op1, op2, r, s, t),
        1 => {
            // L32R at, label: imm16 in bits 23..8; offset = (0xFFFF0000|imm16)<<2 relative to (PC+3)&~3
            let imm16 = (w24 >> 8) & 0xFFFF;
            let off = ((0xFFFF_0000u32 | imm16) << 2) as i32;
            Insn::new(L32r, 3, r, s, t, off, w24).with_regs(&[t])
        }
        2 => {
            let imm8 = (w24 >> 16) & 0xFF;
            let op = match r {
                0 => L8ui,
                1 => L16ui,
                2 => L32i,
                4 => S8i,
                5 => S16i,
                6 => S32i,
                7 => Cache,
                9 => L16si,
                0xA => Movi,
                0xB => L32ai,
                0xC => Addi,
                0xD => Addmi,
                0xE => S32c1i,
                0xF => S32ri,
                _ => return ill,
            };
            let imm = match op {
                L8ui | S8i => imm8 as i32,
                L16ui | S16i | L16si => (imm8 << 1) as i32,
                L32i | S32i | L32ai | S32c1i | S32ri => (imm8 << 2) as i32,
                Movi => sext(((s as u32) << 8) | imm8, 12),
                Addi => sext(imm8, 8),
                Addmi => sext(imm8, 8) << 8,
                Cache => (imm8 << 2) as i32,
                _ => 0,
            };
            let i = Insn::new(op, 3, r, s, t, imm, w24);
            match op {
                Movi => i.with_regs(&[t]),
                Cache => {
                    // DHI/DHU/DII/III etc: only reference as
                    i.with_regs(&[s])
                }
                _ => i.with_regs(&[s, t]),
            }
        }
        3 => {
            // LSCI: floating point loads / stores
            let imm8 = (w24 >> 16) & 0xFF;
            let op = match r {
                0 => Lsi,
                4 => Ssi,
                8 => Lsiu,
                0xC => Ssiu,
                _ => return ill,
            };
            Insn::new(op, 3, r, s, t, (imm8 << 2) as i32, w24).with_regs(&[s])
        }
        4 => Insn::new(Mac16, 3, r, s, t, 0, w24).with_regs(&[s, t, r]),
        5 => {
            let n = (w24 >> 4) & 3;
            let off = sext(w24 >> 6, 18) << 2;
            let op = [Call0, Call4, Call8, Call12][n as usize];
            let i = Insn::new(op, 3, r, s, t, off, w24);
            match n {
                0 => i,
                1 => i.with_regs(&[4]),
                2 => i.with_regs(&[8]),
                _ => i.with_regs(&[12]),
            }
        }
        6 => {
            let n = (w24 >> 4) & 3;
            let m = (w24 >> 6) & 3;
            match n {
                0 => Insn::new(J, 3, r, s, t, sext(w24 >> 6, 18), w24),
                1 => {
                    let op = [Beqz, Bnez, Bltz, Bgez][m as usize];
                    Insn::new(op, 3, r, s, t, sext(w24 >> 12, 12), w24).with_regs(&[s])
                }
                2 => {
                    let op = [Beqi, Bnei, Blti, Bgei][m as usize];
                    // imm holds branch offset; the comparison constant comes from B4CONST[r]
                    Insn::new(op, 3, r, s, t, sext(w24 >> 16, 8), w24).with_regs(&[s])
                }
                _ => match m {
                    0 => {
                        // ENTRY as, imm12 (frame size = imm12 << 3)
                        let imm12 = (w24 >> 12) & 0xFFF;
                        Insn::new(Entry, 3, r, s, t, (imm12 << 3) as i32, w24)
                    }
                    1 => {
                        let imm8 = sext(w24 >> 16, 8);
                        match r {
                            0 => Insn::new(Bf, 3, r, s, t, imm8, w24),
                            1 => Insn::new(Bt, 3, r, s, t, imm8, w24),
                            8 => Insn::new(Loop, 3, r, s, t, ((w24 >> 16) & 0xFF) as i32, w24).with_regs(&[s]),
                            9 => Insn::new(Loopnez, 3, r, s, t, ((w24 >> 16) & 0xFF) as i32, w24).with_regs(&[s]),
                            0xA => Insn::new(Loopgtz, 3, r, s, t, ((w24 >> 16) & 0xFF) as i32, w24).with_regs(&[s]),
                            _ => ill,
                        }
                    }
                    2 => Insn::new(Bltui, 3, r, s, t, sext(w24 >> 16, 8), w24).with_regs(&[s]),
                    _ => Insn::new(Bgeui, 3, r, s, t, sext(w24 >> 16, 8), w24).with_regs(&[s]),
                },
            }
        }
        7 => {
            let imm8 = sext(w24 >> 16, 8);
            let op = match r {
                0 => Bnone,
                1 => Beq,
                2 => Blt,
                3 => Bltu,
                4 => Ball,
                5 => Bbc,
                6 | 7 => Bbci,
                8 => Bany,
                9 => Bne,
                0xA => Bge,
                0xB => Bgeu,
                0xC => Bnall,
                0xD => Bbs,
                _ => Bbsi,
            };
            let i = Insn::new(op, 3, r, s, t, imm8, w24);
            match op {
                Bbci | Bbsi => i.with_regs(&[s]),
                _ => i.with_regs(&[s, t]),
            }
        }
        8 => {
            // L32I.N at, as, imm4<<2
            let w16 = w & 0xFFFF;
            Insn::new(L32i, 2, r, s, t, (r as i32) << 2, w16).with_regs(&[s, t])
        }
        9 => {
            let w16 = w & 0xFFFF;
            Insn::new(S32i, 2, r, s, t, (r as i32) << 2, w16).with_regs(&[s, t])
        }
        0xA => {
            let w16 = w & 0xFFFF;
            Insn::new(Add, 2, r, s, t, 0, w16).with_regs(&[r, s, t])
        }
        0xB => {
            let w16 = w & 0xFFFF;
            let imm = if t == 0 { -1 } else { t as i32 };
            // ADDI.N ar, as, imm  -> represent as Addi with t=dest? Use dedicated mapping:
            // we encode as Addi with "t" := r (dest) so exec can share the code path.
            Insn::new(Addi, 2, r, s, r, imm, w16).with_regs(&[r, s])
        }
        0xC => {
            let w16 = w & 0xFFFF;
            if t & 8 == 0 {
                // MOVI.N as, imm7
                let mut imm = (((t & 7) as i32) << 4) | r as i32;
                if imm >= 96 {
                    imm -= 128;
                }
                // Map onto Movi with t := s (dest)
                Insn::new(Movi, 2, r, s, s, imm, w16).with_regs(&[s])
            } else {
                let imm6 = (((t & 3) as i32) << 4) | r as i32;
                let op = if t & 4 == 0 { Beqz } else { Bnez };
                // narrow branch: target = PC + 4 + imm6 (unsigned); we store imm so that
                // exec's common formula (PC + 4 + imm) works.
                Insn::new(op, 2, r, s, t, imm6, w16).with_regs(&[s])
            }
        }
        0xD => {
            let w16 = w & 0xFFFF;
            match r {
                0 => {
                    // MOV.N at, as  -> OR at, as, as
                    Insn::new(Or, 2, t, s, s, 0, w16).with_regs(&[s, t])
                }
                0xF => match t {
                    0 => Insn::new(RetN, 2, r, s, t, 0, w16),
                    1 => Insn::new(RetwN, 2, r, s, t, 0, w16),
                    2 => Insn::new(BreakN, 2, r, s, t, 0, w16),
                    3 => Insn::new(Nop, 2, r, s, t, 0, w16),
                    6 => Insn::new(Ill, 2, r, s, t, 0, w16),
                    _ => Insn::new(Ill, 2, r, s, t, 0, w16),
                },
                _ => Insn::new(Ill, 2, r, s, t, 0, w16),
            }
        }
        _ => ill,
    }
}

fn decode_qrst(w24: u32, op1: u8, op2: u8, r: u8, s: u8, t: u8) -> Insn {
    use Op::*;
    let ill = Insn::new(Ill, 3, r, s, t, 0, w24);
    let mk = |op: Op, imm: i32| Insn::new(op, 3, r, s, t, imm, w24);
    match op1 {
        0 => match op2 {
            0 => {
                // ST0
                match r {
                    0 => {
                        let m = (t >> 2) & 3;
                        let n = t & 3;
                        match m {
                            0 => ill,
                            2 => match n {
                                0 => mk(Ret, 0),
                                1 => mk(Retw, 0),
                                2 => mk(Jx, 0).with_regs(&[s]),
                                _ => ill,
                            },
                            3 => {
                                let op = [Callx0, Callx4, Callx8, Callx12][n as usize];
                                mk(op, 0).with_regs(&[s, n * 4])
                            }
                            _ => ill,
                        }
                    }
                    1 => mk(Movsp, 0).with_regs(&[s, t]),
                    2 => match t {
                        0 | 1 | 2 | 3 | 8 | 0xC | 0xD | 0xF => mk(Nop, 0),
                        _ => ill,
                    },
                    3 => match t {
                        0 => match s {
                            0 => mk(Rfe, 0),
                            1 => mk(Rfue, 0),
                            2 => mk(Rfde, 0),
                            4 => mk(Rfwo, 0),
                            5 => mk(Rfwu, 0),
                            _ => ill,
                        },
                        1 => mk(Rfi, 0),
                        2 => mk(Rfme, 0),
                        _ => ill,
                    },
                    4 => mk(Break, 0),
                    5 => match s {
                        0 => mk(Syscall, 0),
                        1 => mk(Simcall, 0),
                        _ => ill,
                    },
                    6 => mk(Rsil, 0).with_regs(&[t]),
                    7 => mk(Waiti, 0),
                    8 => mk(Any4, 0),
                    9 => mk(All4, 0),
                    0xA => mk(Any8, 0),
                    0xB => mk(All8, 0),
                    _ => ill,
                }
            }
            1 => mk(And, 0).with_regs(&[r, s, t]),
            2 => mk(Or, 0).with_regs(&[r, s, t]),
            3 => mk(Xor, 0).with_regs(&[r, s, t]),
            4 => {
                // ST1
                match r {
                    0 => mk(Ssr, 0).with_regs(&[s]),
                    1 => mk(Ssl, 0).with_regs(&[s]),
                    2 => mk(Ssa8l, 0).with_regs(&[s]),
                    3 => mk(Ssa8b, 0).with_regs(&[s]),
                    4 => mk(Ssai, (s | ((t & 1) << 4)) as i32),
                    6 => mk(Rer, 0).with_regs(&[s, t]),
                    7 => mk(Wer, 0).with_regs(&[s, t]),
                    8 => mk(Rotw, sext(t as u32, 4)),
                    0xE => mk(Nsa, 0).with_regs(&[s, t]),
                    0xF => mk(Nsau, 0).with_regs(&[s, t]),
                    _ => ill,
                }
            }
            5 => match r {
                3 | 7 | 0xB | 0xF => mk(TlbRead, 0).with_regs(&[s, t]),
                4 | 5 | 6 | 0xC | 0xD | 0xE => mk(TlbNop, 0).with_regs(&[s, t]),
                _ => ill,
            },
            6 => match s {
                0 => mk(Neg, 0).with_regs(&[r, t]),
                1 => mk(Abs, 0).with_regs(&[r, t]),
                _ => ill,
            },
            8 => mk(Add, 0).with_regs(&[r, s, t]),
            9 => mk(Addx2, 0).with_regs(&[r, s, t]),
            0xA => mk(Addx4, 0).with_regs(&[r, s, t]),
            0xB => mk(Addx8, 0).with_regs(&[r, s, t]),
            0xC => mk(Sub, 0).with_regs(&[r, s, t]),
            0xD => mk(Subx2, 0).with_regs(&[r, s, t]),
            0xE => mk(Subx4, 0).with_regs(&[r, s, t]),
            0xF => mk(Subx8, 0).with_regs(&[r, s, t]),
            _ => ill,
        },
        1 => match op2 {
            0 | 1 => {
                let sa = 32 - ((((op2 & 1) as i32) << 4) | t as i32);
                mk(Slli, sa).with_regs(&[r, s])
            }
            2 | 3 => {
                let sa = (((op2 & 1) as i32) << 4) | s as i32;
                mk(Srai, sa).with_regs(&[r, t])
            }
            4 => mk(Srli, s as i32).with_regs(&[r, t]),
            6 => mk(Xsr, (((r as u32) << 4) | s as u32) as i32).with_regs(&[t]),
            8 => mk(Src, 0).with_regs(&[r, s, t]),
            9 => mk(Srl, 0).with_regs(&[r, t]),
            0xA => mk(Sll, 0).with_regs(&[r, s]),
            0xB => mk(Sra, 0).with_regs(&[r, t]),
            0xC => mk(Mul16u, 0).with_regs(&[r, s, t]),
            0xD => mk(Mul16s, 0).with_regs(&[r, s, t]),
            0xF => mk(TlbNop, 0).with_regs(&[s, t]), // IMP group: cache tag ops – treat as nop
            _ => ill,
        },
        2 => match op2 {
            0 => mk(Andb, 0),
            1 => mk(Andbc, 0),
            2 => mk(Orb, 0),
            3 => mk(Orbc, 0),
            4 => mk(Xorb, 0),
            8 => mk(Mull, 0).with_regs(&[r, s, t]),
            0xA => mk(Muluh, 0).with_regs(&[r, s, t]),
            0xB => mk(Mulsh, 0).with_regs(&[r, s, t]),
            0xC => mk(Quou, 0).with_regs(&[r, s, t]),
            0xD => mk(Quos, 0).with_regs(&[r, s, t]),
            0xE => mk(Remu, 0).with_regs(&[r, s, t]),
            0xF => mk(Rems, 0).with_regs(&[r, s, t]),
            _ => ill,
        },
        3 => match op2 {
            0 => mk(Rsr, (((r as u32) << 4) | s as u32) as i32).with_regs(&[t]),
            1 => mk(Wsr, (((r as u32) << 4) | s as u32) as i32).with_regs(&[t]),
            2 => mk(Sext, t as i32 + 7).with_regs(&[r, s]),
            3 => mk(Clamps, t as i32 + 7).with_regs(&[r, s]),
            4 => mk(Min, 0).with_regs(&[r, s, t]),
            5 => mk(Max, 0).with_regs(&[r, s, t]),
            6 => mk(Minu, 0).with_regs(&[r, s, t]),
            7 => mk(Maxu, 0).with_regs(&[r, s, t]),
            8 => mk(Moveqz, 0).with_regs(&[r, s, t]),
            9 => mk(Movnez, 0).with_regs(&[r, s, t]),
            0xA => mk(Movltz, 0).with_regs(&[r, s, t]),
            0xB => mk(Movgez, 0).with_regs(&[r, s, t]),
            0xC => mk(Movf, 0).with_regs(&[r, s]),
            0xD => mk(Movt, 0).with_regs(&[r, s]),
            0xE => mk(Rur, (((s as u32) << 4) | t as u32) as i32).with_regs(&[r]),
            0xF => mk(Wur, (((r as u32) << 4) | s as u32) as i32).with_regs(&[t]),
            _ => ill,
        },
        4 | 5 => {
            let shift = (((op1 & 1) as i32) << 4) | s as i32;
            let mut i = mk(Extui, shift).with_regs(&[r, t]);
            // mask bits = op2 + 1 ; stash in `s` is not possible (s used by shift) -> encode in imm high bits
            i.imm = shift | (((op2 as i32) + 1) << 8);
            i
        }
        8 => {
            let op = match op2 {
                0 => Lsx,
                1 => Lsxu,
                4 => Ssx,
                5 => Ssxu,
                _ => return ill,
            };
            mk(op, 0).with_regs(&[s, t])
        }
        9 => match op2 {
            0 => mk(L32e, ((r as i32) << 2) - 64).with_regs(&[s, t]),
            4 => mk(S32e, ((r as i32) << 2) - 64).with_regs(&[s, t]),
            _ => ill,
        },
        0xA => match op2 {
            0 => mk(AddS, 0),
            1 => mk(SubS, 0),
            2 => mk(MulS, 0),
            4 => mk(MaddS, 0),
            5 => mk(MsubS, 0),
            6 => mk(MaddnS, 0),
            7 => mk(DivnS, 0),
            8 => mk(RoundS, t as i32).with_regs(&[r]),
            9 => mk(TruncS, t as i32).with_regs(&[r]),
            0xA => mk(FloorS, t as i32).with_regs(&[r]),
            0xB => mk(CeilS, t as i32).with_regs(&[r]),
            0xC => mk(FloatS, t as i32).with_regs(&[s]),
            0xD => mk(UfloatS, t as i32).with_regs(&[s]),
            0xE => mk(UtruncS, t as i32).with_regs(&[r]),
            0xF => match t {
                0 => mk(MovS, 0),
                1 => mk(AbsS, 0),
                3 => mk(ConstS, s as i32),
                4 => mk(Rfr, 0).with_regs(&[r]),
                5 => mk(Wfr, 0).with_regs(&[s]),
                6 => mk(NegS, 0),
                7 => mk(Div0S, 0),
                8 => mk(Recip0S, 0),
                9 => mk(Sqrt0S, 0),
                0xA => mk(Rsqrt0S, 0),
                0xB => mk(Nexp01S, 0),
                0xC => mk(MksadjS, 0),
                0xD => mk(MkdadjS, 0),
                0xE => mk(AddexpS, 0),
                0xF => mk(AddexpmS, 0),
                _ => ill,
            },
            _ => ill,
        },
        0xB => match op2 {
            1 => mk(UnS, 0),
            2 => mk(OeqS, 0),
            3 => mk(UeqS, 0),
            4 => mk(OltS, 0),
            5 => mk(UltS, 0),
            6 => mk(OleS, 0),
            7 => mk(UleS, 0),
            8 => mk(MoveqzS, 0).with_regs(&[t]),
            9 => mk(MovnezS, 0).with_regs(&[t]),
            0xA => mk(MovltzS, 0).with_regs(&[t]),
            0xB => mk(MovgezS, 0).with_regs(&[t]),
            0xC => mk(MovfS, 0),
            0xD => mk(MovtS, 0),
            _ => ill,
        },
        _ => ill,
    }
}

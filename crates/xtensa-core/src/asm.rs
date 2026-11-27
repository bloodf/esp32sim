//! A small two-pass Xtensa (ESP32 / LX6) assembler.
//!
//! It accepts the syntax produced by [`crate::disasm`] (so disassembly can be
//! re-assembled) plus labels, constants and a few directives.  It exists so
//! tests — and tools such as mock-firmware generators — can produce machine
//! code without an Xtensa toolchain.
//!
//! ```text
//!         .equ UART0_FIFO, 0x3ff40000
//! start:  li    a2, UART0_FIFO      ; 32-bit constant (inline literal)
//!         movi  a3, 'A'
//!         s32i  a3, a2, 0
//!         j     start
//! ```
//!
//! * comments: `;`, `#` (at line start or after whitespace) and `//`
//! * labels: `name:` (several per line allowed), `.` is the current address
//! * expressions: `+ - * / << >> & | ~` with C precedence,
//!   parentheses, numbers (`12`, `-3`, `0x1f`, `0b101`, `'c'`), labels, constants
//! * directives: `.org addr`, `.align n`, `.word v,…`, `.short v,…`, `.byte v,…`,
//!   `.ascii "s"`, `.asciz "s"`, `.space n`, `.equ/.set name, v`, `.raw3 v`
//! * pseudo instructions: `mov ar, as`, `li ar, expr` (jumps over an inline
//!   literal and loads it with `l32r`), `nop` (`or a1, a1, a1`-free encoding)
use crate::disasm::{sr_name, ur_name};
use std::collections::HashMap;

/// Result of assembling a source text.
#[derive(Clone, Debug, Default)]
pub struct Program {
    /// Address of `bytes[0]`.
    pub base: u32,
    pub bytes: Vec<u8>,
    /// Label and constant values.
    pub symbols: HashMap<String, u32>,
    /// (address, 1-based source line) for every emitted instruction.
    pub lines: Vec<(u32, usize)>,
}

impl Program {
    pub fn symbol(&self, name: &str) -> u32 {
        *self.symbols.get(name).unwrap_or_else(|| panic!("no symbol '{}'", name))
    }
    pub fn end(&self) -> u32 {
        self.base + self.bytes.len() as u32
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AsmError {
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for AsmError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}
impl std::error::Error for AsmError {}

/// Assemble `src` placed at address `base`.
pub fn assemble(src: &str, base: u32) -> Result<Program, AsmError> {
    let stmts = parse(src)?;
    // pass 1: sizes and label addresses (sizes never depend on forward labels)
    let mut syms: HashMap<String, u32> = HashMap::new();
    let mut pc = base;
    for st in &stmts {
        for l in &st.labels {
            if syms.insert(l.clone(), pc).is_some() {
                return Err(AsmError { line: st.line, msg: format!("duplicate label '{}'", l) });
            }
        }
        if let Some((m, ops)) = &st.insn {
            if m == ".equ" || m == ".set" {
                if ops.len() != 2 {
                    return Err(AsmError { line: st.line, msg: ".equ needs a name and a value".into() });
                }
                // constants must be defined before use in pass 1 expressions
                let v = eval(&ops[1], &syms, pc).map_err(|msg| AsmError { line: st.line, msg })?;
                syms.insert(ops[0].trim().to_string(), v);
                continue;
            }
            pc = pc.wrapping_add(size_of(m, ops, pc, &syms).map_err(|msg| AsmError { line: st.line, msg })?);
        }
    }
    // pass 2: encode
    let mut out = Program { base, bytes: Vec::new(), symbols: syms.clone(), lines: Vec::new() };
    let mut pc = base;
    for st in &stmts {
        if let Some((m, ops)) = &st.insn {
            if m == ".equ" || m == ".set" {
                continue;
            }
            let bytes = emit(m, ops, pc, &syms).map_err(|msg| AsmError { line: st.line, msg: format!("{} ({} {})", msg, m, ops.join(", ")) })?;
            if !m.starts_with('.') {
                out.lines.push((pc, st.line));
            }
            pc = pc.wrapping_add(bytes.len() as u32);
            out.bytes.extend_from_slice(&bytes);
        }
    }
    Ok(out)
}

/// Assemble one instruction at `pc` (operands may only use numbers).
pub fn assemble_one(text: &str, pc: u32) -> Result<Vec<u8>, String> {
    let p = assemble(text, pc).map_err(|e| e.msg)?;
    Ok(p.bytes)
}

// ------------------------------------------------------------------ parsing

struct Stmt {
    line: usize,
    labels: Vec<String>,
    insn: Option<(String, Vec<String>)>,
}

fn strip_comment(l: &str) -> &str {
    let mut in_str = false;
    let mut in_chr = false;
    let b = l.as_bytes();
    for i in 0..b.len() {
        let c = b[i];
        if c == b'"' && !in_chr {
            in_str = !in_str;
        } else if c == b'\'' && !in_str {
            in_chr = !in_chr;
        } else if !in_str && !in_chr {
            if c == b';' || (c == b'/' && b.get(i + 1) == Some(&b'/')) {
                return &l[..i];
            }
            if c == b'#' && (i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t') {
                return &l[..i];
            }
        }
    }
    l
}

fn split_operands(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    let mut in_str = false;
    let mut in_chr = false;
    for c in s.chars() {
        match c {
            '"' if !in_chr => in_str = !in_str,
            '\'' if !in_str => in_chr = !in_chr,
            '(' if !in_str && !in_chr => depth += 1,
            ')' if !in_str && !in_chr => depth -= 1,
            ',' if depth == 0 && !in_str && !in_chr => {
                out.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn is_ident(s: &str) -> bool {
    let mut ch = s.chars();
    matches!(ch.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '.') && ch.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$')
}

fn parse(src: &str) -> Result<Vec<Stmt>, AsmError> {
    let mut v = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let mut l = strip_comment(raw).trim();
        let mut labels = Vec::new();
        // labels
        while let Some(p) = l.find(':') {
            let head = l[..p].trim();
            if is_ident(head) && !head.contains(' ') {
                labels.push(head.to_string());
                l = l[p + 1..].trim();
            } else {
                break;
            }
        }
        let insn = if l.is_empty() {
            None
        } else {
            let (m, rest) = match l.find(|c: char| c.is_whitespace()) {
                Some(p) => (&l[..p], l[p..].trim()),
                None => (l, ""),
            };
            Some((m.to_ascii_lowercase(), split_operands(rest)))
        };
        v.push(Stmt { line: i + 1, labels, insn });
    }
    Ok(v)
}

// ------------------------------------------------------------- expressions

fn eval(e: &str, syms: &HashMap<String, u32>, pc: u32) -> Result<u32, String> {
    let toks = tokenize(e)?;
    let mut p = 0;
    let v = eval_expr(&toks, &mut p, syms, pc)?;
    if p != toks.len() {
        return Err(format!("cannot parse expression '{}'", e));
    }
    Ok(v as u32)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(i64),
    Id(String),
    Op(&'static str),
    LP,
    RP,
}

fn tokenize(e: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = e.chars().collect();
    let mut i = 0;
    let mut v = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '\'' {
            // char literal, with \n \r \t \0 \\ \' escapes
            let (val, n) = match (c.get(i + 1), c.get(i + 2), c.get(i + 3)) {
                (Some('\\'), Some(x), Some('\'')) => (
                    match x {
                        'n' => 10,
                        'r' => 13,
                        't' => 9,
                        '0' => 0,
                        other => *other as i64,
                    },
                    4,
                ),
                (Some(x), Some('\''), _) => (*x as i64, 3),
                _ => return Err(format!("bad character literal in '{}'", e)),
            };
            v.push(Tok::Num(val));
            i += n;
        } else if ch.is_ascii_digit() {
            let st = i;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            let s: String = c[st..i].iter().filter(|c| **c != '_').collect();
            let n = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                i64::from_str_radix(h, 16)
            } else if let Some(b) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
                i64::from_str_radix(b, 2)
            } else {
                s.parse::<i64>()
            }
            .map_err(|_| format!("bad number '{}'", s))?;
            v.push(Tok::Num(n));
        } else if ch.is_ascii_alphabetic() || ch == '_' || ch == '.' {
            let st = i;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '_' || c[i] == '.' || c[i] == '$') {
                i += 1;
            }
            v.push(Tok::Id(c[st..i].iter().collect()));
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            if two == "<<" || two == ">>" {
                v.push(Tok::Op(if two == "<<" { "<<" } else { ">>" }));
                i += 2;
                continue;
            }
            let op = match ch {
                '+' => "+",
                '-' => "-",
                '*' => "*",
                '/' => "/",
                '&' => "&",
                '|' => "|",
                '~' => "~",
                '(' => {
                    v.push(Tok::LP);
                    i += 1;
                    continue;
                }
                ')' => {
                    v.push(Tok::RP);
                    i += 1;
                    continue;
                }
                _ => return Err(format!("unexpected '{}' in '{}'", ch, e)),
            };
            v.push(Tok::Op(op));
            i += 1;
        }
    }
    Ok(v)
}

fn eval_atom(t: &[Tok], p: &mut usize, syms: &HashMap<String, u32>, pc: u32) -> Result<i64, String> {
    match t.get(*p) {
        Some(Tok::Num(n)) => {
            *p += 1;
            Ok(*n)
        }
        Some(Tok::Id(id)) => {
            *p += 1;
            if id == "." {
                return Ok(pc as i64);
            }
            syms.get(id).map(|v| *v as i64).ok_or_else(|| format!("undefined symbol '{}'", id))
        }
        Some(Tok::Op("-")) => {
            *p += 1;
            Ok(-eval_atom(t, p, syms, pc)?)
        }
        Some(Tok::Op("~")) => {
            *p += 1;
            Ok(!eval_atom(t, p, syms, pc)?)
        }
        Some(Tok::LP) => {
            *p += 1;
            let v = eval_expr(t, p, syms, pc)?;
            if t.get(*p) != Some(&Tok::RP) {
                return Err("missing ')'".into());
            }
            *p += 1;
            Ok(v)
        }
        other => Err(format!("unexpected {:?}", other)),
    }
}

fn prec(op: &str) -> u8 {
    match op {
        "|" => 1,
        "&" => 2,
        "<<" | ">>" => 3,
        "+" | "-" => 4,
        _ => 5, // * /
    }
}

/// Precedence climbing: `| < & < shifts < + - < * /` (C order).
fn eval_expr(t: &[Tok], p: &mut usize, syms: &HashMap<String, u32>, pc: u32) -> Result<i64, String> {
    eval_prec(t, p, syms, pc, 1)
}

fn eval_prec(t: &[Tok], p: &mut usize, syms: &HashMap<String, u32>, pc: u32, min: u8) -> Result<i64, String> {
    let mut v = eval_atom(t, p, syms, pc)?;
    while let Some(Tok::Op(op)) = t.get(*p) {
        let pr = prec(op);
        if pr < min || *op == "~" {
            break;
        }
        *p += 1;
        let r = eval_prec(t, p, syms, pc, pr + 1)?;
        v = match *op {
            "+" => v.wrapping_add(r),
            "-" => v.wrapping_sub(r),
            "*" => v.wrapping_mul(r),
            "/" => {
                if r == 0 {
                    return Err("division by zero".into());
                }
                v / r
            }
            "<<" => v << (r & 63),
            ">>" => ((v as u64 & 0xFFFF_FFFF) >> (r & 63)) as i64,
            "&" => v & r,
            "|" => v | r,
            _ => return Err(format!("bad operator {}", op)),
        };
    }
    Ok(v)
}

// --------------------------------------------------------------- encoding

fn reg(s: &str, prefix: char) -> Result<u32, String> {
    let s = s.trim();
    let ok = s.len() >= 2 && s.starts_with(prefix);
    let n = if ok { s[1..].parse::<u32>().ok() } else { None };
    match n {
        Some(n) if n < 16 => Ok(n),
        _ => Err(format!("expected {}0..{}15, got '{}'", prefix, prefix, s)),
    }
}
fn ar(s: &str) -> Result<u32, String> {
    reg(s, 'a')
}
fn fr(s: &str) -> Result<u32, String> {
    reg(s, 'f')
}
fn br(s: &str) -> Result<u32, String> {
    reg(s, 'b')
}

fn sr_number(s: &str, ur: bool) -> Result<u32, String> {
    let s = s.trim().to_ascii_lowercase();
    if let Ok(n) = s.parse::<u32>() {
        return Ok(n);
    }
    if let Some(h) = s.strip_prefix("0x") {
        return u32::from_str_radix(h, 16).map_err(|e| e.to_string());
    }
    let alias = match s.as_str() {
        "intset" if !ur => Some(226),
        "threadptr" if ur => Some(231),
        "fcr" if ur => Some(232),
        "fsr" if ur => Some(233),
        _ => None,
    };
    if let Some(n) = alias {
        return Ok(n);
    }
    for n in 0..256 {
        let name = if ur { ur_name(n) } else { sr_name(n) };
        if name.eq_ignore_ascii_case(&s) {
            return Ok(n);
        }
    }
    Err(format!("unknown {} register '{}'", if ur { "user" } else { "special" }, s))
}

fn need(ops: &[String], n: usize) -> Result<(), String> {
    if ops.len() != n {
        Err(format!("expected {} operand(s), got {}", n, ops.len()))
    } else {
        Ok(())
    }
}

fn w3(w: u32) -> Vec<u8> {
    vec![w as u8, (w >> 8) as u8, (w >> 16) as u8]
}
fn w2(w: u32) -> Vec<u8> {
    vec![w as u8, (w >> 8) as u8]
}
fn rrr(op0: u32, op1: u32, op2: u32, r: u32, s: u32, t: u32) -> Vec<u8> {
    w3(op2 << 20 | op1 << 16 | r << 12 | s << 8 | t << 4 | op0)
}
fn range(v: i64, lo: i64, hi: i64, what: &str) -> Result<i64, String> {
    if v < lo || v > hi {
        Err(format!("{} {} out of range {}..{}", what, v, lo, hi))
    } else {
        Ok(v)
    }
}
fn signed(v: u32) -> i64 {
    v as i32 as i64
}

fn li_padding(pc: u32) -> u32 {
    // j (3 bytes) then pad to 4-byte alignment for the literal
    let after_j = pc.wrapping_add(3);
    (4 - (after_j & 3)) & 3
}

fn data_items(ops: &[String]) -> usize {
    ops.len()
}

fn parse_string(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if !(s.starts_with('"') && s.ends_with('"') && s.len() >= 2) {
        return Err(format!("expected a string, got {}", s));
    }
    let mut out = Vec::new();
    let mut it = s[1..s.len() - 1].chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push(10),
                Some('r') => out.push(13),
                Some('t') => out.push(9),
                Some('0') => out.push(0),
                Some(o) => out.extend_from_slice(o.to_string().as_bytes()),
                None => {}
            }
        } else {
            out.extend_from_slice(c.to_string().as_bytes());
        }
    }
    Ok(out)
}

fn size_of(m: &str, ops: &[String], pc: u32, syms: &HashMap<String, u32>) -> Result<u32, String> {
    Ok(match m {
        ".word" | ".long" | ".int" => 4 * data_items(ops) as u32,
        ".short" | ".half" => 2 * data_items(ops) as u32,
        ".byte" => data_items(ops) as u32,
        ".raw3" => 3,
        ".ascii" => parse_string(&ops.join(","))?.len() as u32,
        ".asciz" | ".string" => parse_string(&ops.join(","))?.len() as u32 + 1,
        ".space" | ".skip" | ".zero" => eval(&ops[0], syms, pc)?,
        ".align" | ".balign" => {
            let a = eval(ops.first().map(|s| s.as_str()).unwrap_or("4"), syms, pc)?.max(1);
            (a - pc % a) % a
        }
        ".org" => {
            let t = eval(&ops[0], syms, pc)?;
            if t < pc {
                return Err(format!(".org 0x{:x} is behind the current address 0x{:x}", t, pc));
            }
            t - pc
        }
        ".text" | ".data" | ".global" | ".globl" | ".type" | ".size" | ".section" => 0,
        "li" | "la" => 3 + li_padding(pc) + 4 + 3,
        "mac16" | "tlb/cache-op" => 3,
        _ => {
            if m.ends_with(".n") || m == "mov.n" {
                2
            } else {
                3
            }
        }
    })
}

fn emit(m: &str, ops: &[String], pc: u32, syms: &HashMap<String, u32>) -> Result<Vec<u8>, String> {
    let e = |i: usize| -> Result<i64, String> {
        let s = ops.get(i).ok_or_else(|| format!("missing operand {}", i + 1))?;
        Ok(signed(eval(s, syms, pc)?))
    };
    let eu = |i: usize| -> Result<u32, String> {
        let s = ops.get(i).ok_or_else(|| format!("missing operand {}", i + 1))?;
        eval(s, syms, pc)
    };
    let a = |i: usize| ar(ops.get(i).map(|s| s.as_str()).unwrap_or(""));
    let f = |i: usize| fr(ops.get(i).map(|s| s.as_str()).unwrap_or(""));
    let b = |i: usize| br(ops.get(i).map(|s| s.as_str()).unwrap_or(""));
    // branch offset relative to pc + 4
    let rel = |i: usize| -> Result<i64, String> { Ok(eu(i)?.wrapping_sub(pc.wrapping_add(4)) as i32 as i64) };
    let rrr0 = |op1: u32, op2: u32, r: u32, s: u32, t: u32| rrr(0, op1, op2, r, s, t);

    // ---------------------------------------------------------- directives
    match m {
        ".word" | ".long" | ".int" => {
            let mut v = Vec::new();
            for i in 0..ops.len() {
                v.extend_from_slice(&eu(i)?.to_le_bytes());
            }
            return Ok(v);
        }
        ".short" | ".half" => {
            let mut v = Vec::new();
            for i in 0..ops.len() {
                v.extend_from_slice(&(eu(i)? as u16).to_le_bytes());
            }
            return Ok(v);
        }
        ".byte" => {
            let mut v = Vec::new();
            for i in 0..ops.len() {
                v.push(eu(i)? as u8);
            }
            return Ok(v);
        }
        ".raw3" | "mac16" | "tlb/cache-op" => {
            need(ops, 1)?;
            return Ok(w3(eu(0)?));
        }
        ".ascii" => return parse_string(&ops.join(",")),
        ".asciz" | ".string" => {
            let mut v = parse_string(&ops.join(","))?;
            v.push(0);
            return Ok(v);
        }
        ".space" | ".skip" | ".zero" | ".align" | ".balign" | ".org" => {
            let n = size_of(m, ops, pc, syms)?;
            return Ok(vec![0; n as usize]);
        }
        ".text" | ".data" | ".global" | ".globl" | ".type" | ".size" | ".section" => return Ok(vec![]),
        _ => {}
    }

    // ---------------------------------------------------------- pseudo ops
    match m {
        "li" | "la" => {
            need(ops, 2)?;
            let t = a(0)?;
            let v = eu(1)?;
            let pad = li_padding(pc);
            let lit = pc + 3 + pad;
            let l32r_pc = lit + 4;
            let mut out = Vec::new();
            // j over the literal
            let off = (l32r_pc as i64) - (pc as i64 + 4);
            out.extend(w3(((off as u32) & 0x3FFFF) << 6 | 6));
            out.extend(vec![0u8; pad as usize]);
            out.extend_from_slice(&v.to_le_bytes());
            let imm = ((lit as i64) - ((l32r_pc as i64 + 3) & !3)) >> 2;
            out.extend(w3(((imm as u32) & 0xFFFF) << 8 | t << 4 | 1));
            return Ok(out);
        }
        "mov" => {
            need(ops, 2)?;
            return Ok(rrr0(0, 2, a(0)?, a(1)?, a(1)?));
        }
        _ => {}
    }

    // ------------------------------------------------------------ narrow
    match m {
        "l32i.n" | "s32i.n" => {
            need(ops, 3)?;
            let off = range(e(2)?, 0, 60, "offset")?;
            if off % 4 != 0 {
                return Err("offset must be a multiple of 4".into());
            }
            let op0 = if m == "l32i.n" { 8 } else { 9 };
            return Ok(w2((off as u32 / 4) << 12 | a(1)? << 8 | a(0)? << 4 | op0));
        }
        "add.n" => {
            need(ops, 3)?;
            return Ok(w2(a(0)? << 12 | a(1)? << 8 | a(2)? << 4 | 0xA));
        }
        "addi.n" => {
            need(ops, 3)?;
            let imm = e(2)?;
            let t = match imm {
                -1 => 0,
                1..=15 => imm as u32,
                _ => return Err(format!("addi.n immediate {} must be -1 or 1..15", imm)),
            };
            return Ok(w2(a(0)? << 12 | a(1)? << 8 | t << 4 | 0xB));
        }
        "movi.n" => {
            need(ops, 2)?;
            let imm = range(e(1)?, -32, 95, "movi.n immediate")?;
            let v = if imm < 0 { (imm + 128) as u32 } else { imm as u32 };
            return Ok(w2((v & 15) << 12 | a(0)? << 8 | ((v >> 4) & 7) << 4 | 0xC));
        }
        "beqz.n" | "bnez.n" => {
            need(ops, 2)?;
            let off = range(rel(1)?, 0, 63, "narrow branch offset")? as u32;
            let t = 8 | if m == "bnez.n" { 4 } else { 0 } | (off >> 4);
            return Ok(w2((off & 15) << 12 | a(0)? << 8 | t << 4 | 0xC));
        }
        "mov.n" => {
            need(ops, 2)?;
            return Ok(w2(a(1)? << 8 | a(0)? << 4 | 0xD));
        }
        "ret.n" => return Ok(w2(0xF00D)),
        "retw.n" => return Ok(w2(0xF01D)),
        "break.n" => {
            need(ops, 1)?;
            return Ok(w2(0xF02D | (eu(0)? & 15) << 8));
        }
        "nop.n" => return Ok(w2(0xF03D)),
        "ill.n" => return Ok(w2(0xF06D)),
        _ => {}
    }

    // -------------------------------------------------------------- wide
    let three = |op1: u32, op2: u32| -> Result<Vec<u8>, String> {
        need(ops, 3)?;
        Ok(rrr0(op1, op2, a(0)?, a(1)?, a(2)?))
    };
    let fp3 = |op1: u32, op2: u32| -> Result<Vec<u8>, String> {
        need(ops, 3)?;
        Ok(rrr0(op1, op2, f(0)?, f(1)?, f(2)?))
    };
    let fp2 = |t: u32| -> Result<Vec<u8>, String> {
        need(ops, 2)?;
        Ok(rrr0(0xA, 0xF, f(0)?, f(1)?, t))
    };
    let bool3 = |op2: u32| -> Result<Vec<u8>, String> {
        need(ops, 3)?;
        Ok(rrr0(2, op2, b(0)?, b(1)?, b(2)?))
    };
    let cmp_s = |op2: u32| -> Result<Vec<u8>, String> {
        need(ops, 3)?;
        Ok(rrr0(0xB, op2, b(0)?, f(1)?, f(2)?))
    };
    let movcc_s = |op2: u32| -> Result<Vec<u8>, String> {
        need(ops, 3)?;
        Ok(rrr0(0xB, op2, f(0)?, f(1)?, a(2)?))
    };
    let ld = |r: u32, scale: i64| -> Result<Vec<u8>, String> {
        need(ops, 3)?;
        let off = e(2)?;
        if off % scale != 0 {
            return Err(format!("offset {} must be a multiple of {}", off, scale));
        }
        let imm = range(off / scale, 0, 255, "offset/scale")? as u32;
        Ok(w3(imm << 16 | r << 12 | a(1)? << 8 | a(0)? << 4 | 2))
    };
    let bri12 = |mm: u32| -> Result<Vec<u8>, String> {
        need(ops, 2)?;
        let off = range(rel(1)?, -2048, 2047, "branch offset")? as u32;
        Ok(w3((off & 0xFFF) << 12 | a(0)? << 8 | mm << 6 | 1 << 4 | 6))
    };
    let bri8_const = |mm: u32, n: u32, unsigned: bool| -> Result<Vec<u8>, String> {
        need(ops, 3)?;
        let c = e(1)?;
        let r = if unsigned {
            crate::decode::B4CONSTU.iter().position(|&x| x as i64 == c)
        } else {
            crate::decode::B4CONST.iter().position(|&x| x as i64 == c)
        }
        .ok_or_else(|| format!("constant {} not encodable (b4const{})", c, if unsigned { "u" } else { "" }))? as u32;
        let off = range(rel(2)?, -128, 127, "branch offset")? as u32;
        Ok(w3((off & 0xFF) << 16 | r << 12 | a(0)? << 8 | mm << 6 | n << 4 | 6))
    };
    let rri8_branch = |r: u32| -> Result<Vec<u8>, String> {
        need(ops, 3)?;
        let off = range(rel(2)?, -128, 127, "branch offset")? as u32;
        Ok(w3((off & 0xFF) << 16 | r << 12 | a(0)? << 8 | a(1)? << 4 | 7))
    };
    let bbi = |base: u32| -> Result<Vec<u8>, String> {
        need(ops, 3)?;
        let bit = range(e(1)?, 0, 31, "bit")? as u32;
        let off = range(rel(2)?, -128, 127, "branch offset")? as u32;
        Ok(w3((off & 0xFF) << 16 | (base | bit >> 4) << 12 | a(0)? << 8 | (bit & 15) << 4 | 7))
    };
    let loop_ = |r: u32| -> Result<Vec<u8>, String> {
        need(ops, 2)?;
        let off = range(rel(1)?, 0, 255, "loop end offset")? as u32;
        Ok(w3(off << 16 | r << 12 | a(0)? << 8 | 1 << 6 | 3 << 4 | 6))
    };
    let call = |n: u32| -> Result<Vec<u8>, String> {
        need(ops, 1)?;
        let target = eu(0)?;
        if target & 3 != 0 {
            return Err(format!("call target 0x{:x} must be 4-byte aligned", target));
        }
        let off = (target as i64) - ((pc & !3) as i64 + 4);
        let off = range(off >> 2, -(1 << 17), (1 << 17) - 1, "call offset")? as u32;
        Ok(w3((off & 0x3FFFF) << 6 | n << 4 | 5))
    };
    let st0 = |r: u32, s: u32, t: u32| rrr0(0, 0, r, s, t);

    let v = match m {
        // ---- ST0 / control
        "ret" => st0(0, 0, 8),
        "retw" => st0(0, 0, 9),
        "jx" => st0(0, a(0)?, 0xA),
        "callx0" => st0(0, a(0)?, 0xC),
        "callx4" => st0(0, a(0)?, 0xD),
        "callx8" => st0(0, a(0)?, 0xE),
        "callx12" => st0(0, a(0)?, 0xF),
        "movsp" => {
            need(ops, 2)?;
            st0(1, a(1)?, a(0)?)
        }
        "isync" => st0(2, 0, 0),
        "rsync" => st0(2, 0, 1),
        "esync" => st0(2, 0, 2),
        "dsync" => st0(2, 0, 3),
        "excw" => st0(2, 0, 8),
        "memw" => st0(2, 0, 0xC),
        "extw" => st0(2, 0, 0xD),
        "nop" => st0(2, 0, 0xF),
        "rfe" => st0(3, 0, 0),
        "rfue" => st0(3, 1, 0),
        "rfde" => st0(3, 2, 0),
        "rfwo" => st0(3, 4, 0),
        "rfwu" => st0(3, 5, 0),
        "rfi" => st0(3, eu(0)? & 15, 1),
        "rfme" => st0(3, 0, 2),
        "break" => {
            need(ops, 2)?;
            st0(4, eu(0)? & 15, eu(1)? & 15)
        }
        "syscall" => st0(5, 0, 0),
        "simcall" => st0(5, 1, 0),
        "rsil" => {
            need(ops, 2)?;
            st0(6, eu(1)? & 15, a(0)?)
        }
        "waiti" => st0(7, eu(0)? & 15, 0),
        "any4" | "all4" | "any8" | "all8" => {
            need(ops, 2)?;
            let r = match m {
                "any4" => 8,
                "all4" => 9,
                "any8" => 0xA,
                _ => 0xB,
            };
            st0(r, b(1)?, b(0)?)
        }
        // ---- RST0 ALU
        "and" => three(0, 1)?,
        "or" => three(0, 2)?,
        "xor" => three(0, 3)?,
        "ssr" => rrr0(0, 4, 0, a(0)?, 0),
        "ssl" => rrr0(0, 4, 1, a(0)?, 0),
        "ssa8l" => rrr0(0, 4, 2, a(0)?, 0),
        "ssa8b" => rrr0(0, 4, 3, a(0)?, 0),
        "ssai" => {
            let v = range(e(0)?, 0, 31, "shift")? as u32;
            rrr0(0, 4, 4, v & 15, v >> 4)
        }
        "rer" => {
            need(ops, 2)?;
            rrr0(0, 4, 6, a(1)?, a(0)?)
        }
        "wer" => {
            need(ops, 2)?;
            rrr0(0, 4, 7, a(1)?, a(0)?)
        }
        "rotw" => rrr0(0, 4, 8, 0, (range(e(0)?, -8, 7, "rotw")? as u32) & 15),
        "nsa" => {
            need(ops, 2)?;
            rrr0(0, 4, 0xE, a(1)?, a(0)?)
        }
        "nsau" => {
            need(ops, 2)?;
            rrr0(0, 4, 0xF, a(1)?, a(0)?)
        }
        "neg" => {
            need(ops, 2)?;
            rrr0(0, 6, a(0)?, 0, a(1)?)
        }
        "abs" => {
            need(ops, 2)?;
            rrr0(0, 6, a(0)?, 1, a(1)?)
        }
        "add" => three(0, 8)?,
        "addx2" => three(0, 9)?,
        "addx4" => three(0, 0xA)?,
        "addx8" => three(0, 0xB)?,
        "sub" => three(0, 0xC)?,
        "subx2" => three(0, 0xD)?,
        "subx4" => three(0, 0xE)?,
        "subx8" => three(0, 0xF)?,
        // ---- RST1 shifts / mul16 / xsr
        "slli" => {
            need(ops, 3)?;
            let sa = range(e(2)?, 1, 32, "shift")? as u32;
            let enc = 32 - sa;
            rrr0(1, enc >> 4, a(0)?, a(1)?, enc & 15)
        }
        "srai" => {
            need(ops, 3)?;
            let sa = range(e(2)?, 0, 31, "shift")? as u32;
            rrr0(1, 2 | (sa >> 4), a(0)?, sa & 15, a(1)?)
        }
        "srli" => {
            need(ops, 3)?;
            let sa = range(e(2)?, 0, 15, "shift")? as u32;
            rrr0(1, 4, a(0)?, sa, a(1)?)
        }
        "xsr" | "rsr" | "wsr" => {
            need(ops, 2)?;
            let n = sr_number(&ops[1], false)?;
            let (op1, op2) = match m {
                "xsr" => (1, 6),
                "rsr" => (3, 0),
                _ => (3, 1),
            };
            rrr0(op1, op2, n >> 4, n & 15, a(0)?)
        }
        "src" => three(1, 8)?,
        "srl" => {
            need(ops, 2)?;
            rrr0(1, 9, a(0)?, 0, a(1)?)
        }
        "sll" => {
            need(ops, 2)?;
            rrr0(1, 0xA, a(0)?, a(1)?, 0)
        }
        "sra" => {
            need(ops, 2)?;
            rrr0(1, 0xB, a(0)?, 0, a(1)?)
        }
        "mul16u" => three(1, 0xC)?,
        "mul16s" => three(1, 0xD)?,
        // ---- RST2 booleans / mul / div
        "andb" => bool3(0)?,
        "andbc" => bool3(1)?,
        "orb" => bool3(2)?,
        "orbc" => bool3(3)?,
        "xorb" => bool3(4)?,
        "mull" => three(2, 8)?,
        "muluh" => three(2, 0xA)?,
        "mulsh" => three(2, 0xB)?,
        "quou" => three(2, 0xC)?,
        "quos" => three(2, 0xD)?,
        "remu" => three(2, 0xE)?,
        "rems" => three(2, 0xF)?,
        // ---- RST3
        "sext" | "clamps" => {
            need(ops, 3)?;
            let bits = range(e(2)?, 7, 22, "bit position")? as u32;
            rrr0(3, if m == "sext" { 2 } else { 3 }, a(0)?, a(1)?, bits - 7)
        }
        "min" => three(3, 4)?,
        "max" => three(3, 5)?,
        "minu" => three(3, 6)?,
        "maxu" => three(3, 7)?,
        "moveqz" => three(3, 8)?,
        "movnez" => three(3, 9)?,
        "movltz" => three(3, 0xA)?,
        "movgez" => three(3, 0xB)?,
        "movf" | "movt" => {
            need(ops, 3)?;
            rrr0(3, if m == "movf" { 0xC } else { 0xD }, a(0)?, a(1)?, b(2)?)
        }
        "rur" => {
            need(ops, 2)?;
            let n = sr_number(&ops[1], true)?;
            rrr0(3, 0xE, a(0)?, n >> 4, n & 15)
        }
        "wur" => {
            need(ops, 2)?;
            let n = sr_number(&ops[1], true)?;
            rrr0(3, 0xF, n >> 4, n & 15, a(0)?)
        }
        "extui" => {
            need(ops, 4)?;
            let sh = range(e(2)?, 0, 31, "shift")? as u32;
            let bits = range(e(3)?, 1, 16, "mask bits")? as u32;
            rrr0(4 | (sh >> 4), bits - 1, a(0)?, sh & 15, a(1)?)
        }
        // ---- floating point
        "lsx" | "lsxu" | "ssx" | "ssxu" => {
            need(ops, 3)?;
            let op2 = match m {
                "lsx" => 0,
                "lsxu" => 1,
                "ssx" => 4,
                _ => 5,
            };
            rrr0(8, op2, f(0)?, a(1)?, a(2)?)
        }
        "l32e" | "s32e" => {
            need(ops, 3)?;
            let off = range(e(2)?, -64, -4, "offset")?;
            if off % 4 != 0 {
                return Err("offset must be a multiple of 4".into());
            }
            rrr0(9, if m == "l32e" { 0 } else { 4 }, ((off + 64) / 4) as u32, a(1)?, a(0)?)
        }
        "add.s" => fp3(0xA, 0)?,
        "sub.s" => fp3(0xA, 1)?,
        "mul.s" => fp3(0xA, 2)?,
        "madd.s" => fp3(0xA, 4)?,
        "msub.s" => fp3(0xA, 5)?,
        "maddn.s" => fp3(0xA, 6)?,
        "divn.s" => fp3(0xA, 7)?,
        "round.s" | "trunc.s" | "floor.s" | "ceil.s" | "utrunc.s" => {
            need(ops, 3)?;
            let op2 = match m {
                "round.s" => 8,
                "trunc.s" => 9,
                "floor.s" => 0xA,
                "ceil.s" => 0xB,
                _ => 0xE,
            };
            rrr0(0xA, op2, a(0)?, f(1)?, eu(2)? & 15)
        }
        "float.s" | "ufloat.s" => {
            need(ops, 3)?;
            rrr0(0xA, if m == "float.s" { 0xC } else { 0xD }, f(0)?, a(1)?, eu(2)? & 15)
        }
        "mov.s" => fp2(0)?,
        "abs.s" => fp2(1)?,
        "const.s" => {
            need(ops, 2)?;
            rrr0(0xA, 0xF, f(0)?, eu(1)? & 15, 3)
        }
        "rfr" => {
            need(ops, 2)?;
            rrr0(0xA, 0xF, a(0)?, f(1)?, 4)
        }
        "wfr" => {
            need(ops, 2)?;
            rrr0(0xA, 0xF, f(0)?, a(1)?, 5)
        }
        "neg.s" => fp2(6)?,
        "div0.s" => fp2(7)?,
        "recip0.s" => fp2(8)?,
        "sqrt0.s" => fp2(9)?,
        "rsqrt0.s" => fp2(0xA)?,
        "nexp01.s" => fp2(0xB)?,
        "mksadj.s" => fp2(0xC)?,
        "mkdadj.s" => fp2(0xD)?,
        "addexp.s" => fp2(0xE)?,
        "addexpm.s" => fp2(0xF)?,
        "un.s" => cmp_s(1)?,
        "oeq.s" => cmp_s(2)?,
        "ueq.s" => cmp_s(3)?,
        "olt.s" => cmp_s(4)?,
        "ult.s" => cmp_s(5)?,
        "ole.s" => cmp_s(6)?,
        "ule.s" => cmp_s(7)?,
        "moveqz.s" => movcc_s(8)?,
        "movnez.s" => movcc_s(9)?,
        "movltz.s" => movcc_s(0xA)?,
        "movgez.s" => movcc_s(0xB)?,
        "movf.s" | "movt.s" => {
            need(ops, 3)?;
            rrr0(0xB, if m == "movf.s" { 0xC } else { 0xD }, f(0)?, f(1)?, b(2)?)
        }
        "lsi" | "ssi" | "lsiu" | "ssiu" => {
            need(ops, 3)?;
            let r = match m {
                "lsi" => 0,
                "ssi" => 4,
                "lsiu" => 8,
                _ => 0xC,
            };
            let off = e(2)?;
            if off % 4 != 0 {
                return Err("offset must be a multiple of 4".into());
            }
            let imm = range(off / 4, 0, 255, "offset/4")? as u32;
            w3(imm << 16 | r << 12 | a(1)? << 8 | f(0)? << 4 | 3)
        }
        // ---- L32R
        "l32r" => {
            need(ops, 2)?;
            let target = eu(1)?;
            let base = (pc.wrapping_add(3) & !3) as i64;
            let off = target as i64 - base;
            if off % 4 != 0 {
                return Err("l32r literal must be 4-byte aligned".into());
            }
            range(off, -(1 << 18), -4, "l32r offset")?;
            w3((((off >> 2) as u32) & 0xFFFF) << 8 | a(0)? << 4 | 1)
        }
        // ---- LSAI
        "l8ui" => ld(0, 1)?,
        "l16ui" => ld(1, 2)?,
        "l32i" => ld(2, 4)?,
        "s8i" => ld(4, 1)?,
        "s16i" => ld(5, 2)?,
        "s32i" => ld(6, 4)?,
        "l16si" => ld(9, 2)?,
        "l32ai" => ld(0xB, 4)?,
        "s32c1i" => ld(0xE, 4)?,
        "s32ri" => ld(0xF, 4)?,
        "cache-op" => {
            need(ops, 3)?;
            let imm = range(e(2)? / 4, 0, 255, "offset/4")? as u32;
            w3(imm << 16 | 7 << 12 | a(1)? << 8 | (eu(0)? & 15) << 4 | 2)
        }
        "movi" => {
            need(ops, 2)?;
            let v = range(e(1)?, -2048, 2047, "movi immediate (use li for 32-bit values)")? as u32 & 0xFFF;
            w3((v & 0xFF) << 16 | 0xA << 12 | (v >> 8) << 8 | a(0)? << 4 | 2)
        }
        "addi" => {
            need(ops, 3)?;
            let v = range(e(2)?, -128, 127, "addi immediate")? as u32;
            w3((v & 0xFF) << 16 | 0xC << 12 | a(1)? << 8 | a(0)? << 4 | 2)
        }
        "addmi" => {
            need(ops, 3)?;
            let v = e(2)?;
            if v % 256 != 0 {
                return Err("addmi immediate must be a multiple of 256".into());
            }
            let v = range(v / 256, -128, 127, "addmi immediate/256")? as u32;
            w3((v & 0xFF) << 16 | 0xD << 12 | a(1)? << 8 | a(0)? << 4 | 2)
        }
        // ---- calls / jumps
        "call0" => call(0)?,
        "call4" => call(1)?,
        "call8" => call(2)?,
        "call12" => call(3)?,
        "j" => {
            need(ops, 1)?;
            let off = range(rel(0)?, -(1 << 17), (1 << 17) - 1, "jump offset")? as u32;
            w3((off & 0x3FFFF) << 6 | 6)
        }
        "beqz" => bri12(0)?,
        "bnez" => bri12(1)?,
        "bltz" => bri12(2)?,
        "bgez" => bri12(3)?,
        "beqi" => bri8_const(0, 2, false)?,
        "bnei" => bri8_const(1, 2, false)?,
        "blti" => bri8_const(2, 2, false)?,
        "bgei" => bri8_const(3, 2, false)?,
        "bltui" => bri8_const(2, 3, true)?,
        "bgeui" => bri8_const(3, 3, true)?,
        "entry" => {
            need(ops, 2)?;
            let fs = e(1)?;
            if fs % 8 != 0 {
                return Err("entry frame size must be a multiple of 8".into());
            }
            let imm = range(fs / 8, 0, 4095, "frame size/8")? as u32;
            w3(imm << 12 | a(0)? << 8 | 3 << 4 | 6)
        }
        "bf" | "bt" => {
            need(ops, 2)?;
            let off = range(rel(1)?, -128, 127, "branch offset")? as u32;
            w3((off & 0xFF) << 16 | (if m == "bt" { 1 } else { 0 }) << 12 | b(0)? << 8 | 1 << 6 | 3 << 4 | 6)
        }
        "loop" => loop_(8)?,
        "loopnez" => loop_(9)?,
        "loopgtz" => loop_(0xA)?,
        "bnone" => rri8_branch(0)?,
        "beq" => rri8_branch(1)?,
        "blt" => rri8_branch(2)?,
        "bltu" => rri8_branch(3)?,
        "ball" => rri8_branch(4)?,
        "bbc" => rri8_branch(5)?,
        "bany" => rri8_branch(8)?,
        "bne" => rri8_branch(9)?,
        "bge" => rri8_branch(0xA)?,
        "bgeu" => rri8_branch(0xB)?,
        "bnall" => rri8_branch(0xC)?,
        "bbs" => rri8_branch(0xD)?,
        "bbci" => bbi(6)?,
        "bbsi" => bbi(0xE)?,
        "ill" => w3(0),
        _ => return Err(format!("unknown instruction '{}'", m)),
    };
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_expressions() {
        let p = assemble(
            "
            .equ K, 0x10 + 2
            start: movi a2, K * 2   ; comment
            loop1: addi a2, a2, -1
                   bnez a2, loop1
            end:   .word end - start, 'A'
            ",
            0x4008_0000,
        )
        .unwrap();
        assert_eq!(p.symbol("K"), 0x12);
        assert_eq!(p.symbol("loop1"), 0x4008_0003);
        assert_eq!(p.symbol("end"), 0x4008_0009);
        assert_eq!(&p.bytes[9..13], &9u32.to_le_bytes());
        assert_eq!(&p.bytes[13..17], &65u32.to_le_bytes());
        assert_eq!(p.lines.len(), 3);
    }

    #[test]
    fn operator_precedence() {
        let p = assemble(".equ A, 0x104 + 22 * 4\n.equ B, 1 << 2 + 1\n.equ C, 6 | 1 & 3\n.equ D, (2 + 3) * 4\n.equ E, 10 - 4 - 3", 0).unwrap();
        assert_eq!(p.symbol("A"), 0x104 + 88);
        assert_eq!(p.symbol("B"), 8);
        assert_eq!(p.symbol("C"), 7);
        assert_eq!(p.symbol("D"), 20);
        assert_eq!(p.symbol("E"), 3);
    }

    #[test]
    fn li_loads_any_constant() {
        for base in [0x4008_0000u32, 0x4008_0001, 0x4008_0002, 0x4008_0003] {
            let p = assemble("li a5, 0xdeadbeef", base).unwrap();
            // the literal sits at the first aligned address after the jump
            let lit = (base + 3 + 3) & !3;
            let off = (lit - base) as usize;
            assert_eq!(&p.bytes[off..off + 4], &0xdeadbeefu32.to_le_bytes());
        }
    }

    #[test]
    fn errors_are_reported_with_lines() {
        let e = assemble("nop\n  movi a2, 5000", 0).unwrap_err();
        assert_eq!(e.line, 2);
        assert!(e.msg.contains("out of range"), "{}", e.msg);
        assert!(assemble("foo a1", 0).is_err());
        assert!(assemble("x: nop\nx: nop", 0).is_err());
        assert!(assemble("j nowhere", 0).is_err());
    }
}

//! Local variables for the debugger: walks the DWARF scopes containing the PC,
//! evaluates location expressions (registers, frame base, location lists) and
//! formats values according to their types.

use addr2line::gimli;
use gimli::AttributeValue;
use serde_json::{json, Value};

pub type Slice = gimli::EndianSlice<'static, gimli::RunTimeEndian>;

/// Target state needed to evaluate locations.
pub struct Target<'a> {
    /// Xtensa AR register (current window) by DWARF number 0..15
    pub reg: &'a dyn Fn(u16) -> u32,
    pub mem: &'a mut dyn FnMut(u32, usize) -> Vec<u8>,
    /// stack pointer value at function entry (before ENTRY), i.e. the CFA
    pub cfa: Option<u32>,
}

pub struct Locals {
    dwarf: gimli::Dwarf<Slice>,
    /// (unit header offset, [(lo, hi)]) for quick lookup
    units: Vec<(gimli::DebugInfoOffset<usize>, Vec<(u64, u64)>)>,
}

impl Locals {
    pub fn new(dwarf: gimli::Dwarf<Slice>) -> Locals {
        let mut units = Vec::new();
        let mut it = dwarf.units();
        while let Ok(Some(h)) = it.next() {
            let Some(off) = h.offset().as_debug_info_offset() else { continue };
            let Ok(unit) = dwarf.unit(h) else { continue };
            let mut ranges = Vec::new();
            if let Ok(mut r) = dwarf.unit_ranges(&unit) {
                while let Ok(Some(x)) = r.next() {
                    if x.begin < x.end {
                        ranges.push((x.begin, x.end));
                    }
                }
            }
            if !ranges.is_empty() {
                units.push((off, ranges));
            }
        }
        Locals { dwarf, units }
    }

    /// Local variables and parameters visible at `pc`, innermost scope first.
    pub fn at(&self, pc: u32, t: &mut Target) -> Vec<Value> {
        let pc64 = pc as u64;
        let mut out = Vec::new();
        for (off, ranges) in &self.units {
            if !ranges.iter().any(|&(a, b)| a <= pc64 && pc64 < b) {
                continue;
            }
            let Ok(header) = self.dwarf.debug_info.header_from_offset(*off) else { continue };
            let Ok(unit) = self.dwarf.unit(header) else { continue };
            if let Err(e) = self.scan_unit(&unit, pc64, t, &mut out) {
                out.push(json!({"name": "<error>", "type": "", "value": e.to_string()}));
            }
            break;
        }
        out
    }

    fn scan_unit(&self, unit: &gimli::Unit<Slice>, pc: u64, t: &mut Target, out: &mut Vec<Value>) -> gimli::Result<()> {
        let mut tree = unit.entries_tree(None)?;
        let root = tree.root()?;
        let mut scopes: Vec<Vec<Value>> = Vec::new();
        self.walk(unit, root, pc, t, &mut scopes, None)?;
        // innermost scope first, skip duplicates (shadowed names)
        let mut seen = std::collections::HashSet::new();
        for scope in scopes.into_iter().rev() {
            for v in scope {
                let name = v["name"].as_str().unwrap_or("").to_string();
                if seen.insert(name) {
                    out.push(v);
                }
            }
        }
        Ok(())
    }

    fn contains(&self, unit: &gimli::Unit<Slice>, e: &gimli::DebuggingInformationEntry<Slice>, pc: u64) -> bool {
        if let Ok(mut r) = self.dwarf.die_ranges(unit, e) {
            while let Ok(Some(x)) = r.next() {
                if x.begin <= pc && pc < x.end {
                    return true;
                }
            }
        }
        false
    }

    /// Depth-first walk down the scopes containing `pc`; each matching scope
    /// contributes its variables.
    fn walk(
        &self,
        unit: &gimli::Unit<Slice>,
        node: gimli::EntriesTreeNode<Slice>,
        pc: u64,
        t: &mut Target,
        scopes: &mut Vec<Vec<Value>>,
        frame_base: Option<u32>,
    ) -> gimli::Result<()> {
        let mut children = node.children();
        while let Some(child) = children.next()? {
            let e = child.entry();
            let tag = e.tag();
            let is_scope = matches!(tag, gimli::DW_TAG_subprogram | gimli::DW_TAG_lexical_block | gimli::DW_TAG_inlined_subroutine);
            if is_scope {
                if !self.contains(unit, e, pc) {
                    continue;
                }
                let mut fb = frame_base;
                if tag == gimli::DW_TAG_subprogram {
                    fb = self.frame_base(unit, e, pc, t);
                }
                let mut vars = Vec::new();
                // variables declared directly in this scope
                let mut sub = child.children();
                let mut nested: Vec<gimli::UnitOffset<usize>> = Vec::new();
                while let Some(v) = sub.next()? {
                    let ve = v.entry();
                    match ve.tag() {
                        gimli::DW_TAG_variable | gimli::DW_TAG_formal_parameter => {
                            if let Some(val) = self.variable(unit, ve, pc, t, fb) {
                                vars.push(val);
                            }
                        }
                        gimli::DW_TAG_lexical_block | gimli::DW_TAG_inlined_subroutine => {
                            if self.contains(unit, ve, pc) {
                                nested.push(ve.offset());
                            }
                        }
                        _ => {}
                    }
                }
                scopes.push(vars);
                for off in nested {
                    let mut tr = unit.entries_tree(Some(off))?;
                    let n = tr.root()?;
                    // the nested scope itself: collect its variables via a one-level wrapper
                    self.walk_scope(unit, n, pc, t, scopes, fb)?;
                }
                return Ok(());
            } else if matches!(tag, gimli::DW_TAG_namespace | gimli::DW_TAG_structure_type | gimli::DW_TAG_class_type) {
                // C++ methods / Rust functions live inside namespaces
                self.walk(unit, child, pc, t, scopes, frame_base)?;
                if !scopes.is_empty() {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn walk_scope(
        &self,
        unit: &gimli::Unit<Slice>,
        node: gimli::EntriesTreeNode<Slice>,
        pc: u64,
        t: &mut Target,
        scopes: &mut Vec<Vec<Value>>,
        fb: Option<u32>,
    ) -> gimli::Result<()> {
        let mut vars = Vec::new();
        let mut nested = Vec::new();
        let mut ch = node.children();
        while let Some(v) = ch.next()? {
            let ve = v.entry();
            match ve.tag() {
                gimli::DW_TAG_variable | gimli::DW_TAG_formal_parameter => {
                    if let Some(val) = self.variable(unit, ve, pc, t, fb) {
                        vars.push(val);
                    }
                }
                gimli::DW_TAG_lexical_block | gimli::DW_TAG_inlined_subroutine => {
                    if self.contains(unit, ve, pc) {
                        nested.push(ve.offset());
                    }
                }
                _ => {}
            }
        }
        scopes.push(vars);
        for off in nested {
            let mut tr = unit.entries_tree(Some(off))?;
            let n = tr.root()?;
            self.walk_scope(unit, n, pc, t, scopes, fb)?;
        }
        Ok(())
    }

    fn frame_base(&self, unit: &gimli::Unit<Slice>, e: &gimli::DebuggingInformationEntry<Slice>, pc: u64, t: &mut Target) -> Option<u32> {
        let attr = e.attr_value(gimli::DW_AT_frame_base).ok()??;
        let expr = match attr {
            AttributeValue::Exprloc(x) => x,
            _ => return None,
        };
        match self.eval(unit, expr, pc, t, None)? {
            Loc::Reg(r) => Some((t.reg)(r)),
            Loc::Addr(a) => Some(a),
            Loc::Value(v) => Some(v as u32),
            Loc::Unavailable => None,
        }
    }

    fn variable(&self, unit: &gimli::Unit<Slice>, e: &gimli::DebuggingInformationEntry<Slice>, pc: u64, t: &mut Target, fb: Option<u32>) -> Option<Value> {
        // abstract origin (inlined variables) carries name / type
        let origin = match e.attr_value(gimli::DW_AT_abstract_origin).ok()? {
            Some(AttributeValue::UnitRef(o)) => unit.entry(o).ok(),
            _ => None,
        };
        let get = |at: gimli::DwAt| -> Option<AttributeValue<Slice>> {
            e.attr_value(at).ok().flatten().or_else(|| origin.as_ref().and_then(|o| o.attr_value(at).ok().flatten()))
        };
        let name = match get(gimli::DW_AT_name) {
            Some(v) => self.dwarf.attr_string(unit, v).ok()?.to_string_lossy().into_owned(),
            None => return None,
        };
        let ty = match get(gimli::DW_AT_type) {
            Some(AttributeValue::UnitRef(o)) => Some(o),
            _ => None,
        };
        let (tname, tinfo) = ty.map(|o| self.type_info(unit, o, 0)).unwrap_or(("?".into(), TypeInfo::Unknown(0)));
        // constant values (optimised-out constants)
        if let Some(c) = e.attr_value(gimli::DW_AT_const_value).ok().flatten() {
            let v = c.sdata_value().map(|x| x.to_string()).or_else(|| c.udata_value().map(|x| x.to_string())).unwrap_or_else(|| "<const>".into());
            return Some(json!({"name": name, "type": tname, "value": v}));
        }
        let loc = match e.attr_value(gimli::DW_AT_location).ok().flatten() {
            Some(AttributeValue::Exprloc(x)) => self.eval(unit, x, pc, t, fb),
            Some(AttributeValue::LocationListsRef(off)) => self.loclist(unit, off, pc, t, fb),
            Some(AttributeValue::SecOffset(off)) => self.loclist(unit, gimli::LocationListsOffset(off), pc, t, fb),
            _ => None,
        };
        let value = match loc {
            None | Some(Loc::Unavailable) => "<optimized out>".to_string(),
            Some(l) => format_value(&tinfo, &l, t),
        };
        Some(json!({"name": name, "type": tname, "value": value}))
    }

    fn loclist(&self, unit: &gimli::Unit<Slice>, off: gimli::LocationListsOffset<usize>, pc: u64, t: &mut Target, fb: Option<u32>) -> Option<Loc> {
        let mut it = self.dwarf.locations(unit, off).ok()?;
        while let Ok(Some(entry)) = it.next() {
            if entry.range.begin <= pc && pc < entry.range.end {
                return self.eval(unit, entry.data, pc, t, fb);
            }
        }
        Some(Loc::Unavailable)
    }

    fn eval(&self, unit: &gimli::Unit<Slice>, expr: gimli::Expression<Slice>, _pc: u64, t: &mut Target, fb: Option<u32>) -> Option<Loc> {
        let mut ev = expr.evaluation(unit.encoding());
        let mut res = ev.evaluate().ok()?;
        for _ in 0..64 {
            match res {
                gimli::EvaluationResult::Complete => break,
                gimli::EvaluationResult::RequiresRegister { register, .. } => {
                    let v = (t.reg)(register.0);
                    res = ev.resume_with_register(gimli::Value::Generic(v as u64)).ok()?;
                }
                gimli::EvaluationResult::RequiresFrameBase => {
                    res = ev.resume_with_frame_base(fb? as u64).ok()?;
                }
                gimli::EvaluationResult::RequiresCallFrameCfa => {
                    res = ev.resume_with_call_frame_cfa(t.cfa? as u64).ok()?;
                }
                gimli::EvaluationResult::RequiresMemory { address, size, .. } => {
                    let b = (t.mem)(address as u32, size as usize);
                    let mut v = 0u64;
                    for (i, x) in b.iter().enumerate().take(8) {
                        v |= (*x as u64) << (8 * i);
                    }
                    res = ev.resume_with_memory(gimli::Value::Generic(v)).ok()?;
                }
                gimli::EvaluationResult::RequiresRelocatedAddress(a) => {
                    res = ev.resume_with_relocated_address(a).ok()?;
                }
                gimli::EvaluationResult::RequiresEntryValue(_) => return Some(Loc::Unavailable),
                _ => return Some(Loc::Unavailable),
            }
        }
        let pieces = ev.result();
        let p = pieces.first()?;
        Some(match &p.location {
            gimli::Location::Register { register } => Loc::Reg(register.0),
            gimli::Location::Address { address } => Loc::Addr(*address as u32),
            gimli::Location::Value { value } => Loc::Value(match value {
                gimli::Value::Generic(v) => *v,
                gimli::Value::U32(v) => *v as u64,
                gimli::Value::I32(v) => *v as i64 as u64,
                gimli::Value::F32(v) => v.to_bits() as u64,
                _ => 0,
            }),
            _ => Loc::Unavailable,
        })
    }

    fn type_info(&self, unit: &gimli::Unit<Slice>, off: gimli::UnitOffset<usize>, depth: u32) -> (String, TypeInfo) {
        if depth > 8 {
            return ("…".into(), TypeInfo::Unknown(0));
        }
        let Ok(e) = unit.entry(off) else { return ("?".into(), TypeInfo::Unknown(0)) };
        let name = e.attr_value(gimli::DW_AT_name).ok().flatten().and_then(|v| self.dwarf.attr_string(unit, v).ok()).map(|s| s.to_string_lossy().into_owned());
        let size = e.attr_value(gimli::DW_AT_byte_size).ok().flatten().and_then(|v| v.udata_value()).unwrap_or(0) as usize;
        let inner = match e.attr_value(gimli::DW_AT_type).ok().flatten() {
            Some(AttributeValue::UnitRef(o)) => Some(o),
            _ => None,
        };
        match e.tag() {
            gimli::DW_TAG_base_type => {
                let enc = e.attr_value(gimli::DW_AT_encoding).ok().flatten();
                let kind = match enc {
                    Some(AttributeValue::Encoding(gimli::DW_ATE_float)) => Base::Float,
                    Some(AttributeValue::Encoding(gimli::DW_ATE_signed)) | Some(AttributeValue::Encoding(gimli::DW_ATE_signed_char)) => Base::Signed,
                    Some(AttributeValue::Encoding(gimli::DW_ATE_boolean)) => Base::Bool,
                    _ => Base::Unsigned,
                };
                (name.unwrap_or_else(|| "?".into()), TypeInfo::Base(kind, size.max(1)))
            }
            gimli::DW_TAG_pointer_type | gimli::DW_TAG_reference_type | gimli::DW_TAG_rvalue_reference_type => {
                let (n, _) = inner.map(|o| self.type_info(unit, o, depth + 1)).unwrap_or(("void".into(), TypeInfo::Unknown(0)));
                let is_char = n.contains("char") || n == "u8";
                (format!("{}*", n), TypeInfo::Pointer(is_char))
            }
            gimli::DW_TAG_typedef | gimli::DW_TAG_const_type | gimli::DW_TAG_volatile_type | gimli::DW_TAG_atomic_type => {
                let (n, ti) = inner.map(|o| self.type_info(unit, o, depth + 1)).unwrap_or(("void".into(), TypeInfo::Unknown(0)));
                (name.unwrap_or(n), ti)
            }
            gimli::DW_TAG_enumeration_type => (name.unwrap_or_else(|| "enum".into()), TypeInfo::Base(Base::Signed, size.max(1))),
            gimli::DW_TAG_array_type => {
                let (n, ti) = inner.map(|o| self.type_info(unit, o, depth + 1)).unwrap_or(("?".into(), TypeInfo::Unknown(0)));
                let elem = ti.size();
                // element count from the subrange child
                let mut count = 0usize;
                if let Ok(mut tr) = unit.entries_tree(Some(off)) {
                    if let Ok(root) = tr.root() {
                        let mut ch = root.children();
                        while let Ok(Some(c)) = ch.next() {
                            let ce = c.entry();
                            if ce.tag() == gimli::DW_TAG_subrange_type {
                                if let Some(n) = ce.attr_value(gimli::DW_AT_count).ok().flatten().and_then(|v| v.udata_value()) {
                                    count = n as usize;
                                } else if let Some(u) = ce.attr_value(gimli::DW_AT_upper_bound).ok().flatten().and_then(|v| v.udata_value()) {
                                    count = u as usize + 1;
                                }
                            }
                        }
                    }
                }
                (format!("{}[{}]", n, count), TypeInfo::Array(Box::new(ti), count, elem))
            }
            gimli::DW_TAG_structure_type | gimli::DW_TAG_class_type | gimli::DW_TAG_union_type => {
                let mut fields = Vec::new();
                if depth < 2 {
                    if let Ok(mut tr) = unit.entries_tree(Some(off)) {
                        if let Ok(root) = tr.root() {
                            let mut ch = root.children();
                            while let Ok(Some(c)) = ch.next() {
                                let ce = c.entry();
                                if ce.tag() != gimli::DW_TAG_member {
                                    continue;
                                }
                                let fname = ce.attr_value(gimli::DW_AT_name).ok().flatten().and_then(|v| self.dwarf.attr_string(unit, v).ok()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                                let foff = ce.attr_value(gimli::DW_AT_data_member_location).ok().flatten().and_then(|v| v.udata_value()).unwrap_or(0) as usize;
                                let fty = match ce.attr_value(gimli::DW_AT_type).ok().flatten() {
                                    Some(AttributeValue::UnitRef(o)) => self.type_info(unit, o, depth + 1).1,
                                    _ => TypeInfo::Unknown(0),
                                };
                                if fields.len() < 16 {
                                    fields.push((fname, foff, fty));
                                }
                            }
                        }
                    }
                }
                (name.unwrap_or_else(|| "struct".into()), TypeInfo::Struct(fields, size))
            }
            _ => (name.unwrap_or_else(|| "?".into()), TypeInfo::Unknown(size)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Base {
    Signed,
    Unsigned,
    Float,
    Bool,
}

#[derive(Debug, Clone)]
enum TypeInfo {
    Base(Base, usize),
    Pointer(bool),
    Array(Box<TypeInfo>, usize, usize),
    Struct(Vec<(String, usize, TypeInfo)>, usize),
    Unknown(usize),
}

impl TypeInfo {
    fn size(&self) -> usize {
        match self {
            TypeInfo::Base(_, s) => *s,
            TypeInfo::Pointer(_) => 4,
            TypeInfo::Array(_, n, e) => n * e,
            TypeInfo::Struct(_, s) => *s,
            TypeInfo::Unknown(s) => *s,
        }
    }
}

enum Loc {
    Reg(u16),
    Addr(u32),
    Value(u64),
    Unavailable,
}

fn bytes_of(l: &Loc, size: usize, t: &mut Target) -> Vec<u8> {
    match l {
        Loc::Reg(r) => {
            let v = (t.reg)(*r) as u64 | (((t.reg)(r + 1) as u64) << 32);
            v.to_le_bytes()[..size.min(8)].to_vec()
        }
        Loc::Addr(a) => (t.mem)(*a, size),
        Loc::Value(v) => v.to_le_bytes()[..size.min(8)].to_vec(),
        Loc::Unavailable => vec![],
    }
}

fn fmt_scalar(ti: &TypeInfo, b: &[u8], t: &mut Target) -> String {
    let u = |n: usize| -> u64 { b.iter().take(n).enumerate().fold(0u64, |a, (i, x)| a | ((*x as u64) << (8 * i))) };
    match ti {
        TypeInfo::Base(k, s) => {
            let s = (*s).min(8);
            if b.len() < s {
                return "<unreadable>".into();
            }
            let raw = u(s);
            match k {
                Base::Float if s == 4 => format!("{}", f32::from_bits(raw as u32)),
                Base::Float => format!("{}", f64::from_bits(raw)),
                Base::Bool => (raw != 0).to_string(),
                Base::Signed => {
                    let sh = 64 - 8 * s as u32;
                    let v = ((raw << sh) as i64) >> sh;
                    if s == 1 && (32..127).contains(&v) {
                        format!("{} '{}'", v, v as u8 as char)
                    } else {
                        v.to_string()
                    }
                }
                Base::Unsigned => {
                    if s == 1 && (32..127).contains(&raw) {
                        format!("{} '{}'", raw, raw as u8 as char)
                    } else {
                        raw.to_string()
                    }
                }
            }
        }
        TypeInfo::Pointer(is_char) => {
            let p = u(4) as u32;
            if *is_char && p != 0 {
                let s = (t.mem)(p, 48);
                let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
                if s[..end].iter().all(|&c| (32..127).contains(&c) || c == b'\n' || c == b'\t') {
                    return format!("0x{:08x} \"{}\"", p, String::from_utf8_lossy(&s[..end]).escape_default());
                }
            }
            format!("0x{:08x}", p)
        }
        _ => b.iter().take(16).map(|x| format!("{:02x}", x)).collect::<Vec<_>>().join(" "),
    }
}

fn format_value(ti: &TypeInfo, l: &Loc, t: &mut Target) -> String {
    let size = ti.size().max(1).min(256);
    let b = bytes_of(l, size, t);
    if b.is_empty() {
        return "<optimized out>".into();
    }
    match ti {
        TypeInfo::Struct(fields, _) if !fields.is_empty() => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(n, off, fti)| {
                    let fs = fti.size().max(1);
                    let fb = if off + fs <= b.len() { b[*off..off + fs].to_vec() } else { vec![] };
                    let v = if fb.is_empty() { "?".into() } else { fmt_scalar(fti, &fb, t) };
                    format!("{}: {}", n, v)
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        TypeInfo::Array(elem, n, es) if *es > 0 => {
            let shown = (*n).min(16);
            let parts: Vec<String> = (0..shown).filter(|i| (i + 1) * es <= b.len()).map(|i| fmt_scalar(elem, &b[i * es..(i + 1) * es], t)).collect();
            format!("[{}{}]", parts.join(", "), if *n > shown { ", …" } else { "" })
        }
        _ => fmt_scalar(ti, &b, t),
    }
}

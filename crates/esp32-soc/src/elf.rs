//! ELF helpers: converting an ELF into an ESP32 app image, and symbol lookup.

use object::{Object, ObjectSegment, ObjectSymbol, SymbolKind};
use std::collections::BTreeMap;

fn is_flash_mapped(a: u32) -> bool {
    (0x3F40_0000..0x3F80_0000).contains(&a) || (0x400C_2000..0x40C0_0000).contains(&a)
}

/// Convert an ELF file into an ESP32 application image (like `esptool elf2image`).
pub fn elf_to_image(data: &[u8]) -> Result<Vec<u8>, String> {
    let f = object::File::parse(data).map_err(|e| format!("ELF parse error: {}", e))?;
    let entry = f.entry() as u32;
    let mut segs: Vec<(u32, Vec<u8>)> = Vec::new();
    for s in f.segments() {
        let d = s.data().map_err(|e| e.to_string())?;
        if d.is_empty() {
            continue;
        }
        segs.push((s.address() as u32, d.to_vec()));
    }
    if segs.is_empty() {
        return Err("ELF has no loadable segments".into());
    }
    Ok(build_app_image(entry, &segs))
}

/// Build an ESP32 application image (`0xE9` header + segments) from
/// `(load address, bytes)` pairs.  Flash-mapped segments (IROM/DROM) are
/// aligned so that `image offset % 64K == address % 64K`, as the ROM expects.
pub fn build_app_image(entry: u32, segs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let (flash, ram): (Vec<_>, Vec<_>) = segs.iter().cloned().partition(|(a, _)| is_flash_mapped(*a));
    let mut out = vec![0u8; 24];
    out[0] = 0xE9;
    out[2] = 2; // DIO
    out[3] = 0x20; // 4MB, 40MHz
    out[4..8].copy_from_slice(&entry.to_le_bytes());
    out[8] = 0xEE;
    let mut nseg = 0u8;
    let push = |out: &mut Vec<u8>, addr: u32, d: &[u8], nseg: &mut u8| {
        let mut d = d.to_vec();
        while d.len() % 4 != 0 {
            d.push(0);
        }
        out.extend_from_slice(&addr.to_le_bytes());
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(&d);
        *nseg += 1;
    };
    // flash segments: make (data offset in image) % 64K == addr % 64K.
    // The image is placed at a 64K aligned partition offset, so image offsets suffice.
    let mut flash_sorted = flash;
    flash_sorted.sort_by_key(|(a, _)| *a);
    for (addr, d) in &flash_sorted {
        let want = addr % 0x10000;
        loop {
            let data_off = (out.len() as u32 + 8) % 0x10000;
            if data_off == want {
                break;
            }
            // padding segment: header 8 bytes + pad bytes
            let cur = out.len() as u32 + 8;
            let mut pad = (want + 0x10000 - (cur % 0x10000)) % 0x10000;
            if pad < 8 {
                pad += 0x10000;
            }
            pad -= 8;
            let zeros = vec![0u8; pad as usize];
            push(&mut out, 0, &zeros, &mut nseg);
        }
        push(&mut out, *addr, d, &mut nseg);
    }
    for (addr, d) in &ram {
        push(&mut out, *addr, d, &mut nseg);
    }
    out[1] = nseg;
    // checksum over segment data (the emulator's loader does not verify it)
    let mut ck: u8 = 0xEF;
    let mut p = 24;
    for _ in 0..nseg {
        let len = u32::from_le_bytes(out[p + 4..p + 8].try_into().unwrap()) as usize;
        for b in &out[p + 8..p + 8 + len] {
            ck ^= *b;
        }
        p += 8 + len;
    }
    while (out.len() + 1) % 16 != 0 {
        out.push(0);
    }
    out.push(ck);
    out
}

#[derive(Default, Clone)]
pub struct Symbols {
    /// start address -> (name, size)
    pub funcs: BTreeMap<u32, (String, u32)>,
    pub by_name: std::collections::HashMap<String, u32>,
}

impl Symbols {
    pub fn from_elf(data: &[u8]) -> Result<Symbols, String> {
        let f = object::File::parse(data).map_err(|e| e.to_string())?;
        let mut s = Symbols::default();
        for sym in f.symbols() {
            let name = match sym.name() {
                Ok(n) if !n.is_empty() => n.to_string(),
                _ => continue,
            };
            let addr = sym.address() as u32;
            s.by_name.insert(name.clone(), addr);
            if sym.kind() == SymbolKind::Text {
                s.funcs.insert(addr, (name, sym.size() as u32));
            } else if sym.kind() == SymbolKind::Data || sym.kind() == SymbolKind::Unknown {
                s.funcs.entry(addr).or_insert((name, sym.size() as u32));
            }
        }
        Ok(s)
    }

    pub fn add(&mut self, addr: u32, name: &str, size: u32) {
        self.funcs.insert(addr, (name.to_string(), size));
        self.by_name.insert(name.to_string(), addr);
    }

    /// Find the function containing `addr`.
    pub fn lookup(&self, addr: u32) -> Option<(String, u32)> {
        let (start, (name, size)) = self.funcs.range(..=addr).next_back()?;
        let off = addr - start;
        if *size != 0 && off >= *size && off > 0x4000 {
            return None;
        }
        if *size == 0 && off > 0x2000 {
            return None;
        }
        Some((name.clone(), off))
    }

    pub fn describe(&self, addr: u32) -> String {
        match self.lookup(addr) {
            Some((n, 0)) => n,
            Some((n, o)) => format!("{}+0x{:x}", n, o),
            None => format!("0x{:08x}", addr),
        }
    }
}

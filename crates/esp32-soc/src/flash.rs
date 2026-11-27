//! SPI flash contents, partition table and ESP32 application image handling.

use md5::{Digest, Md5};
use std::fmt;

pub const FLASH_PAGE: u32 = 0x10000;

#[derive(Clone, Debug)]
pub struct Partition {
    pub label: String,
    pub ptype: u8,
    pub subtype: u8,
    pub offset: u32,
    pub size: u32,
    pub flags: u32,
}

impl fmt::Display for Partition {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let t = match self.ptype {
            0 => "app",
            1 => "data",
            _ => "?",
        };
        write!(
            f,
            "{:<10} {:<5} 0x{:02x} offset=0x{:06x} size=0x{:06x} ({} KB)",
            self.label,
            t,
            self.subtype,
            self.offset,
            self.size,
            self.size / 1024
        )
    }
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub load_addr: u32,
    /// absolute flash offset of the segment data
    pub flash_off: u32,
    pub len: u32,
}

#[derive(Clone, Debug)]
pub struct AppImage {
    pub offset: u32,
    pub entry: u32,
    pub segments: Vec<Segment>,
    pub project_name: String,
    pub version: String,
    pub idf_version: String,
}

pub struct Flash {
    pub data: Vec<u8>,
    /// Set when the firmware writes to flash (NVS, file systems...)
    pub dirty: bool,
    pub write_enabled: bool,
}

impl Flash {
    pub fn new(size: usize) -> Flash {
        Flash { data: vec![0xFF; size], dirty: false, write_enabled: false }
    }
    pub fn size(&self) -> u32 {
        self.data.len() as u32
    }
    pub fn write_at(&mut self, off: u32, bytes: &[u8]) {
        let o = off as usize;
        let end = (o + bytes.len()).min(self.data.len());
        if o < end {
            self.data[o..end].copy_from_slice(&bytes[..end - o]);
        }
    }
    pub fn read(&self, off: u32, buf: &mut [u8]) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = *self.data.get(off as usize + i).unwrap_or(&0xFF);
        }
    }
    #[inline]
    pub fn byte(&self, off: u32) -> u8 {
        *self.data.get(off as usize).unwrap_or(&0xFF)
    }
    pub fn u32_at(&self, off: u32) -> u32 {
        let mut b = [0u8; 4];
        self.read(off, &mut b);
        u32::from_le_bytes(b)
    }
    /// NOR flash program semantics: bits can only go 1 -> 0.
    pub fn program(&mut self, off: u32, bytes: &[u8]) {
        for (i, b) in bytes.iter().enumerate() {
            if let Some(d) = self.data.get_mut(off as usize + i) {
                *d &= *b;
            }
        }
        self.dirty = true;
    }
    pub fn erase(&mut self, off: u32, len: u32) {
        let s = off as usize;
        let e = (s + len as usize).min(self.data.len());
        if s < e {
            self.data[s..e].fill(0xFF);
        }
        self.dirty = true;
    }

    pub fn partitions(&self) -> Vec<Partition> {
        parse_partitions(&self.data[0x8000.min(self.data.len())..])
    }

    /// Select the application partition the 2nd-stage bootloader would boot.
    pub fn boot_partition(&self) -> Option<Partition> {
        let parts = self.partitions();
        let ota: Vec<&Partition> = parts.iter().filter(|p| p.ptype == 0 && (0x10..0x20).contains(&p.subtype)).collect();
        if let Some(otadata) = parts.iter().find(|p| p.ptype == 1 && p.subtype == 0) {
            // two otadata sectors; pick the highest valid sequence number
            let mut best: Option<u32> = None;
            for i in 0..2 {
                let off = otadata.offset + i * 0x1000;
                let seq = self.u32_at(off);
                let crc = self.u32_at(off + 28);
                if seq == 0xFFFF_FFFF || seq == 0 {
                    continue;
                }
                let mut b = [0u8; 4];
                self.read(off, &mut b);
                if crc32_le(0xFFFF_FFFF, &b) != crc {
                    continue;
                }
                best = Some(best.map_or(seq, |x| x.max(seq)));
            }
            if let Some(seq) = best {
                if !ota.is_empty() {
                    let idx = ((seq - 1) as usize) % ota.len();
                    return Some(ota[idx].clone());
                }
            }
        }
        if let Some(f) = parts.iter().find(|p| p.ptype == 0 && p.subtype == 0) {
            return Some(f.clone());
        }
        if let Some(o) = ota.first() {
            return Some((*o).clone());
        }
        None
    }
}

pub fn parse_partitions(d: &[u8]) -> Vec<Partition> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 32 <= d.len() && i < 0xC00 {
        let e = &d[i..i + 32];
        if e[0] != 0xAA || e[1] != 0x50 {
            break;
        }
        let label = String::from_utf8_lossy(&e[12..28]).trim_end_matches('\0').to_string();
        v.push(Partition {
            label,
            ptype: e[2],
            subtype: e[3],
            offset: u32::from_le_bytes(e[4..8].try_into().unwrap()),
            size: u32::from_le_bytes(e[8..12].try_into().unwrap()),
            flags: u32::from_le_bytes(e[28..32].try_into().unwrap()),
        });
        i += 32;
    }
    v
}

/// Build a binary partition table (with MD5 entry) from entries.
pub fn build_partition_table(parts: &[Partition]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(&[0xAA, 0x50, p.ptype, p.subtype]);
        out.extend_from_slice(&p.offset.to_le_bytes());
        out.extend_from_slice(&p.size.to_le_bytes());
        let mut label = [0u8; 16];
        let lb = p.label.as_bytes();
        label[..lb.len().min(16)].copy_from_slice(&lb[..lb.len().min(16)]);
        out.extend_from_slice(&label);
        out.extend_from_slice(&p.flags.to_le_bytes());
    }
    let digest = Md5::digest(&out);
    out.extend_from_slice(&[0xEB, 0xEB]);
    out.extend_from_slice(&[0xFF; 14]);
    out.extend_from_slice(&digest);
    out.resize(0xC00, 0xFF);
    out
}

/// The default Arduino-ESP32 partition layout (4 MB flash).
pub fn default_partitions() -> Vec<Partition> {
    let p = |label: &str, ptype: u8, subtype: u8, offset: u32, size: u32| Partition {
        label: label.into(),
        ptype,
        subtype,
        offset,
        size,
        flags: 0,
    };
    vec![
        p("nvs", 1, 0x02, 0x9000, 0x5000),
        p("otadata", 1, 0x00, 0xE000, 0x2000),
        p("app0", 0, 0x10, 0x10000, 0x140000),
        p("app1", 0, 0x11, 0x150000, 0x140000),
        p("spiffs", 1, 0x82, 0x290000, 0x160000),
        p("coredump", 1, 0x03, 0x3F0000, 0x10000),
    ]
}

pub fn parse_app_image(flash: &Flash, offset: u32) -> Result<AppImage, String> {
    if flash.byte(offset) != 0xE9 {
        return Err(format!("no ESP32 image magic (0xE9) at flash offset 0x{:x}", offset));
    }
    let nseg = flash.byte(offset + 1) as u32;
    let entry = flash.u32_at(offset + 4);
    let chip_id = flash.byte(offset + 12) as u16 | ((flash.byte(offset + 13) as u16) << 8);
    if chip_id != 0 && chip_id != 0xFFFF {
        return Err(format!("image is for chip id {} (not ESP32)", chip_id));
    }
    let mut p = offset + 24;
    let mut segments = Vec::new();
    for _ in 0..nseg {
        let la = flash.u32_at(p);
        let len = flash.u32_at(p + 4);
        if len > 16 * 1024 * 1024 {
            return Err("corrupt segment length".into());
        }
        segments.push(Segment { load_addr: la, flash_off: p + 8, len });
        p += 8 + len;
    }
    // esp_app_desc_t lives at the start of the first DROM segment
    let mut project_name = String::new();
    let mut version = String::new();
    let mut idf_version = String::new();
    if let Some(s) = segments.first() {
        if flash.u32_at(s.flash_off) == 0xABCD5432 {
            let rd = |o: u32, n: usize| {
                let mut b = vec![0u8; n];
                flash.read(s.flash_off + o, &mut b);
                String::from_utf8_lossy(&b).trim_end_matches('\0').to_string()
            };
            version = rd(16, 32);
            project_name = rd(48, 32);
            idf_version = rd(112, 32);
        }
    }
    Ok(AppImage { offset, entry, segments, project_name, version, idf_version })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareKind {
    /// A full flash image starting at offset 0 (bootloader at 0x1000)
    MergedFrom0,
    /// A merged image starting with the bootloader (flash offset 0x1000)
    MergedFrom0x1000,
    /// A bare application image (to be placed in the first app partition)
    AppImage,
    /// ELF file
    Elf,
}

pub fn detect_kind(d: &[u8]) -> Result<FirmwareKind, String> {
    if d.len() >= 4 && &d[0..4] == b"\x7fELF" {
        return Ok(FirmwareKind::Elf);
    }
    let has_pt = |off: usize| d.len() > off + 2 && d[off] == 0xAA && d[off + 1] == 0x50;
    if has_pt(0x8000) {
        return Ok(FirmwareKind::MergedFrom0);
    }
    if has_pt(0x7000) && d[0] == 0xE9 {
        return Ok(FirmwareKind::MergedFrom0x1000);
    }
    if !d.is_empty() && d[0] == 0xE9 {
        return Ok(FirmwareKind::AppImage);
    }
    Err("unrecognised firmware format (expected ESP32 .bin image, merged flash image or ELF)".into())
}

/// Create a flash image from a firmware file (plus optional separate bootloader/partition files).
pub fn build_flash(
    firmware: &[u8],
    flash_size: usize,
    partitions_bin: Option<&[u8]>,
    bootloader_bin: Option<&[u8]>,
) -> Result<Flash, String> {
    let mut flash = Flash::new(flash_size);
    match detect_kind(firmware)? {
        FirmwareKind::MergedFrom0 => flash.write_at(0, firmware),
        FirmwareKind::MergedFrom0x1000 => flash.write_at(0x1000, firmware),
        FirmwareKind::AppImage => {
            let pt = match partitions_bin {
                Some(p) => p.to_vec(),
                None => build_partition_table(&default_partitions()),
            };
            flash.write_at(0x8000, &pt);
            if let Some(b) = bootloader_bin {
                flash.write_at(0x1000, b);
            }
            let parts = flash.partitions();
            let app = parts
                .iter()
                .find(|p| p.ptype == 0 && p.subtype == 0)
                .or_else(|| parts.iter().find(|p| p.ptype == 0))
                .ok_or("partition table has no app partition")?;
            flash.write_at(app.offset, firmware);
        }
        FirmwareKind::Elf => {
            let img = crate::elf::elf_to_image(firmware)?;
            let pt = match partitions_bin {
                Some(p) => p.to_vec(),
                None => build_partition_table(&default_partitions()),
            };
            flash.write_at(0x8000, &pt);
            let parts = flash.partitions();
            let app = parts.iter().find(|p| p.ptype == 0).ok_or("partition table has no app partition")?;
            flash.write_at(app.offset, &img);
        }
    }
    if let Some(p) = partitions_bin {
        flash.write_at(0x8000, p);
    }
    flash.dirty = false;
    Ok(flash)
}

pub fn crc32_le(crc: u32, data: &[u8]) -> u32 {
    // ROM semantics: crc32_le(crc, buf, len) = ~crc32(~crc)
    let mut c = !crc;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

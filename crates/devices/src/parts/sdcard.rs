//! microSD card module on SPI (SD SPI mode), backed by a disk image file.
//!
//! Implements the SPI-mode command set the Arduino SD / SdFat libraries,
//! ESP-IDF's sdspi driver and MicroPython's SDCard use: CMD0/8/9/10/12/13/16/
//! 17/18/24/25/55/58/59, ACMD13/22/23/41, with CRC7/CRC16 when CRC is enabled.
//! The card is an SDHC card (block addressing).  The image is created and
//! formatted FAT32 on first use, and the inspector lists the files on it, so
//! data written by a sketch (logs, CSV files) can be looked at directly.

use crate::parts::{prop_f64, prop_str, Props};
use crate::{pin, Device, PinInfo, PinKind::*};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

const BLOCK: usize = 512;

pub fn crc7(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        let mut d = b;
        for _ in 0..8 {
            crc <<= 1;
            if (d ^ crc) & 0x80 != 0 {
                crc ^= 0x09;
            }
            d <<= 1;
        }
    }
    crc & 0x7F
}

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

#[derive(Debug, PartialEq)]
enum Rx {
    /// waiting for a command byte (0x40..0x7F)
    Cmd,
    /// collecting the 6 command bytes
    CmdBytes,
    /// CMD24/25: waiting for a data token
    WaitToken { multi: bool },
    /// receiving a data block (512 bytes + 2 CRC)
    Data { multi: bool },
}

pub struct SdCard {
    path: Option<PathBuf>,
    img: Vec<u8>,
    inserted: bool,
    rx: Rx,
    cmd: Vec<u8>,
    data: Vec<u8>,
    out: VecDeque<u8>,
    app_cmd: bool,
    idle: bool,
    crc_on: bool,
    /// multiple-block read in progress: next block to stream
    reading: Option<u32>,
    write_block: u32,
    dirty_blocks: bool,
    pub reads: u64,
    pub writes: u64,
    last_error: Option<String>,
    /// (writes count when listed, listing)
    files_cache: std::cell::RefCell<Option<(u64, Value)>>,
}

impl SdCard {
    pub fn new(p: &Props) -> SdCard {
        let size_mb = prop_f64(p, &["size_mb", "size"], 64.0).clamp(8.0, 4096.0) as usize;
        let base = p.get("__base_dir").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(PathBuf::from);
        let img_name = prop_str(p, &["image"], "");
        let id = p.get("__id").and_then(|v| v.as_str()).unwrap_or("sd");
        let ephemeral = p.get("__ephemeral").and_then(|v| v.as_bool()).unwrap_or(false);
        let path = match (base, img_name.is_empty()) {
            _ if ephemeral => None,
            (Some(b), true) => Some(b.join(".esp32sim").join(format!("{}.img", id))),
            (Some(b), false) => Some(b.join(img_name)),
            (None, false) => Some(PathBuf::from(img_name)),
            (None, true) => None,
        };
        let mut last_error = None;
        let img = match path.as_ref().map(std::fs::read) {
            Some(Ok(d)) if d.len() >= BLOCK * 2048 => d,
            _ => {
                // new card: FAT32 formatted
                let mut c = std::io::Cursor::new(vec![0u8; size_mb << 20]);
                if let Err(e) = fatfs::format_volume(&mut c, fatfs::FormatVolumeOptions::new().volume_label(*b"ESP32SIM   ")) {
                    last_error = Some(format!("format: {}", e));
                }
                let d = c.into_inner();
                if let Some(pth) = &path {
                    if let Some(dir) = pth.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    if let Err(e) = std::fs::write(pth, &d) {
                        last_error = Some(format!("{}: {}", pth.display(), e));
                    }
                }
                d
            }
        };
        SdCard {
            path,
            img,
            inserted: p.get("inserted").and_then(|v| v.as_bool()).unwrap_or(true),
            rx: Rx::Cmd,
            cmd: Vec::new(),
            data: Vec::new(),
            out: VecDeque::new(),
            app_cmd: false,
            idle: true,
            crc_on: false,
            reading: None,
            write_block: 0,
            dirty_blocks: false,
            reads: 0,
            writes: 0,
            last_error,
            files_cache: std::cell::RefCell::new(None),
        }
    }

    fn blocks(&self) -> u32 {
        (self.img.len() / BLOCK) as u32
    }

    fn push_block(&mut self, blk: u32) {
        let off = blk as usize * BLOCK;
        self.out.push_back(0xFE);
        let data: Vec<u8> = if off + BLOCK <= self.img.len() { self.img[off..off + BLOCK].to_vec() } else { vec![0; BLOCK] };
        let crc = crc16(&data);
        self.out.extend(data);
        self.out.extend([(crc >> 8) as u8, crc as u8]);
        self.reads += 1;
    }

    fn r1(&self) -> u8 {
        if self.idle {
            0x01
        } else {
            0x00
        }
    }

    fn csd(&self) -> [u8; 16] {
        // CSD version 2.0 (SDHC): capacity = (C_SIZE + 1) * 512 KiB
        let c_size = (self.blocks() / 1024).saturating_sub(1);
        let mut c = [0x40, 0x0E, 0x00, 0x32, 0x5B, 0x59, 0x00, 0, 0, 0, 0x7F, 0x80, 0x0A, 0x40, 0x00, 0x00];
        c[7] = ((c_size >> 16) & 0x3F) as u8;
        c[8] = (c_size >> 8) as u8;
        c[9] = c_size as u8;
        c[15] = (crc7(&c[..15]) << 1) | 1;
        c
    }

    fn cid(&self) -> [u8; 16] {
        let mut c = [0x1D, b'S', b'M', b'E', b'S', b'P', b'3', b'2', 0x10, 0x12, 0x34, 0x56, 0x78, 0x01, 0x8A, 0];
        c[15] = (crc7(&c[..15]) << 1) | 1;
        c
    }

    fn push_reg(&mut self, reg: [u8; 16]) {
        self.out.push_back(0x00);
        self.out.push_back(0xFF);
        self.out.push_back(0xFE);
        self.out.extend(reg);
        let crc = crc16(&reg);
        self.out.extend([(crc >> 8) as u8, crc as u8]);
    }

    fn command(&mut self) {
        let c = std::mem::take(&mut self.cmd);
        let idx = c[0] & 0x3F;
        let arg = u32::from_be_bytes([c[1], c[2], c[3], c[4]]);
        self.out.clear();
        self.out.push_back(0xFF); // Ncr
        if self.crc_on && idx != 0 && idx != 8 && (c[5] >> 1) != crc7(&c[..5]) {
            self.out.push_back(self.r1() | 0x08); // COM CRC error
            return;
        }
        let app = std::mem::take(&mut self.app_cmd);
        match (app, idx) {
            (_, 0) => {
                self.idle = true;
                self.reading = None;
                self.out.push_back(0x01);
            }
            (_, 8) => self.out.extend([self.r1(), 0x00, 0x00, 0x01, (arg & 0xFF) as u8]),
            (_, 59) => {
                self.crc_on = arg & 1 != 0;
                self.out.push_back(self.r1());
            }
            (_, 55) => {
                self.app_cmd = true;
                self.out.push_back(self.r1());
            }
            (true, 41) => {
                // initialisation completes on the first ACMD41
                self.idle = false;
                self.out.push_back(0x00);
            }
            (false, 1) => {
                self.idle = false;
                self.out.push_back(0x00);
            }
            (_, 58) => self.out.extend([self.r1(), 0xC0, 0xFF, 0x80, 0x00]),
            (_, 9) => {
                let csd = self.csd();
                self.push_reg(csd);
            }
            (_, 10) => {
                let cid = self.cid();
                self.push_reg(cid);
            }
            (true, 13) => {
                // SD status: R2 + 64-byte data block
                self.out.extend([0x00, 0x00, 0xFF, 0xFE]);
                let d = [0u8; 64];
                self.out.extend(d);
                let crc = crc16(&d);
                self.out.extend([(crc >> 8) as u8, crc as u8]);
            }
            (false, 13) => self.out.extend([0x00, 0x00]),
            (true, 51) => {
                // SCR: SD spec 2.0, SDHC security, 1- and 4-bit bus widths
                self.out.extend([0x00, 0xFF, 0xFE]);
                let scr = [0x02u8, 0x35, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00];
                self.out.extend(scr);
                let crc = crc16(&scr);
                self.out.extend([(crc >> 8) as u8, crc as u8]);
            }
            (false, 6) => {
                // SWITCH_FUNC: 512-bit status, high speed supported, function 0 selected
                self.out.extend([0x00, 0xFF, 0xFE]);
                let mut st = [0u8; 64];
                st[0] = 0x00;
                st[1] = 0x64; // max current 100 mA
                st[12] = 0x80;
                st[13] = 0x03; // group 1 supports functions 0 and 1
                st[16] = if arg & 0x8000_0000 != 0 { (arg & 0xF) as u8 } else { 0 };
                self.out.extend(st);
                let crc = crc16(&st);
                self.out.extend([(crc >> 8) as u8, crc as u8]);
            }
            (true, 22) => {
                self.out.extend([0x00, 0xFF, 0xFE]);
                let d = self.writes.min(u32::MAX as u64) as u32;
                let b = d.to_be_bytes();
                self.out.extend(b);
                let crc = crc16(&b);
                self.out.extend([(crc >> 8) as u8, crc as u8]);
            }
            (_, 12) => {
                self.reading = None;
                self.out.clear();
                self.out.extend([0xFF, 0x00, 0xFF]); // stuff byte, R1b, not busy
            }
            (_, 16) | (true, 23) | (false, 23) | (_, 32) | (_, 33) => self.out.push_back(0x00),
            (_, 38) => self.out.extend([0x00, 0xFF]),
            (_, 17) => {
                if arg >= self.blocks() {
                    self.out.push_back(0x40); // address error
                } else {
                    self.out.extend([0x00, 0xFF]);
                    self.push_block(arg);
                }
            }
            (_, 18) => {
                if arg >= self.blocks() {
                    self.out.push_back(0x40);
                } else {
                    self.out.extend([0x00, 0xFF]);
                    self.push_block(arg);
                    self.reading = Some(arg + 1);
                }
            }
            (_, 24) | (_, 25) => {
                if arg >= self.blocks() {
                    self.out.push_back(0x40);
                } else {
                    self.out.push_back(0x00);
                    self.write_block = arg;
                    self.rx = Rx::WaitToken { multi: idx == 25 };
                }
            }
            _ => self.out.push_back(self.r1() | 0x04), // illegal command
        }
    }

    fn byte_in(&mut self, b: u8) {
        match self.rx {
            Rx::Cmd => {
                if b & 0xC0 == 0x40 {
                    self.cmd.clear();
                    self.cmd.push(b);
                    self.rx = Rx::CmdBytes;
                }
            }
            Rx::CmdBytes => {
                self.cmd.push(b);
                if self.cmd.len() == 6 {
                    self.rx = Rx::Cmd;
                    self.command();
                }
            }
            Rx::WaitToken { multi } => match b {
                0xFE | 0xFC => {
                    self.data.clear();
                    self.rx = Rx::Data { multi };
                }
                0xFD if multi => {
                    // stop transmission token: card busy briefly, then idle
                    self.out.clear();
                    self.out.extend([0xFF, 0xFF]);
                    self.rx = Rx::Cmd;
                }
                _ => {
                    if !multi && b & 0xC0 == 0x40 {
                        // a command instead of data: abort the write
                        self.rx = Rx::CmdBytes;
                        self.cmd = vec![b];
                    }
                }
            },
            Rx::Data { multi } => {
                self.data.push(b);
                if self.data.len() == BLOCK + 2 {
                    let ok = !self.crc_on || crc16(&self.data[..BLOCK]) == u16::from_be_bytes([self.data[BLOCK], self.data[BLOCK + 1]]);
                    if ok {
                        let off = self.write_block as usize * BLOCK;
                        if off + BLOCK <= self.img.len() {
                            self.img[off..off + BLOCK].copy_from_slice(&self.data[..BLOCK]);
                            self.dirty_blocks = true;
                            self.writes += 1;
                            self.persist_block(self.write_block);
                        }
                        self.write_block += 1;
                    }
                    self.out.clear();
                    self.out.push_back(if ok { 0x05 } else { 0x0B }); // data accepted / CRC error
                    self.out.push_back(0xFF);
                    self.rx = if multi { Rx::WaitToken { multi: true } } else { Rx::Cmd };
                }
            }
        }
    }

    fn persist_block(&mut self, blk: u32) {
        let Some(p) = &self.path else { return };
        let off = blk as u64 * BLOCK as u64;
        let r = std::fs::OpenOptions::new().write(true).open(p).and_then(|mut f| {
            f.seek(SeekFrom::Start(off))?;
            f.write_all(&self.img[off as usize..off as usize + BLOCK])
        });
        if let Err(e) = r {
            self.last_error = Some(format!("{}: {}", p.display(), e));
        }
    }

    /// Files on the card (FAT), for the inspector.  Re-read only after writes.
    fn files(&self) -> Value {
        if let Some((w, v)) = self.files_cache.borrow().as_ref() {
            if *w == self.writes {
                return v.clone();
            }
        }
        let v = list_fat(&self.img);
        *self.files_cache.borrow_mut() = Some((self.writes, v.clone()));
        v
    }
}

/// Read-only view of the image for fatfs (writes are discarded).
struct RoImage<'a> {
    data: &'a [u8],
    pos: u64,
}
impl Read for RoImage<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let p = (self.pos as usize).min(self.data.len());
        let n = buf.len().min(self.data.len() - p);
        buf[..n].copy_from_slice(&self.data[p..p + n]);
        self.pos += n as u64;
        Ok(n)
    }
}
impl Write for RoImage<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pos += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Seek for RoImage<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.pos = match to {
            SeekFrom::Start(x) => x,
            SeekFrom::End(x) => (self.data.len() as i64 + x).max(0) as u64,
            SeekFrom::Current(x) => (self.pos as i64 + x).max(0) as u64,
        };
        Ok(self.pos)
    }
}

fn list_fat(img: &[u8]) -> Value {
    let fs = match fatfs::FileSystem::new(RoImage { data: img, pos: 0 }, fatfs::FsOptions::new()) {
        Ok(f) => f,
        Err(_) => return json!({"error": "not a FAT filesystem (unformatted?)"}),
    };
    fn walk<T: Read + Write + Seek>(dir: fatfs::Dir<T>, prefix: &str, out: &mut Vec<Value>, depth: usize) {
        for e in dir.iter().flatten() {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let path = format!("{}/{}", prefix, name);
            if e.is_dir() {
                out.push(json!({"path": path.clone() + "/", "size": 0}));
                if depth < 4 {
                    walk(e.to_dir(), &path, out, depth + 1);
                }
            } else {
                let mut preview = String::new();
                if e.len() > 0 {
                    let mut f = e.to_file();
                    let mut buf = vec![0u8; (e.len() as usize).min(200)];
                    if f.read_exact(&mut buf).is_ok() {
                        preview = String::from_utf8_lossy(&buf).to_string();
                    }
                }
                out.push(json!({"path": path, "size": e.len(), "preview": preview}));
            }
            if out.len() > 200 {
                return;
            }
        }
    }
    let mut list = Vec::new();
    walk(fs.root_dir(), "", &mut list, 0);
    json!(list)
}

impl Device for SdCard {
    fn kind(&self) -> &'static str {
        "sd-card"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("GND", Ground), pin("VCC", Power), pin("MISO", Output), pin("MOSI", Input), pin("SCK", Input), pin("CS", Input)]
    }
    fn spi_pins(&self) -> Option<(usize, usize, Option<usize>, usize)> {
        Some((4, 3, Some(2), 5))
    }
    fn spi_select(&mut self, selected: bool) {
        if !selected {
            // CS released: a pending read stream stops; partial commands are dropped
            if self.rx == Rx::CmdBytes {
                self.rx = Rx::Cmd;
            }
        }
    }
    fn spi_transfer(&mut self, _now: u64, data: &mut [u8], _lv: &dyn Fn(usize) -> bool) {
        for b in data.iter_mut() {
            let mosi = *b;
            if !self.inserted {
                *b = 0xFF;
                continue;
            }
            // streaming multi-block reads: keep the output topped up
            if self.out.is_empty() {
                if let Some(n) = self.reading {
                    if n < self.blocks() {
                        self.out.push_back(0xFF);
                        self.push_block(n);
                        self.reading = Some(n + 1);
                    }
                }
            }
            let miso = self.out.pop_front().unwrap_or(0xFF);
            self.byte_in(mosi);
            *b = miso;
        }
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        match a.get("action").and_then(|v| v.as_str()) {
            Some("eject") | Some("remove") => {
                self.inserted = false;
                Ok(())
            }
            Some("insert") => {
                self.inserted = true;
                self.idle = true;
                Ok(())
            }
            Some("refresh") => {
                *self.files_cache.borrow_mut() = None;
                Ok(())
            }
            _ => Err("sd-card accepts {\"action\": \"insert\"|\"eject\"|\"refresh\"}".into()),
        }
    }
    fn state(&self) -> Value {
        json!({
            "inserted": self.inserted,
            "size_mb": self.img.len() >> 20,
            "reads": self.reads,
            "writes": self.writes,
            "image": self.path.as_ref().map(|p| p.display().to_string()),
            "error": self.last_error,
            "files": self.files(),
        })
    }
    fn summary(&self) -> String {
        if !self.inserted {
            return "no card".into();
        }
        format!("{} MB SDHC · {} blocks read, {} written", self.img.len() >> 20, self.reads, self.writes)
    }
}


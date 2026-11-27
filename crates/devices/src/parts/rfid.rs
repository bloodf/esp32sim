//! MFRC522 13.56 MHz RFID reader (SPI) with virtual ISO14443A cards.
//!
//! The model works at register level (as the MFRC522 Arduino / MicroPython
//! drivers talk to it): FIFO, command register, interrupt flags, bit framing,
//! CRC coprocessor and MFAuthent.  Card side: REQA/WUPA, anticollision and
//! select (4 and 7 byte UIDs), HLTA, MIFARE Classic read/write after
//! authentication, and NTAG21x READ/WRITE/GET_VERSION/FAST_READ.
//! Crypto1 runs inside the real reader chip, so the host only ever sees
//! plaintext; the model checks keys against the sector trailers.

use crate::parts::pn532::{card_from_action, Card, CardKind};
use crate::parts::{hex, Props};
use crate::{pin, Device, PinInfo, PinKind::*, PinView, Rail, Stamp};
use serde_json::{json, Value};
use std::collections::VecDeque;

const COMMAND: usize = 0x01;
const COM_IEN: usize = 0x02;
const DIV_IEN: usize = 0x03;
const COM_IRQ: usize = 0x04;
const DIV_IRQ: usize = 0x05;
const ERROR: usize = 0x06;
const STATUS1: usize = 0x07;
const STATUS2: usize = 0x08;
const FIFO_DATA: usize = 0x09;
const FIFO_LEVEL: usize = 0x0A;
const CONTROL: usize = 0x0C;
const BIT_FRAMING: usize = 0x0D;
const COLL: usize = 0x0E;
const MODE: usize = 0x11;
const TX_CONTROL: usize = 0x14;
const CRC_RESULT_H: usize = 0x21;
const CRC_RESULT_L: usize = 0x22;
const VERSION: usize = 0x37;

const CMD_IDLE: u8 = 0x0;
const CMD_CALC_CRC: u8 = 0x3;
const CMD_TRANSMIT: u8 = 0x4;
const CMD_TRANSCEIVE: u8 = 0xC;
const CMD_MF_AUTHENT: u8 = 0xE;
const CMD_SOFT_RESET: u8 = 0xF;

/// ISO/IEC 14443-3 type A CRC (CRC_A), returned low byte first.
pub fn crc_a(data: &[u8], preset: u16) -> [u8; 2] {
    let mut crc = preset;
    for &d in data {
        let mut b = d ^ (crc as u8);
        b ^= b << 4;
        crc = (crc >> 8) ^ ((b as u16) << 8) ^ ((b as u16) << 3) ^ ((b as u16) >> 4);
    }
    [crc as u8, (crc >> 8) as u8]
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Picc {
    Idle,
    Ready,
    Active,
    Halt,
}

pub struct Mfrc522 {
    regs: [u8; 64],
    fifo: VecDeque<u8>,
    // SPI transaction state
    first: bool,
    addr: usize,
    read: bool,
    saw_cs: bool,
    // card side
    pub card: Option<Card>,
    picc: Picc,
    /// cascade level currently being selected (0..2)
    level: usize,
    auth_sector: Option<usize>,
    pending_write: Option<usize>,
    irq_low: bool,
    dirty: bool,
    in_reset: bool,
    pub log: VecDeque<String>,
}

impl Mfrc522 {
    pub fn new(p: &Props) -> Mfrc522 {
        let mut m = Mfrc522 {
            regs: [0; 64],
            fifo: VecDeque::new(),
            first: true,
            addr: 0,
            read: false,
            saw_cs: false,
            card: None,
            picc: Picc::Idle,
            level: 0,
            auth_sector: None,
            pending_write: None,
            irq_low: false,
            dirty: true,
            in_reset: false,
            log: VecDeque::new(),
        };
        m.reset_regs();
        if let Some(c) = p.get("card").and_then(|v| v.as_str()) {
            if c != "none" && !c.is_empty() {
                let mut a = json!({"action": "tap", "card": c});
                if let Some(u) = p.get("uid") {
                    a["uid"] = u.clone();
                }
                if let Ok(card) = card_from_action(&a) {
                    m.card = Some(card);
                }
            }
        }
        m
    }

    fn reset_regs(&mut self) {
        self.regs = [0; 64];
        self.regs[COMMAND] = 0x20;
        self.regs[COM_IEN] = 0x80;
        self.regs[COM_IRQ] = 0x14;
        self.regs[STATUS1] = 0x21;
        self.regs[0x0B] = 0x08; // WaterLevel
        self.regs[CONTROL] = 0x10;
        self.regs[COLL] = 0x80;
        self.regs[MODE] = 0x3F;
        self.regs[TX_CONTROL] = 0x80;
        self.regs[0x15] = 0x00;
        self.regs[0x18] = 0x84;
        self.regs[0x19] = 0x4D;
        self.regs[0x26] = 0x48;
        self.regs[CRC_RESULT_H] = 0xFF;
        self.regs[CRC_RESULT_L] = 0xFF;
        self.regs[VERSION] = 0x92;
        self.fifo.clear();
    }

    fn note(&mut self, s: String) {
        if self.log.len() > 50 {
            self.log.pop_front();
        }
        self.log.push_back(s);
    }

    fn update_irq(&mut self) {
        let pending = (self.regs[COM_IRQ] & self.regs[COM_IEN] & 0x7F) != 0 || (self.regs[DIV_IRQ] & self.regs[DIV_IEN] & 0x14) != 0;
        // IRqInv (ComIEnReg bit 7) makes the pin active low
        let low = if self.regs[COM_IEN] & 0x80 != 0 { pending } else { !pending };
        if low != self.irq_low {
            self.irq_low = low;
            self.dirty = true;
        }
    }

    fn read_reg(&mut self, a: usize) -> u8 {
        match a {
            FIFO_DATA => self.fifo.pop_front().unwrap_or(0),
            FIFO_LEVEL => self.fifo.len() as u8,
            STATUS1 => {
                let mut v = 0x01; // LoAlert-ish defaults: CRCReady set
                if self.fifo.is_empty() {
                    v |= 0x02;
                }
                v |= 0x20; // CRCReady
                let irq = (self.regs[COM_IRQ] & self.regs[COM_IEN] & 0x7F) != 0;
                if irq {
                    v |= 0x10;
                }
                v
            }
            _ => self.regs[a & 63],
        }
    }

    fn write_reg(&mut self, a: usize, v: u8) {
        match a {
            COMMAND => {
                self.regs[COMMAND] = v & 0x3F;
                self.command(v & 0x0F);
            }
            COM_IRQ | DIV_IRQ => {
                if v & 0x80 != 0 {
                    self.regs[a] |= v & 0x7F;
                } else {
                    self.regs[a] &= !(v & 0x7F);
                }
            }
            FIFO_DATA => {
                if self.fifo.len() < 64 {
                    self.fifo.push_back(v);
                } else {
                    self.regs[ERROR] |= 0x10; // BufferOvfl
                }
            }
            FIFO_LEVEL => {
                if v & 0x80 != 0 {
                    self.fifo.clear();
                    self.regs[ERROR] &= !0x10;
                }
            }
            BIT_FRAMING => {
                self.regs[BIT_FRAMING] = v;
                if v & 0x80 != 0 && self.regs[COMMAND] & 0x0F == CMD_TRANSCEIVE {
                    self.transceive();
                }
            }
            STATUS2 => {
                self.regs[STATUS2] = v;
                if v & 0x08 == 0 {
                    self.auth_sector = None;
                }
            }
            VERSION => {}
            _ => self.regs[a & 63] = v,
        }
        self.update_irq();
    }

    fn crc_preset(&self) -> u16 {
        match self.regs[MODE] & 3 {
            0 => 0x0000,
            1 => 0x6363,
            2 => 0xA671,
            _ => 0xFFFF,
        }
    }

    fn command(&mut self, cmd: u8) {
        match cmd {
            CMD_IDLE => {}
            CMD_SOFT_RESET => {
                self.reset_regs();
                // reset completes immediately: PowerDown bit clear, command Idle
                self.regs[COMMAND] = 0x20;
            }
            CMD_CALC_CRC => {
                let data: Vec<u8> = self.fifo.drain(..).collect();
                let c = crc_a(&data, self.crc_preset());
                self.regs[CRC_RESULT_L] = c[0];
                self.regs[CRC_RESULT_H] = c[1];
                self.regs[DIV_IRQ] |= 0x04;
            }
            CMD_TRANSMIT => {
                self.fifo.clear();
                self.regs[COM_IRQ] |= 0x40 | 0x10;
                self.regs[COMMAND] &= 0xF0;
            }
            CMD_TRANSCEIVE => {
                if self.regs[BIT_FRAMING] & 0x80 != 0 {
                    self.transceive();
                }
            }
            CMD_MF_AUTHENT => self.authenticate(),
            _ => {}
        }
    }

    fn antenna_on(&self) -> bool {
        self.regs[TX_CONTROL] & 0x03 != 0
    }

    fn transceive(&mut self) {
        let tx: Vec<u8> = self.fifo.drain(..).collect();
        let last_bits = self.regs[BIT_FRAMING] & 7;
        self.regs[COM_IRQ] |= 0x40; // TxIRq
        self.regs[ERROR] &= !0x1F;
        let resp = if self.antenna_on() { self.picc_exchange(&tx, last_bits) } else { None };
        match resp {
            Some((data, bits)) => {
                for b in &data {
                    self.fifo.push_back(*b);
                }
                self.regs[CONTROL] = (self.regs[CONTROL] & !7) | (bits & 7);
                self.regs[COM_IRQ] |= 0x20 | 0x10; // RxIRq | IdleIRq
            }
            None => {
                // nothing answered: the receive timer runs out
                self.regs[COM_IRQ] |= 0x01;
            }
        }
        self.update_irq();
    }

    fn authenticate(&mut self) {
        let d: Vec<u8> = self.fifo.drain(..).collect();
        let ok = (|| {
            let card = self.card.as_ref()?;
            if self.picc != Picc::Active || d.len() < 12 || !matches!(card.kind, CardKind::Classic1k) {
                return None;
            }
            let block = d[1] as usize;
            if block >= 64 {
                return None;
            }
            let trailer = ((block / 4) * 4 + 3) * 16;
            let stored = if d[0] == 0x60 { &card.mem[trailer..trailer + 6] } else { &card.mem[trailer + 10..trailer + 16] };
            if &d[2..8] != stored || d[8..12] != card.uid[..4] {
                return None;
            }
            Some(block / 4)
        })();
        match ok {
            Some(sector) => {
                self.auth_sector = Some(sector);
                self.regs[STATUS2] |= 0x08;
                self.regs[COM_IRQ] |= 0x10;
                self.regs[COMMAND] &= 0xF0;
                self.note(format!("authenticated sector {}", sector));
            }
            None => {
                self.auth_sector = None;
                self.regs[STATUS2] &= !0x08;
                self.regs[COM_IRQ] |= 0x01;
                self.note("authentication failed".into());
            }
        }
        self.update_irq();
    }

    /// Cascade-level bytes (4 UID/CT bytes) for selection level `lvl`.
    fn cl_bytes(uid: &[u8], lvl: usize) -> Option<[u8; 4]> {
        match (uid.len(), lvl) {
            (4, 0) => Some([uid[0], uid[1], uid[2], uid[3]]),
            (7, 0) => Some([0x88, uid[0], uid[1], uid[2]]),
            (7, 1) => Some([uid[3], uid[4], uid[5], uid[6]]),
            (10, 0) => Some([0x88, uid[0], uid[1], uid[2]]),
            (10, 1) => Some([0x88, uid[3], uid[4], uid[5]]),
            (10, 2) => Some([uid[6], uid[7], uid[8], uid[9]]),
            _ => None,
        }
    }

    fn with_crc(mut d: Vec<u8>) -> Vec<u8> {
        let c = crc_a(&d, 0x6363);
        d.extend_from_slice(&c);
        d
    }

    /// Card response to a frame: (bytes, valid bits in the last byte; 0 = all 8).
    fn picc_exchange(&mut self, tx: &[u8], last_bits: u8) -> Option<(Vec<u8>, u8)> {
        let card = self.card.clone()?;
        if tx.is_empty() {
            return None;
        }
        let classic = matches!(card.kind, CardKind::Classic1k);
        // short frames: REQA / WUPA
        if tx.len() == 1 && last_bits == 7 {
            let wake = match tx[0] {
                0x26 => self.picc == Picc::Idle,
                0x52 => matches!(self.picc, Picc::Idle | Picc::Halt),
                _ => false,
            };
            if !wake {
                return None;
            }
            self.picc = Picc::Ready;
            self.level = 0;
            self.auth_sector = None;
            self.regs[STATUS2] &= !0x08;
            return Some((if classic { vec![0x04, 0x00] } else { vec![0x44, 0x00] }, 0));
        }
        // anticollision / select
        if matches!(tx[0], 0x93 | 0x95 | 0x97) && tx.len() >= 2 && matches!(self.picc, Picc::Ready | Picc::Active) {
            let lvl = ((tx[0] - 0x93) / 2) as usize;
            let cl = Self::cl_bytes(&card.uid, lvl)?;
            let bcc = cl.iter().fold(0u8, |a, b| a ^ b);
            let full = [cl[0], cl[1], cl[2], cl[3], bcc];
            let nvb = tx[1];
            if nvb == 0x70 && tx.len() >= 9 {
                if tx[2..7] != full {
                    return None;
                }
                let more = (lvl + 1) * 3 + 1 < card.uid.len();
                let sak = if more { 0x04 } else if classic { 0x08 } else { 0x00 };
                if !more {
                    self.picc = Picc::Active;
                    self.note(format!("card {} selected", hex(&card.uid)));
                }
                self.level = lvl + 1;
                return Some((Self::with_crc(vec![sak]), 0));
            }
            // anticollision: answer with the bytes the reader does not know yet
            let known_bytes = ((nvb >> 4) as usize).saturating_sub(2);
            let known_bits = (nvb & 7) as usize;
            if known_bytes > 5 || tx.len() < 2 + known_bytes {
                return None;
            }
            if tx[2..2 + known_bytes] != full[..known_bytes] {
                return None;
            }
            if known_bits == 0 {
                return Some((full[known_bytes..].to_vec(), 0));
            }
            // partial byte: the reader's RxAlign puts our first bits after its own
            let mut out = full[known_bytes..].to_vec();
            let mask = (1u8 << known_bits) - 1;
            if let Some(&sent) = tx.get(2 + known_bytes) {
                out[0] = (out[0] & !mask) | (sent & mask);
            }
            return Some((out, 0));
        }
        if self.picc != Picc::Active {
            return None;
        }
        // second phase of a MIFARE Classic WRITE: 16 data bytes + CRC
        if let Some(block) = self.pending_write.take() {
            if tx.len() >= 16 && self.auth_sector == Some(block / 4) && block != 0 {
                if let Some(c) = self.card.as_mut() {
                    c.mem[block * 16..block * 16 + 16].copy_from_slice(&tx[..16]);
                }
                self.note(format!("block {} written", block));
                return Some((vec![0x0A], 4));
            }
            return Some((vec![0x00], 4)); // NAK
        }
        // frames with CRC_A appended by the host
        if tx.len() >= 3 {
            let n = tx.len() - 2;
            if crc_a(&tx[..n], 0x6363) != [tx[n], tx[n + 1]] {
                return None;
            }
        }
        let cmd = tx[0];
        let arg = tx.get(1).copied().unwrap_or(0) as usize;
        match (cmd, classic) {
            (0x50, _) => {
                // HLTA
                self.picc = Picc::Halt;
                self.auth_sector = None;
                None
            }
            (0x30, true) => {
                if arg >= 64 || self.auth_sector != Some(arg / 4) {
                    return Some((vec![0x04], 4));
                }
                Some((Self::with_crc(card.mem[arg * 16..arg * 16 + 16].to_vec()), 0))
            }
            (0xA0, true) => {
                if arg >= 64 || arg == 0 || self.auth_sector != Some(arg / 4) {
                    return Some((vec![0x04], 4));
                }
                self.pending_write = Some(arg);
                Some((vec![0x0A], 4))
            }
            (0x30, false) => {
                let pages = card.mem.len() / 4;
                let mut d = Vec::with_capacity(16);
                for i in 0..16 {
                    d.push(card.mem[((arg + i / 4) % pages) * 4 + i % 4]);
                }
                Some((Self::with_crc(d), 0))
            }
            (0xA2, false) => {
                let pages = card.mem.len() / 4;
                if arg < 4 || arg >= pages - 5 || tx.len() < 6 {
                    return Some((vec![0x00], 4));
                }
                if let Some(c) = self.card.as_mut() {
                    c.mem[arg * 4..arg * 4 + 4].copy_from_slice(&tx[2..6]);
                }
                self.note(format!("page {} written", arg));
                Some((vec![0x0A], 4))
            }
            (0x60, false) => Some((Self::with_crc(vec![0x00, 0x04, 0x04, 0x02, 0x01, 0x00, 0x0F, 0x03]), 0)),
            (0x3A, false) => {
                let end = tx.get(2).copied().unwrap_or(0) as usize;
                let pages = card.mem.len() / 4;
                if end < arg || end >= pages {
                    return Some((vec![0x00], 4));
                }
                Some((Self::with_crc(card.mem[arg * 4..(end + 1) * 4].to_vec()), 0))
            }
            _ => None,
        }
    }
}

impl Device for Mfrc522 {
    fn kind(&self) -> &'static str {
        "mfrc522"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![
            pin("SDA", Input), // chip select (labelled SDA on RC522 boards)
            pin("SCK", Input),
            pin("MOSI", Input),
            pin("MISO", Output),
            pin("IRQ", Output),
            pin("GND", Ground),
            pin("RST", Input),
            pin("3.3V", Power),
        ]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::ToRail { a: 4, rail: if self.irq_low { Rail::Gnd } else { Rail::V3 }, ohms: 100.0 });
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        // NRSTPD low = hard power down; rising edge resets the chip
        let rst_low = v.level(6) == Some(false);
        if rst_low && !self.in_reset {
            self.in_reset = true;
        } else if !rst_low && self.in_reset {
            self.in_reset = false;
            self.reset_regs();
            self.update_irq();
        }
    }
    fn spi_pins(&self) -> Option<(usize, usize, Option<usize>, usize)> {
        Some((1, 2, Some(3), 0))
    }
    fn spi_select(&mut self, selected: bool) {
        self.saw_cs = true;
        if selected {
            self.first = true;
        }
    }
    fn spi_transfer(&mut self, _now: u64, data: &mut [u8], _lv: &dyn Fn(usize) -> bool) {
        if !self.saw_cs {
            // CS tied low: treat every transfer as a transaction
            self.first = true;
        }
        for b in data.iter_mut() {
            let mosi = *b;
            if self.in_reset {
                *b = 0;
                continue;
            }
            if self.first {
                self.first = false;
                self.read = mosi & 0x80 != 0;
                self.addr = ((mosi >> 1) & 0x3F) as usize;
                *b = 0;
            } else if self.read {
                *b = self.read_reg(self.addr);
                self.addr = ((mosi >> 1) & 0x3F) as usize;
            } else {
                self.write_reg(self.addr, mosi);
                *b = 0;
            }
        }
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        let act = a.get("action").and_then(|v| v.as_str()).unwrap_or("tap");
        if act == "remove" || act == "away" {
            if let Some(c) = &self.card {
                let u = hex(&c.uid);
                self.note(format!("card {} removed", u));
            }
            self.card = None;
            self.picc = Picc::Idle;
            self.auth_sector = None;
            return Ok(());
        }
        let card = card_from_action(a)?;
        self.note(format!("card presented: {}", hex(&card.uid)));
        self.card = Some(card);
        self.picc = Picc::Idle;
        Ok(())
    }
    fn state(&self) -> Value {
        json!({
            "card": self.card.as_ref().map(|c| json!({"uid": hex(&c.uid), "type": match c.kind { CardKind::Classic1k => "mifare-classic-1k", CardKind::Ntag213 => "ntag213" }})),
            "antenna": self.antenna_on(),
            "selected": self.picc == Picc::Active,
            "log": self.log.iter().rev().take(8).collect::<Vec<_>>()
        })
    }
    fn summary(&self) -> String {
        let ant = if self.antenna_on() { "antenna on" } else { "antenna off" };
        match &self.card {
            Some(c) => format!("{} · card {} present", ant, hex(&c.uid)),
            None => format!("{} · no card", ant),
        }
    }
}

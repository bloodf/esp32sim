//! PN532 NFC reader/writer (I2C interface, address 0x24) with virtual cards.

use crate::parts::{hex, prop_u8, Props};
use crate::{pin, Device, PinInfo, PinKind::*, Rail, Stamp};
use serde_json::{json, Value};
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub enum CardKind {
    Classic1k,
    Ntag213,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub kind: CardKind,
    pub uid: Vec<u8>,
    pub mem: Vec<u8>,
}

impl Card {
    pub fn classic(uid: Vec<u8>) -> Card {
        let mut mem = vec![0u8; 64 * 16];
        let bcc = uid.iter().fold(0u8, |a, b| a ^ b);
        mem[..4].copy_from_slice(&uid[..4]);
        mem[4] = bcc;
        mem[5] = 0x08;
        mem[6] = 0x04;
        mem[7] = 0x00;
        for s in 0..16 {
            let t = (s * 4 + 3) * 16;
            mem[t..t + 6].fill(0xFF);
            mem[t + 6..t + 10].copy_from_slice(&[0xFF, 0x07, 0x80, 0x69]);
            mem[t + 10..t + 16].fill(0xFF);
        }
        Card { kind: CardKind::Classic1k, uid, mem }
    }
    pub fn ntag213(uid: Vec<u8>) -> Card {
        let mut mem = vec![0u8; 45 * 4];
        let u = &uid;
        let bcc0 = 0x88 ^ u[0] ^ u[1] ^ u[2];
        let bcc1 = u[3] ^ u[4] ^ u[5] ^ u[6];
        mem[..12].copy_from_slice(&[u[0], u[1], u[2], bcc0, u[3], u[4], u[5], u[6], bcc1, 0x48, 0x00, 0x00]);
        mem[12..16].copy_from_slice(&[0xE1, 0x10, 0x12, 0x00]);
        mem[16..20].copy_from_slice(&[0x03, 0x00, 0xFE, 0x00]); // empty NDEF TLV
        Card { kind: CardKind::Ntag213, uid, mem }
    }
    /// Write an NDEF text record (NTAG) for convenience.
    pub fn set_text(&mut self, text: &str) {
        if let CardKind::Ntag213 = self.kind {
            let payload: Vec<u8> = [&[0x02u8, b'e', b'n'][..], text.as_bytes()].concat();
            let mut rec = vec![0xD1, 0x01, payload.len() as u8, b'T'];
            rec.extend(payload);
            let mut tlv = vec![0x03, rec.len() as u8];
            tlv.extend(rec);
            tlv.push(0xFE);
            for (i, b) in tlv.iter().enumerate() {
                if 16 + i < self.mem.len() - 20 {
                    self.mem[16 + i] = *b;
                }
            }
        }
    }
}

/// Build a virtual card from a UI / scenario action such as
/// `{"action": "tap", "card": "ntag213", "uid": "04:A2:..", "text": "hello"}`.
pub fn card_from_action(a: &Value) -> Result<Card, String> {
    let kind = a.get("card").and_then(|v| v.as_str()).unwrap_or("mifare");
    let ntag = kind.contains("ntag") || kind.contains("213") || kind.contains("ultra");
    let uid = a
        .get("uid")
        .and_then(|v| v.as_str())
        .and_then(parse_uid)
        .unwrap_or_else(|| if ntag { vec![0x04, 0xA2, 0x4F, 0x1A, 0x73, 0x5C, 0x80] } else { vec![0xDE, 0xAD, 0xBE, 0xEF] });
    let mut card = if ntag {
        if uid.len() != 7 {
            return Err("NTAG cards need a 7-byte UID".into());
        }
        Card::ntag213(uid)
    } else {
        if uid.len() != 4 {
            return Err("MIFARE Classic cards need a 4-byte UID".into());
        }
        Card::classic(uid)
    };
    if let Some(t) = a.get("text").and_then(|v| v.as_str()) {
        card.set_text(t);
    }
    Ok(card)
}

pub fn parse_uid(s: &str) -> Option<Vec<u8>> {
    let h: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if h.len() % 2 != 0 || h.is_empty() {
        return None;
    }
    (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()).collect()
}

pub struct Pn532 {
    addr: u8,
    rx: Vec<u8>,
    out: VecDeque<Vec<u8>>,
    reading: Vec<u8>,
    rpos: usize,
    pending_list: bool,
    pub card: Option<Card>,
    authed: Option<usize>,
    pub log: VecDeque<String>,
    irq_low: bool,
    dirty: bool,
}

impl Pn532 {
    pub fn new(p: &Props) -> Pn532 {
        Pn532 {
            addr: prop_u8(p, &["address", "addr"], 0x24),
            rx: Vec::new(),
            out: VecDeque::new(),
            reading: Vec::new(),
            rpos: 0,
            pending_list: false,
            card: None,
            authed: None,
            log: VecDeque::new(),
            irq_low: false,
            dirty: false,
        }
    }

    fn frame(cmd: u8, data: &[u8]) -> Vec<u8> {
        let mut body = vec![0xD5, cmd.wrapping_add(1)];
        body.extend_from_slice(data);
        let len = body.len() as u8;
        let sum = body.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        let mut f = vec![0x00, 0x00, 0xFF, len, len.wrapping_neg()];
        f.extend(body);
        f.push(sum.wrapping_neg());
        f.push(0x00);
        f
    }

    fn note(&mut self, s: String) {
        if self.log.len() > 50 {
            self.log.pop_front();
        }
        self.log.push_back(s);
    }

    fn target_data(&self) -> Vec<u8> {
        let c = self.card.as_ref().unwrap();
        let (atqa, sak) = match c.kind {
            CardKind::Classic1k => ([0x00, 0x04], 0x08),
            CardKind::Ntag213 => ([0x00, 0x44], 0x00),
        };
        let mut d = vec![0x01, 0x01, atqa[0], atqa[1], sak, c.uid.len() as u8];
        d.extend_from_slice(&c.uid);
        d
    }

    fn exchange(&mut self, d: &[u8]) -> Vec<u8> {
        let card = match self.card.as_mut() {
            Some(c) => c,
            None => return vec![0x01],
        };
        if d.is_empty() {
            return vec![0x27];
        }
        match (d[0], &card.kind) {
            (0x60 | 0x61, CardKind::Classic1k) => {
                let block = d.get(1).copied().unwrap_or(0) as usize;
                let key = &d[2..8.min(d.len())];
                let trailer = ((block / 4) * 4 + 3) * 16;
                let stored = if d[0] == 0x60 { &card.mem[trailer..trailer + 6] } else { &card.mem[trailer + 10..trailer + 16] };
                if key == stored {
                    self.authed = Some(block / 4);
                    vec![0x00]
                } else {
                    self.authed = None;
                    vec![0x14]
                }
            }
            (0x30, CardKind::Classic1k) => {
                let block = d.get(1).copied().unwrap_or(0) as usize;
                if self.authed != Some(block / 4) || block >= 64 {
                    return vec![0x14];
                }
                let mut r = vec![0x00];
                r.extend_from_slice(&card.mem[block * 16..block * 16 + 16]);
                r
            }
            (0xA0, CardKind::Classic1k) => {
                let block = d.get(1).copied().unwrap_or(0) as usize;
                if self.authed != Some(block / 4) || block == 0 || block >= 64 || d.len() < 18 {
                    return vec![0x14];
                }
                card.mem[block * 16..block * 16 + 16].copy_from_slice(&d[2..18]);
                vec![0x00]
            }
            (0x30, CardKind::Ntag213) => {
                let page = d.get(1).copied().unwrap_or(0) as usize;
                let mut r = vec![0x00];
                for i in 0..16 {
                    r.push(*card.mem.get(page * 4 + i).unwrap_or(&0));
                }
                r
            }
            (0xA2, CardKind::Ntag213) => {
                let page = d.get(1).copied().unwrap_or(0) as usize;
                if page < 4 || page >= 45 || d.len() < 6 {
                    return vec![0x14];
                }
                card.mem[page * 4..page * 4 + 4].copy_from_slice(&d[2..6]);
                vec![0x00]
            }
            _ => vec![0x27],
        }
    }

    fn command(&mut self, cmd: u8, data: &[u8]) {
        // ACK first
        self.out.push_back(vec![0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00]);
        let resp: Option<Vec<u8>> = match cmd {
            0x02 => Some(vec![0x32, 0x01, 0x06, 0x07]),
            0x14 | 0x32 | 0x12 | 0x08 => Some(vec![]),
            0x00 => Some(data.to_vec()),
            0x04 => Some(vec![0x00, 0x00, if self.card.is_some() { 1 } else { 0 }, 0x80]),
            0x06 => Some(vec![0; data.len() / 2]),
            0x16 => Some(vec![0x00]),
            0x4A => {
                if self.card.is_some() {
                    self.note("InListPassiveTarget: card found".into());
                    Some(self.target_data())
                } else {
                    self.pending_list = true;
                    None
                }
            }
            0x40 => {
                let r = self.exchange(&data[1.min(data.len())..]);
                self.note(format!("InDataExchange {} -> {}", hex(data), hex(&r)));
                Some(r)
            }
            0x42 => {
                let r = self.exchange(data);
                Some(r)
            }
            0x44 | 0x52 | 0x54 => {
                self.authed = None;
                Some(vec![0x00])
            }
            _ => {
                self.note(format!("unsupported command 0x{:02x}", cmd));
                Some(vec![])
            }
        };
        if let Some(r) = resp {
            self.out.push_back(Self::frame(cmd, &r));
        }
    }

    fn parse_rx(&mut self) {
        let b = std::mem::take(&mut self.rx);
        // find start code 00 FF
        let mut i = 0;
        while i + 1 < b.len() && !(b[i] == 0x00 && b[i + 1] == 0xFF) {
            i += 1;
        }
        if i + 4 >= b.len() {
            return;
        }
        let len = b[i + 2] as usize;
        let body = &b[i + 4..(i + 4 + len).min(b.len())];
        if body.len() < 2 || body[0] != 0xD4 {
            return;
        }
        let (cmd, data) = (body[1], body[2..].to_vec());
        self.command(cmd, &data);
    }

    fn update_irq(&mut self) {
        let low = !self.out.is_empty();
        if low != self.irq_low {
            self.irq_low = low;
            self.dirty = true;
        }
    }
}

impl Device for Pn532 {
    fn kind(&self) -> &'static str {
        "pn532"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("GND", Ground), pin("VCC", Power), pin("SDA", Io), pin("SCL", Input), pin("IRQ", Output), pin("RSTO", Output)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::ToRail { a: 4, rail: if self.irq_low { Rail::Gnd } else { Rail::V3 }, ohms: 100.0 });
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn i2c_pins(&self) -> Option<(usize, usize)> {
        Some((3, 2))
    }
    fn i2c_start(&mut self, addr: u8, read: bool) -> bool {
        if addr != self.addr {
            return false;
        }
        if read {
            self.reading = self.out.front().cloned().unwrap_or_default();
            self.rpos = 0;
        } else {
            self.rx.clear();
        }
        true
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        self.rx.push(b);
        true
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        let i = self.rpos;
        self.rpos += 1;
        if i == 0 {
            return if self.reading.is_empty() { 0x00 } else { 0x01 };
        }
        *self.reading.get(i - 1).unwrap_or(&0x00)
    }
    fn i2c_stop(&mut self) {
        if !self.rx.is_empty() {
            self.parse_rx();
        } else if !self.reading.is_empty() && self.rpos > self.reading.len() {
            // the whole pending frame was read
            self.out.pop_front();
        }
        self.reading.clear();
        self.update_irq();
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        let act = a.get("action").and_then(|v| v.as_str()).unwrap_or("tap");
        match act {
            "remove" | "away" => {
                self.card = None;
                self.authed = None;
            }
            _ => {
                let card = card_from_action(a)?;
                self.note(format!("card presented: {}", hex(&card.uid)));
                self.card = Some(card);
                if self.pending_list {
                    self.pending_list = false;
                    let d = self.target_data();
                    self.out.push_back(Self::frame(0x4A, &d));
                    self.update_irq();
                }
            }
        }
        Ok(())
    }
    fn state(&self) -> Value {
        json!({
            "card": self.card.as_ref().map(|c| json!({"uid": hex(&c.uid), "type": match c.kind { CardKind::Classic1k => "mifare-classic-1k", CardKind::Ntag213 => "ntag213" }})),
            "waiting_for_card": self.pending_list,
            "log": self.log.iter().rev().take(8).collect::<Vec<_>>()
        })
    }
    fn summary(&self) -> String {
        match &self.card {
            Some(c) => format!("card {} present", hex(&c.uid)),
            None => if self.pending_list { "waiting for a card".into() } else { "no card".into() },
        }
    }
}

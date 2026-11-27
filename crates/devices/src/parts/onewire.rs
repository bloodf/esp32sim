//! 1-Wire devices, decoded from the bus waveform: DS18B20 temperature sensor.
//!
//! The line is open drain with a pull-up.  The model watches the falling and
//! rising edges the host makes and answers like the real chip:
//! reset → presence pulse; write slots (short low = 1, long low = 0);
//! read slots (the device holds the line low after the host's falling edge to
//! send a 0).  Several DS18B20s on one bus work (wired-AND through the solver),
//! including SEARCH ROM.

use crate::parts::{prop_f64, Props};
use crate::{pin, Device, PinInfo, PinKind::*, PinView, Rail, Stamp};
use serde_json::{json, Value};

const US: u64 = 1_000;

/// Dallas/Maxim CRC8 (polynomial x^8 + x^5 + x^4 + 1, reflected).
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        let mut x = b;
        for _ in 0..8 {
            let mix = (crc ^ x) & 1;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8C;
            }
            x >>= 1;
        }
    }
    crc
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// waiting for a reset pulse
    Idle,
    /// receiving a ROM command byte
    RomCmd,
    /// MATCH ROM: receiving 64 address bits
    Match,
    /// SEARCH ROM: bit index and sub-step (0 = send bit, 1 = send complement, 2 = read direction)
    Search(u8, u8),
    /// receiving a function command byte
    FuncCmd,
    /// sending bytes from `tx`
    Send,
    /// WRITE SCRATCHPAD: receiving 3 bytes
    WriteScratch,
    /// conversion in progress: read slots return 0 until done
    Converting,
}

pub struct Ds18b20 {
    pub temperature: f64,
    rom: [u8; 8],
    th: u8,
    tl: u8,
    config: u8,
    /// temperature register (power-up value 85 °C until the first conversion)
    treg: i16,
    phase: Phase,
    rx_byte: u8,
    rx_bits: u8,
    rx_buf: Vec<u8>,
    tx: Vec<u8>,
    tx_bit: usize,
    convert_done_at: u64,
    // line tracking
    last_level: bool,
    fall_at: u64,
    /// falling edge belonged to a read slot (device may be holding the line)
    read_slot: bool,
    driving_low: bool,
    release_at: Option<u64>,
    presence_at: Option<u64>,
    /// we just let go of the line: the next rising edge is ours, not the host's
    released: bool,
    dirty: bool,
    pub conversions: u64,
    alarm_search: bool,
}

impl Ds18b20 {
    pub fn new(p: &Props, id_seed: u64) -> Ds18b20 {
        let mut rom = [0x28, 0, 0, 0, 0, 0, 0, 0];
        let serial = p
            .get("serial")
            .and_then(|v| v.as_str())
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0x0000_5A1E_0000 ^ id_seed);
        for i in 0..6 {
            rom[1 + i] = (serial >> (8 * i)) as u8;
        }
        rom[7] = crc8(&rom[..7]);
        Ds18b20 {
            temperature: prop_f64(p, &["temperature"], 22.5),
            rom,
            th: 75,
            tl: 70,
            config: 0x7F,
            treg: 85 * 16,
            phase: Phase::Idle,
            rx_byte: 0,
            rx_bits: 0,
            rx_buf: Vec::new(),
            tx: Vec::new(),
            tx_bit: 0,
            convert_done_at: 0,
            last_level: true,
            fall_at: 0,
            read_slot: false,
            driving_low: false,
            release_at: None,
            presence_at: None,
            released: false,
            dirty: false,
            conversions: 0,
            alarm_search: false,
        }
    }

    pub fn rom_hex(&self) -> String {
        self.rom.iter().map(|b| format!("{:02x}", b)).collect()
    }

    fn resolution_bits(&self) -> u32 {
        9 + ((self.config >> 5) & 3) as u32
    }

    fn scratchpad(&self) -> [u8; 9] {
        let t = self.treg as u16;
        let mut s = [t as u8, (t >> 8) as u8, self.th, self.tl, self.config, 0xFF, 0x0C, 0x10, 0];
        s[8] = crc8(&s[..8]);
        s
    }

    fn alarm(&self) -> bool {
        let t = (self.treg >> 4) as i8;
        t >= self.th as i8 || t <= self.tl as i8
    }

    /// The bit this device sends in the current read slot, if it is talking.
    fn read_bit(&mut self, now: u64) -> Option<bool> {
        match self.phase {
            Phase::Send => {
                let byte = *self.tx.get(self.tx_bit / 8)?;
                let bit = byte >> (self.tx_bit % 8) & 1 != 0;
                self.tx_bit += 1;
                if self.tx_bit >= self.tx.len() * 8 {
                    self.phase = Phase::Idle;
                }
                Some(bit)
            }
            Phase::Search(i, step) if step < 2 => {
                let b = self.rom[i as usize / 8] >> (i % 8) & 1 != 0;
                self.phase = Phase::Search(i, step + 1);
                Some(if step == 0 { b } else { !b })
            }
            Phase::Converting => Some(now >= self.convert_done_at),
            _ => None,
        }
    }

    fn write_bit(&mut self, now: u64, bit: bool) {
        match self.phase {
            Phase::Search(i, 2) => {
                let b = self.rom[i as usize / 8] >> (i % 8) & 1 != 0;
                if bit != b {
                    self.phase = Phase::Idle; // not selected: drop out until the next reset
                } else if i == 63 {
                    self.phase = Phase::FuncCmd;
                } else {
                    self.phase = Phase::Search(i + 1, 0);
                }
                return;
            }
            Phase::RomCmd | Phase::FuncCmd | Phase::Match | Phase::WriteScratch => {}
            _ => return,
        }
        self.rx_byte = (self.rx_byte >> 1) | if bit { 0x80 } else { 0 };
        self.rx_bits += 1;
        if self.rx_bits < 8 {
            return;
        }
        let b = self.rx_byte;
        self.rx_bits = 0;
        self.rx_byte = 0;
        self.on_byte(now, b);
    }

    fn on_byte(&mut self, now: u64, b: u8) {
        match self.phase {
            Phase::RomCmd => match b {
                0x33 => {
                    // READ ROM
                    self.tx = self.rom.to_vec();
                    self.tx_bit = 0;
                    self.phase = Phase::Send;
                }
                0x55 => {
                    self.rx_buf.clear();
                    self.phase = Phase::Match;
                }
                0xCC => self.phase = Phase::FuncCmd,
                0xF0 => self.phase = Phase::Search(0, 0),
                0xEC => {
                    // ALARM SEARCH: only devices in alarm take part
                    self.alarm_search = true;
                    self.phase = if self.alarm() { Phase::Search(0, 0) } else { Phase::Idle };
                }
                _ => self.phase = Phase::Idle,
            },
            Phase::Match => {
                self.rx_buf.push(b);
                if self.rx_buf.len() == 8 {
                    self.phase = if self.rx_buf[..] == self.rom[..] { Phase::FuncCmd } else { Phase::Idle };
                }
            }
            Phase::FuncCmd => match b {
                0x44 => {
                    // CONVERT T
                    let ms = [94u64, 188, 375, 750][(self.resolution_bits() - 9) as usize];
                    self.convert_done_at = now + ms * 1_000_000;
                    let bits = self.resolution_bits();
                    let raw = (self.temperature.clamp(-55.0, 125.0) * 16.0).round() as i16;
                    // lower resolutions leave the low bits undefined (0 here)
                    let mask = !((1i16 << (12 - bits)) - 1);
                    self.treg = raw & mask;
                    self.conversions += 1;
                    self.phase = Phase::Converting;
                }
                0xBE => {
                    self.tx = self.scratchpad().to_vec();
                    self.tx_bit = 0;
                    self.phase = Phase::Send;
                }
                0x4E => {
                    self.rx_buf.clear();
                    self.phase = Phase::WriteScratch;
                }
                0x48 | 0xB8 => self.phase = Phase::Idle, // copy / recall EEPROM: instant
                0xB4 => {
                    // READ POWER SUPPLY: externally powered -> read slots return 1
                    self.tx = vec![0xFF];
                    self.tx_bit = 0;
                    self.phase = Phase::Send;
                }
                _ => self.phase = Phase::Idle,
            },
            Phase::WriteScratch => {
                self.rx_buf.push(b);
                match self.rx_buf.len() {
                    1 => self.th = b,
                    2 => self.tl = b,
                    _ => {
                        self.config = (b & 0x60) | 0x1F;
                        self.phase = Phase::Idle;
                    }
                }
            }
            _ => {}
        }
    }
}

impl Device for Ds18b20 {
    fn kind(&self) -> &'static str {
        "ds18b20"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("GND", Ground), pin("DQ", Io), pin("VDD", Power)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        if self.driving_low {
            out.push(Stamp::ToRail { a: 1, rail: Rail::Gnd, ohms: 30.0 });
        }
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn on_solved(&mut self, now: u64, v: &PinView) {
        let level = v.level(1).unwrap_or(true);
        if level == self.last_level {
            return;
        }
        self.last_level = level;
        if !level {
            // falling edge (ours never start a slot: we only pull low after the host did)
            self.released = false;
            if self.driving_low {
                return;
            }
            self.fall_at = now;
            self.read_slot = false;
            if let Some(bit) = self.read_bit(now) {
                self.read_slot = true;
                if !bit {
                    self.driving_low = true;
                    self.dirty = true;
                    self.release_at = Some(now + 30 * US);
                }
            }
        } else {
            if std::mem::take(&mut self.released) {
                return;
            }
            let low_for = now.saturating_sub(self.fall_at);
            if low_for >= 400 * US {
                // reset pulse: answer with presence
                self.phase = Phase::RomCmd;
                self.rx_bits = 0;
                self.rx_byte = 0;
                self.alarm_search = false;
                self.presence_at = Some(now + 20 * US);
                return;
            }
            if !self.read_slot && !self.driving_low {
                self.write_bit(now, low_for < 15 * US);
            }
        }
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        if let Some(t) = self.presence_at {
            if now >= t {
                self.presence_at = None;
                self.driving_low = true;
                self.dirty = true;
                self.release_at = Some(t + 120 * US);
            }
        }
        if let Some(t) = self.release_at {
            if now >= t {
                self.release_at = None;
                self.driving_low = false;
                self.released = true;
                self.dirty = true;
            }
        }
        match (self.presence_at, self.release_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        if let Some(t) = a.get("temperature").and_then(|v| v.as_f64()) {
            self.temperature = t;
            return Ok(());
        }
        Err("ds18b20 accepts {\"temperature\": °C}".into())
    }
    fn state(&self) -> Value {
        json!({"temperature": self.temperature, "rom": self.rom_hex(), "resolution": self.resolution_bits(), "conversions": self.conversions})
    }
    fn summary(&self) -> String {
        format!("{:.2} °C · ROM {} · {} conversions", self.temperature, self.rom_hex(), self.conversions)
    }
}

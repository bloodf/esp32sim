//! I2C I/O expanders, relay boards, ADCs and EEPROMs.

use crate::parts::{hex, prop_f64, prop_str, prop_u8, Props};
use crate::{pin, Device, PinInfo, PinKind::*, PinView, Rail, Stamp};
use serde_json::{json, Value};

// ===================================================================== PCF8574 / PCF8575
pub struct Pcf857x {
    bits: usize,
    kind: &'static str,
    addr: u8,
    pub latch: u16,
    pub inputs: u16,
    last_read: u16,
    int_low: bool,
    wbyte: usize,
    rbyte: usize,
    dirty: bool,
}
const P8: [&str; 8] = ["P0", "P1", "P2", "P3", "P4", "P5", "P6", "P7"];
const P16: [&str; 16] = ["P00", "P01", "P02", "P03", "P04", "P05", "P06", "P07", "P10", "P11", "P12", "P13", "P14", "P15", "P16", "P17"];

impl Pcf857x {
    pub fn new(p: &Props, bits: usize, variant_a: bool) -> Pcf857x {
        let def = if variant_a { 0x38 } else { 0x20 };
        Pcf857x {
            bits,
            kind: match (bits, variant_a) {
                (8, false) => "pcf8574",
                (8, true) => "pcf8574a",
                _ => "pcf8575",
            },
            addr: prop_u8(p, &["address", "addr"], def),
            latch: 0xFFFF,
            inputs: 0xFFFF,
            last_read: 0xFFFF,
            int_low: false,
            wbyte: 0,
            rbyte: 0,
            dirty: false,
        }
    }
    fn mask(&self) -> u16 {
        if self.bits == 8 {
            0xFF
        } else {
            0xFFFF
        }
    }
}
impl Device for Pcf857x {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn pins(&self) -> Vec<PinInfo> {
        let mut v = vec![pin("VCC", Power), pin("GND", Ground), pin("SDA", Io), pin("SCL", Input), pin("INT", Output)];
        let names: &[&str] = if self.bits == 8 { &P8 } else { &P16 };
        for n in names {
            v.push(pin(n, Io));
        }
        v
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        for i in 0..self.bits {
            if self.latch & (1 << i) == 0 {
                out.push(Stamp::ToRail { a: 5 + i, rail: Rail::Gnd, ohms: 30.0 });
            } else {
                out.push(Stamp::ToRail { a: 5 + i, rail: Rail::V3, ohms: 33_000.0 });
            }
        }
        if self.int_low {
            out.push(Stamp::ToRail { a: 4, rail: Rail::Gnd, ohms: 50.0 });
        }
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        let mut inp = 0u16;
        for i in 0..self.bits {
            if v.level(5 + i).unwrap_or(true) {
                inp |= 1 << i;
            }
        }
        self.inputs = inp;
        let int = (inp ^ self.last_read) & self.mask() != 0;
        if int != self.int_low {
            self.int_low = int;
            self.dirty = true;
        }
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
            self.rbyte = 0;
        } else {
            self.wbyte = 0;
        }
        true
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        if self.bits == 8 {
            self.latch = b as u16;
        } else if self.wbyte % 2 == 0 {
            self.latch = (self.latch & 0xFF00) | b as u16;
        } else {
            self.latch = (self.latch & 0x00FF) | ((b as u16) << 8);
        }
        self.wbyte += 1;
        self.dirty = true;
        true
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        let v = self.inputs & self.mask();
        self.last_read = v;
        if self.int_low {
            self.int_low = false;
            self.dirty = true;
        }
        let b = if self.bits == 8 || self.rbyte % 2 == 0 { v as u8 } else { (v >> 8) as u8 };
        self.rbyte += 1;
        b
    }
    fn state(&self) -> Value {
        json!({"address": self.addr, "output_latch": self.latch & self.mask(), "inputs": self.inputs & self.mask(), "bits": self.bits})
    }
    fn summary(&self) -> String {
        let w = self.bits;
        format!("@0x{:02x} pins={:0w$b}", self.addr, self.inputs & self.mask(), w = w)
    }
}

// ===================================================================== I2C relay board (PCF8574 + relays)
pub struct I2cRelay {
    addr: u8,
    channels: usize,
    active_low: bool,
    pub latch: u8,
    dirty: bool,
}
const RELAY_NAMES: [[&str; 3]; 8] = [
    ["COM1", "NO1", "NC1"],
    ["COM2", "NO2", "NC2"],
    ["COM3", "NO3", "NC3"],
    ["COM4", "NO4", "NC4"],
    ["COM5", "NO5", "NC5"],
    ["COM6", "NO6", "NC6"],
    ["COM7", "NO7", "NC7"],
    ["COM8", "NO8", "NC8"],
];
impl I2cRelay {
    pub fn new(p: &Props) -> I2cRelay {
        I2cRelay {
            addr: prop_u8(p, &["address", "addr"], 0x20),
            channels: prop_f64(p, &["channels"], 4.0).clamp(1.0, 8.0) as usize,
            active_low: p.get("active_low").and_then(|v| v.as_bool()).unwrap_or(true),
            latch: 0xFF,
            dirty: true,
        }
    }
    fn on(&self, c: usize) -> bool {
        (self.latch & (1 << c) != 0) != self.active_low
    }
}
impl Device for I2cRelay {
    fn kind(&self) -> &'static str {
        "i2c-relay"
    }
    fn pins(&self) -> Vec<PinInfo> {
        let mut v = vec![pin("VCC", Power), pin("GND", Ground), pin("SDA", Io), pin("SCL", Input)];
        for c in 0..self.channels {
            for n in RELAY_NAMES[c] {
                v.push(pin(n, Passive));
            }
        }
        v
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        for c in 0..self.channels {
            let base = 4 + c * 3;
            out.push(Stamp::Resistor { a: base, b: if self.on(c) { base + 1 } else { base + 2 }, ohms: 0.05 });
        }
    }
    fn i2c_pins(&self) -> Option<(usize, usize)> {
        Some((3, 2))
    }
    fn i2c_start(&mut self, addr: u8, _read: bool) -> bool {
        addr == self.addr
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        if b != self.latch {
            self.latch = b;
            self.dirty = true;
        }
        true
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        self.latch
    }
    fn state(&self) -> Value {
        let on: Vec<bool> = (0..self.channels).map(|c| self.on(c)).collect();
        json!({"address": self.addr, "on": on})
    }
    fn summary(&self) -> String {
        (0..self.channels).map(|c| if self.on(c) { "ON" } else { "off" }).collect::<Vec<_>>().join(" ")
    }
}

// ===================================================================== MCP23017 16-bit I/O expander
pub struct Mcp23017 {
    addr: u8,
    regs: [u8; 0x16],
    ptr: usize,
    got_ptr: bool,
    pub inputs: u16,
    int_flag: u16,
    dirty: bool,
}
impl Mcp23017 {
    pub fn new(p: &Props) -> Mcp23017 {
        let mut regs = [0u8; 0x16];
        regs[0] = 0xFF;
        regs[1] = 0xFF;
        Mcp23017 { addr: prop_u8(p, &["address", "addr"], 0x20), regs, ptr: 0, got_ptr: false, inputs: 0xFFFF, int_flag: 0, dirty: false }
    }
    fn iodir(&self) -> u16 {
        self.regs[0] as u16 | ((self.regs[1] as u16) << 8)
    }
    fn olat(&self) -> u16 {
        self.regs[0x14] as u16 | ((self.regs[0x15] as u16) << 8)
    }
    fn pullup(&self) -> u16 {
        self.regs[0x0C] as u16 | ((self.regs[0x0D] as u16) << 8)
    }
}
impl Device for Mcp23017 {
    fn kind(&self) -> &'static str {
        "mcp23017"
    }
    fn pins(&self) -> Vec<PinInfo> {
        let mut v = vec![pin("VDD", Power), pin("VSS", Ground), pin("SDA", Io), pin("SCL", Input), pin("INTA", Output), pin("INTB", Output), pin("RESET", Input)];
        for n in ["GPA0", "GPA1", "GPA2", "GPA3", "GPA4", "GPA5", "GPA6", "GPA7", "GPB0", "GPB1", "GPB2", "GPB3", "GPB4", "GPB5", "GPB6", "GPB7"] {
            v.push(pin(n, Io));
        }
        v
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        let (dir, lat, pu) = (self.iodir(), self.olat(), self.pullup());
        for i in 0..16 {
            let bit = 1u16 << i;
            if dir & bit == 0 {
                out.push(Stamp::ToRail { a: 7 + i, rail: if lat & bit != 0 { Rail::V3 } else { Rail::Gnd }, ohms: 30.0 });
            } else if pu & bit != 0 {
                out.push(Stamp::ToRail { a: 7 + i, rail: Rail::V3, ohms: 100_000.0 });
            }
        }
        if self.int_flag & 0xFF != 0 {
            out.push(Stamp::ToRail { a: 4, rail: Rail::Gnd, ohms: 50.0 });
        }
        if self.int_flag & 0xFF00 != 0 {
            out.push(Stamp::ToRail { a: 5, rail: Rail::Gnd, ohms: 50.0 });
        }
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        let mut inp = 0u16;
        for i in 0..16 {
            if v.level(7 + i).unwrap_or(false) {
                inp |= 1 << i;
            }
        }
        let gpinten = self.regs[4] as u16 | ((self.regs[5] as u16) << 8);
        let changed = (inp ^ self.inputs) & gpinten & self.iodir();
        if changed != 0 {
            self.int_flag |= changed;
            self.regs[0x0E] = self.int_flag as u8;
            self.regs[0x0F] = (self.int_flag >> 8) as u8;
            self.regs[0x10] = inp as u8;
            self.regs[0x11] = (inp >> 8) as u8;
            self.dirty = true;
        }
        self.inputs = inp;
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
        if !read {
            self.got_ptr = false;
        }
        true
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        if !self.got_ptr {
            self.ptr = b as usize % 0x16;
            self.got_ptr = true;
            return true;
        }
        match self.ptr {
            0x12 => self.regs[0x14] = b,
            0x13 => self.regs[0x15] = b,
            p => self.regs[p] = b,
        }
        self.dirty = true;
        self.ptr = (self.ptr + 1) % 0x16;
        true
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        let v = match self.ptr {
            0x12 => {
                self.int_flag &= 0xFF00;
                self.dirty = true;
                (self.inputs & 0xFF) as u8 ^ self.regs[2]
            }
            0x13 => {
                self.int_flag &= 0x00FF;
                self.dirty = true;
                (self.inputs >> 8) as u8 ^ self.regs[3]
            }
            p => self.regs[p],
        };
        self.ptr = (self.ptr + 1) % 0x16;
        v
    }
    fn state(&self) -> Value {
        json!({"address": self.addr, "iodir": self.iodir(), "olat": self.olat(), "inputs": self.inputs})
    }
    fn summary(&self) -> String {
        format!("@0x{:02x} GPIO={:016b}", self.addr, self.inputs)
    }
}

// ===================================================================== ADS1115 / ADS1015
pub struct Ads1x15 {
    bits12: bool,
    addr: u8,
    ptr: u8,
    wbytes: Vec<u8>,
    rbyte: usize,
    config: u16,
    lo: u16,
    hi: u16,
    conv: i16,
    volts: [f64; 4],
}
impl Ads1x15 {
    pub fn new(p: &Props, bits12: bool) -> Ads1x15 {
        Ads1x15 { bits12, addr: prop_u8(p, &["address", "addr"], 0x48), ptr: 0, wbytes: Vec::new(), rbyte: 0, config: 0x8583, lo: 0x8000, hi: 0x7FFF, conv: 0, volts: [0.0; 4] }
    }
    fn convert(&mut self) {
        let mux = (self.config >> 12) & 7;
        let v = match mux {
            0 => self.volts[0] - self.volts[1],
            1 => self.volts[0] - self.volts[3],
            2 => self.volts[1] - self.volts[3],
            3 => self.volts[2] - self.volts[3],
            m => self.volts[(m - 4) as usize],
        };
        let fs = [6.144, 4.096, 2.048, 1.024, 0.512, 0.256, 0.256, 0.256][((self.config >> 9) & 7) as usize];
        let raw = (v / fs * 32768.0).round().clamp(-32768.0, 32767.0) as i32;
        self.conv = if self.bits12 { ((raw >> 4) << 4) as i16 } else { raw as i16 };
    }
}
impl Device for Ads1x15 {
    fn kind(&self) -> &'static str {
        if self.bits12 {
            "ads1015"
        } else {
            "ads1115"
        }
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VDD", Power), pin("GND", Ground), pin("SCL", Input), pin("SDA", Io), pin("ADDR", Input), pin("ALRT", Output), pin("A0", Analog), pin("A1", Analog), pin("A2", Analog), pin("A3", Analog)]
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        for i in 0..4 {
            self.volts[i] = v.v(6 + i).unwrap_or(0.0);
        }
        if self.config & 0x0100 == 0 {
            self.convert(); // continuous mode
        }
    }
    fn i2c_pins(&self) -> Option<(usize, usize)> {
        Some((2, 3))
    }
    fn i2c_start(&mut self, addr: u8, read: bool) -> bool {
        if addr != self.addr {
            return false;
        }
        if read {
            self.rbyte = 0;
        } else {
            self.wbytes.clear();
        }
        true
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        self.wbytes.push(b);
        if self.wbytes.len() == 1 {
            self.ptr = b & 3;
        } else if self.wbytes.len() == 3 {
            let v = ((self.wbytes[1] as u16) << 8) | self.wbytes[2] as u16;
            match self.ptr {
                1 => {
                    self.config = v & 0x7FFF;
                    if v & 0x8000 != 0 || v & 0x0100 == 0 {
                        self.convert();
                    }
                }
                2 => self.lo = v,
                3 => self.hi = v,
                _ => {}
            }
        }
        true
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        let v: u16 = match self.ptr {
            0 => self.conv as u16,
            1 => self.config | 0x8000, // OS=1: not converting
            2 => self.lo,
            _ => self.hi,
        };
        let b = if self.rbyte % 2 == 0 { (v >> 8) as u8 } else { v as u8 };
        self.rbyte += 1;
        b
    }
    fn state(&self) -> Value {
        json!({"address": self.addr, "inputs": self.volts, "last": self.conv})
    }
    fn summary(&self) -> String {
        format!("A0={:.3}V A1={:.3}V A2={:.3}V A3={:.3}V", self.volts[0], self.volts[1], self.volts[2], self.volts[3])
    }
}

// ===================================================================== MCP3421 18-bit delta-sigma ADC
pub struct Mcp3421 {
    addr: u8,
    config: u8,
    vin: f64,
    rbyte: usize,
    data: [u8; 4],
}
impl Mcp3421 {
    pub fn new(p: &Props) -> Mcp3421 {
        Mcp3421 { addr: prop_u8(p, &["address", "addr"], 0x68), config: 0x90, vin: 0.0, rbyte: 0, data: [0; 4] }
    }
    fn sample(&mut self) {
        let bits = [12, 14, 16, 18][((self.config >> 2) & 3) as usize];
        let gain = [1.0, 2.0, 4.0, 8.0][(self.config & 3) as usize];
        let lsb = 2.0 * 2.048 / (1u64 << bits) as f64;
        let max = (1i64 << (bits - 1)) - 1;
        let code = ((self.vin * gain / lsb).round() as i64).clamp(-max - 1, max);
        let cfg = self.config & 0x7F; // RDY = 0: new data
        if bits == 18 {
            let c = (code as u32) & 0x3FFFF;
            let c = if code < 0 { c | 0xFFFC_0000 } else { c };
            self.data = [(c >> 16) as u8, (c >> 8) as u8, c as u8, cfg];
        } else {
            let c = code as i16 as u16;
            self.data = [(c >> 8) as u8, c as u8, cfg, cfg];
        }
    }
}
impl Device for Mcp3421 {
    fn kind(&self) -> &'static str {
        "mcp3421"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VIN+", Analog), pin("VSS", Ground), pin("SCL", Input), pin("SDA", Io), pin("VDD", Power), pin("VIN-", Analog)]
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        self.vin = v.v(0).unwrap_or(0.0) - v.v(5).unwrap_or(0.0);
    }
    fn i2c_pins(&self) -> Option<(usize, usize)> {
        Some((2, 3))
    }
    fn i2c_start(&mut self, addr: u8, read: bool) -> bool {
        if addr != self.addr {
            return false;
        }
        if read {
            self.sample();
            self.rbyte = 0;
        }
        true
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        self.config = b;
        true
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        let v = self.data[self.rbyte.min(3)];
        self.rbyte += 1;
        v
    }
    fn state(&self) -> Value {
        json!({"address": self.addr, "vin": self.vin, "config": self.config})
    }
    fn summary(&self) -> String {
        format!("@0x{:02x} VIN={:.5} V", self.addr, self.vin)
    }
}

// ===================================================================== AT24Cxx EEPROM
pub struct At24c {
    addr: u8,
    mem: Vec<u8>,
    ptr: usize,
    wcount: usize,
    two_byte: bool,
    page: usize,
}
impl At24c {
    pub fn new(p: &Props) -> At24c {
        let kib = prop_f64(p, &["kbit", "size_kbit"], 32.0) as usize;
        let bytes = kib * 128;
        At24c { addr: prop_u8(p, &["address", "addr"], 0x50), mem: vec![0xFF; bytes.max(128)], ptr: 0, wcount: 0, two_byte: bytes > 2048, page: if bytes > 4096 { 64 } else { 32 } }
    }
}
impl Device for At24c {
    fn kind(&self) -> &'static str {
        "at24c"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("GND", Ground), pin("SCL", Input), pin("SDA", Io), pin("WP", Input)]
    }
    fn i2c_pins(&self) -> Option<(usize, usize)> {
        Some((2, 3))
    }
    fn i2c_start(&mut self, addr: u8, read: bool) -> bool {
        if addr != self.addr {
            return false;
        }
        if !read {
            self.wcount = 0;
        }
        true
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        let hdr = if self.two_byte { 2 } else { 1 };
        if self.wcount < hdr {
            self.ptr = if self.wcount == 0 && self.two_byte { (b as usize) << 8 } else { (self.ptr & !0xFF) | b as usize };
            self.ptr %= self.mem.len();
        } else {
            let page_base = self.ptr & !(self.page - 1);
            let off = (self.ptr - page_base + (self.wcount - hdr)) % self.page;
            let a = (page_base + off) % self.mem.len();
            self.mem[a] = b;
        }
        self.wcount += 1;
        if self.wcount > hdr && self.wcount - hdr >= 1 {
            // pointer after page write
        }
        true
    }
    fn i2c_stop(&mut self) {
        let hdr = if self.two_byte { 2 } else { 1 };
        if self.wcount > hdr {
            self.ptr = (self.ptr + self.wcount - hdr) % self.mem.len();
        }
        self.wcount = 0;
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        let v = self.mem[self.ptr];
        self.ptr = (self.ptr + 1) % self.mem.len();
        v
    }
    fn state(&self) -> Value {
        json!({"address": self.addr, "size": self.mem.len(), "head": hex(&self.mem[..64.min(self.mem.len())])})
    }
    fn summary(&self) -> String {
        format!("{} bytes @0x{:02x}", self.mem.len(), self.addr)
    }
}

pub fn _unused(p: &Props) -> &str {
    prop_str(p, &["x"], "")
}

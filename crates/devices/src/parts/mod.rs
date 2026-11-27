//! Device models.

pub mod basic;
pub mod display;
pub mod i2c_io;
pub mod i2c_sensors;
pub mod keypad;
pub mod logic;
pub mod motion;
pub mod onewire;
pub mod pn532;
pub mod rfid;
pub mod sdcard;
pub mod serial;
pub mod timed;
pub mod ws2812;

use serde_json::{Map, Value};

pub type Props = Map<String, Value>;

/// Parse "4.7k", "10M", "220R", "100nF", "2.2uF", "10µF", "4k7", "1e3" etc.
pub fn parse_si(s: &str) -> Option<f64> {
    let t = s.trim().replace('Ω', "").replace("ohm", "").replace("Ohm", "");
    let t = t.trim_end_matches(['F', 'f', 'H', 'V', 'v', 'A', 'R', 'r']).trim();
    if let Ok(v) = t.parse::<f64>() {
        return Some(v);
    }
    let (num, mult) = match t.chars().last()? {
        'k' | 'K' => (&t[..t.len() - 1], 1e3),
        'M' => (&t[..t.len() - 1], 1e6),
        'G' => (&t[..t.len() - 1], 1e9),
        'm' => (&t[..t.len() - 1], 1e-3),
        'u' => (&t[..t.len() - 1], 1e-6),
        'µ' => (&t[..t.len() - 'µ'.len_utf8()], 1e-6),
        'n' => (&t[..t.len() - 1], 1e-9),
        'p' => (&t[..t.len() - 1], 1e-12),
        _ => {
            // "4k7" style
            for (i, c) in t.char_indices() {
                let m = match c {
                    'k' | 'K' => 1e3,
                    'M' => 1e6,
                    'R' | 'r' => 1.0,
                    _ => continue,
                };
                let s2 = format!("{}.{}", &t[..i], &t[i + 1..]);
                return s2.parse::<f64>().ok().map(|v| v * m);
            }
            return None;
        }
    };
    num.trim().parse::<f64>().ok().map(|v| v * mult)
}

pub fn prop_f64(p: &Props, keys: &[&str], default: f64) -> f64 {
    for k in keys {
        if let Some(v) = p.get(*k) {
            match v {
                Value::Number(n) => return n.as_f64().unwrap_or(default),
                Value::String(s) => {
                    if let Some(x) = s.strip_prefix("0x").or(s.strip_prefix("0X")) {
                        if let Ok(v) = u64::from_str_radix(x, 16) {
                            return v as f64;
                        }
                    }
                    if let Some(x) = parse_si(s) {
                        return x;
                    }
                }
                Value::Bool(b) => return if *b { 1.0 } else { 0.0 },
                _ => {}
            }
        }
    }
    default
}

pub fn prop_str<'a>(p: &'a Props, keys: &[&str], default: &'a str) -> &'a str {
    for k in keys {
        if let Some(Value::String(s)) = p.get(*k) {
            return s.as_str();
        }
    }
    default
}

pub fn prop_u8(p: &Props, keys: &[&str], default: u8) -> u8 {
    prop_f64(p, keys, default as f64) as u8
}

pub fn hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Generic register-file I2C target: first written byte selects the register
/// pointer, subsequent bytes are written/read with auto-increment.
#[derive(Clone)]
pub struct RegTarget {
    pub addr: u8,
    pub regs: Vec<u8>,
    pub ptr: usize,
    pub got_ptr: bool,
    pub active: bool,
}

impl RegTarget {
    pub fn new(addr: u8, size: usize) -> RegTarget {
        RegTarget { addr, regs: vec![0; size], ptr: 0, got_ptr: false, active: false }
    }
    pub fn start(&mut self, addr: u8, read: bool) -> bool {
        if addr != self.addr {
            self.active = false;
            return false;
        }
        self.active = true;
        if !read {
            self.got_ptr = false;
        }
        true
    }
    /// Returns Some((register, value)) for a data write, None for the pointer byte.
    pub fn write(&mut self, b: u8) -> Option<(usize, u8)> {
        if !self.got_ptr {
            self.ptr = b as usize % self.regs.len();
            self.got_ptr = true;
            None
        } else {
            let r = self.ptr;
            self.regs[r] = b;
            self.ptr = (self.ptr + 1) % self.regs.len();
            Some((r, b))
        }
    }
    pub fn read_next(&mut self) -> (usize, u8) {
        let r = self.ptr;
        let v = self.regs[r];
        self.ptr = (self.ptr + 1) % self.regs.len();
        (r, v)
    }
}

pub fn put_be16(regs: &mut [u8], at: usize, v: i32) {
    let v = v.clamp(-32768, 32767) as i16 as u16;
    regs[at] = (v >> 8) as u8;
    regs[at + 1] = v as u8;
}
pub fn put_le16(regs: &mut [u8], at: usize, v: i32) {
    let v = v.clamp(-32768, 32767) as i16 as u16;
    regs[at] = v as u8;
    regs[at + 1] = (v >> 8) as u8;
}

/// Read an {x,y,z} triple from an input action.
pub fn xyz(a: &Value, key: &str) -> Option<[f64; 3]> {
    let v = a.get(key)?;
    if let Some(arr) = v.as_array() {
        if arr.len() == 3 {
            return Some([arr[0].as_f64()?, arr[1].as_f64()?, arr[2].as_f64()?]);
        }
    }
    Some([v.get("x")?.as_f64()?, v.get("y")?.as_f64()?, v.get("z")?.as_f64()?])
}

#[cfg(test)]
mod tests {
    use super::parse_si;
    #[test]
    fn si_values() {
        let close = |s: &str, v: f64| {
            let got = parse_si(s).unwrap_or_else(|| panic!("{} did not parse", s));
            assert!((got - v).abs() <= v.abs() * 1e-9, "{} -> {} (expected {})", s, got, v);
        };
        close("220", 220.0);
        close("4.7k", 4700.0);
        close("4k7", 4700.0);
        close("10M", 10e6);
        close("220R", 220.0);
        close("100n", 100e-9);
        close("100nF", 100e-9);
        close("10u", 10e-6);
        close("2.2uF", 2.2e-6);
        close("10µF", 10e-6);
        close("47p", 47e-12);
        close("1e3", 1000.0);
        close("10 kΩ", 10e3);
    }
}

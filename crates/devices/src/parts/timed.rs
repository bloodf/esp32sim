//! Devices with timing based protocols: DHT11/DHT22, HC-SR04, rotary encoder.

use crate::parts::{prop_f64, Props};
use crate::{pin, Device, PinInfo, PinKind::*, PinView, Rail, Stamp};
use serde_json::{json, Value};
use std::collections::VecDeque;

const US: u64 = 1_000;

// ------------------------------------------------------------------ DHT11 / DHT22
pub struct Dht {
    dht22: bool,
    pub temperature: f64,
    pub humidity: f64,
    last_level: bool,
    /// when the host pulled the idle line low (start signal in progress)
    host_fall: Option<u64>,
    /// scheduled output changes: (time, drive_low)
    schedule: VecDeque<(u64, bool)>,
    driving_low: bool,
    dirty: bool,
    pub reads: u64,
}

impl Dht {
    pub fn new(p: &Props, dht22: bool) -> Dht {
        Dht {
            dht22,
            temperature: prop_f64(p, &["temperature"], 24.0),
            humidity: prop_f64(p, &["humidity"], 45.0),
            last_level: true,
            host_fall: None,
            schedule: VecDeque::new(),
            driving_low: false,
            dirty: false,
            reads: 0,
        }
    }
    fn frame(&self) -> [u8; 5] {
        let mut d = [0u8; 5];
        if self.dht22 {
            let h = (self.humidity.clamp(0.0, 100.0) * 10.0).round() as u16;
            let t = (self.temperature.abs() * 10.0).round() as u16 & 0x7FFF;
            let t = if self.temperature < 0.0 { t | 0x8000 } else { t };
            d[0] = (h >> 8) as u8;
            d[1] = h as u8;
            d[2] = (t >> 8) as u8;
            d[3] = t as u8;
        } else {
            d[0] = self.humidity.clamp(0.0, 100.0) as u8;
            d[1] = ((self.humidity.fract()) * 10.0) as u8;
            d[2] = self.temperature.clamp(0.0, 60.0) as u8;
            d[3] = ((self.temperature.fract()) * 10.0) as u8;
        }
        d[4] = d[0].wrapping_add(d[1]).wrapping_add(d[2]).wrapping_add(d[3]);
        d
    }
    fn start_response(&mut self, t0: u64) {
        // 20..40us after host release: 80us low, 80us high, then 40 bits
        let mut t = t0 + 30 * US;
        let mut push = |s: &mut VecDeque<(u64, bool)>, dt: u64, low: bool| {
            s.push_back((t, low));
            t += dt;
        };
        self.schedule.clear();
        push(&mut self.schedule, 80 * US, true);
        push(&mut self.schedule, 80 * US, false);
        for byte in self.frame() {
            for bit in (0..8).rev() {
                let one = byte & (1 << bit) != 0;
                push(&mut self.schedule, 50 * US, true);
                push(&mut self.schedule, if one { 70 * US } else { 27 * US }, false);
            }
        }
        push(&mut self.schedule, 50 * US, true);
        push(&mut self.schedule, 0, false);
        self.reads += 1;
    }
}

impl Device for Dht {
    fn kind(&self) -> &'static str {
        if self.dht22 {
            "dht22"
        } else {
            "dht11"
        }
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("SDA", Io), pin("NC", Passive), pin("GND", Ground)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        // module pull-up (most breakout boards have 10k) + open-drain output
        out.push(Stamp::ToRail { a: 1, rail: Rail::V3, ohms: 10_000.0 });
        if self.driving_low {
            out.push(Stamp::ToRail { a: 1, rail: Rail::Gnd, ohms: 40.0 });
        }
    }
    fn on_solved(&mut self, now: u64, v: &PinView) {
        let level = v.level(1).unwrap_or(true);
        if self.schedule.is_empty() && !self.driving_low {
            if self.last_level && !level {
                self.host_fall = Some(now);
            } else if !self.last_level && level {
                // only a low pulse made by the host counts as a start signal
                // (not the end of the sensor's own frame)
                if let Some(fall) = self.host_fall.take() {
                    let low_for = now.saturating_sub(fall);
                    let need = if self.dht22 { 500 * US } else { 10_000 * US };
                    if low_for >= need && low_for < 100_000 * US {
                        self.start_response(now);
                    }
                }
            }
        } else {
            self.host_fall = None;
        }
        self.last_level = level;
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        while let Some(&(t, low)) = self.schedule.front() {
            if t > now {
                break;
            }
            self.schedule.pop_front();
            if low != self.driving_low {
                self.driving_low = low;
                self.dirty = true;
            }
        }
        self.schedule.front().map(|x| x.0)
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        if let Some(t) = a.get("temperature").and_then(|v| v.as_f64()) {
            self.temperature = t;
        }
        if let Some(h) = a.get("humidity").and_then(|v| v.as_f64()) {
            self.humidity = h;
        }
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"temperature": self.temperature, "humidity": self.humidity, "reads": self.reads})
    }
    fn summary(&self) -> String {
        format!("{:.1} °C, {:.1} %RH (read {} times)", self.temperature, self.humidity, self.reads)
    }
}

// ------------------------------------------------------------------ HC-SR04 ultrasonic sensor
pub struct HcSr04 {
    pub distance_cm: f64,
    trig_rise: Option<u64>,
    last_trig: bool,
    echo_high: bool,
    schedule: VecDeque<(u64, bool)>,
    dirty: bool,
    pub pings: u64,
}
impl HcSr04 {
    pub fn new(p: &Props) -> HcSr04 {
        HcSr04 { distance_cm: prop_f64(p, &["distance"], 100.0), trig_rise: None, last_trig: false, echo_high: false, schedule: VecDeque::new(), dirty: false, pings: 0 }
    }
}
impl Device for HcSr04 {
    fn kind(&self) -> &'static str {
        "hc-sr04"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("TRIG", Input), pin("ECHO", Output), pin("GND", Ground)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::ToRail { a: 2, rail: if self.echo_high { Rail::V3 } else { Rail::Gnd }, ohms: 50.0 });
    }
    fn on_solved(&mut self, now: u64, v: &PinView) {
        let trig = v.level(1).unwrap_or(false);
        if trig && !self.last_trig {
            self.trig_rise = Some(now);
        } else if !trig && self.last_trig {
            if let Some(t) = self.trig_rise.take() {
                if now - t >= 8 * US && self.schedule.is_empty() && !self.echo_high {
                    let start = now + 250 * US;
                    let width = if self.distance_cm > 400.0 { 38_000 * US } else { (self.distance_cm.max(2.0) * 58.3 * US as f64) as u64 };
                    self.schedule.push_back((start, true));
                    self.schedule.push_back((start + width, false));
                    self.pings += 1;
                }
            }
        }
        self.last_trig = trig;
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        while let Some(&(t, hi)) = self.schedule.front() {
            if t > now {
                break;
            }
            self.schedule.pop_front();
            self.echo_high = hi;
            self.dirty = true;
        }
        self.schedule.front().map(|x| x.0)
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        self.distance_cm = a.get("distance").or(a.get("value")).and_then(|v| v.as_f64()).ok_or("expected {\"distance\": cm}")?;
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"distance": self.distance_cm, "pings": self.pings})
    }
    fn summary(&self) -> String {
        format!("{:.1} cm ({} pings)", self.distance_cm, self.pings)
    }
}

// ------------------------------------------------------------------ rotary encoder (KY-040)
pub struct Encoder {
    a: bool,
    b: bool,
    pressed: bool,
    schedule: VecDeque<(u64, bool, bool)>,
    dirty: bool,
    pub position: i64,
}
impl Encoder {
    pub fn new(_p: &Props) -> Encoder {
        Encoder { a: true, b: true, pressed: false, schedule: VecDeque::new(), dirty: false, position: 0 }
    }
}
impl Device for Encoder {
    fn kind(&self) -> &'static str {
        "rotary-encoder"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("CLK", Output), pin("DT", Output), pin("SW", Output), pin("VCC", Power), pin("GND", Ground)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        // module has 10k pull-ups; contacts pull to GND
        for (i, lvl) in [(0usize, self.a), (1usize, self.b)] {
            out.push(Stamp::Resistor { a: i, b: 3, ohms: 10_000.0 });
            if !lvl {
                out.push(Stamp::Resistor { a: i, b: 4, ohms: 1.0 });
            }
        }
        out.push(Stamp::Resistor { a: 2, b: 3, ohms: 10_000.0 });
        if self.pressed {
            out.push(Stamp::Resistor { a: 2, b: 4, ohms: 1.0 });
        }
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        while let Some(&(t, a, b)) = self.schedule.front() {
            if t > now {
                break;
            }
            self.schedule.pop_front();
            self.a = a;
            self.b = b;
            self.dirty = true;
        }
        self.schedule.front().map(|x| x.0)
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn input(&mut self, now: u64, a: &Value) -> Result<(), String> {
        if let Some(p) = a.get("pressed").and_then(|v| v.as_bool()) {
            self.pressed = p;
            return Ok(());
        }
        let steps = a.get("steps").or(a.get("rotate")).and_then(|v| v.as_i64()).ok_or("expected {\"steps\": n} or {\"pressed\": bool}")?;
        let mut t = now.max(self.schedule.back().map(|x| x.0).unwrap_or(0));
        let cw = steps > 0;
        for _ in 0..steps.unsigned_abs() {
            // one detent = full quadrature cycle
            let seq: [(bool, bool); 4] = if cw { [(false, true), (false, false), (true, false), (true, true)] } else { [(true, false), (false, false), (false, true), (true, true)] };
            for (x, y) in seq {
                t += 2_000 * US;
                self.schedule.push_back((t, x, y));
            }
            self.position += if cw { 1 } else { -1 };
        }
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"position": self.position, "pressed": self.pressed})
    }
    fn summary(&self) -> String {
        format!("position {}", self.position)
    }
}

// ------------------------------------------------------------------ IR receiver + remote (NEC)
/// VS1838B-style 38 kHz IR receiver (OUT is low while a burst is received)
/// paired with a virtual 21-key "Car MP3" remote that sends NEC frames.
pub struct IrReceiver {
    out_low: bool,
    schedule: VecDeque<(u64, bool)>,
    dirty: bool,
    pub address: u8,
    pub last_key: Option<String>,
    pub frames: u64,
}

/// NEC command codes of the common 21-key remote shipped with Arduino kits.
pub const IR_KEYS: &[(&str, u8)] = &[
    ("CH-", 0x45), ("CH", 0x46), ("CH+", 0x47), ("PREV", 0x44), ("NEXT", 0x40), ("PLAY", 0x43), ("VOL-", 0x07), ("VOL+", 0x15),
    ("EQ", 0x09), ("0", 0x16), ("100+", 0x19), ("200+", 0x0D), ("1", 0x0C), ("2", 0x18), ("3", 0x5E), ("4", 0x08), ("5", 0x1C),
    ("6", 0x5A), ("7", 0x42), ("8", 0x52), ("9", 0x4A),
];

impl IrReceiver {
    pub fn new(p: &Props) -> IrReceiver {
        IrReceiver { out_low: false, schedule: VecDeque::new(), dirty: false, address: prop_f64(p, &["address"], 0.0) as u8, last_key: None, frames: 0 }
    }
    /// Queue an NEC frame (and `repeats` repeat codes, 108 ms apart) starting at `t`.
    fn send_nec(&mut self, t0: u64, address: u8, command: u8, repeats: u32) {
        let mut t = t0.max(self.schedule.back().map(|x| x.0 + 40_000 * US).unwrap_or(0));
        let start = t;
        let mark = |s: &mut VecDeque<(u64, bool)>, t: &mut u64, mark_us: u64, space_us: u64| {
            s.push_back((*t, true));
            *t += mark_us * US;
            s.push_back((*t, false));
            *t += space_us * US;
        };
        mark(&mut self.schedule, &mut t, 9000, 4500);
        let bytes = [address, !address, command, !command];
        for b in bytes {
            for i in 0..8 {
                let one = b >> i & 1 != 0;
                mark(&mut self.schedule, &mut t, 562, if one { 1687 } else { 562 });
            }
        }
        mark(&mut self.schedule, &mut t, 562, 0);
        for r in 0..repeats {
            let mut t = start + (r as u64 + 1) * 108_000 * US;
            mark(&mut self.schedule, &mut t, 9000, 2250);
            mark(&mut self.schedule, &mut t, 562, 0);
        }
        self.frames += 1;
    }
}

impl Device for IrReceiver {
    fn kind(&self) -> &'static str {
        "ir-receiver"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("OUT", Output), pin("GND", Ground), pin("VCC", Power)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::ToRail { a: 0, rail: if self.out_low { Rail::Gnd } else { Rail::V3 }, ohms: if self.out_low { 50.0 } else { 20_000.0 } });
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        while let Some(&(t, low)) = self.schedule.front() {
            if t > now {
                break;
            }
            self.schedule.pop_front();
            if low != self.out_low {
                self.out_low = low;
                self.dirty = true;
            }
        }
        self.schedule.front().map(|x| x.0)
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn input(&mut self, now: u64, a: &Value) -> Result<(), String> {
        let repeats = a.get("repeats").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let address = a.get("address").and_then(|v| v.as_u64()).map(|x| x as u8).unwrap_or(self.address);
        if let Some(c) = a.get("code").or(a.get("command")).and_then(|v| v.as_u64()) {
            self.last_key = Some(format!("0x{:02X}", c));
            self.send_nec(now, address, c as u8, repeats);
            return Ok(());
        }
        if let Some(k) = a.get("key").and_then(|v| v.as_str()) {
            let code = IR_KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|x| x.1).ok_or_else(|| format!("unknown remote key '{}'", k))?;
            self.last_key = Some(k.to_uppercase());
            self.send_nec(now, address, code, repeats);
            return Ok(());
        }
        Err("ir-receiver accepts {\"key\": \"OK\"|\"1\"|\"VOL+\"...} or {\"code\": 0x45, \"address\": 0}".into())
    }
    fn state(&self) -> Value {
        json!({"last_key": self.last_key, "frames": self.frames, "receiving": !self.schedule.is_empty(), "keys": IR_KEYS.iter().map(|k| k.0).collect::<Vec<_>>()})
    }
    fn summary(&self) -> String {
        match &self.last_key {
            Some(k) => format!("last key {} ({} frames sent)", k, self.frames),
            None => "press a key on the remote".into(),
        }
    }
}

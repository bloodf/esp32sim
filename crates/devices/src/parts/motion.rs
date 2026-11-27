//! Load cell amplifier, power monitor and motor drivers:
//! HX711, INA219, L298N (two DC motors), A4988 and ULN2003 (stepper motors).

use crate::parts::{prop_f64, prop_u8, Props};
use crate::{pin, Device, PinInfo, PinKind::*, PinView, PwmState, Rail, Stamp};
use serde_json::{json, Value};

// ===================================================================== HX711
/// 24-bit load-cell ADC.  DOUT goes low when a conversion is ready; the host
/// clocks 25..27 pulses on PD_SCK (24 data bits MSB first + gain select).
pub struct Hx711 {
    /// grams on the load cell
    pub weight: f64,
    /// raw counts per gram (channel A, gain 128)
    scale: f64,
    /// raw reading with nothing on the cell
    offset: f64,
    noise: f64,
    value: u32,
    bit: u32,
    gain_pulses: u32,
    dout_high: bool,
    ready_at: Option<u64>,
    last_sck: bool,
    sck_rise: u64,
    period_ns: u64,
    powered_down: bool,
    dirty: bool,
    seed: u32,
    pub readings: u64,
}

impl Hx711 {
    pub fn new(p: &Props) -> Hx711 {
        let rate = prop_f64(p, &["rate", "sps"], 10.0).clamp(1.0, 80.0);
        Hx711 {
            weight: prop_f64(p, &["weight"], 0.0),
            scale: prop_f64(p, &["scale"], 420.0),
            offset: prop_f64(p, &["offset"], 8_400.0),
            noise: prop_f64(p, &["noise"], 0.0),
            value: 0,
            bit: 0,
            gain_pulses: 25,
            dout_high: true,
            ready_at: Some((1e9 / rate) as u64),
            last_sck: false,
            sck_rise: 0,
            period_ns: (1e9 / rate) as u64,
            powered_down: false,
            dirty: true,
            seed: 0x1234_5678,
            readings: 0,
        }
    }
    fn sample(&mut self) -> u32 {
        // tiny deterministic noise so averaging code paths get exercised
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        let n = if self.noise > 0.0 { ((self.seed % 2001) as f64 / 1000.0 - 1.0) * self.noise } else { 0.0 };
        let gain_factor = match self.gain_pulses {
            26 => 0.25, // channel B, gain 32
            27 => 0.5,  // channel A, gain 64
            _ => 1.0,
        };
        let raw = ((self.offset + self.weight * self.scale) * gain_factor + n).round() as i64;
        raw.clamp(-0x80_0000, 0x7F_FFFF) as i32 as u32 & 0xFF_FFFF
    }
}

impl Device for Hx711 {
    fn kind(&self) -> &'static str {
        "hx711"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("GND", Ground), pin("DT", Output), pin("SCK", Input), pin("E+", Passive), pin("E-", Passive), pin("A-", Passive), pin("A+", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::ToRail { a: 2, rail: if self.dout_high { Rail::V3 } else { Rail::Gnd }, ohms: 100.0 });
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn on_solved(&mut self, now: u64, v: &PinView) {
        let sck = v.level(3).unwrap_or(false);
        if sck == self.last_sck {
            return;
        }
        self.last_sck = sck;
        if sck {
            self.sck_rise = now;
            if self.powered_down {
                return;
            }
            // a conversion is shifted out only once DOUT signalled "ready"
            if self.ready_at.is_none() || self.bit > 0 {
                self.bit += 1;
                if self.bit <= 24 {
                    let b = self.value >> (24 - self.bit) & 1 != 0;
                    if b != self.dout_high {
                        self.dout_high = b;
                        self.dirty = true;
                    }
                } else if !self.dout_high {
                    // 25th pulse: DOUT back high until the next conversion
                    self.dout_high = true;
                    self.dirty = true;
                }
            }
        } else {
            let high_for = now.saturating_sub(self.sck_rise);
            if high_for > 60_000 {
                // PD_SCK high > 60 us: power down; leaving it resets to gain 128
                self.powered_down = false;
                self.gain_pulses = 25;
                self.bit = 0;
                self.dout_high = true;
                self.dirty = true;
                self.ready_at = Some(now + 400_000_000);
                return;
            }
            if self.bit >= 25 {
                // more pulses may follow to select the gain; settle on the next poll
                self.gain_pulses = self.bit;
                if self.ready_at.is_none() {
                    self.ready_at = Some(now + self.period_ns);
                }
            }
        }
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        if let Some(t) = self.ready_at {
            if now >= t && !(self.bit > 0 && self.bit < 25) {
                self.ready_at = None;
                self.bit = 0;
                self.value = self.sample();
                self.readings += 1;
                self.dout_high = false;
                self.dirty = true;
            }
        }
        self.ready_at
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        if let Some(w) = a.get("weight").and_then(|v| v.as_f64()) {
            self.weight = w;
            return Ok(());
        }
        if a.get("action").and_then(|v| v.as_str()) == Some("tare") {
            self.weight = 0.0;
            return Ok(());
        }
        Err("hx711 accepts {\"weight\": grams}".into())
    }
    fn state(&self) -> Value {
        json!({"weight": self.weight, "raw": (self.offset + self.weight * self.scale).round(), "readings": self.readings})
    }
    fn summary(&self) -> String {
        format!("{:.1} g on the load cell · raw ≈ {:.0}", self.weight, self.offset + self.weight * self.scale)
    }
}

// ===================================================================== INA219
/// Current / power monitor.  A 0.1 Ω shunt sits between VIN+ and VIN-, so the
/// chip measures the real simulated current.  With nothing wired to VIN± the
/// `current_ma` / `bus_voltage` properties are reported instead.
pub struct Ina219 {
    addr: u8,
    shunt: f64,
    ptr: u8,
    got_ptr: bool,
    rbyte: u8,
    wbuf: Vec<u8>,
    config: u16,
    cal: u16,
    /// measured (or configured) values
    pub current_ma: f64,
    pub bus_v: f64,
    manual: bool,
}

impl Ina219 {
    pub fn new(p: &Props) -> Ina219 {
        Ina219 {
            addr: prop_u8(p, &["address", "addr"], 0x40),
            shunt: prop_f64(p, &["shunt", "shunt_ohms"], 0.1),
            ptr: 0,
            got_ptr: false,
            rbyte: 0,
            wbuf: Vec::new(),
            config: 0x399F,
            cal: 0,
            current_ma: prop_f64(p, &["current_ma", "current"], 0.0),
            bus_v: prop_f64(p, &["bus_voltage", "voltage"], 5.0),
            manual: true,
        }
    }
    fn shunt_reg(&self) -> i16 {
        // LSB 10 uV
        (self.current_ma / 1000.0 * self.shunt / 10e-6).round().clamp(-32000.0, 32000.0) as i16
    }
    fn reg(&self, r: u8) -> u16 {
        match r {
            0 => self.config,
            1 => self.shunt_reg() as u16,
            2 => {
                let v = (self.bus_v.max(0.0) / 0.004).round().min(8191.0) as u16;
                (v << 3) | 0x02 // CNVR
            }
            3 => {
                let cur = self.reg(4) as i16 as i64;
                let bus = (self.reg(2) >> 3) as i64;
                ((cur * bus / 5000).clamp(0, 0xFFFF)) as u16
            }
            4 => {
                if self.cal == 0 {
                    return 0;
                }
                ((self.shunt_reg() as i64 * self.cal as i64) / 4096).clamp(-32768, 32767) as i16 as u16
            }
            5 => self.cal,
            _ => 0,
        }
    }
}

impl Device for Ina219 {
    fn kind(&self) -> &'static str {
        "ina219"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("GND", Ground), pin("SCL", Input), pin("SDA", Io), pin("VIN+", Passive), pin("VIN-", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::Resistor { a: 4, b: 5, ohms: self.shunt });
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        if let (Some(a), Some(b)) = (v.v(4), v.v(5)) {
            self.manual = false;
            self.current_ma = (a - b) / self.shunt * 1000.0;
            self.bus_v = b;
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
            self.got_ptr = false;
            self.wbuf.clear();
        }
        true
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        if !self.got_ptr {
            self.ptr = b;
            self.got_ptr = true;
        } else {
            self.wbuf.push(b);
            if self.wbuf.len() == 2 {
                let v = u16::from_be_bytes([self.wbuf[0], self.wbuf[1]]);
                match self.ptr {
                    0 => {
                        if v & 0x8000 != 0 {
                            self.config = 0x399F;
                            self.cal = 0;
                        } else {
                            self.config = v;
                        }
                    }
                    5 => self.cal = v & 0xFFFE,
                    _ => {}
                }
                self.wbuf.clear();
            }
        }
        true
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        let v = self.reg(self.ptr);
        let b = if self.rbyte % 2 == 0 { (v >> 8) as u8 } else { v as u8 };
        self.rbyte += 1;
        b
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        if !self.manual {
            return Err("INA219 measures the circuit current (VIN+ → VIN- is wired)".into());
        }
        if let Some(c) = a.get("current_ma").and_then(|v| v.as_f64()) {
            self.current_ma = c;
        }
        if let Some(v) = a.get("bus_voltage").and_then(|v| v.as_f64()) {
            self.bus_v = v;
        }
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"current_ma": self.current_ma, "bus_voltage": self.bus_v, "power_mw": self.current_ma * self.bus_v, "measured": !self.manual})
    }
    fn summary(&self) -> String {
        format!("{:.2} V · {:.1} mA · {:.1} mW", self.bus_v, self.current_ma, self.current_ma * self.bus_v)
    }
}

// ===================================================================== L298N
/// Dual H-bridge with two DC motors attached (for robot-car style projects).
pub struct L298n {
    /// signed speed -1..1 of motor A / B
    pub speed: [f64; 2],
    /// accumulated revolutions (for the spinning animation)
    pub revs: [f64; 2],
    rpm: f64,
    last_t: u64,
    pwm: [Option<PwmState>; 2],
}

impl L298n {
    pub fn new(p: &Props) -> L298n {
        L298n { speed: [0.0; 2], revs: [0.0; 2], rpm: prop_f64(p, &["rpm", "max_rpm"], 200.0), last_t: 0, pwm: [None; 2] }
    }
    fn integrate(&mut self, now: u64) {
        let dt = now.saturating_sub(self.last_t) as f64 / 1e9;
        self.last_t = now;
        for m in 0..2 {
            self.revs[m] += self.speed[m] * self.rpm / 60.0 * dt;
        }
    }
}

impl Device for L298n {
    fn kind(&self) -> &'static str {
        "l298n"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("ENA", Input), pin("IN1", Input), pin("IN2", Input), pin("IN3", Input), pin("IN4", Input), pin("ENB", Input), pin("12V", Power), pin("GND", Ground), pin("5V", Power)]
    }
    fn on_solved(&mut self, now: u64, v: &PinView) {
        self.integrate(now);
        for m in 0..2 {
            let (en, a, b) = if m == 0 { (0, 1, 2) } else { (5, 3, 4) };
            // EN left unconnected = jumper fitted (always enabled)
            let duty_en = match v.pwm[en] {
                Some(p) => p.duty,
                None => match v.level(en) {
                    Some(true) | None => 1.0,
                    Some(false) => 0.0,
                },
            };
            let lvl = |i: usize| -> f64 {
                match v.pwm[i] {
                    Some(p) => p.duty,
                    None => {
                        if v.level(i) == Some(true) {
                            1.0
                        } else {
                            0.0
                        }
                    }
                }
            };
            self.pwm[m] = v.pwm[en];
            self.speed[m] = (lvl(a) - lvl(b)) * duty_en;
        }
    }
    fn state(&self) -> Value {
        json!({"speed": self.speed, "revs": self.revs, "rpm": [self.speed[0] * self.rpm, self.speed[1] * self.rpm]})
    }
    fn summary(&self) -> String {
        let d = |s: f64| {
            if s.abs() < 0.01 {
                "stopped".to_string()
            } else {
                format!("{} {:.0}%", if s > 0.0 { "fwd" } else { "rev" }, s.abs() * 100.0)
            }
        };
        format!("motor A {} · motor B {}", d(self.speed[0]), d(self.speed[1]))
    }
}

// ===================================================================== steppers
/// A4988 / DRV8825 style STEP/DIR driver with a NEMA17 motor.
pub struct A4988 {
    pub position: i64,
    steps_per_rev: f64,
    last_step: bool,
    pub steps: u64,
}

impl A4988 {
    pub fn new(p: &Props) -> A4988 {
        let micro = prop_f64(p, &["microsteps"], 1.0).max(1.0);
        A4988 { position: 0, steps_per_rev: prop_f64(p, &["steps_per_rev"], 200.0) * micro, last_step: false, steps: 0 }
    }
}

impl Device for A4988 {
    fn kind(&self) -> &'static str {
        "a4988"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("EN", Input), pin("STEP", Input), pin("DIR", Input), pin("SLEEP", Input), pin("RESET", Input), pin("VDD", Power), pin("GND", Ground)]
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        let step = v.level(1).unwrap_or(false);
        let enabled = v.level(0) != Some(true); // EN is active low (pulled down on most boards)
        let awake = v.level(3) != Some(false) && v.level(4) != Some(false);
        if step && !self.last_step && enabled && awake {
            self.position += if v.level(2) == Some(true) { 1 } else { -1 };
            self.steps += 1;
        }
        self.last_step = step;
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        if a.get("action").and_then(|v| v.as_str()) == Some("zero") {
            self.position = 0;
            return Ok(());
        }
        Err("a4988 accepts {\"action\":\"zero\"}".into())
    }
    fn state(&self) -> Value {
        json!({"position": self.position, "angle": self.position as f64 * 360.0 / self.steps_per_rev, "steps": self.steps})
    }
    fn summary(&self) -> String {
        format!("position {} steps ({:.1}°)", self.position, self.position as f64 * 360.0 / self.steps_per_rev)
    }
}

/// 28BYJ-48 geared stepper on a ULN2003 board (IN1..IN4 drive the coils).
pub struct Uln2003 {
    pub position: i64,
    /// last coil pattern index in the 8-entry half-step table
    phase: Option<usize>,
    coils: u8,
    steps_per_rev: f64,
}

/// Half-step sequence (IN1..IN4 bit patterns); full-step and wave drive are subsets.
const HALF_STEPS: [u8; 8] = [0b0001, 0b0011, 0b0010, 0b0110, 0b0100, 0b1100, 0b1000, 0b1001];

impl Uln2003 {
    pub fn new(p: &Props) -> Uln2003 {
        Uln2003 { position: 0, phase: None, coils: 0, steps_per_rev: prop_f64(p, &["steps_per_rev"], 4096.0) }
    }
}

impl Device for Uln2003 {
    fn kind(&self) -> &'static str {
        "uln2003"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("IN1", Input), pin("IN2", Input), pin("IN3", Input), pin("IN4", Input), pin("VCC", Power), pin("GND", Ground)]
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        let mut c = 0u8;
        for i in 0..4 {
            if v.level(i) == Some(true) {
                c |= 1 << i;
            }
        }
        if c == self.coils {
            return;
        }
        self.coils = c;
        if let Some(idx) = HALF_STEPS.iter().position(|&p| p == c) {
            if let Some(prev) = self.phase {
                // distance in half steps around the 8-entry cycle (-3..=4)
                let mut d = idx as i64 - prev as i64;
                if d > 4 {
                    d -= 8;
                } else if d < -3 {
                    d += 8;
                }
                self.position += d;
            }
            self.phase = Some(idx);
        }
    }
    fn state(&self) -> Value {
        json!({"position": self.position, "angle": self.position as f64 * 360.0 / self.steps_per_rev, "coils": self.coils})
    }
    fn summary(&self) -> String {
        format!("position {} half-steps ({:.1}°) · coils {:04b}", self.position, self.position as f64 * 360.0 / self.steps_per_rev, self.coils)
    }
}

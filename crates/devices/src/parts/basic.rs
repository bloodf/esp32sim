//! Passive parts, switches, LEDs and simple actuators.

use crate::parts::{prop_f64, prop_str, Props};
use crate::{pin, Device, PinInfo, PinKind::*, PinView, PwmState, Rail, Stamp};
use serde_json::{json, Value};

fn action_str(a: &Value) -> String {
    match a {
        Value::String(s) => s.to_ascii_lowercase(),
        Value::Object(m) => m.get("action").and_then(|v| v.as_str()).unwrap_or("").to_ascii_lowercase(),
        _ => String::new(),
    }
}

// ------------------------------------------------------------------ resistor
pub struct Resistor {
    pub ohms: f64,
}
impl Resistor {
    pub fn new(p: &Props) -> Resistor {
        Resistor { ohms: prop_f64(p, &["value", "resistance", "ohms"], 1000.0) }
    }
}
impl Device for Resistor {
    fn kind(&self) -> &'static str {
        "resistor"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("1", Passive), pin("2", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::Resistor { a: 0, b: 1, ohms: self.ohms });
    }
    fn state(&self) -> Value {
        json!({"ohms": self.ohms})
    }
    fn summary(&self) -> String {
        format!("{} Ω", fmt_si(self.ohms))
    }
}

pub fn fmt_si(v: f64) -> String {
    if v >= 1e6 {
        format!("{}M", trim(v / 1e6))
    } else if v >= 1e3 {
        format!("{}k", trim(v / 1e3))
    } else {
        trim(v)
    }
}
fn trim(v: f64) -> String {
    let s = format!("{:.2}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ------------------------------------------------------------------ capacitor (charges through the circuit)
pub struct Capacitor {
    pub farads: f64,
    v: f64,
}
impl Capacitor {
    pub fn new(p: &Props) -> Capacitor {
        Capacitor { farads: prop_f64(p, &["value", "capacitance"], 100e-9), v: 0.0 }
    }
}
impl Device for Capacitor {
    fn kind(&self) -> &'static str {
        "capacitor"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("1", Passive), pin("2", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::Capacitor { a: 0, b: 1, farads: self.farads, tag: 0 });
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        self.v = v.v(0).unwrap_or(0.0) - v.v(1).unwrap_or(0.0);
    }
    fn state(&self) -> Value {
        json!({"farads": self.farads, "volts": self.v})
    }
    fn summary(&self) -> String {
        let f = self.farads;
        let cap = if f >= 1e-6 { format!("{}µF", trim(f * 1e6)) } else if f >= 1e-9 { format!("{}nF", trim(f * 1e9)) } else { format!("{}pF", trim(f * 1e12)) };
        format!("{} · {:.2} V", cap, self.v)
    }
}

// ------------------------------------------------------------------ potentiometer
pub struct Potentiometer {
    pub ohms: f64,
    pub pos: f64,
}
impl Potentiometer {
    pub fn new(p: &Props) -> Potentiometer {
        Potentiometer { ohms: prop_f64(p, &["value", "resistance"], 10_000.0), pos: prop_f64(p, &["position", "initial"], 0.5).clamp(0.0, 1.0) }
    }
}
impl Device for Potentiometer {
    fn kind(&self) -> &'static str {
        "potentiometer"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("1", Passive), pin("W", Passive), pin("2", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::Resistor { a: 0, b: 1, ohms: (self.ohms * self.pos).max(1.0) });
        out.push(Stamp::Resistor { a: 1, b: 2, ohms: (self.ohms * (1.0 - self.pos)).max(1.0) });
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        let v = a.get("value").or(a.get("position")).and_then(|x| x.as_f64()).or(a.as_f64()).ok_or("expected {\"value\": 0..1}")?;
        self.pos = v.clamp(0.0, 1.0);
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"position": self.pos, "ohms": self.ohms})
    }
    fn summary(&self) -> String {
        format!("{:.0}%", self.pos * 100.0)
    }
}

// ------------------------------------------------------------------ LED
pub struct Led {
    pub color: String,
    vf: f64,
    pub brightness: f64,
    pub current: f64,
    pub burned: bool,
    max_ma: f64,
}
pub fn led_vf(color: &str) -> f64 {
    match color {
        "red" => 1.8,
        "orange" | "amber" => 2.0,
        "yellow" => 2.1,
        "green" => 2.2,
        "blue" | "white" | "purple" | "violet" | "pink" => 3.0,
        "ir" | "infrared" => 1.2,
        _ => 2.0,
    }
}
impl Led {
    pub fn new(p: &Props) -> Led {
        let color = prop_str(p, &["color", "colour"], "red").to_ascii_lowercase();
        let vf = prop_f64(p, &["vf", "forward_voltage"], led_vf(&color));
        Led { color, vf, brightness: 0.0, current: 0.0, burned: false, max_ma: prop_f64(p, &["max_current_ma"], 30.0) }
    }
}
impl Device for Led {
    fn kind(&self) -> &'static str {
        "led"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("A", Passive), pin("C", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        if !self.burned {
            out.push(Stamp::Diode { a: 0, k: 1, vf: self.vf, ron: 10.0, tag: 0 });
        }
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        let i = (v.diode_current)(0);
        let duty = v.pwm[0].or(v.pwm[1]).map(|p| if v.pwm[0].is_some() { p.duty } else { 1.0 - p.duty }).unwrap_or(1.0);
        self.current = i * duty;
        if i * 1000.0 > self.max_ma * 3.0 && !self.burned {
            self.burned = true;
            self.current = 0.0;
        }
        self.brightness = (self.current / 0.015).clamp(0.0, 1.0).powf(0.6);
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        if action_str(a) == "repair" || action_str(a) == "reset" {
            self.burned = false;
            return Ok(());
        }
        Err("led accepts {\"action\":\"repair\"}".into())
    }
    fn state(&self) -> Value {
        json!({"color": self.color, "brightness": self.brightness, "current_ma": self.current * 1000.0, "burned": self.burned})
    }
    fn summary(&self) -> String {
        if self.burned {
            "BURNED OUT (too much current - add a resistor)".into()
        } else if self.brightness > 0.01 {
            format!("ON {:.0}% ({:.1} mA)", self.brightness * 100.0, self.current * 1000.0)
        } else {
            "off".into()
        }
    }
}

// ------------------------------------------------------------------ RGB LED
pub struct RgbLed {
    anode: bool,
    pub rgb: [f64; 3],
}
impl RgbLed {
    pub fn new(p: &Props) -> RgbLed {
        RgbLed { anode: prop_str(p, &["common"], "cathode").starts_with('a'), rgb: [0.0; 3] }
    }
}
impl Device for RgbLed {
    fn kind(&self) -> &'static str {
        "rgb-led"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("R", Passive), pin("COM", Passive), pin("G", Passive), pin("B", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        let vfs = [1.8, 2.2, 3.0];
        for (i, p) in [0usize, 2, 3].iter().enumerate() {
            if self.anode {
                out.push(Stamp::Diode { a: 1, k: *p, vf: vfs[i], ron: 10.0, tag: i as u32 });
            } else {
                out.push(Stamp::Diode { a: *p, k: 1, vf: vfs[i], ron: 10.0, tag: i as u32 });
            }
        }
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        for (i, p) in [0usize, 2, 3].iter().enumerate() {
            let cur = (v.diode_current)(i as u32);
            let duty = v.pwm[*p].map(|w| if self.anode { 1.0 - w.duty } else { w.duty }).unwrap_or(1.0);
            self.rgb[i] = ((cur * duty) / 0.015).clamp(0.0, 1.0).powf(0.6);
        }
    }
    fn state(&self) -> Value {
        json!({"r": self.rgb[0], "g": self.rgb[1], "b": self.rgb[2]})
    }
    fn summary(&self) -> String {
        format!("R{:.0}% G{:.0}% B{:.0}%", self.rgb[0] * 100.0, self.rgb[1] * 100.0, self.rgb[2] * 100.0)
    }
}

// ------------------------------------------------------------------ push button / switches
pub struct PushButton {
    pub pressed: bool,
    release_at: Option<u64>,
    dirty: bool,
    /// bounce simulation: number of extra transitions on press
    bounce: u32,
    bounce_left: u32,
    bounce_at: u64,
    pub presses: u64,
}
impl PushButton {
    pub fn new(p: &Props) -> PushButton {
        PushButton {
            pressed: false,
            release_at: None,
            dirty: false,
            bounce: prop_f64(p, &["bounce"], 0.0) as u32,
            bounce_left: 0,
            bounce_at: 0,
            presses: 0,
        }
    }
}
impl Device for PushButton {
    fn kind(&self) -> &'static str {
        "pushbutton"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("1", Passive), pin("2", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        if self.pressed {
            out.push(Stamp::Resistor { a: 0, b: 1, ohms: 0.05 });
        }
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        if self.bounce_left > 0 && now >= self.bounce_at {
            self.pressed = !self.pressed;
            self.bounce_left -= 1;
            self.bounce_at = now + 300_000;
            self.dirty = true;
        }
        if let Some(t) = self.release_at {
            if now >= t && self.bounce_left == 0 {
                self.pressed = false;
                self.release_at = None;
                self.dirty = true;
            }
        }
        let mut next = self.release_at;
        if self.bounce_left > 0 {
            next = Some(next.map_or(self.bounce_at, |n| n.min(self.bounce_at)));
        }
        next
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn input(&mut self, now: u64, a: &Value) -> Result<(), String> {
        let act = action_str(a);
        let pressed = match act.as_str() {
            "press" | "down" | "on" => true,
            "release" | "up" | "off" => false,
            "toggle" => !self.pressed,
            "click" | "tap" => {
                let ms = a.get("ms").and_then(|v| v.as_u64()).unwrap_or(150);
                self.release_at = Some(now + ms * 1_000_000);
                true
            }
            _ => match a.get("pressed").and_then(|v| v.as_bool()) {
                Some(b) => b,
                None => return Err("pushbutton actions: press, release, toggle, click".into()),
            },
        };
        if pressed && !self.pressed {
            self.presses += 1;
            if self.bounce > 0 {
                self.bounce_left = self.bounce * 2;
                self.bounce_at = now + 300_000;
            }
        }
        self.pressed = pressed;
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"pressed": self.pressed, "presses": self.presses})
    }
    fn summary(&self) -> String {
        if self.pressed { "pressed".into() } else { "released".into() }
    }
}

pub struct Switch {
    pub on: bool,
    spdt: bool,
}
impl Switch {
    pub fn new(p: &Props, spdt: bool) -> Switch {
        Switch { on: p.get("on").and_then(|v| v.as_bool()).unwrap_or(false), spdt }
    }
}
impl Device for Switch {
    fn kind(&self) -> &'static str {
        if self.spdt {
            "slide-switch"
        } else {
            "switch"
        }
    }
    fn pins(&self) -> Vec<PinInfo> {
        if self.spdt {
            vec![pin("1", Passive), pin("C", Passive), pin("2", Passive)]
        } else {
            vec![pin("1", Passive), pin("2", Passive)]
        }
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        if self.spdt {
            out.push(Stamp::Resistor { a: 1, b: if self.on { 2 } else { 0 }, ohms: 0.05 });
        } else if self.on {
            out.push(Stamp::Resistor { a: 0, b: 1, ohms: 0.05 });
        }
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        match action_str(a).as_str() {
            "on" | "close" => self.on = true,
            "off" | "open" => self.on = false,
            "toggle" | "click" => self.on = !self.on,
            _ => match a.get("on").and_then(|v| v.as_bool()) {
                Some(b) => self.on = b,
                None => return Err("switch actions: on, off, toggle".into()),
            },
        }
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"on": self.on})
    }
    fn summary(&self) -> String {
        if self.on { "on".into() } else { "off".into() }
    }
}

// ------------------------------------------------------------------ variable resistors (sensors)
pub struct Ldr {
    pub lux: f64,
}
impl Ldr {
    pub fn new(p: &Props) -> Ldr {
        Ldr { lux: prop_f64(p, &["lux", "light"], 300.0) }
    }
    fn ohms(&self) -> f64 {
        // typical GL5528: ~10k @ 10 lux, gamma 0.7
        (10_000.0 * (self.lux.max(0.01) / 10.0).powf(-0.7)).clamp(50.0, 2e6)
    }
}
impl Device for Ldr {
    fn kind(&self) -> &'static str {
        "ldr"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("1", Passive), pin("2", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::Resistor { a: 0, b: 1, ohms: self.ohms() });
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        self.lux = a.get("lux").or(a.get("value")).and_then(|v| v.as_f64()).ok_or("expected {\"lux\": n}")?;
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"lux": self.lux, "ohms": self.ohms()})
    }
    fn summary(&self) -> String {
        format!("{:.0} lux ({}Ω)", self.lux, fmt_si(self.ohms()))
    }
}

pub struct Ntc {
    pub celsius: f64,
    r25: f64,
    beta: f64,
}
impl Ntc {
    pub fn new(p: &Props) -> Ntc {
        Ntc { celsius: prop_f64(p, &["temperature"], 25.0), r25: prop_f64(p, &["value", "r25"], 10_000.0), beta: prop_f64(p, &["beta"], 3950.0) }
    }
    fn ohms(&self) -> f64 {
        let t = self.celsius + 273.15;
        self.r25 * (self.beta * (1.0 / t - 1.0 / 298.15)).exp()
    }
}
impl Device for Ntc {
    fn kind(&self) -> &'static str {
        "ntc"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("1", Passive), pin("2", Passive)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::Resistor { a: 0, b: 1, ohms: self.ohms() });
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        self.celsius = a.get("temperature").or(a.get("value")).and_then(|v| v.as_f64()).ok_or("expected {\"temperature\": °C}")?;
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"temperature": self.celsius, "ohms": self.ohms()})
    }
    fn summary(&self) -> String {
        format!("{:.1} °C", self.celsius)
    }
}

// ------------------------------------------------------------------ buzzer
pub struct Buzzer {
    active: bool,
    pub on: bool,
    pub freq: f64,
    pwm: Option<PwmState>,
}
impl Buzzer {
    pub fn new(p: &Props, active: bool) -> Buzzer {
        Buzzer { active: prop_str(p, &["mode"], if active { "active" } else { "passive" }) == "active", on: false, freq: 0.0, pwm: None }
    }
}
impl Device for Buzzer {
    fn kind(&self) -> &'static str {
        if self.active {
            "buzzer-active"
        } else {
            "buzzer"
        }
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("+", Input), pin("-", Ground)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::Resistor { a: 0, b: 1, ohms: 100.0 });
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        let dv = v.v(0).unwrap_or(0.0) - v.v(1).unwrap_or(0.0);
        match (v.pwm[0], self.active) {
            (Some(p), _) if p.running && p.duty > 0.01 && p.duty < 0.99 => {
                self.on = true;
                self.freq = if self.active { 2700.0 } else { p.freq_hz };
            }
            _ => {
                self.on = dv > 1.5 && self.active;
                self.freq = if self.on { 2700.0 } else { 0.0 };
            }
        }
    }
    fn pwm(&mut self, _now: u64, _pin: usize, pwm: PwmState) {
        self.pwm = if pwm.running { Some(pwm) } else { None };
    }
    fn state(&self) -> Value {
        json!({"on": self.on, "freq": self.freq, "active": self.active})
    }
    fn summary(&self) -> String {
        if self.on { format!("beeping {:.0} Hz", self.freq) } else { "silent".into() }
    }
}

// ------------------------------------------------------------------ relay module (IN/VCC/GND + COM/NO/NC)
pub struct RelayModule {
    active_low: bool,
    pub energized: bool,
    channels: usize,
    states: Vec<bool>,
    dirty: bool,
}
impl RelayModule {
    pub fn new(p: &Props) -> RelayModule {
        let ch = prop_f64(p, &["channels"], 1.0).clamp(1.0, 8.0) as usize;
        RelayModule { active_low: p.get("active_low").and_then(|v| v.as_bool()).unwrap_or(false), energized: false, channels: ch, states: vec![false; ch], dirty: false }
    }
}
const RELAY_PINS: [[&str; 4]; 8] = [
    ["IN1", "COM1", "NO1", "NC1"],
    ["IN2", "COM2", "NO2", "NC2"],
    ["IN3", "COM3", "NO3", "NC3"],
    ["IN4", "COM4", "NO4", "NC4"],
    ["IN5", "COM5", "NO5", "NC5"],
    ["IN6", "COM6", "NO6", "NC6"],
    ["IN7", "COM7", "NO7", "NC7"],
    ["IN8", "COM8", "NO8", "NC8"],
];
impl Device for RelayModule {
    fn kind(&self) -> &'static str {
        "relay"
    }
    fn pins(&self) -> Vec<PinInfo> {
        let mut v = vec![pin("VCC", Power), pin("GND", Ground)];
        if self.channels == 1 {
            v.extend([pin("IN", Input), pin("COM", Passive), pin("NO", Passive), pin("NC", Passive)]);
        } else {
            for c in 0..self.channels {
                let n = RELAY_PINS[c];
                v.extend([pin(n[0], Input), pin(n[1], Passive), pin(n[2], Passive), pin(n[3], Passive)]);
            }
        }
        v
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        for c in 0..self.channels {
            let base = 2 + c * 4;
            // optocoupler input: ~10k to the supply keeps IN defined like real modules
            if self.active_low {
                out.push(Stamp::ToRail { a: base, rail: Rail::V3, ohms: 10_000.0 });
            } else {
                out.push(Stamp::ToRail { a: base, rail: Rail::Gnd, ohms: 10_000.0 });
            }
            let to = if self.states[c] { base + 2 } else { base + 3 };
            out.push(Stamp::Resistor { a: base + 1, b: to, ohms: 0.05 });
        }
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        for c in 0..self.channels {
            let lv = v.level(2 + c * 4).unwrap_or(self.active_low);
            let on = lv != self.active_low;
            if on != self.states[c] {
                self.states[c] = on;
                self.dirty = true;
            }
        }
        self.energized = self.states[0];
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn state(&self) -> Value {
        json!({"on": self.states, "active_low": self.active_low})
    }
    fn summary(&self) -> String {
        self.states.iter().map(|s| if *s { "ON" } else { "off" }).collect::<Vec<_>>().join(" ")
    }
}

// ------------------------------------------------------------------ servo
pub struct Servo {
    pub angle: f64,
    pub pulse_us: f64,
    min_us: f64,
    max_us: f64,
}
impl Servo {
    pub fn new(p: &Props) -> Servo {
        Servo { angle: 90.0, pulse_us: 0.0, min_us: prop_f64(p, &["min_us"], 500.0), max_us: prop_f64(p, &["max_us"], 2500.0) }
    }
}
impl Device for Servo {
    fn kind(&self) -> &'static str {
        "servo"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("PWM", Input), pin("V+", Power), pin("GND", Ground)]
    }
    fn pwm(&mut self, _now: u64, pin: usize, p: PwmState) {
        if pin == 0 && p.running && p.freq_hz > 0.0 {
            self.pulse_us = p.duty / p.freq_hz * 1e6;
            if self.pulse_us > 300.0 && self.pulse_us < 3000.0 {
                self.angle = ((self.pulse_us - self.min_us) / (self.max_us - self.min_us) * 180.0).clamp(0.0, 180.0);
            }
        }
    }
    fn state(&self) -> Value {
        json!({"angle": self.angle, "pulse_us": self.pulse_us})
    }
    fn summary(&self) -> String {
        format!("{:.0}° ({:.0} µs)", self.angle, self.pulse_us)
    }
}

// ------------------------------------------------------------------ joystick (2 pots + button)
pub struct Joystick {
    x: f64,
    y: f64,
    pressed: bool,
}
impl Joystick {
    pub fn new(_p: &Props) -> Joystick {
        Joystick { x: 0.5, y: 0.5, pressed: false }
    }
}
impl Device for Joystick {
    fn kind(&self) -> &'static str {
        "joystick"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("VERT", Analog), pin("HORZ", Analog), pin("SEL", Output), pin("GND", Ground)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        for (pinx, pos) in [(1usize, self.y), (2usize, self.x)] {
            out.push(Stamp::Resistor { a: 0, b: pinx, ohms: (10_000.0 * (1.0 - pos)).max(1.0) });
            out.push(Stamp::Resistor { a: pinx, b: 4, ohms: (10_000.0 * pos).max(1.0) });
        }
        if self.pressed {
            out.push(Stamp::Resistor { a: 3, b: 4, ohms: 0.05 });
        }
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        if let Some(x) = a.get("x").and_then(|v| v.as_f64()) {
            self.x = x.clamp(0.0, 1.0);
        }
        if let Some(y) = a.get("y").and_then(|v| v.as_f64()) {
            self.y = y.clamp(0.0, 1.0);
        }
        if let Some(p) = a.get("pressed").and_then(|v| v.as_bool()) {
            self.pressed = p;
        }
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"x": self.x, "y": self.y, "pressed": self.pressed})
    }
    fn summary(&self) -> String {
        format!("x={:.2} y={:.2}{}", self.x, self.y, if self.pressed { " [pressed]" } else { "" })
    }
}

// ------------------------------------------------------------------ digital output sensors (PIR, tilt, IR obstacle...)
pub struct DigitalSensor {
    kind: &'static str,
    pub active: bool,
    active_high: bool,
    hold_until: Option<u64>,
    hold_ms: u64,
    dirty: bool,
}
impl DigitalSensor {
    pub fn new(p: &Props, kind: &'static str, active_high: bool, hold_ms: u64) -> DigitalSensor {
        DigitalSensor {
            kind,
            active: false,
            active_high: p.get("active_high").and_then(|v| v.as_bool()).unwrap_or(active_high),
            hold_until: None,
            hold_ms: prop_f64(p, &["hold_ms"], hold_ms as f64) as u64,
            dirty: false,
        }
    }
}
impl Device for DigitalSensor {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("OUT", Output), pin("GND", Ground)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        let high = self.active == self.active_high;
        out.push(Stamp::ToRail { a: 1, rail: if high { Rail::V3 } else { Rail::Gnd }, ohms: 100.0 });
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        if let Some(t) = self.hold_until {
            if now >= t {
                self.active = false;
                self.hold_until = None;
                self.dirty = true;
            }
        }
        self.hold_until
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn input(&mut self, now: u64, a: &Value) -> Result<(), String> {
        match action_str(a).as_str() {
            "trigger" | "detect" | "motion" => {
                self.active = true;
                self.hold_until = Some(now + self.hold_ms * 1_000_000);
            }
            "on" | "activate" => self.active = true,
            "off" | "clear" => self.active = false,
            "toggle" => self.active = !self.active,
            _ => match a.get("active").and_then(|v| v.as_bool()) {
                Some(b) => self.active = b,
                None => return Err("actions: trigger, on, off, toggle".into()),
            },
        }
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"active": self.active})
    }
    fn summary(&self) -> String {
        if self.active { "ACTIVE".into() } else { "idle".into() }
    }
}

/// A logic/analog signal source the user can set (for testing inputs).
pub struct SignalSource {
    pub volts: f64,
}
impl SignalSource {
    pub fn new(p: &Props) -> SignalSource {
        SignalSource { volts: prop_f64(p, &["volts", "value"], 0.0) }
    }
}
impl Device for SignalSource {
    fn kind(&self) -> &'static str {
        "signal"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("OUT", Output), pin("GND", Ground)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::Source { a: 0, volts: self.volts, ohms: 10.0 });
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        if let Some(v) = a.get("volts").or(a.get("value")).and_then(|v| v.as_f64()) {
            self.volts = v;
            return Ok(());
        }
        match action_str(a).as_str() {
            "high" | "on" => self.volts = 3.3,
            "low" | "off" => self.volts = 0.0,
            "toggle" => self.volts = if self.volts > 1.6 { 0.0 } else { 3.3 },
            _ => return Err("signal: {\"volts\": v} or high/low/toggle".into()),
        }
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"volts": self.volts})
    }
    fn summary(&self) -> String {
        format!("{:.2} V", self.volts)
    }
}

// ------------------------------------------------------------------ comparator modules (LM393 boards)
/// The common "sensor + LM393 comparator" breakout: AO is the raw analog value,
/// DO flips when it crosses the threshold set by the on-board trimmer.
/// Presets: soil moisture, MQ gas, sound (KY-038), flame, rain, water level.
pub struct AnalogModule {
    kind: &'static str,
    label: &'static str,
    /// what the user controls, 0..1 (moisture, gas level, loudness, ...)
    pub level: f64,
    /// trimmer position, 0..1 (DO switches when `level` exceeds it)
    pub threshold: f64,
    /// AO falls as the level rises (moisture, flame, rain modules)
    inverted: bool,
    /// DO is pulled low when triggered (most LM393 boards)
    do_active_low: bool,
    has_do: bool,
    vcc: f64,
    dirty: bool,
}
impl AnalogModule {
    pub fn new(p: &Props, kind: &'static str) -> AnalogModule {
        let (label, inverted, do_active_low, has_do, level) = match kind {
            "soil-moisture" => ("moisture", true, true, true, 0.3),
            "mq2" | "gas-sensor" => ("gas", false, true, true, 0.1),
            "sound-sensor" => ("sound", false, false, true, 0.1),
            "flame-sensor" => ("flame", true, true, true, 0.0),
            "rain-sensor" => ("rain", true, true, true, 0.0),
            _ => ("level", false, true, false, 0.2), // water-level
        };
        AnalogModule {
            kind,
            label,
            level: prop_f64(p, &["level", "value", label], level).clamp(0.0, 1.0),
            threshold: prop_f64(p, &["threshold"], 0.5).clamp(0.0, 1.0),
            inverted,
            do_active_low,
            has_do,
            vcc: 3.3,
            dirty: false,
        }
    }
    fn triggered(&self) -> bool {
        self.level > self.threshold
    }
}
impl Device for AnalogModule {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn pins(&self) -> Vec<PinInfo> {
        if self.has_do {
            vec![pin("VCC", Power), pin("GND", Ground), pin("DO", Output), pin("AO", Analog)]
        } else {
            vec![pin("VCC", Power), pin("GND", Ground), pin("S", Analog)]
        }
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        let frac = if self.inverted { 1.0 - self.level } else { self.level };
        // water level sensors only reach ~80 % of VCC when fully submerged
        let frac = if self.has_do { frac } else { frac * 0.8 };
        let ao = if self.has_do { 3 } else { 2 };
        out.push(Stamp::Source { a: ao, volts: frac * self.vcc, ohms: 1000.0 });
        if self.has_do {
            let low = self.triggered() == self.do_active_low;
            // LM393 output: open collector with a 10k pull-up to VCC on the board
            if low {
                out.push(Stamp::Source { a: 2, volts: 0.0, ohms: 100.0 });
            } else {
                out.push(Stamp::Source { a: 2, volts: self.vcc, ohms: 10_000.0 });
            }
        }
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        // outputs follow the module supply (5 V on VCC gives a 5 V AO: mind the ESP32 pin!)
        let vcc = v.v(0).unwrap_or(0.0);
        if (vcc - self.vcc).abs() > 0.05 {
            self.vcc = vcc;
            self.dirty = true;
        }
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        let mut ok = false;
        for k in ["level", "value", self.label] {
            if let Some(x) = a.get(k).and_then(|v| v.as_f64()) {
                self.level = x.clamp(0.0, 1.0);
                ok = true;
            }
        }
        if let Some(x) = a.get("threshold").and_then(|v| v.as_f64()) {
            self.threshold = x.clamp(0.0, 1.0);
            ok = true;
        }
        if ok {
            Ok(())
        } else {
            Err(format!("{} accepts {{\"{}\": 0..1, \"threshold\": 0..1}}", self.kind, self.label))
        }
    }
    fn state(&self) -> Value {
        json!({"level": self.level, "threshold": self.threshold, "triggered": self.has_do && self.triggered(), "label": self.label})
    }
    fn summary(&self) -> String {
        let mut s = format!("{} {:.0}%", self.label, self.level * 100.0);
        if self.has_do {
            s += if self.triggered() { " · DO triggered" } else { " · DO idle" };
        }
        s
    }
}

// ------------------------------------------------------------------ capacitive touch electrode
/// A bare metal pad / wire for the ESP32's capacitive touch pins (T0..T9).
pub struct TouchPad {
    pub touched: bool,
}
impl TouchPad {
    pub fn new(_p: &Props) -> TouchPad {
        TouchPad { touched: false }
    }
}
impl Device for TouchPad {
    fn kind(&self) -> &'static str {
        "touch-pad"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("PAD", Passive)]
    }
    fn touched(&self, _pin: usize) -> bool {
        self.touched
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        match action_str(a).as_str() {
            "press" | "touch" => self.touched = true,
            "release" | "untouch" => self.touched = false,
            "toggle" => self.touched = !self.touched,
            _ => return Err("touch-pad accepts {\"action\": \"touch\"|\"release\"|\"toggle\"}".into()),
        }
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"touched": self.touched})
    }
    fn summary(&self) -> String {
        if self.touched { "finger on the pad".into() } else { "not touched".into() }
    }
}

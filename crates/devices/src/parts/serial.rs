//! UART modules: NEO-6M GPS (NMEA output) and HC-05 Bluetooth serial bridge.

use crate::parts::{prop_f64, prop_str, Props};
use crate::{pin, Device, PinInfo, PinKind::*, Rail, Stamp};
use serde_json::{json, Value};
use std::collections::VecDeque;

const SEC: u64 = 1_000_000_000;

// ===================================================================== GPS
/// u-blox NEO-6M style GPS: NMEA 0183 sentences once per second on TX.
pub struct Gps {
    pub lat: f64,
    pub lon: f64,
    pub alt: f64,
    pub speed_kmh: f64,
    pub course: f64,
    pub sats: u32,
    pub fix: bool,
    out: VecDeque<u8>,
    next_fix: u64,
    period: u64,
    /// seconds since 00:00:00 UTC of the fixed base date (2024-06-01)
    base_secs: u64,
    pub sentences: u64,
}

fn nmea(body: &str) -> String {
    let cs = body.bytes().fold(0u8, |a, b| a ^ b);
    format!("${}*{:02X}\r\n", body, cs)
}

fn nmea_coord(v: f64, lat: bool) -> (String, char) {
    let hemi = match (lat, v >= 0.0) {
        (true, true) => 'N',
        (true, false) => 'S',
        (false, true) => 'E',
        (false, false) => 'W',
    };
    let a = v.abs();
    let deg = a.trunc();
    let min = (a - deg) * 60.0;
    (if lat { format!("{:02}{:08.5}", deg as u32, min) } else { format!("{:03}{:08.5}", deg as u32, min) }, hemi)
}

impl Gps {
    pub fn new(p: &Props) -> Gps {
        let rate = prop_f64(p, &["rate_hz", "rate"], 1.0).clamp(0.2, 10.0);
        let (h, m, s) = (12u64, 0u64, 0u64);
        Gps {
            lat: prop_f64(p, &["lat", "latitude"], 40.4168),
            lon: prop_f64(p, &["lon", "lng", "longitude"], -3.7038),
            alt: prop_f64(p, &["alt", "altitude"], 650.0),
            speed_kmh: prop_f64(p, &["speed", "speed_kmh"], 0.0),
            course: prop_f64(p, &["course"], 0.0),
            sats: prop_f64(p, &["sats", "satellites"], 8.0) as u32,
            fix: p.get("fix").and_then(|v| v.as_bool()).unwrap_or(true),
            out: VecDeque::new(),
            next_fix: (SEC as f64 / rate) as u64,
            period: (SEC as f64 / rate) as u64,
            base_secs: h * 3600 + m * 60 + s,
            sentences: 0,
        }
    }

    fn emit(&mut self, now: u64) {
        let t = self.base_secs + now / SEC;
        let cs = (now % SEC) / 10_000_000; // centiseconds
        let (hh, mm, ss) = ((t / 3600) % 24, (t / 60) % 60, t % 60);
        let day = 1 + (t / 86400) as u32;
        let time = format!("{:02}{:02}{:02}.{:02}", hh, mm, ss, cs);
        let date = format!("{:02}0624", day.min(28));
        let (la, ns) = nmea_coord(self.lat, true);
        let (lo, ew) = nmea_coord(self.lon, false);
        let knots = self.speed_kmh / 1.852;
        let lines = if self.fix {
            vec![
                nmea(&format!("GPRMC,{},A,{},{},{},{},{:.3},{:.2},{},,,A", time, la, ns, lo, ew, knots, self.course, date)),
                nmea(&format!("GPVTG,{:.2},T,,M,{:.3},N,{:.3},K,A", self.course, knots, self.speed_kmh)),
                nmea(&format!("GPGGA,{},{},{},{},{},1,{:02},1.01,{:.1},M,50.0,M,,", time, la, ns, lo, ew, self.sats, self.alt)),
                nmea("GPGSA,A,3,04,05,09,12,17,19,24,25,,,,,1.78,1.01,1.47"),
                nmea(&format!("GPGLL,{},{},{},{},{},A,A", la, ns, lo, ew, time)),
            ]
        } else {
            vec![
                nmea(&format!("GPRMC,{},V,,,,,,,{},,,N", time, date)),
                nmea("GPVTG,,,,,,,,,N"),
                nmea(&format!("GPGGA,{},,,,,0,00,99.99,,,,,,", time)),
                nmea("GPGSA,A,1,,,,,,,,,,,,,99.99,99.99,99.99"),
                nmea(&format!("GPGLL,,,,,{},V,N", time)),
            ]
        };
        for l in lines {
            if self.out.len() < 4096 {
                self.out.extend(l.bytes());
            }
            self.sentences += 1;
        }
    }
}

impl Device for Gps {
    fn kind(&self) -> &'static str {
        "gps-neo6m"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("RX", Input), pin("TX", Output), pin("GND", Ground), pin("PPS", Output)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        // idle UART line is high
        out.push(Stamp::ToRail { a: 2, rail: Rail::V3, ohms: 1000.0 });
    }
    fn uart_pins(&self) -> Option<(usize, usize)> {
        Some((1, 2))
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        while now >= self.next_fix {
            let t = self.next_fix;
            self.emit(t);
            self.next_fix += self.period;
        }
        Some(self.next_fix)
    }
    fn uart_transmit(&mut self, now: u64) -> Option<u8> {
        self.poll(now);
        self.out.pop_front()
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        let mut ok = false;
        let mut set = |k: &str, v: &mut f64| {
            if let Some(x) = a.get(k).and_then(|x| x.as_f64()) {
                *v = x;
                ok = true;
            }
        };
        set("lat", &mut self.lat);
        set("lon", &mut self.lon);
        set("alt", &mut self.alt);
        set("speed", &mut self.speed_kmh);
        set("course", &mut self.course);
        if let Some(x) = a.get("sats").and_then(|x| x.as_u64()) {
            self.sats = x as u32;
            ok = true;
        }
        if let Some(b) = a.get("fix").and_then(|x| x.as_bool()) {
            self.fix = b;
            ok = true;
        }
        if ok {
            Ok(())
        } else {
            Err("gps-neo6m accepts {lat, lon, alt, speed (km/h), course, sats, fix}".into())
        }
    }
    fn state(&self) -> Value {
        json!({"lat": self.lat, "lon": self.lon, "alt": self.alt, "speed": self.speed_kmh, "course": self.course, "sats": self.sats, "fix": self.fix, "sentences": self.sentences})
    }
    fn summary(&self) -> String {
        if self.fix {
            format!("fix {:.5}, {:.5} · {:.0} m · {} sats", self.lat, self.lon, self.alt, self.sats)
        } else {
            "searching for satellites (no fix)".into()
        }
    }
}

// ===================================================================== HC-05
/// HC-05 Bluetooth SPP module.  While no phone is connected, text from the
/// ESP32 is treated as AT commands; once connected (from the inspector) it is
/// a transparent serial link to a virtual phone terminal.
pub struct Hc05 {
    name: String,
    pub connected: bool,
    to_esp: VecDeque<u8>,
    at_line: Vec<u8>,
    /// text received by the "phone"
    pub phone_rx: String,
    pub log: VecDeque<String>,
    dirty: bool,
}

impl Hc05 {
    pub fn new(p: &Props) -> Hc05 {
        Hc05 {
            name: prop_str(p, &["name"], "HC-05").to_string(),
            connected: p.get("connected").and_then(|v| v.as_bool()).unwrap_or(false),
            to_esp: VecDeque::new(),
            at_line: Vec::new(),
            phone_rx: String::new(),
            log: VecDeque::new(),
            dirty: true,
        }
    }
    fn reply(&mut self, s: &str) {
        self.to_esp.extend(s.bytes());
    }
    fn at_command(&mut self, cmd: &str) {
        let c = cmd.trim().to_ascii_uppercase();
        if !c.starts_with("AT") {
            return;
        }
        if c == "AT" {
            self.reply("OK\r\n");
        } else if c == "AT+NAME?" {
            let n = self.name.clone();
            self.reply(&format!("+NAME:{}\r\nOK\r\n", n));
        } else if let Some(n) = cmd.trim().strip_prefix("AT+NAME=") {
            self.name = n.to_string();
            self.reply("OK\r\n");
        } else if c == "AT+VERSION?" {
            self.reply("+VERSION:2.0-20100601\r\nOK\r\n");
        } else if c == "AT+ADDR?" {
            self.reply("+ADDR:98d3:31:fd5a1c\r\nOK\r\n");
        } else if c == "AT+STATE?" {
            let st = if self.connected { "CONNECTED" } else { "INITIALIZED" };
            self.reply(&format!("+STATE:{}\r\nOK\r\n", st));
        } else {
            self.reply("OK\r\n");
        }
    }
}

impl Device for Hc05 {
    fn kind(&self) -> &'static str {
        "hc-05"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("STATE", Output), pin("RXD", Input), pin("TXD", Output), pin("GND", Ground), pin("VCC", Power), pin("EN", Input)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::ToRail { a: 2, rail: Rail::V3, ohms: 1000.0 });
        out.push(Stamp::ToRail { a: 0, rail: if self.connected { Rail::V3 } else { Rail::Gnd }, ohms: 200.0 });
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn uart_pins(&self) -> Option<(usize, usize)> {
        Some((1, 2))
    }
    fn uart_receive(&mut self, _now: u64, b: u8) {
        if self.connected {
            self.phone_rx.push(b as char);
            if self.phone_rx.len() > 4000 {
                self.phone_rx.drain(..1000);
            }
        } else {
            if b == b'\n' {
                let line = String::from_utf8_lossy(&self.at_line).to_string();
                self.at_line.clear();
                self.at_command(&line);
            } else if b != b'\r' && self.at_line.len() < 128 {
                self.at_line.push(b);
            }
        }
    }
    fn uart_transmit(&mut self, _now: u64) -> Option<u8> {
        self.to_esp.pop_front()
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        if let Some(t) = a.get("send").and_then(|v| v.as_str()) {
            if !self.connected {
                return Err("no phone connected (connect first)".into());
            }
            self.to_esp.extend(t.bytes());
            if a.get("newline").and_then(|v| v.as_bool()).unwrap_or(true) {
                self.to_esp.push_back(b'\n');
            }
            return Ok(());
        }
        match a.get("action").and_then(|v| v.as_str()) {
            Some("connect") => {
                self.connected = true;
                self.dirty = true;
                Ok(())
            }
            Some("disconnect") => {
                self.connected = false;
                self.dirty = true;
                Ok(())
            }
            Some("clear") => {
                self.phone_rx.clear();
                Ok(())
            }
            _ => Err("hc-05 accepts {\"action\": \"connect\"|\"disconnect\"|\"clear\"} or {\"send\": \"text\"}".into()),
        }
    }
    fn state(&self) -> Value {
        json!({"connected": self.connected, "name": self.name, "phone_rx": self.phone_rx})
    }
    fn summary(&self) -> String {
        if self.connected {
            format!("{} connected · phone received {} chars", self.name, self.phone_rx.len())
        } else {
            format!("{} waiting for a connection (AT mode)", self.name)
        }
    }
}

//! Shared helpers: build boards from YAML and drive them the way the ESP32
//! peripherals do (the chip itself is mocked by these calls).
#![allow(dead_code)]
use devices::circuit::Board;
use devices::config::ProjectConfig;
use esp32_soc::io::{IoBridge, PinDrive, SpiPins};
use serde_json::Value;

pub const MS: u64 = 1_000_000;
pub const US: u64 = 1_000;

pub fn board(yaml: &str) -> Board {
    let mut b = ProjectConfig::parse(yaml).unwrap_or_else(|e| panic!("parse: {}\n{}", e, yaml)).build_board().unwrap_or_else(|e| panic!("build: {}", e));
    b.gpio_update(0, &PinDrive::default());
    b
}

/// `components` given as YAML flow items; `wires` as `a b c` net lists.
pub fn board_of(components: &[&str], nets: &[&str]) -> Board {
    let mut y = String::from("components:\n");
    for c in components {
        y.push_str(&format!("  - {}\n", c));
    }
    y.push_str("connections:\n");
    for n in nets {
        let items: Vec<&str> = n.split_whitespace().collect();
        y.push_str(&format!("  - [{}]\n", items.join(", ")));
    }
    board(&y)
}

pub fn state(b: &Board, id: &str) -> Value {
    let snap = b.snapshot();
    snap["components"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("no component {}", id))["state"].clone()
}

pub fn summary(b: &Board, id: &str) -> String {
    let i = b.find(id).unwrap();
    b.comps[i].dev.summary()
}

pub fn drive(pins: &[(u8, bool)]) -> PinDrive {
    let mut d = PinDrive::default();
    for &(p, v) in pins {
        d.out_en |= 1 << p;
        if v {
            d.out_level |= 1 << p;
        }
    }
    d
}

/// Standard wiring of an I2C module on GPIO21 (SDA) / GPIO22 (SCL).
pub fn i2c_board(id: &str, part: &str) -> Board {
    board_of(&[part], &[&format!("esp32.D21 {id}.SDA"), &format!("esp32.D22 {id}.SCL")])
}

/// I2C master on a board (what the ESP32's I2C controller does).
pub struct I2c<'a> {
    pub b: &'a mut Board,
    pub t: u64,
    pub scl: Option<u8>,
    pub sda: Option<u8>,
}

impl<'a> I2c<'a> {
    pub fn new(b: &'a mut Board) -> I2c<'a> {
        let t = b.now + MS;
        I2c { b, t, scl: Some(22), sda: Some(21) }
    }
    fn tick(&mut self) -> u64 {
        self.t += 100 * US;
        self.t
    }
    pub fn wait(&mut self, ns: u64) {
        self.t += ns;
        self.b.advance(self.t);
    }
    pub fn probe(&mut self, addr: u8) -> bool {
        let t = self.tick();
        let ok = self.b.i2c_start(t, self.scl, self.sda, addr, false);
        let t = self.tick();
        self.b.i2c_stop(t, self.scl, self.sda);
        ok
    }
    /// START, address+W, bytes, STOP. Returns false on any NACK.
    pub fn write(&mut self, addr: u8, bytes: &[u8]) -> bool {
        let t = self.tick();
        let mut ok = self.b.i2c_start(t, self.scl, self.sda, addr, false);
        for &x in bytes {
            if !ok {
                break;
            }
            let t = self.tick();
            ok = self.b.i2c_write(t, self.scl, self.sda, x);
        }
        let t = self.tick();
        self.b.i2c_stop(t, self.scl, self.sda);
        ok
    }
    /// START, address+R, n bytes (last one NACKed), STOP.
    pub fn read(&mut self, addr: u8, n: usize) -> Option<Vec<u8>> {
        let t = self.tick();
        if !self.b.i2c_start(t, self.scl, self.sda, addr, true) {
            let t = self.tick();
            self.b.i2c_stop(t, self.scl, self.sda);
            return None;
        }
        let mut v = Vec::new();
        for i in 0..n {
            let t = self.tick();
            v.push(self.b.i2c_read(t, self.scl, self.sda, i + 1 < n));
        }
        let t = self.tick();
        self.b.i2c_stop(t, self.scl, self.sda);
        Some(v)
    }
    /// Register read: START addr+W reg, repeated START addr+R n bytes, STOP.
    pub fn read_reg(&mut self, addr: u8, reg: u8, n: usize) -> Vec<u8> {
        let t = self.tick();
        assert!(self.b.i2c_start(t, self.scl, self.sda, addr, false), "no ACK from 0x{:02x}", addr);
        let t = self.tick();
        self.b.i2c_write(t, self.scl, self.sda, reg);
        let t = self.tick();
        assert!(self.b.i2c_start(t, self.scl, self.sda, addr, true));
        let mut v = Vec::new();
        for i in 0..n {
            let t = self.tick();
            v.push(self.b.i2c_read(t, self.scl, self.sda, i + 1 < n));
        }
        let t = self.tick();
        self.b.i2c_stop(t, self.scl, self.sda);
        v
    }
    pub fn reg8(&mut self, addr: u8, reg: u8) -> u8 {
        self.read_reg(addr, reg, 1)[0]
    }
    pub fn write_reg(&mut self, addr: u8, reg: u8, val: &[u8]) -> bool {
        let mut v = vec![reg];
        v.extend_from_slice(val);
        self.write(addr, &v)
    }
    pub fn scan(&mut self) -> Vec<u8> {
        (0x08..0x78).filter(|&a| self.probe(a)).collect()
    }
}

pub fn be16(v: &[u8]) -> i16 {
    i16::from_be_bytes([v[0], v[1]])
}
pub fn le16(v: &[u8]) -> i16 {
    i16::from_le_bytes([v[0], v[1]])
}

/// SPI master: full-duplex transfer with a hardware CS pad.
pub fn spi(b: &mut Board, t: u64, sck: u8, mosi: u8, miso: Option<u8>, cs: u8, data: &[u8]) -> Vec<u8> {
    let mut d = data.to_vec();
    b.spi_transfer(t, 2, SpiPins { sclk: Some(sck), mosi: Some(mosi), miso, cs: Some(cs) }, &mut d, 8 * data.len() as u32);
    d
}

/// Route UART `port` to TX/RX pads (as the GPIO matrix would).
pub fn uart_route(b: &mut Board, port: u8, tx: u8, rx: u8, extra: &[(u8, bool)]) {
    let mut d = drive(extra);
    d.out_en |= 1 << tx;
    d.out_level |= 1 << tx;
    d.out_signal[tx as usize] = match port {
        0 => 14,
        1 => 17,
        _ => 198,
    };
    d.uart_rx_pads[port as usize] = Some(rx);
    b.gpio_update(b.now, &d);
}

pub fn uart_send(b: &mut Board, t: u64, port: u8, bytes: &[u8]) {
    for (i, &x) in bytes.iter().enumerate() {
        b.uart_tx(t + i as u64 * 87 * US, port, x);
    }
}

/// Collect what the device sends back on `port` until `until` (ns).
pub fn uart_collect(b: &mut Board, from: u64, until: u64, port: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut t = from;
    while t < until {
        while let Some(x) = b.uart_rx(t, port) {
            out.push(x);
        }
        t += 100 * US;
    }
    out
}

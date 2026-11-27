//! I2C sensors: IMUs, magnetometers, barometers, RTCs, light / humidity sensors
//! and multi-chip boards (GY-86 "10DOF flight controller", GY-87, GY-91).

use crate::parts::{prop_f64, prop_u8, xyz, Props};
use crate::{pin, Device, PinInfo, PinKind::*};
use serde_json::{json, Value};

/// Physical quantities seen by the sensors of one module.
#[derive(Clone, Debug)]
pub struct Phys {
    /// acceleration in g
    pub accel: [f64; 3],
    /// angular rate in degrees/s
    pub gyro: [f64; 3],
    /// magnetic field in µT
    pub mag: [f64; 3],
    pub temp: f64,
    /// hPa
    pub pressure: f64,
    pub humidity: f64,
    pub lux: f64,
    /// orientation used to derive accel/mag (degrees)
    pub roll: f64,
    pub pitch: f64,
    pub yaw: f64,
    /// Unix time at emulation time 0 (RTCs)
    pub epoch0: i64,
    pub now: u64,
}

impl Default for Phys {
    fn default() -> Self {
        let epoch0 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(1_700_000_000);
        let mut p = Phys {
            accel: [0.0, 0.0, 1.0],
            gyro: [0.0; 3],
            mag: [20.0, 0.0, -40.0],
            temp: 24.0,
            pressure: 1013.25,
            humidity: 45.0,
            lux: 300.0,
            roll: 0.0,
            pitch: 0.0,
            yaw: 0.0,
            epoch0,
            now: 0,
        };
        p.apply_orientation();
        p
    }
}

impl Phys {
    pub fn apply_orientation(&mut self) {
        let (r, p, y) = (self.roll.to_radians(), self.pitch.to_radians(), self.yaw.to_radians());
        // gravity in the sensor frame for roll (x) / pitch (y)
        self.accel = [-p.sin(), r.sin() * p.cos(), r.cos() * p.cos()];
        // earth field: 40 µT horizontal (north), -30 µT vertical, rotated by yaw then tilt
        let (hx, hy, hz) = (40.0 * y.cos(), -40.0 * y.sin(), -30.0);
        let x1 = hx * p.cos() - hz * p.sin();
        let z1 = hx * p.sin() + hz * p.cos();
        let y1 = hy * r.cos() + z1 * r.sin();
        let z2 = -hy * r.sin() + z1 * r.cos();
        self.mag = [x1, y1, z2];
    }

    pub fn input(&mut self, a: &Value) -> Result<(), String> {
        let mut any = false;
        if let Some(v) = xyz(a, "accel") {
            self.accel = v;
            any = true;
        }
        if let Some(v) = xyz(a, "gyro") {
            self.gyro = v;
            any = true;
        }
        if let Some(v) = xyz(a, "mag") {
            self.mag = v;
            any = true;
        }
        let mut orient = false;
        for (k, f) in [("roll", &mut self.roll), ("pitch", &mut self.pitch), ("yaw", &mut self.yaw)] {
            if let Some(v) = a.get(k).and_then(|x| x.as_f64()) {
                *f = v;
                orient = true;
            }
        }
        if orient {
            self.apply_orientation();
            any = true;
        }
        if let Some(v) = a.get("temperature").and_then(|x| x.as_f64()) {
            self.temp = v;
            any = true;
        }
        if let Some(v) = a.get("pressure").and_then(|x| x.as_f64()) {
            self.pressure = v;
            any = true;
        }
        if let Some(v) = a.get("altitude").and_then(|x| x.as_f64()) {
            self.pressure = 1013.25 * (1.0 - v / 44330.0).powf(5.255);
            any = true;
        }
        if let Some(v) = a.get("humidity").and_then(|x| x.as_f64()) {
            self.humidity = v;
            any = true;
        }
        if let Some(v) = a.get("lux").and_then(|x| x.as_f64()) {
            self.lux = v;
            any = true;
        }
        if let Some(v) = a.get("time").and_then(|x| x.as_i64()) {
            self.epoch0 = v - (self.now / 1_000_000_000) as i64;
            any = true;
        }
        if any {
            Ok(())
        } else {
            Err("inputs: accel{x,y,z} g, gyro{x,y,z} °/s, mag{x,y,z} µT, roll/pitch/yaw °, temperature, pressure hPa, altitude m, humidity, lux, time (unix)".into())
        }
    }

    pub fn altitude(&self) -> f64 {
        44330.0 * (1.0 - (self.pressure / 1013.25).powf(1.0 / 5.255))
    }

    pub fn json(&self) -> Value {
        json!({
            "accel": self.accel, "gyro": self.gyro, "mag": self.mag, "temperature": self.temp,
            "pressure": self.pressure, "altitude": self.altitude(), "humidity": self.humidity, "lux": self.lux,
            "roll": self.roll, "pitch": self.pitch, "yaw": self.yaw
        })
    }
}

/// An I2C chip inside a module.
pub trait Chip: Send {
    fn responds(&self, addr: u8, ctx: &ChipCtx) -> bool;
    fn start(&mut self, read: bool, ph: &Phys);
    fn write(&mut self, b: u8, ph: &mut Phys) -> bool;
    fn read(&mut self, ph: &Phys) -> u8;
    fn stop(&mut self) {}
    fn name(&self) -> &'static str;
}

/// Cross-chip context (e.g. MPU9250 I2C bypass enabling the AK8963).
#[derive(Default, Clone, Copy)]
pub struct ChipCtx {
    pub bypass: bool,
}

fn be16(v: f64) -> [u8; 2] {
    let x = v.round().clamp(-32768.0, 32767.0) as i16 as u16;
    [(x >> 8) as u8, x as u8]
}
fn le16(v: f64) -> [u8; 2] {
    let x = v.round().clamp(-32768.0, 32767.0) as i16 as u16;
    [x as u8, (x >> 8) as u8]
}
fn bcd(v: u32) -> u8 {
    (((v / 10) << 4) | (v % 10)) as u8
}
fn unbcd(b: u8) -> u32 {
    ((b >> 4) as u32) * 10 + (b & 0xF) as u32
}

// ------------------------------------------------------------------ MPU6050 / MPU6500 / MPU9250
pub struct Mpu {
    addr: u8,
    who: u8,
    regs: [u8; 128],
    ptr: usize,
    got: bool,
}
impl Mpu {
    pub fn new(addr: u8, who: u8) -> Mpu {
        let mut regs = [0u8; 128];
        regs[0x6B] = 0x40;
        regs[0x75] = who;
        Mpu { addr, who, regs, ptr: 0, got: false }
    }
    fn value(&self, r: usize, ph: &Phys) -> u8 {
        match r {
            0x3A => 0x01,
            0x3B..=0x40 => {
                let sens = 16384.0 / (1 << ((self.regs[0x1C] >> 3) & 3)) as f64;
                let i = (r - 0x3B) / 2;
                be16(ph.accel[i] * sens)[(r - 0x3B) % 2]
            }
            0x41 | 0x42 => {
                let raw = if self.who == 0x68 { (ph.temp - 36.53) * 340.0 } else { (ph.temp - 21.0) * 333.87 };
                be16(raw)[r - 0x41]
            }
            0x43..=0x48 => {
                let sens = [131.0, 65.5, 32.8, 16.4][((self.regs[0x1B] >> 3) & 3) as usize];
                let i = (r - 0x43) / 2;
                be16(ph.gyro[i] * sens)[(r - 0x43) % 2]
            }
            0x72 | 0x73 => 0, // FIFO count
            _ => self.regs[r],
        }
    }
    pub fn bypass(&self) -> bool {
        self.regs[0x37] & 0x02 != 0 && self.regs[0x6A] & 0x20 == 0
    }
}
impl Chip for Mpu {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == self.addr
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if !read {
            self.got = false;
        }
    }
    fn write(&mut self, b: u8, _ph: &mut Phys) -> bool {
        if !self.got {
            self.ptr = (b & 0x7F) as usize;
            self.got = true;
        } else {
            if self.ptr == 0x6B && b & 0x80 != 0 {
                let who = self.who;
                *self = Mpu::new(self.addr, who);
                self.got = true;
                return true;
            }
            if self.ptr != 0x75 {
                self.regs[self.ptr] = b;
            }
            self.ptr = (self.ptr + 1) & 0x7F;
        }
        true
    }
    fn read(&mut self, ph: &Phys) -> u8 {
        let v = self.value(self.ptr, ph);
        self.ptr = (self.ptr + 1) & 0x7F;
        v
    }
    fn name(&self) -> &'static str {
        match self.who {
            0x68 => "MPU6050",
            0x70 => "MPU6500",
            _ => "MPU9250",
        }
    }
}

pub struct Ak8963 {
    regs: [u8; 0x13],
    ptr: usize,
    got: bool,
}
impl Ak8963 {
    pub fn new() -> Ak8963 {
        let mut regs = [0u8; 0x13];
        regs[0] = 0x48;
        regs[1] = 0x9A;
        regs[0x10] = 128;
        regs[0x11] = 128;
        regs[0x12] = 128;
        Ak8963 { regs, ptr: 0, got: false }
    }
}
impl Chip for Ak8963 {
    fn responds(&self, addr: u8, c: &ChipCtx) -> bool {
        addr == 0x0C && c.bypass
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if !read {
            self.got = false;
        }
    }
    fn write(&mut self, b: u8, _ph: &mut Phys) -> bool {
        if !self.got {
            self.ptr = b as usize % 0x13;
            self.got = true;
        } else {
            if self.ptr == 0x0B && b & 1 != 0 {
                *self = Ak8963::new();
                return true;
            }
            if self.ptr >= 0x0A && self.ptr < 0x10 {
                self.regs[self.ptr] = b;
            }
            self.ptr = (self.ptr + 1) % 0x13;
        }
        true
    }
    fn read(&mut self, ph: &Phys) -> u8 {
        let bit16 = self.regs[0x0A] & 0x10 != 0;
        let scale = if bit16 { 1.0 / 0.15 } else { 1.0 / 0.6 };
        let r = self.ptr;
        let v = match r {
            0x02 => 0x01,
            0x03..=0x08 => le16(ph.mag[(r - 3) / 2] * scale)[(r - 3) % 2],
            0x09 => if bit16 { 0x10 } else { 0x00 },
            _ => self.regs[r],
        };
        self.ptr = (self.ptr + 1) % 0x13;
        v
    }
    fn name(&self) -> &'static str {
        "AK8963"
    }
}

// ------------------------------------------------------------------ ADXL345
pub struct Adxl345 {
    addr: u8,
    regs: [u8; 64],
    ptr: usize,
    got: bool,
}
impl Adxl345 {
    pub fn new(addr: u8) -> Adxl345 {
        let mut regs = [0u8; 64];
        regs[0] = 0xE5;
        regs[0x2C] = 0x0A;
        regs[0x30] = 0x82;
        Adxl345 { addr, regs, ptr: 0, got: false }
    }
}
impl Chip for Adxl345 {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == self.addr
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if !read {
            self.got = false;
        }
    }
    fn write(&mut self, b: u8, _ph: &mut Phys) -> bool {
        if !self.got {
            self.ptr = (b & 0x3F) as usize;
            self.got = true;
        } else {
            if self.ptr != 0 && self.ptr != 0x30 {
                self.regs[self.ptr] = b;
            }
            self.ptr = (self.ptr + 1) & 0x3F;
        }
        true
    }
    fn read(&mut self, ph: &Phys) -> u8 {
        let r = self.ptr;
        let fmt = self.regs[0x31];
        let v = match r {
            0x32..=0x37 => {
                let range = fmt & 3;
                let lsb_per_g = if fmt & 0x08 != 0 { 256.0 } else { 256.0 / (1 << range) as f64 };
                le16(ph.accel[(r - 0x32) / 2] * lsb_per_g)[(r - 0x32) % 2]
            }
            0x39 => 0,
            _ => self.regs[r],
        };
        self.ptr = (self.ptr + 1) & 0x3F;
        v
    }
    fn name(&self) -> &'static str {
        "ADXL345"
    }
}

// ------------------------------------------------------------------ HMC5883L
pub struct Hmc5883l {
    regs: [u8; 13],
    ptr: usize,
    got: bool,
}
impl Hmc5883l {
    pub fn new() -> Hmc5883l {
        let mut regs = [0u8; 13];
        regs[0] = 0x10;
        regs[1] = 0x20;
        regs[2] = 0x01;
        regs[9] = 0x01;
        regs[10] = b'H';
        regs[11] = b'4';
        regs[12] = b'3';
        Hmc5883l { regs, ptr: 0, got: false }
    }
}
impl Chip for Hmc5883l {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == 0x1E
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if !read {
            self.got = false;
        }
    }
    fn write(&mut self, b: u8, _ph: &mut Phys) -> bool {
        if !self.got {
            self.ptr = b as usize % 13;
            self.got = true;
        } else {
            if self.ptr < 3 {
                self.regs[self.ptr] = b;
            }
            self.ptr = (self.ptr + 1) % 13;
        }
        true
    }
    fn read(&mut self, ph: &Phys) -> u8 {
        let gain = [1370.0, 1090.0, 820.0, 660.0, 440.0, 390.0, 330.0, 230.0][(self.regs[1] >> 5) as usize];
        let g = |i: usize| ph.mag[i] / 100.0 * gain;
        let r = self.ptr;
        let v = match r {
            3 | 4 => be16(g(0))[r - 3],
            5 | 6 => be16(g(2))[r - 5],
            7 | 8 => be16(g(1))[r - 7],
            _ => self.regs[r],
        };
        // pointer wraps from 12 to 0 but auto-increment stops at data regs (3..8 then 3)
        self.ptr = if r == 8 { 3 } else { (r + 1) % 13 };
        v
    }
    fn name(&self) -> &'static str {
        "HMC5883L"
    }
}

pub struct Qmc5883l {
    regs: [u8; 14],
    ptr: usize,
    got: bool,
}
impl Qmc5883l {
    pub fn new() -> Qmc5883l {
        let mut regs = [0u8; 14];
        regs[0x0D] = 0xFF;
        Qmc5883l { regs, ptr: 0, got: false }
    }
}
impl Chip for Qmc5883l {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == 0x0D
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if !read {
            self.got = false;
        }
    }
    fn write(&mut self, b: u8, _ph: &mut Phys) -> bool {
        if !self.got {
            self.ptr = b as usize % 14;
            self.got = true;
        } else {
            self.regs[self.ptr] = b;
            self.ptr = (self.ptr + 1) % 14;
        }
        true
    }
    fn read(&mut self, ph: &Phys) -> u8 {
        let lsb_per_gauss = if self.regs[9] & 0x10 != 0 { 3000.0 } else { 12000.0 };
        let r = self.ptr;
        let v = match r {
            0..=5 => le16(ph.mag[r / 2] / 100.0 * lsb_per_gauss)[r % 2],
            6 => 0x01,
            7 | 8 => le16(ph.temp * 100.0)[r - 7],
            _ => self.regs[r],
        };
        self.ptr = (self.ptr + 1) % 14;
        v
    }
    fn name(&self) -> &'static str {
        "QMC5883L"
    }
}

// ------------------------------------------------------------------ MS5611 barometer
pub struct Ms5611 {
    addr: u8,
    c: [u16; 8],
    cmd: u8,
    adc: u32,
    rcount: usize,
}
impl Ms5611 {
    pub fn new(addr: u8) -> Ms5611 {
        Ms5611 { addr, c: [0, 40127, 36924, 23317, 23282, 33464, 28312, 0], cmd: 0, adc: 0, rcount: 0 }
    }
    fn d2_for(&self, t: f64) -> f64 {
        let temp = t * 100.0;
        self.c[5] as f64 * 256.0 + (temp - 2000.0) * 8388608.0 / self.c[6] as f64
    }
    fn d1_for(&self, t: f64, p_hpa: f64) -> f64 {
        let dt = self.d2_for(t) - self.c[5] as f64 * 256.0;
        let off = self.c[2] as f64 * 65536.0 + self.c[4] as f64 * dt / 128.0;
        let sens = self.c[1] as f64 * 32768.0 + self.c[3] as f64 * dt / 256.0;
        let p = p_hpa * 100.0;
        (p * 32768.0 + off) * 2097152.0 / sens
    }
}
impl Chip for Ms5611 {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == self.addr
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if read {
            self.rcount = 0;
        }
    }
    fn write(&mut self, b: u8, ph: &mut Phys) -> bool {
        self.cmd = b;
        match b {
            0x40..=0x48 => self.adc = self.d1_for(ph.temp, ph.pressure).clamp(0.0, 16777215.0) as u32,
            0x50..=0x58 => self.adc = self.d2_for(ph.temp).clamp(0.0, 16777215.0) as u32,
            _ => {}
        }
        true
    }
    fn read(&mut self, _ph: &Phys) -> u8 {
        let i = self.rcount;
        self.rcount += 1;
        match self.cmd {
            0xA0..=0xAE => {
                let w = self.c[((self.cmd - 0xA0) / 2) as usize];
                if i == 0 {
                    (w >> 8) as u8
                } else {
                    w as u8
                }
            }
            0x00 => (self.adc >> (16 - 8 * i.min(2))) as u8,
            _ => 0,
        }
    }
    fn name(&self) -> &'static str {
        "MS5611"
    }
}

// ------------------------------------------------------------------ BMP280 / BME280
pub struct Bmp280 {
    addr: u8,
    bme: bool,
    regs: [u8; 256],
    ptr: usize,
    got: bool,
}
const BMP_T: [i64; 3] = [27504, 26435, -1000];
const BMP_P: [i64; 9] = [36477, -10685, 3024, 2855, 140, -7, 15500, -14600, 6000];
const BME_H: (i64, i64, i64, i64, i64, i64) = (75, 362, 0, 313, 50, 30);

impl Bmp280 {
    pub fn new(addr: u8, bme: bool) -> Bmp280 {
        let mut regs = [0u8; 256];
        regs[0xD0] = if bme { 0x60 } else { 0x58 };
        let put16 = |r: &mut [u8; 256], a: usize, v: i64| {
            let x = v as i16 as u16;
            r[a] = x as u8;
            r[a + 1] = (x >> 8) as u8;
        };
        put16(&mut regs, 0x88, BMP_T[0]);
        put16(&mut regs, 0x8A, BMP_T[1]);
        put16(&mut regs, 0x8C, BMP_T[2]);
        for (i, v) in BMP_P.iter().enumerate() {
            put16(&mut regs, 0x8E + 2 * i, *v);
        }
        if bme {
            let (h1, h2, h3, h4, h5, h6) = BME_H;
            regs[0xA1] = h1 as u8;
            put16(&mut regs, 0xE1, h2);
            regs[0xE3] = h3 as u8;
            regs[0xE4] = (h4 >> 4) as u8;
            regs[0xE5] = ((h4 & 0xF) | ((h5 & 0xF) << 4)) as u8;
            regs[0xE6] = (h5 >> 4) as u8;
            regs[0xE7] = h6 as u8;
        }
        Bmp280 { addr, bme, regs, ptr: 0, got: false }
    }
    fn t_fine(adc_t: i64) -> i64 {
        let (t1, t2, t3) = (BMP_T[0], BMP_T[1], BMP_T[2]);
        let var1 = (((adc_t >> 3) - (t1 << 1)) * t2) >> 11;
        let var2 = (((((adc_t >> 4) - t1) * ((adc_t >> 4) - t1)) >> 12) * t3) >> 14;
        var1 + var2
    }
    fn comp_p(adc_p: i64, t_fine: i64) -> f64 {
        let p = BMP_P;
        let mut var1 = t_fine - 128000;
        let mut var2 = var1 * var1 * p[5];
        var2 += (var1 * p[4]) << 17;
        var2 += p[3] << 35;
        var1 = ((var1 * var1 * p[2]) >> 8) + ((var1 * p[1]) << 12);
        var1 = (((1i64 << 47) + var1) * p[0]) >> 33;
        if var1 == 0 {
            return 0.0;
        }
        let mut pp = 1048576 - adc_p;
        pp = (((pp << 31) - var2) * 3125) / var1;
        let var1 = (p[8] * (pp >> 13) * (pp >> 13)) >> 25;
        let var2 = (p[7] * pp) >> 19;
        pp = ((pp + var1 + var2) >> 8) + (p[6] << 4);
        pp as f64 / 256.0
    }
    fn comp_h(adc_h: i64, t_fine: i64) -> f64 {
        let (h1, h2, h3, h4, h5, h6) = BME_H;
        let mut v = t_fine - 76800;
        v = ((((adc_h << 14) - (h4 << 20) - (h5 * v)) + 16384) >> 15)
            * (((((((v * h6) >> 10) * (((v * h3) >> 11) + 32768)) >> 10) + 2097152) * h2 + 8192) >> 14);
        v -= ((((v >> 15) * (v >> 15)) >> 7) * h1) >> 4;
        v = v.clamp(0, 419430400);
        (v >> 12) as f64 / 1024.0
    }
    /// Find raw ADC values that make the Bosch compensation produce the physical values.
    fn raws(&self, ph: &Phys) -> (i64, i64, i64) {
        let target_tf = ((ph.temp * 100.0 * 256.0 - 128.0) / 5.0).round() as i64;
        let (mut lo, mut hi) = (0i64, (1 << 20) - 1);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if Self::t_fine(mid) < target_tf {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let adc_t = lo;
        let tf = Self::t_fine(adc_t);
        let target_p = ph.pressure * 100.0;
        let (mut lo, mut hi) = (0i64, (1 << 20) - 1);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if Self::comp_p(mid, tf) > target_p {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let adc_p = lo;
        let (mut lo, mut hi) = (0i64, 65535i64);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if Self::comp_h(mid, tf) < ph.humidity {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        (adc_t, adc_p, lo)
    }
}
impl Chip for Bmp280 {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == self.addr
    }
    fn start(&mut self, read: bool, ph: &Phys) {
        if !read {
            self.got = false;
        } else {
            let (t, p, h) = self.raws(ph);
            self.regs[0xF7] = (p >> 12) as u8;
            self.regs[0xF8] = (p >> 4) as u8;
            self.regs[0xF9] = ((p & 0xF) << 4) as u8;
            self.regs[0xFA] = (t >> 12) as u8;
            self.regs[0xFB] = (t >> 4) as u8;
            self.regs[0xFC] = ((t & 0xF) << 4) as u8;
            if self.bme {
                self.regs[0xFD] = (h >> 8) as u8;
                self.regs[0xFE] = h as u8;
            }
        }
    }
    fn write(&mut self, b: u8, _ph: &mut Phys) -> bool {
        if !self.got {
            self.ptr = b as usize;
            self.got = true;
        } else {
            match self.ptr {
                0xE0 => {}
                0xF2 | 0xF4 | 0xF5 => self.regs[self.ptr] = b,
                _ => {}
            }
            self.ptr = (self.ptr + 1) & 0xFF;
        }
        true
    }
    fn read(&mut self, _ph: &Phys) -> u8 {
        let v = match self.ptr {
            0xF3 => 0, // not measuring
            p => self.regs[p],
        };
        self.ptr = (self.ptr + 1) & 0xFF;
        v
    }
    fn name(&self) -> &'static str {
        if self.bme {
            "BME280"
        } else {
            "BMP280"
        }
    }
}

// ------------------------------------------------------------------ BMP180
pub struct Bmp180 {
    regs: [u8; 256],
    ptr: usize,
    got: bool,
}
const B180: [i64; 11] = [408, -72, -14383, 32741, 32757, 23153, 6190, 4, -32768, -8711, 2868];
impl Bmp180 {
    pub fn new() -> Bmp180 {
        let mut regs = [0u8; 256];
        for (i, v) in B180.iter().enumerate() {
            let x = *v as i16 as u16;
            regs[0xAA + 2 * i] = (x >> 8) as u8;
            regs[0xAB + 2 * i] = x as u8;
        }
        regs[0xD0] = 0x55;
        Bmp180 { regs, ptr: 0, got: false }
    }
    fn true_temp(ut: i64) -> (i64, i64) {
        let (ac5, ac6, mc, md) = (B180[4] as u16 as i64, B180[5] as u16 as i64, B180[9], B180[10]);
        let x1 = ((ut - ac6) * ac5) >> 15;
        let x2 = (mc << 11) / (x1 + md);
        let b5 = x1 + x2;
        ((b5 + 8) >> 4, b5)
    }
    fn true_press(up: i64, b5: i64, oss: u32) -> i64 {
        let (ac1, ac2, ac3, ac4, b1, b2) = (B180[0], B180[1], B180[2], B180[3] as u16 as i64, B180[6], B180[7]);
        let b6 = b5 - 4000;
        let mut x1 = (b2 * ((b6 * b6) >> 12)) >> 11;
        let mut x2 = (ac2 * b6) >> 11;
        let mut x3 = x1 + x2;
        let b3 = ((((ac1 * 4) + x3) << oss) + 2) / 4;
        x1 = (ac3 * b6) >> 13;
        x2 = (b1 * ((b6 * b6) >> 12)) >> 16;
        x3 = ((x1 + x2) + 2) >> 2;
        let b4 = (ac4 * ((x3 + 32768) as u64 as i64)) >> 15;
        let b7 = (up - b3) * (50000 >> oss);
        if b4 == 0 {
            return 0;
        }
        let mut p = if b7 < 0x8000_0000 { (b7 * 2) / b4 } else { (b7 / b4) * 2 };
        x1 = (p >> 8) * (p >> 8);
        x1 = (x1 * 3038) >> 16;
        x2 = (-7357 * p) >> 16;
        p += (x1 + x2 + 3791) >> 4;
        p
    }
}
impl Chip for Bmp180 {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == 0x77
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if !read {
            self.got = false;
        }
    }
    fn write(&mut self, b: u8, ph: &mut Phys) -> bool {
        if !self.got {
            self.ptr = b as usize;
            self.got = true;
            return true;
        }
        if self.ptr == 0xF4 {
            self.regs[0xF4] = b;
            // temperature target -> UT (binary search)
            let tt = (ph.temp * 10.0).round() as i64;
            let (mut lo, mut hi) = (0i64, 65535i64);
            while lo < hi {
                let m = (lo + hi) / 2;
                if Self::true_temp(m).0 < tt {
                    lo = m + 1;
                } else {
                    hi = m;
                }
            }
            let ut = lo;
            if b == 0x2E {
                self.regs[0xF6] = (ut >> 8) as u8;
                self.regs[0xF7] = ut as u8;
            } else {
                let oss = ((b >> 6) & 3) as u32;
                let b5 = Self::true_temp(ut).1;
                let target = (ph.pressure * 100.0).round() as i64;
                let (mut lo, mut hi) = (0i64, (1 << (16 + oss)) - 1);
                while lo < hi {
                    let m = (lo + hi) / 2;
                    if Self::true_press(m, b5, oss) < target {
                        lo = m + 1;
                    } else {
                        hi = m;
                    }
                }
                let up = lo << (8 - oss);
                self.regs[0xF6] = (up >> 16) as u8;
                self.regs[0xF7] = (up >> 8) as u8;
                self.regs[0xF8] = up as u8;
            }
        }
        self.ptr += 1;
        true
    }
    fn read(&mut self, _ph: &Phys) -> u8 {
        let v = self.regs[self.ptr & 0xFF];
        self.ptr += 1;
        v
    }
    fn name(&self) -> &'static str {
        "BMP180"
    }
}

// ------------------------------------------------------------------ DS3231 / DS1307 RTC
pub struct Rtc {
    ds1307: bool,
    regs: [u8; 64],
    ptr: usize,
    got: bool,
    dirty_time: bool,
}
impl Rtc {
    pub fn new(ds1307: bool) -> Rtc {
        let mut regs = [0u8; 64];
        regs[0x0E] = 0x1C;
        Rtc { ds1307, regs, ptr: 0, got: false, dirty_time: false }
    }
    fn fill_time(&mut self, ph: &Phys) {
        let t = ph.epoch0 + (ph.now / 1_000_000_000) as i64;
        let days = t.div_euclid(86400);
        let secs = t.rem_euclid(86400);
        let (y, m, d) = civil_from_days(days);
        let wday = ((days + 4).rem_euclid(7)) + 1; // 1970-01-01 was a Thursday; 1 = Sunday
        self.regs[0] = bcd((secs % 60) as u32);
        self.regs[1] = bcd(((secs / 60) % 60) as u32);
        self.regs[2] = bcd((secs / 3600) as u32);
        self.regs[3] = wday as u8;
        self.regs[4] = bcd(d);
        self.regs[5] = bcd(m) | if y >= 2100 { 0x80 } else { 0 };
        self.regs[6] = bcd((y % 100) as u32);
        if !self.ds1307 {
            let t = (ph.temp * 4.0).round() as i32;
            self.regs[0x11] = (t >> 2) as i8 as u8;
            self.regs[0x12] = ((t & 3) << 6) as u8;
        }
    }
    fn time_from_regs(&self) -> i64 {
        let s = unbcd(self.regs[0] & 0x7F) as i64;
        let mi = unbcd(self.regs[1] & 0x7F) as i64;
        let h = if self.regs[2] & 0x40 != 0 {
            let h12 = unbcd(self.regs[2] & 0x1F) as i64 % 12;
            h12 + if self.regs[2] & 0x20 != 0 { 12 } else { 0 }
        } else {
            unbcd(self.regs[2] & 0x3F) as i64
        };
        let d = unbcd(self.regs[4] & 0x3F);
        let m = unbcd(self.regs[5] & 0x1F);
        let y = 2000 + unbcd(self.regs[6]) as i64;
        days_from_civil(y, m, d) * 86400 + h * 3600 + mi * 60 + s
    }
}
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
impl Chip for Rtc {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == 0x68
    }
    fn start(&mut self, read: bool, ph: &Phys) {
        if !read {
            self.got = false;
        } else {
            self.fill_time(ph);
        }
    }
    fn write(&mut self, b: u8, ph: &mut Phys) -> bool {
        if !self.got {
            self.ptr = b as usize & 0x3F;
            self.got = true;
            // make sure registers not written by this transaction hold the current time
            self.fill_time(ph);
        } else {
            let r = self.ptr;
            self.regs[r] = b;
            self.ptr = (self.ptr + 1) & 0x3F;
            if r <= 6 {
                self.dirty_time = true;
                // firmware sets the clock: move the RTC epoch accordingly
                ph.epoch0 = self.time_from_regs() - (ph.now / 1_000_000_000) as i64;
            }
        }
        true
    }
    fn read(&mut self, _ph: &Phys) -> u8 {
        let v = self.regs[self.ptr];
        self.ptr = (self.ptr + 1) & 0x3F;
        v
    }
    fn stop(&mut self) {}
    fn name(&self) -> &'static str {
        if self.ds1307 {
            "DS1307"
        } else {
            "DS3231"
        }
    }
}

// ------------------------------------------------------------------ BH1750, SHT31, AHT20
pub struct Bh1750 {
    addr: u8,
    mode: u8,
    rc: usize,
}
impl Chip for Bh1750 {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == self.addr
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if read {
            self.rc = 0;
        }
    }
    fn write(&mut self, b: u8, _ph: &mut Phys) -> bool {
        if b & 0xF0 == 0x10 || b & 0xF0 == 0x20 {
            self.mode = b;
        }
        true
    }
    fn read(&mut self, ph: &Phys) -> u8 {
        let factor = if self.mode & 0x0F == 0x01 { 2.4 } else { 1.2 };
        let c = (ph.lux * factor).round().clamp(0.0, 65535.0) as u16;
        let v = if self.rc == 0 { (c >> 8) as u8 } else { c as u8 };
        self.rc += 1;
        v
    }
    fn name(&self) -> &'static str {
        "BH1750"
    }
}

fn crc8_31(d: &[u8]) -> u8 {
    let mut c: u8 = 0xFF;
    for &b in d {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x31 } else { c << 1 };
        }
    }
    c
}

pub struct Sht31 {
    addr: u8,
    out: Vec<u8>,
    rc: usize,
    cmd: Vec<u8>,
}
impl Chip for Sht31 {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == self.addr
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if read {
            self.rc = 0;
        } else {
            self.cmd.clear();
        }
    }
    fn write(&mut self, b: u8, ph: &mut Phys) -> bool {
        self.cmd.push(b);
        if self.cmd.len() == 2 {
            let t = ((ph.temp + 45.0) / 175.0 * 65535.0).round().clamp(0.0, 65535.0) as u16;
            let h = (ph.humidity / 100.0 * 65535.0).round().clamp(0.0, 65535.0) as u16;
            let tb = [(t >> 8) as u8, t as u8];
            let hb = [(h >> 8) as u8, h as u8];
            self.out = vec![tb[0], tb[1], crc8_31(&tb), hb[0], hb[1], crc8_31(&hb)];
            if self.cmd == [0xF3, 0x2D] {
                self.out = vec![0x80, 0x10, crc8_31(&[0x80, 0x10])];
            }
        }
        true
    }
    fn read(&mut self, _ph: &Phys) -> u8 {
        let v = self.out.get(self.rc).copied().unwrap_or(0xFF);
        self.rc += 1;
        v
    }
    fn name(&self) -> &'static str {
        "SHT31"
    }
}

pub struct Aht20 {
    out: Vec<u8>,
    rc: usize,
}
impl Chip for Aht20 {
    fn responds(&self, addr: u8, _c: &ChipCtx) -> bool {
        addr == 0x38
    }
    fn start(&mut self, read: bool, _ph: &Phys) {
        if read {
            self.rc = 0;
        }
    }
    fn write(&mut self, b: u8, ph: &mut Phys) -> bool {
        if b == 0xAC {
            let h = (ph.humidity / 100.0 * 1048576.0).round().clamp(0.0, 1048575.0) as u32;
            let t = ((ph.temp + 50.0) / 200.0 * 1048576.0).round().clamp(0.0, 1048575.0) as u32;
            let mut o = vec![0x18u8, (h >> 12) as u8, (h >> 4) as u8, (((h & 0xF) << 4) | (t >> 16)) as u8, (t >> 8) as u8, t as u8];
            let crc = crc8_31(&o);
            o.push(crc);
            self.out = o;
        }
        true
    }
    fn read(&mut self, _ph: &Phys) -> u8 {
        let v = if self.out.is_empty() { 0x18 } else { self.out.get(self.rc).copied().unwrap_or(0xFF) };
        self.rc += 1;
        v
    }
    fn name(&self) -> &'static str {
        "AHT20"
    }
}

// ------------------------------------------------------------------ module wrapper
pub struct SensorModule {
    kind: &'static str,
    chips: Vec<Box<dyn Chip>>,
    pub ph: Phys,
    active: Option<usize>,
    mpu_index: Option<usize>,
    extra_pins: Vec<&'static str>,
}

impl SensorModule {
    pub fn new(kind: &'static str, p: &Props) -> Option<SensorModule> {
        let addr = |d: u8| prop_u8(p, &["address", "addr"], d);
        let (chips, extra): (Vec<Box<dyn Chip>>, Vec<&'static str>) = match kind {
            "mpu6050" => (vec![Box::new(Mpu::new(addr(0x68), 0x68))], vec!["XDA", "XCL", "AD0", "INT"]),
            "mpu6500" => (vec![Box::new(Mpu::new(addr(0x68), 0x70))], vec!["AD0", "INT"]),
            "mpu9250" => (vec![Box::new(Mpu::new(addr(0x68), 0x71)), Box::new(Ak8963::new())], vec!["AD0", "INT"]),
            "adxl345" => (vec![Box::new(Adxl345::new(addr(0x53)))], vec!["CS", "SDO", "INT1", "INT2"]),
            "hmc5883l" => (vec![Box::new(Hmc5883l::new())], vec!["DRDY"]),
            "qmc5883l" => (vec![Box::new(Qmc5883l::new())], vec!["DRDY"]),
            "ms5611" => (vec![Box::new(Ms5611::new(addr(0x77)))], vec!["CSB", "SDO"]),
            "bmp280" => (vec![Box::new(Bmp280::new(addr(0x76), false))], vec!["CSB", "SDO"]),
            "bme280" => (vec![Box::new(Bmp280::new(addr(0x76), true))], vec!["CSB", "SDO"]),
            "bmp180" => (vec![Box::new(Bmp180::new())], vec![]),
            "ds3231" => (vec![Box::new(Rtc::new(false))], vec!["SQW", "32K"]),
            "ds1307" => (vec![Box::new(Rtc::new(true))], vec!["SQW"]),
            "bh1750" => (vec![Box::new(Bh1750 { addr: addr(0x23), mode: 0x10, rc: 0 })], vec!["ADDR"]),
            "sht31" => (vec![Box::new(Sht31 { addr: addr(0x44), out: vec![], rc: 0, cmd: vec![] })], vec!["ADDR", "ALERT"]),
            "aht20" => (vec![Box::new(Aht20 { out: vec![], rc: 0 })], vec![]),
            // 10DOF flight controller board: MPU6050 + HMC5883L + MS5611
            "gy-86" => (vec![Box::new(Mpu::new(0x68, 0x68)), Box::new(Hmc5883l::new()), Box::new(Ms5611::new(0x77))], vec!["INT", "DRDY"]),
            // MPU6050 + HMC5883L + BMP180
            "gy-87" => (vec![Box::new(Mpu::new(0x68, 0x68)), Box::new(Hmc5883l::new()), Box::new(Bmp180::new())], vec!["INT", "DRDY"]),
            // MPU9250 + BMP280
            "gy-91" => (vec![Box::new(Mpu::new(0x68, 0x71)), Box::new(Ak8963::new()), Box::new(Bmp280::new(0x76, false))], vec!["INT"]),
            _ => return None,
        };
        let mut ph = Phys::default();
        ph.temp = prop_f64(p, &["temperature"], ph.temp);
        ph.pressure = prop_f64(p, &["pressure"], ph.pressure);
        ph.humidity = prop_f64(p, &["humidity"], ph.humidity);
        ph.lux = prop_f64(p, &["lux"], ph.lux);
        let mpu_index = if matches!(kind, "mpu9250" | "gy-91") { Some(0) } else { None };
        Some(SensorModule { kind, chips, ph, active: None, mpu_index, extra_pins: extra })
    }

    fn ctx(&self) -> ChipCtx {
        // the AK8963 becomes visible when the MPU9250's bypass mux is enabled
        let bypass = match self.mpu_index {
            Some(_) => true,
            None => false,
        };
        ChipCtx { bypass }
    }
}

impl Device for SensorModule {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn pins(&self) -> Vec<PinInfo> {
        let mut v = vec![pin("VCC", Power), pin("GND", Ground), pin("SCL", Input), pin("SDA", Io)];
        for n in &self.extra_pins {
            v.push(pin(n, Io));
        }
        v
    }
    fn i2c_pins(&self) -> Option<(usize, usize)> {
        Some((2, 3))
    }
    fn i2c_start(&mut self, addr: u8, read: bool) -> bool {
        let ctx = self.ctx();
        self.active = None;
        for (i, c) in self.chips.iter_mut().enumerate() {
            if c.responds(addr, &ctx) {
                c.start(read, &self.ph);
                self.active = Some(i);
                return true;
            }
        }
        false
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        match self.active {
            Some(i) => self.chips[i].write(b, &mut self.ph),
            None => false,
        }
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        match self.active {
            Some(i) => self.chips[i].read(&self.ph),
            None => 0xFF,
        }
    }
    fn i2c_stop(&mut self) {
        if let Some(i) = self.active.take() {
            self.chips[i].stop();
        }
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        self.ph.now = now;
        None
    }
    fn on_solved(&mut self, now: u64, _v: &crate::PinView) {
        self.ph.now = now;
    }
    fn input(&mut self, _now: u64, a: &Value) -> Result<(), String> {
        self.ph.input(a)
    }
    fn state(&self) -> Value {
        let chips: Vec<&str> = self.chips.iter().map(|c| c.name()).collect();
        let mut v = self.ph.json();
        v["chips"] = json!(chips);
        v
    }
    fn summary(&self) -> String {
        let p = &self.ph;
        match self.kind {
            "ds3231" | "ds1307" => {
                let t = p.epoch0 + (p.now / 1_000_000_000) as i64;
                let (y, m, d) = civil_from_days(t.div_euclid(86400));
                let s = t.rem_euclid(86400);
                format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, s / 3600, (s / 60) % 60, s % 60)
            }
            "bh1750" => format!("{:.0} lux", p.lux),
            "sht31" | "aht20" => format!("{:.1} °C {:.1} %RH", p.temp, p.humidity),
            "bmp280" | "bme280" | "bmp180" | "ms5611" => format!("{:.1} °C {:.2} hPa ({:.0} m)", p.temp, p.pressure, p.altitude()),
            _ => format!("acc=({:.2},{:.2},{:.2})g gyro=({:.0},{:.0},{:.0})°/s", p.accel[0], p.accel[1], p.accel[2], p.gyro[0], p.gyro[1], p.gyro[2]),
        }
    }
}

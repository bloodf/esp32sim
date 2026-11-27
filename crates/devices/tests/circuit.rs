//! Circuit-level tests: YAML project -> Board, driven through the same IoBridge
//! calls the SoC peripherals make.
use devices::config::ProjectConfig;
use esp32_soc::io::{IoBridge, PinDrive};
use serde_json::{json, Value};

fn board(yaml: &str) -> devices::circuit::Board {
    ProjectConfig::parse(yaml).expect("parse").build_board().expect("build")
}

fn state(b: &devices::circuit::Board, id: &str) -> Value {
    let snap = b.snapshot();
    snap["components"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap()["state"].clone()
}

fn drive_out(pins: &[(u8, bool)]) -> PinDrive {
    let mut d = PinDrive::default();
    for &(p, v) in pins {
        d.out_en |= 1 << p;
        if v {
            d.out_level |= 1 << p;
        }
    }
    d
}

#[test]
fn led_with_resistor_current() {
    let mut b = board(
        "components:\n  - {id: r1, type: resistor, value: 220}\n  - {id: led, type: led, color: red}\nconnections:\n  - [esp32.D4, r1.1]\n  - [r1.2, led.A]\n  - [led.C, GND]\n",
    );
    b.gpio_update(0, &drive_out(&[(4, true)]));
    b.advance(1000);
    let ma = state(&b, "led")["current_ma"].as_f64().unwrap();
    // (3.3 - 1.8) / (220 + 30 driver + 10 LED) ~= 5.8 mA
    assert!((5.0..6.5).contains(&ma), "led current {} mA", ma);
    b.gpio_update(2000, &drive_out(&[(4, false)]));
    b.advance(3000);
    assert!(state(&b, "led")["current_ma"].as_f64().unwrap() < 0.01);
}

#[test]
fn led_without_resistor_burns() {
    // straight across the 5 V rail with no resistor
    let mut b = board("components:\n  - {id: led, type: led, color: red}\nconnections:\n  - [5V, led.A]\n  - [led.C, GND]\n");
    b.gpio_update(0, &PinDrive::default());
    b.advance(1000);
    assert_eq!(state(&b, "led")["burned"], json!(true), "{}", state(&b, "led"));
}

#[test]
fn button_with_pullup_reads_low_when_pressed() {
    let mut b = board("components:\n  - {id: btn, type: pushbutton}\n  - {id: r, type: resistor, value: 10k}\nconnections:\n  - [esp32.D15, btn.1, r.1]\n  - [btn.2, GND]\n  - [r.2, 3V3]\n");
    b.gpio_update(0, &PinDrive::default());
    b.advance(10);
    assert_eq!(b.gpio_read(10, 15), Some(true));
    b.input("btn", &json!({"action": "press"})).unwrap();
    assert_eq!(b.gpio_read(20, 15), Some(false));
    b.input("btn", &json!({"action": "release"})).unwrap();
    assert_eq!(b.gpio_read(30, 15), Some(true));
}

#[test]
fn potentiometer_divider_to_adc() {
    let mut b = board("components:\n  - {id: pot, type: potentiometer, value: 10k, position: 0.25}\nconnections:\n  - [pot.1, 3V3]\n  - [pot.2, GND]\n  - [pot.W, esp32.D34]\n");
    b.gpio_update(0, &PinDrive::default());
    b.advance(10);
    let v = b.adc_read(10, 34);
    // position 0 is the pin-1 end: 0.25 -> 75 % of 3.3 V
    assert!((v - 2.475).abs() < 0.05, "wiper {} V", v);
}

#[test]
fn keypad_scan_sees_pressed_key() {
    let mut b = board(
        "components:\n  - {id: kp, type: keypad-4x4}\nconnections:\n  - [esp32.D13, kp.R1]\n  - [esp32.D12, kp.R2]\n  - [esp32.D14, kp.R3]\n  - [esp32.D27, kp.R4]\n  - [esp32.D26, kp.C1]\n  - [esp32.D25, kp.C2]\n  - [esp32.D33, kp.C3]\n  - [esp32.D32, kp.C4]\n",
    );
    b.input("kp", &json!({"key": "6", "action": "press"})).unwrap();
    // columns are inputs with pull-ups; drive row 2 low, others high
    let mut d = drive_out(&[(13, true), (12, false), (14, true), (27, true)]);
    d.pull_up = (1 << 26) | (1 << 25) | (1 << 33) | (1 << 32);
    d.in_en = d.pull_up;
    b.gpio_update(100, &d);
    b.advance(200);
    let cols: Vec<Option<bool>> = [26u8, 25, 33, 32].iter().map(|&p| b.gpio_read(200, p)).collect();
    // "6" is row 2, column 3
    assert_eq!(cols[2], Some(false), "cols {:?}", cols);
    assert_ne!(cols[0], Some(false));
}

#[test]
fn i2c_pcf8574_and_scan() {
    let mut b = board(
        "components:\n  - {id: io, type: pcf8574, address: 0x20}\n  - {id: led, type: led}\n  - {id: r, type: resistor, value: 330}\nconnections:\n  - [esp32.D21, io.SDA]\n  - [esp32.D22, io.SCL]\n  - [io.P0, r.1]\n  - [r.2, led.C]\n  - [led.A, 3V3]\n",
    );
    let (scl, sda) = (Some(22), Some(21));
    // absent address NACKs
    assert!(!b.i2c_start(0, scl, sda, 0x50, false));
    b.i2c_stop(0, scl, sda);
    // write P0 low -> LED (anode at 3V3) sinks into the expander
    assert!(b.i2c_start(10, scl, sda, 0x20, false));
    assert!(b.i2c_write(20, scl, sda, 0xFE));
    b.i2c_stop(30, scl, sda);
    b.advance(100);
    assert!(state(&b, "led")["brightness"].as_f64().unwrap() > 0.2);
    assert!(b.i2c_start(200, scl, sda, 0x20, true));
    assert_eq!(b.i2c_read(210, scl, sda, false), 0xFE);
    b.i2c_stop(220, scl, sda);
}

#[test]
fn i2c_mpu6050_whoami() {
    let mut b = board("components:\n  - {id: imu, type: mpu6050}\nconnections:\n  - [esp32.D21, imu.SDA]\n  - [esp32.D22, imu.SCL]\n");
    let (scl, sda) = (Some(22), Some(21));
    assert!(b.i2c_start(0, scl, sda, 0x68, false));
    b.i2c_write(1, scl, sda, 0x75);
    assert!(b.i2c_start(2, scl, sda, 0x68, true));
    assert_eq!(b.i2c_read(3, scl, sda, false), 0x68);
    b.i2c_stop(4, scl, sda);
}

#[test]
fn every_catalog_part_instantiates() {
    let cat = devices::registry::catalog_json();
    let parts = cat.as_array().expect("catalog parts");
    assert!(parts.len() >= 50, "only {} parts", parts.len());
    for p in parts {
        let t = p["type"].as_str().unwrap();
        let yaml = format!("components:\n  - {{id: x, type: {}}}\nconnections: []\n", t);
        ProjectConfig::parse(&yaml).unwrap().build_board().unwrap_or_else(|e| panic!("{}: {}", t, e));
    }
}

#[test]
fn config_rejects_bad_wiring() {
    let c = ProjectConfig::parse("components:\n  - {id: led, type: led}\nconnections:\n  - [esp32.D99, led.A]\n").unwrap();
    assert!(c.build_board().is_err());
    let c = ProjectConfig::parse("components:\n  - {id: led, type: nosuchpart}\n").unwrap();
    assert!(c.build_board().is_err());
}

/// Bit-banged 1-Wire master driving the board like MicroPython's onewire module.
struct OwHost {
    t: u64,
    pin: u8,
}
impl OwHost {
    fn drive(&mut self, b: &mut devices::circuit::Board, low: bool) {
        let mut d = PinDrive::default();
        d.out_en = 1 << self.pin;
        d.open_drain = 1 << self.pin;
        d.pull_up = 1 << self.pin;
        d.in_en = 1 << self.pin;
        if !low {
            d.out_level = 1 << self.pin;
        }
        b.gpio_update(self.t, &d);
    }
    fn wait(&mut self, us: u64) {
        self.t += us * 1000;
    }
    fn read(&mut self, b: &mut devices::circuit::Board) -> bool {
        b.gpio_read(self.t, self.pin).unwrap_or(true)
    }
    fn reset(&mut self, b: &mut devices::circuit::Board) -> bool {
        self.drive(b, true);
        self.wait(480);
        self.drive(b, false);
        self.wait(40);
        let presence = !self.read(b);
        self.wait(420);
        presence
    }
    fn write_bit(&mut self, b: &mut devices::circuit::Board, bit: bool) {
        self.drive(b, true);
        self.wait(if bit { 5 } else { 60 });
        self.drive(b, false);
        self.wait(if bit { 60 } else { 5 });
    }
    fn read_bit(&mut self, b: &mut devices::circuit::Board) -> bool {
        self.drive(b, true);
        self.wait(2);
        self.drive(b, false);
        self.wait(8);
        let v = self.read(b);
        self.wait(50);
        v
    }
    fn write_byte(&mut self, b: &mut devices::circuit::Board, v: u8) {
        for i in 0..8 {
            self.write_bit(b, v >> i & 1 != 0);
        }
    }
    fn read_byte(&mut self, b: &mut devices::circuit::Board) -> u8 {
        (0..8).fold(0u8, |acc, i| acc | (self.read_bit(b) as u8) << i)
    }
}

#[test]
fn ds18b20_onewire_protocol() {
    let mut b = board("components:\n  - {id: t1, type: ds18b20, temperature: 23.0625}\n  - {id: r, type: resistor, value: 4.7k}\nconnections:\n  - [esp32.D4, t1.DQ, r.1]\n  - [r.2, 3V3]\n  - [GND, t1.GND]\n  - [3V3, t1.VDD]\n");
    let mut h = OwHost { t: 1_000_000, pin: 4 };
    h.drive(&mut b, false);
    h.wait(100);
    assert!(h.reset(&mut b), "presence pulse");
    h.write_byte(&mut b, 0x33); // READ ROM
    let rom: Vec<u8> = (0..8).map(|_| h.read_byte(&mut b)).collect();
    assert_eq!(rom[0], 0x28, "rom {:02x?}", rom);
    assert_eq!(devices::parts::onewire::crc8(&rom[..7]), rom[7]);
    assert!(h.reset(&mut b));
    h.write_byte(&mut b, 0xCC);
    h.write_byte(&mut b, 0x44); // CONVERT T
    h.wait(800_000);
    assert!(h.read_bit(&mut b), "conversion finished");
    assert!(h.reset(&mut b));
    h.write_byte(&mut b, 0xCC);
    h.write_byte(&mut b, 0xBE);
    let sp: Vec<u8> = (0..9).map(|_| h.read_byte(&mut b)).collect();
    assert_eq!(devices::parts::onewire::crc8(&sp[..8]), sp[8], "scratchpad {:02x?}", sp);
    let t = i16::from_le_bytes([sp[0], sp[1]]) as f64 / 16.0;
    assert_eq!(t, 23.0625);
}

#[test]
fn rc_charging_curve() {
    // GPIO4 -> 10k -> node -> 10uF -> GND, node read by the ADC on GPIO34: tau = 0.1 s
    let mut b = board("components:\n  - {id: r, type: resistor, value: 10k}\n  - {id: c, type: capacitor, value: 10u}\nconnections:\n  - [esp32.D4, r.1]\n  - [r.2, c.1, esp32.D34]\n  - [c.2, GND]\n");
    let ms = 1_000_000u64;
    b.gpio_update(0, &drive_out(&[(4, false)]));
    b.advance(10 * ms);
    assert!(b.adc_read(10 * ms, 34) < 0.01);
    b.gpio_update(100 * ms, &drive_out(&[(4, true)]));
    // right after the step the capacitor still holds 0 V
    assert!(b.adc_read(100 * ms, 34) < 0.05);
    let v_tau = b.adc_read(200 * ms, 34) as f64;
    assert!((v_tau - 3.3 * (1.0 - (-1.0f64).exp())).abs() < 0.05, "v(tau) = {}", v_tau);
    let v_5tau = b.adc_read(600 * ms, 34);
    assert!(v_5tau > 3.25, "v(5 tau) = {}", v_5tau);
    // discharge
    b.gpio_update(1000 * ms, &drive_out(&[(4, false)]));
    let v = b.adc_read(1100 * ms, 34) as f64;
    assert!((v - 3.3 * (-1.0f64).exp()).abs() < 0.05, "discharge v(tau) = {}", v);
}

#[test]
fn rc_debounced_button() {
    // 10k pull-up, 100nF to GND, button to GND: release edge rises with tau = 1 ms
    let mut b = board("components:\n  - {id: btn, type: pushbutton}\n  - {id: r, type: resistor, value: 10k}\n  - {id: c, type: capacitor, value: 100n}\nconnections:\n  - [esp32.D15, btn.1, r.1, c.1]\n  - [r.2, 3V3]\n  - [btn.2, GND]\n  - [c.2, GND]\n");
    let us = 1_000u64;
    b.gpio_update(0, &PinDrive::default());
    b.advance(20_000 * us);
    assert_eq!(b.gpio_read(20_000 * us, 15), Some(true));
    b.now = 20_000 * us;
    b.input("btn", &json!({"action": "press"})).unwrap();
    assert_eq!(b.gpio_read(20_100 * us, 15), Some(false), "button discharges the cap quickly");
    b.now = 30_000 * us;
    b.input("btn", &json!({"action": "release"})).unwrap();
    assert_eq!(b.gpio_read(30_300 * us, 15), Some(false), "still low 0.3 ms after release");
    assert_eq!(b.gpio_read(32_000 * us, 15), Some(true), "high after 2 tau");
}

#[test]
fn rc_filter_averages_pwm() {
    let mut b = board("components:\n  - {id: r, type: resistor, value: 10k}\n  - {id: c, type: capacitor, value: 1u}\nconnections:\n  - [esp32.D25, r.1]\n  - [r.2, c.1, esp32.D34]\n  - [c.2, GND]\n");
    b.gpio_update(0, &PinDrive::default());
    b.pwm_update(0, 25, devices::PwmState { freq_hz: 5000.0, duty: 0.25, running: true });
    let v = b.adc_read(100_000_000, 34);
    assert!((v - 0.825).abs() < 0.02, "filtered PWM = {} V", v);
}

#[test]
fn lm393_sensor_module() {
    let mut b = board("components:\n  - {id: soil, type: soil-moisture, moisture: 0.25, threshold: 0.5}\nconnections:\n  - [3V3, soil.VCC]\n  - [GND, soil.GND]\n  - [esp32.D34, soil.AO]\n  - [esp32.D35, soil.DO]\n");
    b.gpio_update(0, &PinDrive::default());
    b.advance(10);
    // dry soil: high analog voltage, comparator idle (DO high)
    assert!((b.adc_read(10, 34) - 0.75 * 3.3).abs() < 0.05);
    assert_eq!(b.gpio_read(10, 35), Some(true));
    b.input("soil", &json!({"moisture": 0.8})).unwrap();
    assert!((b.adc_read(20, 34) - 0.2 * 3.3).abs() < 0.05);
    assert_eq!(b.gpio_read(20, 35), Some(false), "DO pulled low when wet");
}

#[test]
fn touch_pad_on_t0() {
    let mut b = board("components:\n  - {id: pad, type: touch-pad}\nconnections:\n  - [esp32.D4, pad.PAD]\n");
    assert!(!b.touch_read(0, 4));
    b.input("pad", &json!({"action": "touch"})).unwrap();
    assert!(b.touch_read(10, 4));
}

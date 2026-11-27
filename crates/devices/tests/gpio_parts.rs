//! GPIO-level parts: the test plays the ESP32 side (driving pads, reading
//! levels / voltages, generating PWM and RMT waveforms) and decodes the
//! parts' timing protocols the way the Arduino libraries do.
mod common;
use common::*;
use devices::circuit::Board;
use devices::PwmState;
use esp32_soc::io::{IoBridge, PinDrive, RmtItem};
use serde_json::json;

/// Sample `pin` every `step` ns in [from, to) and return (time, level) transitions.
fn edges(b: &mut Board, pin: u8, from: u64, to: u64, step: u64) -> Vec<(u64, bool)> {
    let mut out = Vec::new();
    let mut last = None;
    let mut t = from;
    while t < to {
        b.advance(t);
        let l = b.gpio_read(t, pin).unwrap_or(false);
        if last != Some(l) {
            out.push((t, l));
            last = Some(l);
        }
        t += step;
    }
    out
}

/// Durations of the high (true) or low (false) pulses.
fn pulses(e: &[(u64, bool)], level: bool) -> Vec<u64> {
    e.windows(2).filter(|w| w[0].1 == level).map(|w| w[1].0 - w[0].0).collect()
}

// ---------------------------------------------------------- timing sensors

#[test]
fn hc_sr04_echo_width_follows_distance() {
    for cm in [10.0, 50.0, 200.0] {
        let mut b = board_of(&[&format!("{{id: us, type: hc-sr04, distance: {}}}", cm)], &["esp32.D5 us.TRIG", "esp32.D18 us.ECHO", "5V us.VCC", "GND us.GND"]);
        b.gpio_update(MS, &drive(&[(5, true)]));
        b.gpio_update(MS + 12 * US, &drive(&[(5, false)]));
        let e = edges(&mut b, 18, MS + 12 * US, 40 * MS, 2 * US);
        let high = pulses(&e, true);
        assert_eq!(high.len(), 1, "one echo pulse: {:?}", e);
        let want = cm * 58.0 * 1000.0;
        assert!((high[0] as f64 - want).abs() / want < 0.05, "{} cm: echo {} ns", cm, high[0]);
    }
}

#[test]
fn hc_sr04_ignores_short_trigger() {
    let mut b = board_of(&["{id: us, type: hc-sr04}"], &["esp32.D5 us.TRIG", "esp32.D18 us.ECHO"]);
    b.gpio_update(MS, &drive(&[(5, true)]));
    b.gpio_update(MS + 2 * US, &drive(&[(5, false)]));
    let e = edges(&mut b, 18, MS + 3 * US, 20 * MS, 5 * US);
    assert!(pulses(&e, true).is_empty(), "no echo for a 2 µs trigger");
}

/// DHT11/22 protocol: host start pulse, then 40 bits encoded as high-pulse widths.
fn dht_read(b: &mut Board, start_low_us: u64) -> Vec<u8> {
    let t0 = MS;
    b.gpio_update(t0, &drive(&[(4, false)]));
    let t1 = t0 + start_low_us * US;
    b.gpio_update(t1, &PinDrive::default()); // release: pull-up takes over
    let e = edges(b, 4, t1, t1 + 8 * MS, US);
    let highs = pulses(&e, true);
    // 80 µs response high, then exactly 40 data bits and nothing more
    let resp = highs.iter().position(|&h| h > 75 * US && h < 90 * US).expect("response pulse");
    let bits: Vec<bool> = highs[resp + 1..].iter().map(|&h| h > 50 * US).collect();
    assert_eq!(bits.len(), 40, "exactly one frame: {} bits", bits.len());
    (0..5).map(|k| bits[k * 8..k * 8 + 8].iter().fold(0u8, |a, &x| (a << 1) | x as u8)).collect()
}

#[test]
fn dht22_frame_with_checksum() {
    let mut b = board_of(&["{id: dht, type: dht22, temperature: -7.5, humidity: 61.3}", "{id: r, type: resistor, value: 10k}"], &["esp32.D4 dht.SDA r.1", "r.2 3V3", "3V3 dht.VCC", "GND dht.GND"]);
    let d = dht_read(&mut b, 1_100);
    assert_eq!(d[4], d[0].wrapping_add(d[1]).wrapping_add(d[2]).wrapping_add(d[3]), "checksum {:02x?}", d);
    let h = u16::from_be_bytes([d[0], d[1]]) as f64 / 10.0;
    let traw = u16::from_be_bytes([d[2] & 0x7F, d[3]]) as f64 / 10.0;
    let t = if d[2] & 0x80 != 0 { -traw } else { traw };
    assert_eq!((h, t), (61.3, -7.5));
    assert_eq!(state(&b, "dht")["reads"], json!(1));
}

#[test]
fn dht11_integer_frame() {
    let mut b = board_of(&["{id: dht, type: dht11, temperature: 23, humidity: 40}", "{id: r, type: resistor, value: 10k}"], &["esp32.D4 dht.SDA r.1", "r.2 3V3"]);
    let d = dht_read(&mut b, 20_000);
    assert_eq!((d[0], d[2]), (40, 23), "{:02x?}", d);
    assert_eq!(d[4], d[0].wrapping_add(d[1]).wrapping_add(d[2]).wrapping_add(d[3]));
}

#[test]
fn ir_receiver_sends_nec_frames() {
    let mut b = board_of(&["{id: ir, type: ir-receiver}"], &["esp32.D15 ir.OUT", "3V3 ir.VCC", "GND ir.GND"]);
    b.advance(MS);
    b.input("ir", &json!({"code": 0x45, "address": 0x00})).unwrap();
    let e = edges(&mut b, 15, MS, 120 * MS, 10 * US);
    // from the 9 ms leader on: alternating low / high pulse widths
    let lead = e.iter().position(|&(_, l)| !l).unwrap();
    let w: Vec<u64> = e[lead..].windows(2).map(|x| x[1].0 - x[0].0).collect();
    assert!((w[0] as f64 - 9e6).abs() < 0.2e6, "9 ms leader: {}", w[0]);
    assert!((w[1] as f64 - 4.5e6).abs() < 0.2e6, "4.5 ms space: {}", w[1]);
    let highs: Vec<u64> = w.iter().skip(3).step_by(2).copied().collect();
    let bits: Vec<bool> = highs[..32].iter().map(|&h| h > 1_000_000).collect();
    let byte = |k: usize| bits[k * 8..k * 8 + 8].iter().rev().fold(0u8, |a, &x| (a << 1) | x as u8);
    assert_eq!((byte(0), byte(1), byte(2), byte(3)), (0x00, 0xFF, 0x45, 0xBA), "address, ~address, command, ~command");
    assert_eq!(state(&b, "ir")["frames"], json!(1));
}

#[test]
fn hx711_shifts_out_the_load_cell_reading() {
    let mut b = board_of(&["{id: s, type: hx711, weight: 100, scale: 420, offset: 8400}"], &["esp32.D16 s.DT", "esp32.D4 s.SCK", "3V3 s.VCC", "GND s.GND"]);
    let mut t = 200 * MS;
    b.gpio_update(t, &drive(&[(4, false)]));
    b.advance(t);
    assert_eq!(b.gpio_read(t, 16), Some(false), "DT low = data ready");
    let mut v: u32 = 0;
    for i in 0..25 {
        t += 2 * US;
        b.gpio_update(t, &drive(&[(4, true)]));
        t += 2 * US;
        if i < 24 {
            v = (v << 1) | b.gpio_read(t, 16).unwrap() as u32;
        }
        b.gpio_update(t, &drive(&[(4, false)]));
    }
    let raw = ((v << 8) as i32) >> 8;
    assert_eq!(raw, 8400 + 100 * 420);
}

#[test]
fn rotary_encoder_quadrature_and_switch() {
    let mut b = board_of(&["{id: enc, type: rotary-encoder}"], &["esp32.D32 enc.CLK", "esp32.D33 enc.DT", "esp32.D25 enc.SW", "3V3 enc.VCC", "GND enc.GND"]);
    b.advance(MS);
    b.input("enc", &json!({"steps": 2})).unwrap();
    // decode like the common "CLK falling edge, DT sets the direction" routine
    let mut pos = 0i32;
    let mut last_clk = true;
    let mut t = MS;
    while t < 40 * MS {
        b.advance(t);
        let clk = b.gpio_read(t, 32).unwrap();
        let dt = b.gpio_read(t, 33).unwrap();
        if last_clk && !clk {
            pos += if dt { 1 } else { -1 };
        }
        last_clk = clk;
        t += 100 * US;
    }
    assert_eq!(pos, 2);
    assert_eq!(state(&b, "enc")["position"], json!(2));
    b.input("enc", &json!({"pressed": true})).unwrap();
    let mut d = PinDrive::default();
    d.pull_up |= 1 << 25;
    b.gpio_update(41 * MS, &d);
    assert_eq!(b.gpio_read(42 * MS, 25), Some(false), "switch closes to GND");
}

// -------------------------------------------------------- bit-banged chips

#[test]
fn hc595_shift_and_latch() {
    let mut b = board_of(&["{id: sr, type: 74hc595}"], &["esp32.D23 sr.DS", "esp32.D18 sr.SHCP", "esp32.D5 sr.STCP", "GND sr.OE", "3V3 sr.MR", "3V3 sr.VCC", "GND sr.GND"]);
    let mut t = MS;
    let mut set = |b: &mut Board, ds: bool, sh: bool, st: bool| {
        t += 5 * US;
        b.gpio_update(t, &drive(&[(23, ds), (18, sh), (5, st)]));
    };
    let value = 0b1011_0010u8;
    for i in (0..8).rev() {
        let bit = value >> i & 1 != 0;
        set(&mut b, bit, false, false);
        set(&mut b, bit, true, false);
    }
    assert_eq!(state(&b, "sr")["outputs"], json!(0), "not latched yet");
    set(&mut b, false, false, true);
    set(&mut b, false, false, false);
    assert_eq!(state(&b, "sr")["outputs"], json!(value));
    assert_eq!(b.pad_level(23), Some(false));
}

/// TM1637 two-wire protocol (CLK + open-drain DIO with pull-up).
struct Tm {
    t: u64,
}
impl Tm {
    fn set(&mut self, b: &mut Board, clk: bool, dio: Option<bool>) {
        self.t += 10 * US;
        let mut d = drive(&[(18, clk)]);
        if let Some(v) = dio {
            d.out_en |= 1 << 19;
            if v {
                d.out_level |= 1 << 19;
            }
        }
        b.gpio_update(self.t, &d);
    }
    fn start(&mut self, b: &mut Board) {
        self.set(b, true, Some(true));
        self.set(b, true, Some(false));
    }
    fn stop(&mut self, b: &mut Board) {
        self.set(b, false, Some(false));
        self.set(b, true, Some(false));
        self.set(b, true, Some(true));
    }
    fn byte(&mut self, b: &mut Board, v: u8) -> bool {
        for i in 0..8 {
            let bit = v >> i & 1 != 0;
            self.set(b, false, Some(bit));
            self.set(b, true, Some(bit));
        }
        self.set(b, false, None);
        self.set(b, true, None);
        let ack = b.gpio_read(self.t, 19) == Some(false);
        self.set(b, false, None);
        ack
    }
}

#[test]
fn tm1637_display_protocol() {
    let mut b = board_of(&["{id: disp, type: tm1637}", "{id: r, type: resistor, value: 10k}"], &["esp32.D18 disp.CLK", "esp32.D19 disp.DIO r.1", "r.2 3V3", "3V3 disp.VCC", "GND disp.GND"]);
    let mut tm = Tm { t: MS };
    tm.start(&mut b);
    assert!(tm.byte(&mut b, 0x40), "data command ACK");
    tm.stop(&mut b);
    tm.start(&mut b);
    assert!(tm.byte(&mut b, 0xC0));
    for seg in [0x3F, 0x06, 0x5B, 0x4F] {
        // "0123"
        assert!(tm.byte(&mut b, seg));
    }
    tm.stop(&mut b);
    tm.start(&mut b);
    assert!(tm.byte(&mut b, 0x8A)); // display on, brightness 2
    tm.stop(&mut b);
    let st = state(&b, "disp");
    assert_eq!(st["segments"], json!("3f065b4f"));
    assert_eq!(st["on"], json!(true));
    assert_eq!(st["brightness"], json!(2));
    assert!(summary(&b, "disp").starts_with("[0123]"), "{}", summary(&b, "disp"));
}

#[test]
fn lcd_parallel_4bit_mode() {
    let mut b = board_of(
        &["{id: lcd, type: lcd-parallel}"],
        &["esp32.D12 lcd.RS", "esp32.D13 lcd.E", "esp32.D14 lcd.D4", "esp32.D27 lcd.D5", "esp32.D26 lcd.D6", "esp32.D25 lcd.D7", "GND lcd.RW lcd.VSS", "5V lcd.VDD"],
    );
    let mut t = 50 * MS;
    let mut nib = |b: &mut Board, n: u8, rs: bool| {
        let pins = [(14u8, n & 1 != 0), (27, n & 2 != 0), (26, n & 4 != 0), (25, n & 8 != 0), (12, rs)];
        let mut p = pins.to_vec();
        p.push((13, false));
        t += 2 * US;
        b.gpio_update(t, &drive(&p));
        p.pop();
        p.push((13, true));
        t += 2 * US;
        b.gpio_update(t, &drive(&p));
        p.pop();
        p.push((13, false));
        t += 50 * US;
        b.gpio_update(t, &drive(&p));
    };
    for _ in 0..3 {
        nib(&mut b, 3, false);
    }
    nib(&mut b, 2, false);
    for c in [0x28u8, 0x0C, 0x01, 0x06] {
        nib(&mut b, c >> 4, false);
        nib(&mut b, c & 0xF, false);
    }
    for c in "LiquidCrystal".bytes() {
        nib(&mut b, c >> 4, true);
        nib(&mut b, c & 0xF, true);
    }
    assert_eq!(state(&b, "lcd")["text"][0].as_str().unwrap().trim_end(), "LiquidCrystal");
}

// --------------------------------------------------------- RMT / PWM parts

fn ws2812_items(grb: &[u8]) -> Vec<RmtItem> {
    let mut v = Vec::new();
    for &byte in grb {
        for i in (0..8).rev() {
            let one = byte >> i & 1 != 0;
            v.push(RmtItem { level: true, duration_ns: if one { 800 } else { 400 } });
            v.push(RmtItem { level: false, duration_ns: if one { 450 } else { 850 } });
        }
    }
    v
}

#[test]
fn ws2812_strip_decodes_grb_frames() {
    let mut b = board_of(&["{id: strip, type: ws2812, count: 3}"], &["esp32.D13 strip.DIN", "5V strip.VCC", "GND strip.GND"]);
    // red, green, blue in GRB byte order
    b.rmt_tx(MS, 13, &ws2812_items(&[0x00, 0xFF, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x80]));
    b.advance(2 * MS);
    let st = state(&b, "strip");
    assert_eq!(st["pixels"], json!(["ff0000", "00ff00", "000080"]));
    assert_eq!(st["frames"], json!(1));
    // a shorter second frame only updates the first pixel
    b.rmt_tx(3 * MS, 13, &ws2812_items(&[0x10, 0x20, 0x30]));
    b.advance(4 * MS);
    assert_eq!(state(&b, "strip")["pixels"][0], json!("201030"));
}

#[test]
fn servo_angle_from_pulse_width() {
    let mut b = board_of(&["{id: sv, type: servo}"], &["esp32.D18 sv.PWM", "5V sv.V+", "GND sv.GND"]);
    for (pulse_ms, angle) in [(0.5, 0.0), (1.5, 90.0), (2.5, 180.0), (1.0, 45.0)] {
        b.pwm_update(MS, 18, PwmState { freq_hz: 50.0, duty: pulse_ms / 20.0, running: true });
        let a = state(&b, "sv")["angle"].as_f64().unwrap();
        assert!((a - angle).abs() < 0.5, "{} ms -> {}°", pulse_ms, a);
    }
}

#[test]
fn buzzers_passive_and_active() {
    let mut b = board_of(&["{id: pz, type: buzzer}", "{id: ab, type: buzzer-active}"], &["esp32.D25 pz.+", "pz.- GND", "esp32.D26 ab.+", "ab.- GND"]);
    b.pwm_update(MS, 25, PwmState { freq_hz: 440.0, duty: 0.5, running: true });
    b.advance(2 * MS);
    assert_eq!(state(&b, "pz")["on"], json!(true));
    assert_eq!(state(&b, "pz")["freq"], json!(440.0));
    // a passive buzzer is silent on DC
    b.pwm_update(3 * MS, 25, PwmState { freq_hz: 0.0, duty: 0.0, running: false });
    b.gpio_update(3 * MS, &drive(&[(25, true), (26, true)]));
    b.advance(4 * MS);
    assert_eq!(state(&b, "pz")["on"], json!(false));
    assert_eq!(state(&b, "ab")["on"], json!(true), "active buzzer beeps on DC");
    b.gpio_update(5 * MS, &drive(&[(26, false)]));
    assert_eq!(state(&b, "ab")["on"], json!(false));
}

#[test]
fn relay_switches_a_load() {
    let mut b = board_of(
        &["{id: rl, type: relay}", "{id: lamp, type: led, color: yellow}", "{id: r, type: resistor, value: 150}"],
        &["esp32.D27 rl.IN", "5V rl.VCC rl.COM", "GND rl.GND", "rl.NO r.1", "r.2 lamp.A", "lamp.C GND"],
    );
    b.gpio_update(MS, &drive(&[(27, false)]));
    assert_eq!(state(&b, "rl")["on"], json!([false]));
    assert_eq!(state(&b, "lamp")["brightness"].as_f64().unwrap(), 0.0);
    b.gpio_update(2 * MS, &drive(&[(27, true)]));
    b.advance(3 * MS);
    assert_eq!(state(&b, "rl")["on"], json!([true]));
    assert!(state(&b, "lamp")["brightness"].as_f64().unwrap() > 0.5, "{}", state(&b, "lamp"));
}

#[test]
fn rgb_led_mixes_channels() {
    let mut b = board_of(
        &["{id: rgb, type: rgb-led}", "{id: r1, type: resistor, value: 220}", "{id: r2, type: resistor, value: 220}", "{id: r3, type: resistor, value: 220}"],
        &["esp32.D25 r1.1", "r1.2 rgb.R", "esp32.D26 r2.1", "r2.2 rgb.G", "esp32.D27 r3.1", "r3.2 rgb.B", "rgb.COM GND"],
    );
    b.gpio_update(MS, &drive(&[(25, true), (26, false), (27, true)]));
    b.advance(2 * MS);
    let st = state(&b, "rgb");
    // the blue die (Vf ~3 V) is dimmer than red at 3.3 V through 220 Ω
    assert!(st["r"].as_f64().unwrap() > 0.3 && st["b"].as_f64().unwrap() > 0.1, "{}", st);
    assert!(st["b"].as_f64().unwrap() < st["r"].as_f64().unwrap());
    assert_eq!(st["g"].as_f64().unwrap(), 0.0);
    // PWM dims the green channel proportionally
    b.pwm_update(3 * MS, 26, PwmState { freq_hz: 5000.0, duty: 0.5, running: true });
    b.advance(4 * MS);
    let g = state(&b, "rgb")["g"].as_f64().unwrap();
    let r = state(&b, "rgb")["r"].as_f64().unwrap();
    assert!(g > 0.1 && g < r, "g {} r {}", g, r);
}

#[test]
fn seven_segment_common_cathode() {
    let mut b = board_of(&["{id: seg, type: 7segment}"], &["esp32.D13 seg.A", "esp32.D12 seg.B", "esp32.D14 seg.C", "esp32.D27 seg.D", "esp32.D26 seg.E", "esp32.D25 seg.F", "esp32.D33 seg.G", "seg.COM GND"]);
    // "2": a b d e g
    b.gpio_update(MS, &drive(&[(13, true), (12, true), (14, false), (27, true), (26, true), (25, false), (33, true)]));
    b.advance(2 * MS);
    assert_eq!(state(&b, "seg")["segments"], json!("5b"));
}

// --------------------------------------------------------------- motors

#[test]
fn l298n_direction_and_pwm_speed() {
    let mut b = board_of(&["{id: drv, type: l298n, rpm: 120}"], &["esp32.D25 drv.ENA", "esp32.D26 drv.IN1", "esp32.D27 drv.IN2", "GND drv.GND"]);
    b.gpio_update(MS, &drive(&[(25, true), (26, true), (27, false)]));
    b.advance(2 * MS);
    assert_eq!(state(&b, "drv")["speed"][0], json!(1.0));
    b.gpio_update(3 * MS, &drive(&[(25, true), (26, false), (27, true)]));
    b.advance(4 * MS);
    assert_eq!(state(&b, "drv")["speed"][0], json!(-1.0));
    b.pwm_update(5 * MS, 25, PwmState { freq_hz: 1000.0, duty: 0.5, running: true });
    b.advance(6 * MS);
    let s = state(&b, "drv");
    assert!((s["speed"][0].as_f64().unwrap() + 0.5).abs() < 0.01, "{}", s);
    assert!((s["rpm"][0].as_f64().unwrap() + 60.0).abs() < 0.5);
    // both inputs equal: brake
    b.gpio_update(7 * MS, &drive(&[(25, true), (26, true), (27, true)]));
    b.pwm_update(7 * MS, 25, PwmState { freq_hz: 0.0, duty: 0.0, running: false });
    b.advance(8 * MS);
    assert_eq!(state(&b, "drv")["speed"][0], json!(0.0));
}

#[test]
fn a4988_counts_steps_with_direction() {
    let mut b = board_of(&["{id: st, type: a4988}"], &["esp32.D26 st.STEP", "esp32.D27 st.DIR", "GND st.EN", "3V3 st.SLEEP st.RESET"]);
    let mut t = MS;
    let mut pulse = |b: &mut Board, dir: bool, n: usize| {
        for _ in 0..n {
            t += 500 * US;
            b.gpio_update(t, &drive(&[(26, true), (27, dir)]));
            t += 500 * US;
            b.gpio_update(t, &drive(&[(26, false), (27, dir)]));
        }
    };
    pulse(&mut b, true, 10);
    assert_eq!(state(&b, "st")["position"], json!(10));
    pulse(&mut b, false, 3);
    let s = state(&b, "st");
    assert_eq!(s["position"], json!(7));
    assert!((s["angle"].as_f64().unwrap() - 12.6).abs() < 1e-9);
}

#[test]
fn a4988_disabled_driver_ignores_steps() {
    let mut b = board_of(&["{id: st, type: a4988}"], &["esp32.D26 st.STEP", "esp32.D27 st.DIR", "3V3 st.EN"]);
    b.gpio_update(MS, &drive(&[(26, true)]));
    b.gpio_update(2 * MS, &drive(&[(26, false)]));
    assert_eq!(state(&b, "st")["position"], json!(0));
}

#[test]
fn uln2003_steps_forward_and_back() {
    let mut b = board_of(&["{id: m, type: uln2003}"], &["esp32.D13 m.IN1", "esp32.D12 m.IN2", "esp32.D14 m.IN3", "esp32.D27 m.IN4", "5V m.VCC", "GND m.GND"]);
    let seq = [[1, 0, 0, 0], [1, 1, 0, 0], [0, 1, 0, 0], [0, 1, 1, 0], [0, 0, 1, 0], [0, 0, 1, 1], [0, 0, 0, 1], [1, 0, 0, 1]];
    let pins = [13u8, 12, 14, 27];
    let mut t = MS;
    let mut step = |b: &mut Board, s: &[i32; 4]| {
        t += 2 * MS;
        let d: Vec<(u8, bool)> = pins.iter().zip(s.iter()).map(|(&p, &v)| (p, v == 1)).collect();
        b.gpio_update(t, &drive(&d));
    };
    for _ in 0..2 {
        for s in &seq {
            step(&mut b, s);
        }
    }
    let fwd = state(&b, "m")["position"].as_i64().unwrap();
    assert!(fwd >= 15, "half-steps forward: {}", fwd);
    // currently at seq[7]: walk back through 6, 5, ..., 0, 7
    for s in seq[..7].iter().rev().chain(seq[7..].iter()) {
        step(&mut b, s);
    }
    let back = state(&b, "m")["position"].as_i64().unwrap();
    assert_eq!(fwd - back, 8, "8 half-steps back");
}

// ------------------------------------------------------ inputs & sensors

#[test]
fn keypad_4x3_scanning_finds_hash() {
    let mut b = board_of(
        &["{id: kp, type: keypad-4x3}"],
        &["esp32.D13 kp.R1", "esp32.D12 kp.R2", "esp32.D14 kp.R3", "esp32.D27 kp.R4", "esp32.D26 kp.C1", "esp32.D25 kp.C2", "esp32.D33 kp.C3"],
    );
    b.input("kp", &json!({"key": "#", "action": "press"})).unwrap();
    let rows = [13u8, 12, 14, 27];
    let cols = [26u8, 25, 33];
    let mut found = None;
    for (ri, &r) in rows.iter().enumerate() {
        let mut d = drive(&rows.iter().map(|&p| (p, p != r)).collect::<Vec<_>>());
        for &c in &cols {
            d.pull_up |= 1 << c;
        }
        b.gpio_update(MS * (ri as u64 + 1), &d);
        for (ci, &c) in cols.iter().enumerate() {
            if b.gpio_read(MS * (ri as u64 + 1), c) == Some(false) {
                found = Some((ri, ci));
            }
        }
    }
    assert_eq!(found, Some((3, 2)), "'#' is row 4, column 3");
    assert_eq!(state(&b, "kp")["pressed"], json!(["#"]));
}

#[test]
fn switches_connect_their_contacts() {
    let mut b = board_of(&["{id: sw, type: switch}", "{id: ss, type: slide-switch}"], &["esp32.D4 sw.1", "sw.2 GND", "esp32.D5 ss.C", "ss.1 GND", "ss.2 3V3"]);
    let mut d = PinDrive::default();
    d.pull_up |= 1 << 4;
    b.gpio_update(MS, &d);
    assert_eq!(b.gpio_read(MS, 4), Some(true));
    b.input("sw", &json!({"on": true})).unwrap();
    assert_eq!(b.gpio_read(2 * MS, 4), Some(false));
    let a = b.gpio_read(2 * MS, 5);
    b.input("ss", &json!({"action": "toggle"})).unwrap();
    let c = b.gpio_read(3 * MS, 5);
    assert!(a.is_some() && c.is_some() && a != c, "slide switch changes the common pin: {:?} -> {:?}", a, c);
}

#[test]
fn joystick_axes_and_button() {
    let mut b = board_of(&["{id: js, type: joystick}"], &["3V3 js.VCC", "GND js.GND", "esp32.D34 js.HORZ", "esp32.D35 js.VERT", "esp32.D32 js.SEL"]);
    assert!((b.adc_read(MS, 34) - 1.65).abs() < 0.05, "centred");
    b.input("js", &json!({"x": 1.0, "y": 0.0})).unwrap();
    assert!(b.adc_read(2 * MS, 34) > 3.2);
    assert!(b.adc_read(2 * MS, 35) < 0.1);
    b.input("js", &json!({"pressed": true})).unwrap();
    let mut d = PinDrive::default();
    d.pull_up |= 1 << 32;
    b.gpio_update(3 * MS, &d);
    assert_eq!(b.gpio_read(3 * MS, 32), Some(false));
}

#[test]
fn ldr_and_ntc_voltage_dividers() {
    let mut b = board_of(
        &["{id: ldr, type: ldr, lux: 10}", "{id: r1, type: resistor, value: 10k}", "{id: ntc, type: ntc, temperature: 25}", "{id: r2, type: resistor, value: 10k}"],
        &["3V3 ldr.1", "ldr.2 r1.1 esp32.D34", "r1.2 GND", "3V3 ntc.1", "ntc.2 r2.1 esp32.D35", "r2.2 GND"],
    );
    let dark = b.adc_read(MS, 34);
    b.input("ldr", &json!({"lux": 1000})).unwrap();
    let bright = b.adc_read(2 * MS, 34);
    assert!(bright > dark + 0.5, "more light -> lower LDR resistance: {} -> {}", dark, bright);
    // 10k NTC at 25 °C equals the fixed resistor
    assert!((b.adc_read(3 * MS, 35) - 1.65).abs() < 0.02);
    b.input("ntc", &json!({"temperature": 60})).unwrap();
    let v = b.adc_read(4 * MS, 35) as f64;
    // B3950: R(60 °C) = 10k * exp(3950 * (1/333.15 - 1/298.15)) ~= 2.49k
    let r = 10_000.0 * (3950.0 * (1.0 / 333.15 - 1.0 / 298.15f64)).exp();
    let want = 3.3 * 10_000.0 / (10_000.0 + r);
    assert!((v - want).abs() < 0.05, "{} vs {}", v, want);
}

#[test]
fn pir_holds_its_output() {
    let mut b = board_of(&["{id: pir, type: pir, hold_ms: 500}"], &["esp32.D27 pir.OUT", "5V pir.VCC", "GND pir.GND"]);
    assert_eq!(b.gpio_read(MS, 27), Some(false));
    b.now = 10 * MS;
    b.input("pir", &json!({"action": "trigger"})).unwrap();
    assert_eq!(b.gpio_read(11 * MS, 27), Some(true));
    assert_eq!(b.gpio_read(400 * MS, 27), Some(true));
    assert_eq!(b.gpio_read(600 * MS, 27), Some(false), "released after hold time");
}

#[test]
fn digital_sensor_modules_polarity() {
    // (type, pin name, idle level, active level)
    for (kind, active_high) in [("ir-obstacle", false), ("touch-sensor", true), ("hall-sensor", false), ("vibration-sensor", true), ("tilt-sensor", false)] {
        let mut b = board_of(&[&format!("{{id: s, type: {}}}", kind)], &["esp32.D4 s.OUT", "3V3 s.VCC", "GND s.GND"]);
        let idle = b.gpio_read(MS, 4);
        b.input("s", &json!({"action": "on"})).unwrap();
        let active = b.gpio_read(2 * MS, 4);
        assert_eq!((idle, active), (Some(!active_high), Some(active_high)), "{}", kind);
    }
}

#[test]
fn analog_sensor_modules() {
    for (kind, out) in [("mq2", "AO"), ("sound-sensor", "AO"), ("flame-sensor", "AO"), ("rain-sensor", "AO"), ("water-level", "S")] {
        let mut b = board_of(&[&format!("{{id: s, type: {}}}", kind)], &["3V3 s.VCC", "GND s.GND", &format!("esp32.D34 s.{}", out)]);
        let before = b.adc_read(MS, 34);
        b.input("s", &json!({"level": 0.9})).unwrap();
        let after = b.adc_read(2 * MS, 34);
        assert!((before - after).abs() > 0.1, "{}: {} -> {}", kind, before, after);
        assert!((0.0..=3.31).contains(&after));
    }
}

#[test]
fn signal_source_sets_a_voltage() {
    let mut b = board_of(&["{id: sig, type: signal}"], &["esp32.D36 sig.OUT"]);
    b.input("sig", &json!({"volts": 1.2})).unwrap();
    assert!((b.adc_read(MS, 36) - 1.2).abs() < 0.01);
}

//! Catalog-wide checks: every part type is constructible, self-describing and
//! robust against arbitrary bus traffic, pin voltages and user input (a
//! misbehaving firmware or a malformed GUI request must never crash the
//! simulator).  Also covers aliases, boards, pad names and YAML round-trips.
mod common;
use common::*;
use devices::config::ProjectConfig;
use devices::parts::Props;
use devices::registry::{self, CATALOG};
use devices::{PinView, PwmState, RmtItem};
use serde_json::json;
use std::collections::HashSet;

/// Small deterministic PRNG (xorshift64*).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn defaults(kind: &str) -> Props {
    let e = CATALOG.iter().find(|e| e.kind == kind).unwrap();
    let mut p: Props = serde_json::from_str(e.props).expect("catalog props are valid JSON");
    p.insert("__ephemeral".into(), json!(true));
    p.insert("__id".into(), json!("x"));
    p
}

#[test]
fn catalog_entries_are_unique_and_documented() {
    let mut kinds = HashSet::new();
    for e in CATALOG {
        assert!(kinds.insert(e.kind), "duplicate kind {}", e.kind);
        assert!(!e.name.is_empty() && !e.description.is_empty() && !e.category.is_empty(), "{}", e.kind);
        assert!(serde_json::from_str::<Props>(e.props).is_ok(), "{} props", e.kind);
    }
    assert!(CATALOG.len() >= 85, "{} parts", CATALOG.len());
}

#[test]
fn every_part_has_sane_pins_and_state() {
    for e in CATALOG {
        let d = registry::create(e.kind, &defaults(e.kind)).unwrap_or_else(|err| panic!("{}: {}", e.kind, err));
        assert_eq!(d.kind(), e.kind, "kind() of {}", e.kind);
        let pins = d.pins();
        assert!(!pins.is_empty(), "{} has pins", e.kind);
        let mut names = HashSet::new();
        for p in &pins {
            assert!(names.insert(p.name), "{}: duplicate pin {}", e.kind, p.name);
        }
        // bus pin indices point at real pins
        let n = pins.len();
        if let Some((a, b)) = d.i2c_pins() {
            assert!(a < n && b < n, "{} i2c pins", e.kind);
        }
        if let Some((s, m, mi, cs)) = d.spi_pins() {
            assert!(s < n && m < n && cs < n && mi.map(|x| x < n).unwrap_or(true), "{} spi pins", e.kind);
        }
        if let Some((rx, tx)) = d.uart_pins() {
            assert!(rx < n && tx < n, "{} uart pins", e.kind);
        }
        let st = d.state();
        assert!(serde_json::to_string(&st).is_ok());
        let _ = d.summary();
        if let Some((w, h, rgb)) = d.framebuffer() {
            assert_eq!(rgb.len(), w * h * 3, "{} framebuffer size", e.kind);
        }
    }
}

#[test]
fn catalog_json_lists_pins_for_the_gui() {
    let cat = registry::catalog_json();
    let arr = cat.as_array().unwrap();
    assert_eq!(arr.len(), CATALOG.len());
    for p in arr {
        assert!(p["pins"].as_array().map(|a| !a.is_empty()).unwrap_or(false), "{}", p["type"]);
        for pin in p["pins"].as_array().unwrap() {
            let k = pin["kind"].as_str().unwrap();
            assert!(["power", "ground", "input", "output", "io", "passive", "analog"].contains(&k), "{}", k);
        }
    }
}

#[test]
fn aliases_resolve_to_canonical_parts() {
    for (alias, kind) in [
        ("button", "pushbutton"),
        ("neopixel", "ws2812"),
        ("WS2812B", "ws2812"),
        ("lcd_i2c", "lcd1602-i2c"),
        ("gy-521", "mpu6050"),
        ("gy-271", "hmc5883l"),
        ("rc522", "mfrc522"),
        ("microsd", "sd-card"),
        ("neo-6m", "gps-neo6m"),
        ("hc05", "hc-05"),
        ("28byj-48", "uln2003"),
        ("drv8825", "a4988"),
        ("am2302", "dht22"),
        ("ultrasonic", "hc-sr04"),
        ("pcf8574t", "pcf8574"),
    ] {
        let d = registry::create(alias, &Props::new()).unwrap_or_else(|e| panic!("{}: {}", alias, e));
        assert_eq!(d.kind(), kind, "{}", alias);
    }
    let err = registry::create("flux-capacitor", &Props::new()).err().unwrap();
    assert!(err.contains("unknown component type"), "{}", err);
}

#[test]
fn every_part_survives_random_traffic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for e in CATALOG {
        let kind = e.kind;
        let seed = rng.next();
        let r = std::panic::catch_unwind(move || {
            let mut rng = Rng(seed | 1);
            let mut d = registry::create(kind, &defaults(kind)).unwrap();
            let n = d.pins().len();
            let mut now = 0u64;
            for round in 0..300 {
                now += rng.below(200_000);
                match rng.below(9) {
                    0 => {
                        if d.i2c_start(rng.byte() & 0x7F, rng.below(2) == 0) {
                            for _ in 0..rng.below(40) {
                                if rng.below(2) == 0 {
                                    d.i2c_write(rng.byte());
                                } else {
                                    d.i2c_read(rng.below(2) == 0);
                                }
                            }
                        }
                        d.i2c_stop();
                    }
                    1 => {
                        let mut data: Vec<u8> = (0..rng.below(600)).map(|_| rng.byte()).collect();
                        let levels: Vec<bool> = (0..n).map(|_| rng.below(2) == 0).collect();
                        d.spi_select(true);
                        d.spi_transfer(now, &mut data, &|p| levels.get(p).copied().unwrap_or(false));
                        d.spi_select(false);
                    }
                    2 => {
                        for _ in 0..rng.below(64) {
                            d.uart_receive(now, rng.byte());
                        }
                        for _ in 0..rng.below(64) {
                            let _ = d.uart_transmit(now);
                        }
                    }
                    3 => {
                        let items: Vec<RmtItem> = (0..rng.below(200)).map(|_| RmtItem { level: rng.below(2) == 0, duration_ns: rng.below(100_000) as u32 }).collect();
                        d.waveform(now, rng.below(n as u64) as usize, &items);
                    }
                    4 => {
                        let f = rng.below(50_000) as f64;
                        d.pwm(now, rng.below(n as u64) as usize, PwmState { freq_hz: f, duty: rng.below(1001) as f64 / 1000.0, running: rng.below(3) != 0 });
                    }
                    5 => {
                        let volts: Vec<Option<f64>> = (0..n).map(|_| if rng.below(5) == 0 { None } else { Some(rng.below(5001) as f64 / 1000.0) }).collect();
                        let pwm: Vec<Option<PwmState>> = vec![None; n];
                        let cur = |_t: u32| 0.01;
                        d.on_solved(now, &PinView { volts: &volts, diode_current: &cur, pwm: &pwm });
                        let mut st = Vec::new();
                        d.stamps(&mut st);
                        let _ = d.take_dirty();
                    }
                    6 => {
                        let _ = d.poll(now);
                    }
                    7 => {
                        let junk = [
                            json!(null),
                            json!({}),
                            json!({"action": "press"}),
                            json!({"action": "release"}),
                            json!({"action": "toggle"}),
                            json!({"action": "trigger"}),
                            json!({"action": "tap", "uid": "01020304"}),
                            json!({"value": -1e9}),
                            json!({"value": "NaN"}),
                            json!({"key": "Z", "action": "press"}),
                            json!({"temperature": 1e6, "humidity": -50}),
                            json!({"steps": -5}),
                            json!({"type": "123#*"}),
                            json!({"x": 2.0, "y": -1.0}),
                            json!({"send": "hello"}),
                            json!({"code": 300}),
                            json!([1, 2, 3]),
                            json!("string"),
                        ];
                        let _ = d.input(now, &junk[rng.below(junk.len() as u64) as usize]);
                    }
                    _ => {
                        let _ = d.touched(rng.below(n as u64) as usize);
                        let _ = serde_json::to_string(&d.state()).unwrap();
                        let _ = d.summary();
                        let _ = d.framebuffer();
                    }
                }
                let _ = round;
            }
        });
        assert!(r.is_ok(), "part '{}' panicked under random traffic", kind);
    }
}

#[test]
fn boards_and_pad_names() {
    let b = registry::boards_json();
    let arr = b.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    for board in arr {
        let pads: Vec<i64> = board["left"].as_array().unwrap().iter().chain(board["right"].as_array().unwrap()).map(|p| p["gpio"].as_i64().unwrap()).collect();
        assert!(pads.iter().any(|&g| g == 21) && pads.iter().any(|&g| g == 22), "{} has the I2C pins", board["id"]);
        assert!(pads.iter().any(|&g| g == -1), "has GND");
    }
    for (label, gpio) in [
        ("D4", Some(4)),
        ("GPIO13", Some(13)),
        ("IO0", Some(0)),
        ("P25", Some(25)),
        ("VP", Some(36)),
        ("SVN", Some(39)),
        ("TX2", Some(17)),
        ("RX0", Some(3)),
        ("A0", Some(36)),
        ("T9", Some(32)),
        ("DAC2", Some(26)),
        ("sda", Some(21)),
        ("GPIO20", None),
        ("D29", None),
        ("GPIO40", None),
        ("banana", None),
    ] {
        assert_eq!(registry::pad_number(label), gpio, "{}", label);
    }
    assert_eq!(registry::board_def("nodemcu").id, "nodemcu-32s");
    assert_eq!(registry::board_def("whatever").id, "esp32-devkit-v1");
}

#[test]
fn yaml_roundtrip_of_every_part() {
    let mut y = String::from("name: everything\nboard: esp32-devkitc\ncomponents:\n");
    for (i, e) in CATALOG.iter().enumerate() {
        y.push_str(&format!("  - {{id: p{}, type: {}, x: {}, y: {}}}\n", i, e.kind, i * 10, i * 5));
    }
    y.push_str("connections:\n  - [esp32.IO21, p0.A]\n  - {from: p1.R, to: GND, color: red}\n");
    let cfg = ProjectConfig::parse(&y).unwrap();
    let text = cfg.to_yaml();
    let back = ProjectConfig::parse(&text).unwrap_or_else(|e| panic!("re-parse: {}\n{}", e, text));
    assert_eq!(back.components.len(), CATALOG.len());
    assert_eq!(back.board, "esp32-devkitc");
    for (a, b) in cfg.components.iter().zip(back.components.iter()) {
        assert_eq!((a.id.as_str(), a.kind.as_str()), (b.id.as_str(), b.kind.as_str()));
        assert_eq!(a.props, b.props, "{}", a.id);
    }
    assert_eq!(text, back.to_yaml(), "emitter is stable");
    assert!(text.contains("  - [esp32.IO21, p0.A]"), "simple wires stay compact:\n{}", text);
    // and the whole thing builds into a board
    let b = back.build_board().unwrap();
    assert!(b.comps.len() >= CATALOG.len());
}

#[test]
fn yaml_errors_are_reported() {
    for (y, needle) in [
        ("components:\n  - {id: a, type: led}\n  - {id: a, type: led}\nconnections: []\n", "duplicate"),
        ("components:\n  - {id: esp32, type: led}\nconnections: []\n", "reserved"),
        ("components:\n  - {id: a, type: nosuchpart}\nconnections: []\n", "unknown component type"),
        ("components:\n  - {id: a, type: led}\nconnections:\n  - [a.X, GND]\n", "unknown pin"),
        ("components:\n  - {id: a, type: led}\nconnections:\n  - [b.A, GND]\n", "b"),
        ("components:\n  - {id: a, type: led}\nconnections:\n  - [a.A, esp32.D99]\n", "D99"),
    ] {
        let r = ProjectConfig::parse(y).and_then(|c| c.build_board().map(|_| ()));
        let e = r.err().unwrap_or_else(|| panic!("accepted:\n{}", y));
        assert!(e.contains(needle), "{:?} should mention {:?}", e, needle);
    }
    assert!(ProjectConfig::parse("components: [ {id: a").is_err(), "syntax error");
}

#[test]
fn wiring_helpers_and_nets() {
    let b = board_of(&["{id: r, type: resistor}", "{id: l, type: led}"], &["esp32.D4 r.1", "r.2 l.A", "l.C GND"]);
    let members: Vec<String> = b.nets.iter().map(|n| n.members.join(" ")).collect();
    assert!(members.iter().any(|m| m.contains("r.2") && m.contains("l.A")), "{:?}", members);
    assert!(members.iter().any(|m| m.contains("r.1") && m.contains("esp32")), "{:?}", members);
    // the snapshot reports connected pads (GPIO4 here, GPIO2 = board LED,
    // GPIO0 = BOOT button) and every component
    let snap = b.snapshot();
    for g in ["4", "2", "0"] {
        assert!(snap["pads"].get(g).is_some(), "pad {} in {}", g, snap["pads"]);
    }
    assert!(snap["pads"].get("5").is_none(), "unconnected pads are omitted");
    assert!(snap["components"].as_array().unwrap().iter().any(|c| c["id"] == "r" && c["kind"] == "resistor"));
    // the built-in board LED and BOOT button are always present
    assert!(b.find("boot_button").is_some());
    assert!(b.find("board_led").is_some());
}

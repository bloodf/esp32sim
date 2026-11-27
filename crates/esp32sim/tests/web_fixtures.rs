//! Fixtures shared with the JavaScript unit tests (ui-tests/): the part
//! catalog with pins and default states, and the board definitions.  The
//! JS tests check that the GUI draws every part with exactly these pins.
//!
//! Regenerate after changing parts:  UPDATE_FIXTURES=1 cargo test -p esp32sim --test web_fixtures
use serde_json::{json, Value};

fn catalog_fixture() -> Value {
    let mut parts = Vec::new();
    for e in devices::registry::CATALOG {
        let mut props: devices::parts::Props = serde_json::from_str(e.props).unwrap();
        let pins: Vec<Value> = {
            props.insert("__ephemeral".into(), json!(true));
            let d = devices::registry::create(e.kind, &props).unwrap();
            d.pins().iter().map(|p| json!({"name": p.name, "kind": p.kind})).collect()
        };
        props.remove("__ephemeral");
        let mut p2 = props.clone();
        p2.insert("__ephemeral".into(), json!(true));
        let state = devices::registry::create(e.kind, &p2).unwrap().state();
        parts.push(json!({"type": e.kind, "name": e.name, "category": e.category, "props": props, "pins": pins, "state": state}));
    }
    // pad_number() on many labels: the GUI's padNumber() must agree on all
    let mut labels: Vec<String> = vec![];
    for n in 0..46 {
        for pre in ["", "GPIO", "IO", "D", "G", "P", "gpio", "d"] {
            labels.push(format!("{}{}", pre, n));
        }
    }
    for a in [
        "TX", "TX0", "TXD0", "U0TXD", "RX", "RX0", "RXD0", "U0RXD", "TX2", "U2TXD", "RX2", "U2RXD", "VP", "SVP", "SENSOR_VP", "VN", "SVN", "SENSOR_VN", "SDA", "SCL",
        "MOSI", "MISO", "SCK", "SCLK", "SS", "CS", "LED", "BUILTIN_LED", "LED_BUILTIN", "BOOT", "DAC1", "DAC2", "CLK", "SD0", "SD1", "SD2", "SD3", "CMD", "A0", "A3",
        "A4", "A5", "A6", "A7", "A10", "A11", "A12", "A13", "A14", "A15", "A16", "A17", "A18", "A19", "T0", "T1", "T2", "T3", "T4", "T5", "T6", "T7", "T8", "T9",
        "sda", " d4 ", "EN", "GND", "3V3", "VIN", "5V", "A1", "T10", "GPIOX", "", "banana",
    ] {
        labels.push(a.to_string());
    }
    let pads: serde_json::Map<String, Value> = labels.iter().map(|l| (l.clone(), json!(devices::registry::pad_number(l)))).collect();
    json!({"parts": parts, "boards": devices::registry::boards_json(), "pad_numbers": pads})
}

#[test]
fn web_catalog_fixture_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui-tests/fixtures/catalog.json");
    let text = serde_json::to_string_pretty(&catalog_fixture()).unwrap() + "\n";
    if std::env::var_os("UPDATE_FIXTURES").is_some() || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(on_disk == text, "ui-tests/fixtures/catalog.json is stale: run UPDATE_FIXTURES=1 cargo test -p esp32sim --test web_fixtures");
}

#[test]
fn fixture_is_deterministic() {
    assert_eq!(catalog_fixture(), catalog_fixture());
}

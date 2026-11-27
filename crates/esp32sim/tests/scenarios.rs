//! Full scenarios with REAL firmware images (Arduino-ESP32, MicroPython,
//! esp-hal) booting in the emulator and running the scripted `test:` steps of
//! the example projects.  They take ~15 s (MicroPython boots a full IDF), so
//! they are opt-in:
//!
//!     cargo test --release -p esp32sim --test scenarios -- --ignored
//!     scripts/test.sh scenarios
use std::path::PathBuf;
use std::process::Command;

fn run_example(name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name);
    let out = Command::new(env!("CARGO_BIN_EXE_esp32sim")).arg("test").arg(&dir).output().expect("run esp32sim");
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{} failed:\n{}", name, text);
    assert!(text.contains("PASS"), "{}", text);
}

#[test]
#[ignore = "slow: real firmware (scripts/test.sh scenarios)"]
fn arduino_serial_io_example() {
    run_example("arduino-serial-io");
}

#[test]
#[ignore = "slow: real firmware (scripts/test.sh scenarios)"]
fn micropython_lab_example() {
    run_example("micropython-lab");
}

#[test]
#[ignore = "slow: real firmware (scripts/test.sh scenarios)"]
fn micropython_modules_example() {
    run_example("micropython-modules");
}

#[test]
#[ignore = "slow: real firmware (scripts/test.sh scenarios)"]
fn rust_esp_hal_example() {
    run_example("rust-esp-hal");
}

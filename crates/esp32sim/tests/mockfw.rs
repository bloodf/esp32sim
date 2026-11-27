//! The mock firmware fixture: committed binary in sync with its assembly
//! sources, and its scenario passes through the in-process runner.
use esp32sim::testing::{mock_firmware, write_mock_project, TempDir, MOCK_PROJECT_YAML};

#[test]
fn committed_binary_matches_the_sources() {
    let committed = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mockfw/mockfw.bin")).unwrap();
    assert!(
        committed == mock_firmware().image,
        "tests/fixtures/mockfw/mockfw.bin is stale: run `cargo run -p esp32sim --example mockfw -- crates/esp32sim/tests/fixtures/mockfw`"
    );
}

#[test]
fn firmware_layout() {
    let fw = mock_firmware();
    assert_eq!(fw.image[0], 0xE9);
    assert_eq!(fw.code.symbol("_start"), 0x4008_0400);
    assert!(fw.data.symbols.contains_key("s_ready"));
    // every instruction line is accounted for in the line table
    assert!(fw.code.lines.len() > 150);
    assert!(MOCK_PROJECT_YAML.contains("firmware: mockfw.bin"));
}

#[test]
fn scenario_passes_in_process() {
    let d = TempDir::new("mockfw-run");
    write_mock_project(d.path()).unwrap();
    assert_eq!(esp32sim::testrun::run(d.path(), 30.0, false), Ok(0));
}

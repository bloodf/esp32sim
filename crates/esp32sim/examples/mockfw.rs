//! Write the mock firmware project (esp32sim.yaml + mockfw.bin) to a folder:
//!
//!     cargo run -p esp32sim --example mockfw -- <dir>
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "mockfw-project".into());
    let p = esp32sim::testing::write_mock_project(std::path::Path::new(&dir)).expect("write project");
    println!("{}", p.display());
}

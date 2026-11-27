//! Test support: the `mockfw` firmware (Xtensa assembly assembled with
//! `xtensa_core::asm`) and helpers to create throw-away projects.  Used by
//! the integration tests, the Playwright GUI tests and `--example mockfw`.
use std::path::{Path, PathBuf};

const CODE_SRC: &str = include_str!("../tests/fixtures/mockfw/mockfw_code.s");
const DATA_SRC: &str = include_str!("../tests/fixtures/mockfw/mockfw_data.s");
/// The fixture project file (circuit + scenario) that goes with the firmware.
pub const MOCK_PROJECT_YAML: &str = include_str!("../tests/fixtures/mockfw/esp32sim.yaml");

pub const CODE_ADDR: u32 = 0x4008_0400;
pub const DATA_ADDR: u32 = 0x3FFB_1000;

/// Assembled mock firmware: app image plus the symbol table (for debugger tests).
pub struct MockFirmware {
    pub image: Vec<u8>,
    pub code: xtensa_core::Program,
    pub data: xtensa_core::Program,
}

/// Assemble the mock firmware into an ESP32 application image.
pub fn mock_firmware() -> MockFirmware {
    let data = xtensa_core::assemble(DATA_SRC, DATA_ADDR).expect("mockfw data");
    let rom = esp32_soc::rom::RomTable::load();
    let mut pre = String::new();
    for name in ["ets_delay_us", "software_reset"] {
        pre.push_str(&format!(".equ {}, 0x{:08x}\n", name, rom.by_name.get(name).expect("ROM symbol")));
    }
    let mut syms: Vec<_> = data.symbols.iter().collect();
    syms.sort();
    for (k, v) in syms {
        pre.push_str(&format!(".equ {}, 0x{:08x}\n", k, v));
    }
    let code = xtensa_core::assemble(&format!("{}\n{}", pre, CODE_SRC), CODE_ADDR).expect("mockfw code");
    let image = esp32_soc::elf::build_app_image(code.symbol("_start"), &[(CODE_ADDR, code.bytes.clone()), (DATA_ADDR, data.bytes.clone())]);
    MockFirmware { image, code, data }
}

/// Write the mock project (esp32sim.yaml + mockfw.bin) into `dir`.
pub fn write_mock_project(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("mockfw.bin"), mock_firmware().image)?;
    let p = dir.join("esp32sim.yaml");
    std::fs::write(&p, MOCK_PROJECT_YAML)?;
    Ok(p)
}

/// A temporary directory removed on drop (no external crate needed).
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("esp32sim-{}-{}-{}", tag, std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("temp dir");
        TempDir(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

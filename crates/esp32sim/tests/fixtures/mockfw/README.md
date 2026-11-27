# mockfw fixture

A tiny firmware written in Xtensa assembly (`mockfw_code.s` + `mockfw_data.s`)
and assembled with the project's own assembler (`xtensa_core::asm`), so the
end-to-end tests need no ESP32 toolchain and run in milliseconds of CPU time.

`mockfw.bin` is generated from the sources; regenerate it with

    cargo run -p esp32sim --example mockfw -- crates/esp32sim/tests/fixtures/mockfw

(`tests/mockfw.rs` fails if the committed binary is out of date).
The same project is copied by the Playwright GUI tests (`e2e/`).

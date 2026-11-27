# rust-esp-hal

Firmware written in Rust with [esp-hal](https://github.com/esp-rs/esp-hal) (bare
metal, no ESP-IDF). Both ELF files come from the test resources of the
[espflash](https://crates.io/crates/espflash) crate (MIT / Apache-2.0):

* `blinky.elf`: `esp32_hal_blinky`, which toggles GPIO15 every 500 ms. This is
  what `esp32sim.yaml` runs and tests.
* `hello.elf`: an esp-hal 0.15 "hello world" with full DWARF debug info. It
  prints `Hello world!` and then `Loop...` every 500 ms. The debugger tests use
  it to check breakpoints by `main.rs:<line>` against real rustc output:

      esp32sim run hello.elf

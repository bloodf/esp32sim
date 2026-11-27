# esp32sim desktop app (Tauri)

The same emulator, circuit editor and IDE as `esp32sim gui`, in a native
window instead of a browser tab. The engine runs in-process (the GUI server
on a free loopback port); the desktop shell adds:

* native menus — File (New / Open Project Folder / Open Firmware / Save All /
  Settings / Quit), Edit, Simulation (Build & Run, Run, Pause, Reset, Reload,
  Erase flash data), View (zoom, reload, open in a web browser, developer
  tools in debug builds), Help (About);
* native folder / file pickers: the **Browse…** buttons in the New / Open
  project dialogs appear only in the desktop app;
* a clean shutdown (the simulator saves NVS / file-system flash data).

Settings, recent projects and AI keys are shared with the command-line tool
(`~/.config/esp32sim/settings.json`).

## Prerequisites

* Rust 1.90 or newer
* **Linux**: the WebKitGTK development packages, e.g. on Debian / Ubuntu

      sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
        libjavascriptcoregtk-4.1-dev libxdo-dev libssl-dev build-essential

* **Windows 10/11**: the MSVC build tools; the WebView2 runtime is already installed
* **macOS**: the Xcode command line tools

## Run

    cargo run --release --manifest-path desktop/src-tauri/Cargo.toml -- examples/micropython-lab

The argument (optional) is a project folder, an `esp32sim.yaml` or a firmware
`.bin` / `.elf`; without it the app opens the most recent project.

## Installers (.deb / .AppImage / .msi / .dmg)

    cargo install tauri-cli --version '^2' --locked
    cd desktop/src-tauri && cargo tauri build

The bundles are written to `desktop/src-tauri/target/release/bundle/`.
(`.github/workflows/ci.yml` builds them for Linux, Windows and macOS.)

## Tests

The shell is tested on Tauri's mock runtime (no window or WebView needed):
the engine starts with the native hooks, the main window points at it, every
menu entry exists and works, projects can be switched and shutdown closes the
engine.

    cargo test --manifest-path desktop/src-tauri/Cargo.toml --no-default-features --features mock

(On Linux this still links against GTK 3, so the packages above are needed.)

This folder is its own Cargo workspace, so building or testing the main
workspace never requires the WebView packages.

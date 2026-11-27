//! esp32sim — ESP32 board emulator with circuit simulation, console, GUI and IDE.
//!
//! The binary (`src/main.rs`) is a thin command-line wrapper; everything is
//! available as a library so the desktop (Tauri) shell and the integration
//! tests can start the GUI server, run scenarios or drive the simulator
//! in-process.

pub mod ai;
pub mod cli;
pub mod console;
pub mod debug;
pub mod ide;
pub mod locals;
pub mod project;
pub mod server;
pub mod sim;
pub mod testing;
pub mod testrun;

pub use server::{NativeHooks, ServerHandle, ServerOptions};

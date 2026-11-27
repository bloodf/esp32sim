//! ESP32 system-on-chip emulator.
//!
//! * `xtensa_core` provides the CPU cores.
//! * [`sys::Sys`] implements memories and peripherals.
//! * [`machine::Machine`] boots firmware images and schedules both cores.
//! * [`io::IoBridge`] connects the chip to the simulated circuit.

pub mod elf;
pub mod flash;
pub mod hle;
pub mod io;
pub mod machine;
pub mod periph;
pub mod rom;
pub mod sys;

pub use machine::{Machine, MachineConfig, RunState};

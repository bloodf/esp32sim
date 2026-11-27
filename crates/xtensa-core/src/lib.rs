//! Xtensa LX6 CPU core used by the ESP32 emulator.
pub mod asm;
pub mod cpu;
pub mod decode;
pub mod disasm;

pub use asm::{assemble, AsmError, Program};
pub use cpu::{Bus, Cpu, MemFault, StepResult};
pub use decode::{decode, insn_len, Insn, Op};
pub use disasm::{disasm, format_insn};

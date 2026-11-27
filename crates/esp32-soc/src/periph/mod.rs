//! ESP32 peripheral models.

pub mod dport;
pub mod efuse;
pub mod frc;
pub mod gpio;
pub mod i2c;
pub mod ledc;
pub mod rmt;
pub mod rtc;
pub mod spi;
pub mod timg;
pub mod uart;

use crate::io::IoBridge;
use std::collections::HashMap;

/// Peripheral interrupt sources (interrupt matrix inputs).
pub mod src {
    pub const TG0_T0_LEVEL: usize = 14;
    pub const TG0_T1_LEVEL: usize = 15;
    pub const TG0_WDT_LEVEL: usize = 16;
    pub const TG0_LACT_LEVEL: usize = 17;
    pub const TG1_T0_LEVEL: usize = 18;
    pub const TG1_T1_LEVEL: usize = 19;
    pub const TG1_WDT_LEVEL: usize = 20;
    pub const TG1_LACT_LEVEL: usize = 21;
    pub const GPIO: usize = 22;
    pub const GPIO_NMI: usize = 23;
    pub const FROM_CPU_0: usize = 24;
    pub const SPI0: usize = 28;
    pub const SPI1: usize = 29;
    pub const SPI2: usize = 30;
    pub const SPI3: usize = 31;
    pub const UART0: usize = 34;
    pub const UART1: usize = 35;
    pub const UART2: usize = 36;
    pub const LEDC: usize = 43;
    pub const RTC_CORE: usize = 46;
    pub const RMT: usize = 47;
    pub const PCNT: usize = 48;
    pub const I2C_EXT0: usize = 49;
    pub const I2C_EXT1: usize = 50;
    pub const SPI1_DMA: usize = 52;
    pub const SPI2_DMA: usize = 53;
    pub const SPI3_DMA: usize = 54;
    pub const FRC1: usize = 56;
    pub const FRC2: usize = 57;
    pub const TG0_T0_EDGE: usize = 58;
    pub const TG0_LACT_EDGE: usize = 61;
    pub const TG1_T0_EDGE: usize = 62;
    pub const NUM: usize = 69;
}

/// Interrupt source levels and matrix state.
pub struct Irq {
    pub sources: u128,
    /// map[cpu][source] = CPU interrupt number
    pub map: [[u8; src::NUM]; 2],
    pub dirty: bool,
    /// GPIO interrupt source is per-CPU (PRO/APP enable bits in GPIO_PINn)
    pub gpio_cpu: [bool; 2],
}

impl Irq {
    pub fn new() -> Irq {
        // reset value of the map registers is 16 (an internal timer interrupt -> effectively unmapped)
        Irq { sources: 0, map: [[16; src::NUM]; 2], dirty: true, gpio_cpu: [false; 2] }
    }
    #[inline]
    pub fn set(&mut self, source: usize, level: bool) {
        let bit = 1u128 << source;
        let old = self.sources & bit != 0;
        if old != level {
            if level {
                self.sources |= bit;
            } else {
                self.sources &= !bit;
            }
            self.dirty = true;
        }
    }
    /// Compute the external interrupt lines seen by `cpu`.
    pub fn lines(&self, cpu: usize) -> u32 {
        let mut lines = 0u32;
        let mut s = self.sources;
        while s != 0 {
            let i = s.trailing_zeros() as usize;
            s &= s - 1;
            if i == src::GPIO && !self.gpio_cpu[cpu] {
                continue;
            }
            if i < src::NUM {
                let n = self.map[cpu][i] as u32;
                let internal = matches!(n, 6 | 7 | 11 | 15 | 16 | 29);
                if !internal && n < 32 {
                    lines |= 1 << n;
                }
            }
        }
        lines
    }
}

/// Fallback register storage for registers without a dedicated model.
#[derive(Default)]
pub struct RegFile {
    pub regs: HashMap<u32, u32>,
}

impl RegFile {
    pub fn read(&self, addr: u32) -> u32 {
        *self.regs.get(&addr).unwrap_or(&0)
    }
    pub fn write(&mut self, addr: u32, v: u32) {
        self.regs.insert(addr, v);
    }
}

/// Context passed to peripherals during register accesses and ticks.
pub struct Ctx<'a> {
    pub now: u64,
    pub apb_hz: u64,
    pub cpu_hz: u64,
    pub io: &'a mut dyn IoBridge,
    pub irq: &'a mut Irq,
}

/// Helpers for converting between time and counter ticks.
pub fn ns_to_ticks(ns: u64, hz: u64) -> u64 {
    ((ns as u128 * hz as u128) / 1_000_000_000u128) as u64
}
pub fn ticks_to_ns(ticks: u64, hz: u64) -> u64 {
    if hz == 0 {
        return u64::MAX;
    }
    ((ticks as u128 * 1_000_000_000u128 + hz as u128 - 1) / hz as u128) as u64
}

/// Merge a sub-word write into a 32-bit register value.
pub fn merge(old: u32, addr: u32, size: u8, val: u32) -> u32 {
    match size {
        4 => val,
        2 => {
            let sh = (addr & 2) * 8;
            (old & !(0xFFFF << sh)) | ((val & 0xFFFF) << sh)
        }
        _ => {
            let sh = (addr & 3) * 8;
            (old & !(0xFF << sh)) | ((val & 0xFF) << sh)
        }
    }
}

//! RTC_CNTL (RTC control / clocks / reset) and SENS (SAR ADC, DAC).

use super::{Ctx, RegFile};

pub const OPTIONS0: u32 = 0x00;
pub const TIME_UPDATE: u32 = 0x0C;
pub const TIME0: u32 = 0x10;
pub const TIME1: u32 = 0x14;
pub const RESET_STATE: u32 = 0x34;
pub const INT_RAW: u32 = 0x40;
pub const INT_CLR: u32 = 0x48;
pub const STATE0: u32 = 0x18;
pub const SLP_TIMER0: u32 = 0x04;
pub const SLP_TIMER1: u32 = 0x08;
pub const WAKEUP_STATE: u32 = 0x38;
pub const EXT_WAKEUP_CONF: u32 = 0x60;
pub const DIG_PWC: u32 = 0x84;
pub const EXT_WAKEUP1: u32 = 0xCC;
pub const EXT_WAKEUP1_STATUS: u32 = 0xD0;
/// RTC GPIO number -> GPIO
pub const RTC_GPIO: [u8; 18] = [36, 37, 38, 39, 34, 35, 25, 26, 33, 32, 4, 0, 2, 15, 13, 12, 14, 27];
pub const CLK_CONF: u32 = 0x70;
pub const SW_CPU_STALL: u32 = 0xAC;
pub const STORE4: u32 = 0xB0;
pub const STORE5: u32 = 0xB4;

/// ADC1 channel -> GPIO
pub const ADC1_GPIO: [u8; 8] = [36, 37, 38, 39, 32, 33, 34, 35];
/// ADC2 channel -> GPIO
pub const ADC2_GPIO: [u8; 10] = [4, 0, 2, 15, 13, 12, 14, 27, 25, 26];

pub struct Rtc {
    pub regs: RegFile,
    pub sens: RegFile,
    pub rtc_i2c: RegFile,
    pub latched_time: u64,
    pub slow_hz: u64,
    /// set when software requests a system reset
    pub reset_request: Option<u32>,
    pub reset_reason: u32,
    /// software set RTC_CNTL_STATE0.SLEEP_EN (light or deep sleep)
    pub sleep_request: bool,
}

impl Rtc {
    pub fn new() -> Rtc {
        let mut regs = RegFile::default();
        regs.write(OPTIONS0, 0x1C49_2000);
        regs.write(RESET_STATE, 1 | (1 << 6) | (1 << 12) | (1 << 13));
        regs.write(CLK_CONF, (1 << 27) | (3 << 12) | (172 << 17) | (1 << 10) | (1 << 9));
        regs.write(STORE4, 0x0028_0028); // XTAL 40 MHz
        let apb = 80_000_000u32 >> 12;
        regs.write(STORE5, apb | (apb << 16));
        regs.write(0x7C, 0x0B00_0000 | (4 << 22) | (4 << 25)); // RTC_CNTL_REG: dig dbias etc.
        regs.write(0x13C, 0x1604280);
        let mut sens = RegFile::default();
        sens.write(0x00, 0x0703_0002 | (3 << 16));
        sens.write(0x90, 3 << 16);
        // attenuation registers reset to 11 dB on every channel
        sens.write(0x34, 0xFFFF_FFFF);
        sens.write(0x38, 0xFFFF_FFFF);
        sens.write(0xFC, 0x1605240);
        Rtc {
            regs,
            sens,
            rtc_i2c: RegFile::default(),
            latched_time: 0,
            slow_hz: 150_000,
            reset_request: None,
            reset_reason: 1,
            sleep_request: false,
        }
    }

    /// Whether `cpu` is stalled through the RTC stall registers.
    pub fn cpu_stalled(&self, cpu: usize) -> bool {
        let o = self.regs.read(OPTIONS0);
        let s = self.regs.read(SW_CPU_STALL);
        if cpu == 1 {
            o & 3 == 2 && (s >> 20) & 0x3F == 0x21
        } else {
            (o >> 2) & 3 == 2 && (s >> 26) & 0x3F == 0x21
        }
    }

    pub fn soc_clk_sel(&self) -> u32 {
        (self.regs.read(CLK_CONF) >> 27) & 3
    }

    pub fn read(&mut self, off: u32, ctx: &mut Ctx) -> u32 {
        let _ = ctx;
        match off {
            TIME_UPDATE => self.regs.read(off) | (1 << 30),
            TIME0 => self.latched_time as u32,
            TIME1 => (self.latched_time >> 32) as u32 & 0xFFFF,
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, val: u32, ctx: &mut Ctx) {
        match off {
            OPTIONS0 => {
                if val & (1 << 31) != 0 {
                    self.reset_request = Some(0x0C); // SW_CPU_RESET / RTC_SW_SYS_RESET
                }
                if val & (1 << 5) != 0 {
                    self.reset_request = Some(0x0C);
                }
                self.regs.write(off, val & !((1 << 31) | (1 << 5) | (1 << 4)));
            }
            STATE0 => {
                if val & (1 << 31) != 0 {
                    self.sleep_request = true;
                }
                self.regs.write(off, val);
            }
            INT_CLR => {
                let raw = self.regs.read(INT_RAW);
                self.regs.write(INT_RAW, raw & !val);
            }
            TIME_UPDATE => {
                if val & (1 << 31) != 0 {
                    self.latched_time = ((ctx.now as u128 * self.slow_hz as u128) / 1_000_000_000u128) as u64;
                }
                self.regs.write(off, val & !(1 << 31));
            }
            _ => self.regs.write(off, val),
        }
    }

    // ------------------------------------------------------------ SENS
    fn adc_convert(&mut self, unit: usize, ctx: &mut Ctx) -> u32 {
        let (start_reg, ctrl_reg, atten_reg, gpios): (u32, u32, u32, &[u8]) = if unit == 1 {
            (0x54, 0x00, 0x34, &ADC1_GPIO)
        } else {
            (0x94, 0x90, 0x38, &ADC2_GPIO)
        };
        let st = self.sens.read(start_reg);
        let pads = (st >> 19) & 0xFFF;
        let ch = if pads == 0 { 0 } else { pads.trailing_zeros() as usize };
        let gpio = *gpios.get(ch).unwrap_or(&gpios[0]);
        let atten = (self.sens.read(atten_reg) >> (2 * ch)) & 3;
        let vfs = [1.1f32, 1.5, 2.2, 3.3][atten as usize];
        let ctrl = self.sens.read(ctrl_reg);
        let bits = 9 + ((ctrl >> 16) & 3);
        let max = (1u32 << bits) - 1;
        let v = ctx.io.adc_read(ctx.now, gpio);
        let mut raw = ((v / vfs).clamp(0.0, 1.0) * max as f32).round() as u32;
        // SARx_DATA_INV set -> the controller inverts its output
        let inv_bit = if unit == 1 { 28 } else { 29 };
        if ctrl & (1 << inv_bit) != 0 {
            raw = max - raw;
        }
        if std::env::var("DEBUG_ADC").is_ok() {
            eprintln!("adc{} start={:08x} ch={} gpio={} atten={} ctrl={:08x} v={:.3} raw={}", unit, st, ch, gpio, atten, ctrl, v, raw);
        }
        raw
    }

    pub fn sens_read(&mut self, off: u32) -> u32 {
        match off {
            0x3C => self.sens.read(off) & !(0xFF << 22),
            _ => self.sens.read(off),
        }
    }

    pub fn sens_write(&mut self, off: u32, val: u32, ctx: &mut Ctx) {
        match off {
            0x54 | 0x94 => {
                let old = self.sens.read(off);
                let mut v = (val & !0x1FFFF) | (old & 0xFFFF);
                let starting = val & (1 << 17) != 0 && old & (1 << 17) == 0;
                if val & (1 << 17) == 0 {
                    v &= !(1 << 16);
                }
                self.sens.write(off, v);
                if starting {
                    let raw = self.adc_convert(if off == 0x54 { 1 } else { 2 }, ctx);
                    let v2 = (v & !0xFFFF) | raw | (1 << 16);
                    self.sens.write(off, v2);
                }
            }
            _ => self.sens.write(off, val),
        }
    }
}

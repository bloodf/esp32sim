//! GPIO matrix, IO_MUX and RTC_IO pads.

use super::{src, Ctx, RegFile};
use crate::io::{PinDrive, NUM_GPIO};

/// IO_MUX register offset for each GPIO (0 = no pad).
pub const IOMUX_OFF: [u32; NUM_GPIO] = [
    0x44, 0x88, 0x40, 0x84, 0x48, 0x6C, 0x60, 0x64, 0x68, 0x54, 0x58, 0x5C, 0x34, 0x38, 0x30, 0x3C, 0x4C, 0x50, 0x70, 0x74,
    0x78, 0x7C, 0x80, 0x8C, 0x90, 0x24, 0x28, 0x2C, 0, 0, 0, 0, 0x1C, 0x20, 0x14, 0x18, 0x04, 0x08, 0x0C, 0x10,
];

/// RTC IO index for each GPIO (-1 = not an RTC pad).
pub const RTC_NUM: [i8; NUM_GPIO] = [
    11, -1, 12, -1, 10, -1, -1, -1, -1, -1, -1, -1, 15, 14, 16, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, 6, 7, 17, -1, -1,
    -1, -1, 9, 8, 4, 5, 0, 1, 2, 3,
];

/// Description of an RTC pad: (register offset in RTC_IO, mux_sel bit, rue bit, rde bit, fun_ie bit)
pub fn rtc_pad(gpio: usize) -> Option<(u32, u32, u32, u32, u32)> {
    Some(match gpio {
        36 => (0x7C, 27, 99, 99, 19),
        37 => (0x7C, 26, 99, 99, 14),
        38 => (0x7C, 25, 99, 99, 9),
        39 => (0x7C, 24, 99, 99, 4),
        34 => (0x80, 29, 99, 99, 23),
        35 => (0x80, 28, 99, 99, 18),
        25 => (0x84, 17, 27, 28, 11),
        26 => (0x88, 17, 27, 28, 11),
        33 => (0x8C, 18, 27, 28, 11),
        32 => (0x8C, 17, 22, 23, 5),
        4 => (0x94, 19, 27, 28, 13),
        0 => (0x98, 19, 27, 28, 13),
        2 => (0x9C, 19, 27, 28, 13),
        15 => (0xA0, 19, 27, 28, 13),
        13 => (0xA4, 19, 27, 28, 13),
        12 => (0xA8, 19, 27, 28, 13),
        14 => (0xAC, 19, 27, 28, 13),
        27 => (0xB0, 19, 27, 28, 13),
        _ => return None,
    })
}

pub const SIG_GPIO: u32 = 0x100;

pub struct Gpio {
    pub out: u64,
    pub enable: u64,
    pub pin: [u32; NUM_GPIO],
    pub in_sel: Vec<u32>,
    pub out_sel: [u32; NUM_GPIO],
    pub status: u64,
    /// interrupt-enabled pins at the previous tick
    last_int_mask: u64,
    pub iomux: [u32; NUM_GPIO],
    pub iomux_ctrl: u32,
    pub rtc: RegFile,
    pub rtc_out: u32,
    pub rtc_enable: u32,
    pub last_in: u64,
    pub drive: PinDrive,
    pub drive_dirty: bool,
    pub strap: u32,
}

impl Gpio {
    pub fn new() -> Gpio {
        let mut iomux = [0u32; NUM_GPIO];
        for (g, v) in iomux.iter_mut().enumerate() {
            if IOMUX_OFF[g] == 0 {
                continue;
            }
            // reset: function 0 for flash/uart pads, GPIO (2) otherwise; input enabled
            *v = match g {
                1 | 3 | 6..=11 => (0 << 12) | (1 << 9) | (2 << 10),
                _ => (2 << 12) | (1 << 9) | (2 << 10),
            };
            if matches!(g, 1 | 3 | 6..=11) {
                *v |= 1 << 8; // pull-up on uart/flash pads
            }
        }
        let mut rtc = RegFile::default();
        for g in 0..NUM_GPIO {
            if let Some((off, _mux, _rue, rde, _ie)) = rtc_pad(g) {
                let _ = rde;
                let v = rtc.read(off);
                rtc.write(off, v);
            }
        }
        Gpio {
            out: 0,
            enable: 0,
            pin: [0; NUM_GPIO],
            in_sel: vec![0x30; 256],
            out_sel: [SIG_GPIO; NUM_GPIO],
            status: 0,
            last_int_mask: 0,
            iomux,
            iomux_ctrl: 0x3FF,
            rtc,
            rtc_out: 0,
            rtc_enable: 0,
            last_in: 0,
            drive: PinDrive { out_signal: [SIG_GPIO as u16; NUM_GPIO], ..Default::default() },
            drive_dirty: true,
            strap: 0x13, // SPI boot, GPIO0 high
        }
    }

    #[allow(dead_code)]
    fn rtc_muxed(&self, g: usize) -> bool {
        match rtc_pad(g) {
            Some((off, mux, ..)) => self.rtc.read(off) & (1 << mux) != 0,
            None => false,
        }
    }

    /// Recompute the pad drive state.
    pub fn compute_drive(&self) -> PinDrive {
        let mut d = PinDrive { out_signal: [SIG_GPIO as u16; NUM_GPIO], ..Default::default() };
        d.uart_rx_pads = [None; 3];
        for g in 0..NUM_GPIO {
            if IOMUX_OFF[g] == 0 {
                continue;
            }
            let bit = 1u64 << g;
            let mux = self.iomux[g];
            let func = (mux >> 12) & 7;
            let mut pu = mux & (1 << 8) != 0;
            let mut pd = mux & (1 << 7) != 0;
            let mut ie = mux & (1 << 9) != 0;
            if let Some((off, muxb, rue, rde, fie)) = rtc_pad(g) {
                let r = self.rtc.read(off);
                // On ESP32 pulls of RTC pads are controlled by RTC_IO
                pu = rue < 32 && r & (1 << rue) != 0;
                pd = rde < 32 && r & (1 << rde) != 0;
                if r & (1 << muxb) != 0 {
                    // pad routed to RTC domain
                    ie = r & (1 << fie) != 0;
                    let rn = RTC_NUM[g] as u32;
                    if self.rtc_enable & (1 << (rn + 14)) != 0 {
                        d.out_en |= bit;
                        if self.rtc_out & (1 << (rn + 14)) != 0 {
                            d.out_level |= bit;
                        }
                    }
                    if self.rtc.read(0x28 + 4 * rn) & (1 << 2) != 0 {
                        d.open_drain |= bit;
                    }
                    if pu {
                        d.pull_up |= bit;
                    }
                    if pd {
                        d.pull_down |= bit;
                    }
                    if ie {
                        d.in_en |= bit;
                    }
                    continue;
                }
            }
            if pu {
                d.pull_up |= bit;
            }
            if pd {
                d.pull_down |= bit;
            }
            if ie {
                d.in_en |= bit;
            }
            if self.pin[g] & (1 << 2) != 0 {
                d.open_drain |= bit;
            }
            if func == 2 {
                let os = self.out_sel[g];
                let sig = os & 0x1FF;
                let inv = os & (1 << 9) != 0;
                d.out_signal[g] = sig as u16;
                if sig == SIG_GPIO {
                    if self.enable & bit != 0 {
                        d.out_en |= bit;
                    }
                    if (self.out & bit != 0) ^ inv {
                        d.out_level |= bit;
                    }
                } else {
                    // peripheral signal: OE from ENABLE reg if oen_sel, otherwise from peripheral (assume enabled)
                    let oen_sel = os & (1 << 10) != 0;
                    let en = if oen_sel { self.enable & bit != 0 } else { true };
                    if en {
                        d.out_en |= bit;
                    }
                    // idle level of most peripheral outputs is high (UART TX, I2C, SPI CS)
                    if !inv {
                        d.out_level |= bit;
                    }
                }
            } else {
                // direct IO_MUX function (UART0 on 1/3, flash pads, etc.)
                d.out_signal[g] = 0x200 + func as u16;
                if (g == 1 && func == 0) || (g == 6 || g == 11) {
                    d.out_en |= bit;
                    d.out_level |= bit;
                }
            }
        }
        // UART RX routing through the GPIO matrix (U0RXD=14, U1RXD=17, U2RXD=198)
        for (port, sig) in [14usize, 17, 198].iter().enumerate() {
            let v = self.in_sel[*sig];
            if v & (1 << 7) != 0 && (v & 0x3F) < NUM_GPIO as u32 {
                d.uart_rx_pads[port] = Some((v & 0x3F) as u8);
            } else if port == 0 && (self.iomux[3] >> 12) & 7 == 0 {
                d.uart_rx_pads[0] = Some(3);
            }
        }
        // DAC pads
        for (i, g) in [25usize, 26].iter().enumerate() {
            let off = 0x84 + 4 * i as u32;
            let r = self.rtc.read(off);
            if r & (1 << 18) != 0 {
                d.dac[i] = Some(((r >> 19) & 0xFF) as f32 / 255.0 * 3.3);
                let _ = g;
            }
        }
        d
    }

    fn mark(&mut self) {
        self.drive_dirty = true;
    }

    pub fn read(&mut self, off: u32, ctx: &mut Ctx) -> u32 {
        match off {
            0x04 => self.out as u32,
            0x10 => (self.out >> 32) as u32 & 0xFF,
            0x20 => self.enable as u32,
            0x2C => (self.enable >> 32) as u32 & 0xFF,
            0x38 => self.strap,
            0x3C => self.sample_inputs(ctx) as u32,
            0x40 => (self.sample_inputs(ctx) >> 32) as u32 & 0xFF,
            0x44 => self.status as u32,
            0x50 => (self.status >> 32) as u32 & 0xFF,
            0x60 => self.cpu_int_status(1) as u32,
            0x68 => self.cpu_int_status(0) as u32,
            0x74 => (self.cpu_int_status(1) >> 32) as u32,
            0x7C => (self.cpu_int_status(0) >> 32) as u32,
            o if (0x88..0x128).contains(&o) => self.pin[((o - 0x88) / 4) as usize],
            o if (0x130..0x530).contains(&o) => self.in_sel[((o - 0x130) / 4) as usize],
            o if (0x530..0x5D0).contains(&o) => self.out_sel[((o - 0x530) / 4) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, val: u32, ctx: &mut Ctx) {
        let lo = |v: u64, x: u32| (v & !0xFFFF_FFFF) | x as u64;
        let hi = |v: u64, x: u32| (v & 0xFFFF_FFFF) | (((x & 0xFF) as u64) << 32);
        match off {
            0x04 => self.out = lo(self.out, val),
            0x08 => self.out |= val as u64,
            0x0C => self.out &= !(val as u64),
            0x10 => self.out = hi(self.out, val),
            0x14 => self.out |= ((val & 0xFF) as u64) << 32,
            0x18 => self.out &= !(((val & 0xFF) as u64) << 32),
            0x20 => self.enable = lo(self.enable, val),
            0x24 => self.enable |= val as u64,
            0x28 => self.enable &= !(val as u64),
            0x2C => self.enable = hi(self.enable, val),
            0x30 => self.enable |= ((val & 0xFF) as u64) << 32,
            0x34 => self.enable &= !(((val & 0xFF) as u64) << 32),
            0x44 => self.status = lo(self.status, val),
            0x48 => self.status |= val as u64,
            0x4C => self.status &= !(val as u64),
            0x50 => self.status = hi(self.status, val),
            0x54 => self.status |= ((val & 0xFF) as u64) << 32,
            0x58 => self.status &= !(((val & 0xFF) as u64) << 32),
            o if (0x88..0x128).contains(&o) => self.pin[((o - 0x88) / 4) as usize] = val,
            o if (0x130..0x530).contains(&o) => self.in_sel[((o - 0x130) / 4) as usize] = val,
            o if (0x530..0x5D0).contains(&o) => self.out_sel[((o - 0x530) / 4) as usize] = val,
            _ => {}
        }
        if off <= 0x34 || (0x88..0x128).contains(&off) || (0x530..0x5D0).contains(&off) {
            self.mark();
        }
        if (0x44..=0x58).contains(&off) || (0x88..0x128).contains(&off) {
            self.update_irq(ctx);
        }
    }

    pub fn iomux_read(&self, off: u32) -> u32 {
        if off == 0 {
            return self.iomux_ctrl;
        }
        match IOMUX_OFF.iter().position(|&o| o == off) {
            Some(g) => self.iomux[g],
            None => 0,
        }
    }
    pub fn iomux_write(&mut self, off: u32, val: u32) {
        if off == 0 {
            self.iomux_ctrl = val;
            return;
        }
        if let Some(g) = IOMUX_OFF.iter().position(|&o| o == off) {
            self.iomux[g] = val;
            self.mark();
        }
    }

    pub fn rtc_read(&mut self, off: u32, ctx: &mut Ctx) -> u32 {
        match off {
            0x00 => self.rtc_out,
            0x0C => self.rtc_enable,
            0x24 => {
                let inp = self.sample_inputs(ctx);
                let mut v = 0;
                for g in 0..NUM_GPIO {
                    if RTC_NUM[g] >= 0 && inp & (1 << g) != 0 {
                        v |= 1 << (RTC_NUM[g] as u32 + 14);
                    }
                }
                v
            }
            0xC8 => 0x1603160,
            _ => self.rtc.read(off),
        }
    }
    pub fn rtc_write(&mut self, off: u32, val: u32) {
        match off {
            0x00 => self.rtc_out = val,
            0x04 => self.rtc_out |= val,
            0x08 => self.rtc_out &= !val,
            0x0C => self.rtc_enable = val,
            0x10 => self.rtc_enable |= val,
            0x14 => self.rtc_enable &= !val,
            _ => self.rtc.write(off, val),
        }
        self.mark();
    }

    /// Current logic level at each pad as seen by the input buffers.
    pub fn sample_inputs(&mut self, ctx: &mut Ctx) -> u64 {
        if self.drive_dirty {
            self.drive = self.compute_drive();
            self.drive_dirty = false;
            ctx.io.gpio_update(ctx.now, &self.drive);
        }
        let d = &self.drive;
        let mut v = 0u64;
        for g in 0..NUM_GPIO {
            if IOMUX_OFF[g] == 0 {
                continue;
            }
            let bit = 1u64 << g;
            let level = match ctx.io.gpio_read(ctx.now, g as u8) {
                Some(l) => l,
                None => {
                    if d.out_en & bit != 0 && !(d.open_drain & bit != 0 && d.out_level & bit != 0) {
                        d.out_level & bit != 0
                    } else if d.pull_up & bit != 0 {
                        true
                    } else if d.pull_down & bit != 0 {
                        false
                    } else {
                        // floating: GPIO0 strapping pad has external pull-up on dev boards
                        g == 0
                    }
                }
            };
            if level {
                v |= bit;
            }
        }
        v
    }

    /// Flush drive changes to the outside world.
    pub fn flush(&mut self, ctx: &mut Ctx) {
        if self.drive_dirty {
            self.drive = self.compute_drive();
            self.drive_dirty = false;
            ctx.io.gpio_update(ctx.now, &self.drive);
        }
    }

    fn int_enabled_mask(&self) -> u64 {
        let mut m = 0;
        for g in 0..NUM_GPIO {
            if (self.pin[g] >> 7) & 7 != 0 && (self.pin[g] >> 13) & 0x1F != 0 {
                m |= 1 << g;
            }
        }
        m
    }

    fn cpu_int_status(&self, cpu: usize) -> u64 {
        let mut v = 0;
        for g in 0..NUM_GPIO {
            let ena = (self.pin[g] >> 13) & 0x1F;
            let want = if cpu == 0 { ena & (1 << 2) != 0 } else { ena & 1 != 0 };
            if want && self.status & (1 << g) != 0 {
                v |= 1 << g;
            }
        }
        v
    }

    fn update_irq(&mut self, ctx: &mut Ctx) {
        let pro = self.cpu_int_status(0) != 0;
        let app = self.cpu_int_status(1) != 0;
        ctx.irq.gpio_cpu = [pro, app];
        ctx.irq.set(src::GPIO, pro || app);
        ctx.irq.dirty = true;
    }

    /// Sample inputs for pins with interrupts enabled; latch status bits.
    pub fn tick(&mut self, ctx: &mut Ctx) {
        self.flush(ctx);
        let m = self.int_enabled_mask();
        if m == 0 {
            self.last_int_mask = 0;
            return;
        }
        let inp = self.sample_inputs(ctx);
        // pins whose interrupt was just enabled start from their current level
        // (no spurious edge from a level sampled long ago)
        let newly = m & !self.last_int_mask;
        let prev = (self.last_in & !newly) | (inp & newly);
        let rising = inp & !prev;
        let falling = !inp & prev;
        self.last_in = inp;
        self.last_int_mask = m;
        let mut st = self.status;
        for g in 0..NUM_GPIO {
            let bit = 1u64 << g;
            if m & bit == 0 {
                continue;
            }
            let t = (self.pin[g] >> 7) & 7;
            let hit = match t {
                1 => rising & bit != 0,
                2 => falling & bit != 0,
                3 => (rising | falling) & bit != 0,
                4 => inp & bit == 0,
                5 => inp & bit != 0,
                _ => false,
            };
            if hit {
                st |= bit;
            }
        }
        if st != self.status {
            self.status = st;
        }
        self.update_irq(ctx);
    }
}

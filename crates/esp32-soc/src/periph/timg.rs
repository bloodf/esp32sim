//! Timer groups (TIMG0/1): two general purpose timers, the LAC timer,
//! the watchdog (registers only) and RTC slow clock calibration.

use super::{src, Ctx, RegFile};

#[derive(Clone, Copy, Default)]
pub struct Timer {
    pub config: u32,
    pub base_count: u64,
    pub base_ns: u64,
    pub latched: u64,
    pub alarm: u64,
    pub load: u64,
}

impl Timer {
    fn enabled(&self) -> bool {
        self.config & (1 << 31) != 0
    }
    fn increase(&self) -> bool {
        self.config & (1 << 30) != 0
    }
    fn divider(&self) -> u64 {
        let d = ((self.config >> 13) & 0xFFFF) as u64;
        if d == 0 {
            65536
        } else if d == 1 {
            2
        } else {
            d
        }
    }
    fn alarm_en(&self) -> bool {
        self.config & (1 << 10) != 0
    }
    fn autoreload(&self) -> bool {
        self.config & (1 << 29) != 0
    }
    fn hz(&self, apb: u64) -> u64 {
        apb / self.divider()
    }
    pub fn count(&self, now: u64, apb: u64) -> u64 {
        if !self.enabled() {
            return self.base_count;
        }
        let dt = now.saturating_sub(self.base_ns);
        let ticks = ((dt as u128 * self.hz(apb) as u128) / 1_000_000_000u128) as u64;
        if self.increase() {
            self.base_count.wrapping_add(ticks)
        } else {
            self.base_count.wrapping_sub(ticks)
        }
    }
    fn rebase(&mut self, now: u64, apb: u64) {
        self.base_count = self.count(now, apb);
        self.base_ns = now;
    }
    fn set_config(&mut self, v: u32, now: u64, apb: u64) {
        self.rebase(now, apb);
        self.config = v;
    }
    /// Time at which the counter reaches the alarm value.
    fn alarm_time(&self, now: u64, apb: u64) -> Option<u64> {
        if !self.enabled() || !self.alarm_en() {
            return None;
        }
        let c = self.count(now, apb);
        let remaining = if self.increase() {
            if c >= self.alarm {
                return Some(now);
            }
            self.alarm - c
        } else {
            if c <= self.alarm {
                return Some(now);
            }
            c - self.alarm
        };
        let hz = self.hz(apb).max(1);
        let ns = ((remaining as u128 * 1_000_000_000u128 + hz as u128 - 1) / hz as u128) as u64;
        Some(now + ns.max(1))
    }
}

pub struct Timg {
    pub idx: u8,
    pub t: [Timer; 2],
    pub lact: Timer,
    pub regs: RegFile,
    pub int_raw: u32,
    pub int_ena: u32,
    pub xtal_hz: u64,
    pub wdt_fed_ns: u64,
}

impl Timg {
    pub fn new(idx: u8) -> Timg {
        let mut t = [Timer::default(); 2];
        for x in t.iter_mut() {
            x.config = (1 << 30) | (1 << 13); // increase, divider 1
        }
        let mut lact = Timer::default();
        lact.config = (1 << 30) | (1 << 13);
        let mut regs = RegFile::default();
        regs.write(0x48, 0x0004_C000); // wdtconfig0 default-ish
        regs.write(0x68, 0x0001_3000 | (1 << 15));
        Timg { idx, t, lact, regs, int_raw: 0, int_ena: 0, xtal_hz: 40_000_000, wdt_fed_ns: 0 }
    }

    fn lvl_src(&self) -> usize {
        if self.idx == 0 {
            src::TG0_T0_LEVEL
        } else {
            src::TG1_T0_LEVEL
        }
    }

    fn update_irq(&mut self, ctx: &mut Ctx) {
        let st = self.int_raw & self.int_ena;
        let base = self.lvl_src();
        // level sources: T0, T1, WDT, LACT
        for bit in 0..4 {
            ctx.irq.set(base + bit, st & (1 << bit) != 0);
        }
    }

    fn timer_for(&mut self, off: u32) -> Option<(usize, u32)> {
        if off < 0x48 {
            Some(((off / 0x24) as usize, off % 0x24))
        } else {
            None
        }
    }

    pub fn read(&mut self, off: u32, ctx: &mut Ctx) -> u32 {
        if let Some((n, r)) = self.timer_for(off) {
            let t = &self.t[n];
            return match r {
                0x00 => t.config,
                0x04 => t.latched as u32,
                0x08 => (t.latched >> 32) as u32,
                0x10 => t.alarm as u32,
                0x14 => (t.alarm >> 32) as u32,
                0x18 => t.load as u32,
                0x1C => (t.load >> 32) as u32,
                _ => 0,
            };
        }
        match off {
            0x70 => self.lact.config,
            0x78 => self.lact.latched as u32,
            0x7C => (self.lact.latched >> 32) as u32,
            0x84 => self.lact.alarm as u32,
            0x88 => (self.lact.alarm >> 32) as u32,
            0x8C => self.lact.load as u32,
            0x90 => (self.lact.load >> 32) as u32,
            0x98 => self.int_ena,
            0x9C => self.int_raw,
            0xA0 => self.int_raw & self.int_ena,
            0xF8 => 0x1604290,
            _ => {
                let _ = ctx;
                self.regs.read(off)
            }
        }
    }

    pub fn write(&mut self, off: u32, val: u32, ctx: &mut Ctx) {
        let now = ctx.now;
        let apb = ctx.apb_hz;
        if let Some((n, r)) = self.timer_for(off) {
            let t = &mut self.t[n];
            match r {
                0x00 => t.set_config(val, now, apb),
                0x0C => t.latched = t.count(now, apb),
                0x10 => t.alarm = (t.alarm & !0xFFFF_FFFF) | val as u64,
                0x14 => t.alarm = (t.alarm & 0xFFFF_FFFF) | ((val as u64) << 32),
                0x18 => t.load = (t.load & !0xFFFF_FFFF) | val as u64,
                0x1C => t.load = (t.load & 0xFFFF_FFFF) | ((val as u64) << 32),
                0x20 => {
                    t.base_count = t.load;
                    t.base_ns = now;
                }
                _ => {}
            }
            return;
        }
        match off {
            0x70 => self.lact.set_config(val, now, apb),
            0x80 => self.lact.latched = self.lact.count(now, apb),
            0x84 => self.lact.alarm = (self.lact.alarm & !0xFFFF_FFFF) | val as u64,
            0x88 => self.lact.alarm = (self.lact.alarm & 0xFFFF_FFFF) | ((val as u64) << 32),
            0x8C => self.lact.load = (self.lact.load & !0xFFFF_FFFF) | val as u64,
            0x90 => self.lact.load = (self.lact.load & 0xFFFF_FFFF) | ((val as u64) << 32),
            0x94 => {
                self.lact.base_count = self.lact.load;
                self.lact.base_ns = now;
            }
            0x98 => {
                self.int_ena = val & 0xF;
                self.update_irq(ctx);
            }
            0xA4 => {
                self.int_raw &= !val;
                self.update_irq(ctx);
            }
            0x60 => self.wdt_fed_ns = now,
            0x68 => {
                // RTC calibration
                let mut v = val;
                if val & (1 << 31) != 0 || val & (1 << 12) != 0 {
                    let max = ((val >> 16) & 0x7FFF) as u64;
                    let sel = (val >> 13) & 3;
                    let slow_hz: u64 = match sel {
                        1 => 8_500_000 / 256,
                        2 => 32_768,
                        _ => 150_000,
                    };
                    let cal = max * self.xtal_hz / slow_hz;
                    self.regs.write(0x6C, ((cal as u32) & 0x01FF_FFFF) << 7);
                    v |= 1 << 15;
                } else {
                    v &= !(1 << 15);
                }
                self.regs.write(off, v);
            }
            _ => self.regs.write(off, val),
        }
    }

    /// Process alarms; return next event time.
    pub fn tick(&mut self, ctx: &mut Ctx) -> Option<u64> {
        let now = ctx.now;
        let apb = ctx.apb_hz;
        let mut next: Option<u64> = None;
        let mut fired = 0u32;
        for i in 0..3 {
            let t = if i < 2 { &mut self.t[i] } else { &mut self.lact };
            if let Some(at) = t.alarm_time(now, apb) {
                if at <= now {
                    // alarm
                    fired |= if i < 2 { 1 << i } else { 1 << 3 };
                    t.rebase(now, apb);
                    t.config &= !(1 << 10); // alarm_en auto-clears
                    if t.autoreload() {
                        t.base_count = t.load;
                        t.base_ns = now;
                    }
                } else {
                    next = Some(next.map_or(at, |n| n.min(at)));
                }
            }
        }
        if fired != 0 {
            self.int_raw |= fired;
            self.update_irq(ctx);
        }
        next
    }
}

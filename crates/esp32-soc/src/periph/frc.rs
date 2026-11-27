//! Legacy FRC timers at 0x3FF47000 (FRC1 / FRC2, "TIMER1"/"TIMER2" interrupt
//! sources).  ESP-IDF up to v4.0 (Arduino-ESP32 1.0.x) runs `esp_timer` on FRC2,
//! so without them `esp_timer_get_time()` never advances.
//!
//! Register block per timer (base 0x3FF47000 + 0x20 * i):
//!   +0x00 LOAD   (FRC1: reload value, 23 bit down counter; FRC2: sets the count)
//!   +0x04 COUNT  (read only)
//!   +0x08 CTRL   bit8 int status (ro), bit7 enable, bit6 autoload,
//!                bits3:1 prescaler (0 = /1, 2 = /16, 4 = /256), bit0 level interrupt
//!   +0x0C INT    write bit0 to clear
//!   +0x10 ALARM  (FRC2 only: interrupt when the up-counter reaches it)

use super::{src, Ctx};

#[derive(Default, Clone)]
struct Frc {
    load: u32,
    ctrl: u32,
    alarm: u32,
    /// counter value at `base_ns`
    base: u32,
    base_ns: u64,
    /// counter ticks already accounted for below `base_ns` (fractional carry)
    int_pending: bool,
}

pub struct FrcTimers {
    t: [Frc; 2],
}

impl Frc {
    fn div(&self) -> u64 {
        match (self.ctrl >> 1) & 7 {
            2 | 3 => 16,
            4 | 5 | 6 | 7 => 256,
            _ => 1,
        }
    }
    fn enabled(&self) -> bool {
        self.ctrl & (1 << 7) != 0
    }
    fn ticks_since(&self, now: u64, apb: u64) -> u64 {
        if !self.enabled() {
            return 0;
        }
        (now.saturating_sub(self.base_ns) as u128 * apb as u128 / (1_000_000_000u128 * self.div() as u128)) as u64
    }
    fn ns_for_ticks(&self, ticks: u64, apb: u64) -> u64 {
        ((ticks as u128 * self.div() as u128 * 1_000_000_000u128 + apb as u128 - 1) / apb as u128) as u64
    }
    fn rebase(&mut self, now: u64, apb: u64, up: bool) {
        let t = self.ticks_since(now, apb);
        self.base = if up { self.base.wrapping_add(t as u32) } else { (self.base as u64).saturating_sub(t) as u32 };
        // keep sub-tick remainder by moving the base by whole ticks only
        self.base_ns += self.ns_for_ticks(t, apb).min(now - self.base_ns.min(now));
        if !self.enabled() {
            self.base_ns = now;
        }
    }
}

impl FrcTimers {
    pub fn new() -> FrcTimers {
        FrcTimers { t: [Frc::default(), Frc::default()] }
    }

    fn count(&self, i: usize, now: u64, apb: u64) -> u32 {
        let f = &self.t[i];
        let t = f.ticks_since(now, apb);
        if i == 0 {
            // 23-bit down counter
            (f.base as u64).saturating_sub(t) as u32 & 0x7F_FFFF
        } else {
            f.base.wrapping_add(t as u32)
        }
    }

    pub fn read(&mut self, off: u32, ctx: &mut Ctx) -> u32 {
        let i = ((off >> 5) & 1) as usize;
        if off >= 0x40 {
            return 0;
        }
        self.tick(ctx);
        match off & 0x1F {
            0x00 => self.t[i].load,
            0x04 => self.count(i, ctx.now, ctx.apb_hz),
            0x08 => self.t[i].ctrl | if self.t[i].int_pending { 1 << 8 } else { 0 },
            0x10 => self.t[i].alarm,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, val: u32, ctx: &mut Ctx) {
        if off >= 0x40 {
            return;
        }
        let i = ((off >> 5) & 1) as usize;
        let (now, apb) = (ctx.now, ctx.apb_hz);
        let up = i == 1;
        match off & 0x1F {
            0x00 => {
                let f = &mut self.t[i];
                f.load = if i == 0 { val & 0x7F_FFFF } else { val };
                f.base = f.load;
                f.base_ns = now;
            }
            0x08 => {
                self.t[i].rebase(now, apb, up);
                self.t[i].ctrl = val & 0xCF;
                self.t[i].base_ns = now.min(self.t[i].base_ns.max(now));
            }
            0x0C => {
                if val & 1 != 0 {
                    self.t[i].int_pending = false;
                }
            }
            0x10 => {
                self.t[i].alarm = val;
            }
            _ => {}
        }
        self.tick(ctx);
    }

    /// Raise interrupts that are due; return the next time one will be.
    pub fn tick(&mut self, ctx: &mut Ctx) -> Option<u64> {
        let (now, apb) = (ctx.now, ctx.apb_hz);
        let mut next: Option<u64> = None;
        // FRC1: down counter, fires at zero
        if self.t[0].enabled() {
            let f = &self.t[0];
            let remaining = f.base as u64;
            let elapsed = f.ticks_since(now, apb);
            if elapsed >= remaining {
                self.t[0].int_pending = true;
                let f = &mut self.t[0];
                f.base_ns += f.ns_for_ticks(remaining, apb);
                if f.ctrl & (1 << 6) != 0 && f.load > 0 {
                    f.base = f.load;
                } else {
                    f.base = 0;
                    f.ctrl &= !(1 << 7);
                }
            }
            let f = &self.t[0];
            if f.enabled() && f.base > 0 {
                let at = f.base_ns + f.ns_for_ticks(f.base as u64, apb);
                next = Some(at.max(now + 1));
            }
        }
        // FRC2: up counter, fires when it reaches the alarm value
        if self.t[1].enabled() {
            let f = &self.t[1];
            let cur = self.count(1, now, apb);
            let start = f.base;
            let dist_alarm = f.alarm.wrapping_sub(start) as u64;
            let elapsed = f.ticks_since(now, apb);
            if elapsed >= dist_alarm && dist_alarm != 0 || (cur == f.alarm && !self.t[1].int_pending && elapsed > 0) {
                self.t[1].int_pending = true;
                // move the base to the alarm point so it fires once per wrap
                let f = &mut self.t[1];
                f.base_ns += f.ns_for_ticks(dist_alarm, apb);
                f.base = f.alarm;
            }
            let f = &self.t[1];
            let d = f.alarm.wrapping_sub(self.count(1, now, apb)) as u64;
            let d = if d == 0 { 1u64 << 32 } else { d };
            let at = now + f.ns_for_ticks(d, apb).max(1);
            next = Some(next.map_or(at, |n: u64| n.min(at)));
        }
        ctx.irq.set(src::FRC1, self.t[0].int_pending);
        ctx.irq.set(src::FRC2, self.t[1].int_pending);
        next
    }
}

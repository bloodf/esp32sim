//! LEDC PWM controller (8 high-speed + 8 low-speed channels, 4+4 timers).

use super::{src, Ctx, RegFile};
use crate::io::PwmState;

#[derive(Clone, Copy, Default)]
struct Fade {
    start_ns: u64,
    step_ns: u64,
    num: u32,
    scale: u32,
    inc: bool,
    duty0: u32,
}

pub struct Ledc {
    pub regs: RegFile,
    pub int_raw: u32,
    fades: [Option<Fade>; 16],
    /// applied duty (in 1/16 LSB units) per channel (0..7 HS, 8..15 LS)
    duty_applied: [u32; 16],
    pub changed: bool,
}

impl Ledc {
    pub fn new() -> Ledc {
        Ledc { regs: RegFile::default(), int_raw: 0, fades: [None; 16], duty_applied: [0; 16], changed: true }
    }

    fn ch_base(ch: usize) -> u32 {
        if ch < 8 {
            ch as u32 * 0x14
        } else {
            0xA0 + (ch as u32 - 8) * 0x14
        }
    }

    fn timer_conf(&self, ch: usize) -> (u32, bool) {
        let conf0 = self.regs.read(Self::ch_base(ch));
        let t = conf0 & 3;
        let hs = ch < 8;
        let off = if hs { 0x140 + t * 8 } else { 0x160 + t * 8 };
        (self.regs.read(off), hs)
    }

    /// PWM period in ns and resolution bits for channel `ch`.
    fn period(&self, ch: usize, apb_hz: u64) -> Option<(f64, u32)> {
        let (tconf, hs) = self.timer_conf(ch);
        if tconf & (1 << 23) != 0 || tconf & (1 << 24) != 0 {
            return None; // paused or in reset
        }
        let res = tconf & 0x1F;
        let div = ((tconf >> 5) & 0x3FFFF) as f64 / 256.0;
        if div < 1.0 {
            return None;
        }
        let tick_sel = tconf & (1 << 25) != 0;
        let clk = if hs {
            if tick_sel {
                apb_hz as f64
            } else {
                1_000_000.0
            }
        } else if tick_sel {
            if self.regs.read(0x190) & 1 != 0 {
                apb_hz as f64
            } else {
                8_000_000.0
            }
        } else {
            1_000_000.0
        };
        let freq = clk / div / (1u64 << res) as f64;
        if freq <= 0.0 {
            return None;
        }
        Some((1e9 / freq, res))
    }

    /// Current output state of channel `ch` (0..15).
    pub fn channel_state(&self, ch: usize, now: u64, apb_hz: u64) -> PwmState {
        let conf0 = self.regs.read(Self::ch_base(ch));
        let idle = conf0 & (1 << 3) != 0;
        let out_en = conf0 & (1 << 2) != 0;
        match self.period(ch, apb_hz) {
            Some((period_ns, res)) if out_en => {
                let duty = self.current_duty(ch, now) as f64 / 16.0;
                let frac = (duty / (1u64 << res) as f64).clamp(0.0, 1.0);
                PwmState { freq_hz: 1e9 / period_ns, duty: frac, running: true }
            }
            _ => PwmState { freq_hz: 0.0, duty: if idle { 1.0 } else { 0.0 }, running: false },
        }
    }

    fn current_duty(&self, ch: usize, now: u64) -> u32 {
        match &self.fades[ch] {
            Some(f) => {
                let steps = if f.step_ns == 0 { f.num as u64 } else { ((now.saturating_sub(f.start_ns)) / f.step_ns).min(f.num as u64) };
                let delta = (steps as u32).saturating_mul(f.scale << 4);
                if f.inc {
                    f.duty0.saturating_add(delta)
                } else {
                    f.duty0.saturating_sub(delta)
                }
            }
            None => self.duty_applied[ch],
        }
    }

    pub fn read(&mut self, off: u32, ctx: &mut Ctx) -> u32 {
        match off {
            0x180 => self.int_raw,
            0x184 => self.int_raw & self.regs.read(0x188),
            0x1FC => 0x1605190,
            o if o < 0x140 && (o % 0x14) == 0x10 => {
                // duty_r
                let ch = if o < 0xA0 { (o / 0x14) as usize } else { 8 + ((o - 0xA0) / 0x14) as usize };
                self.current_duty(ch, ctx.now)
            }
            o if (0x140..0x180).contains(&o) && (o % 8) == 4 => {
                // timer value: approximate
                0
            }
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, val: u32, ctx: &mut Ctx) {
        match off {
            0x18C => {
                self.int_raw &= !val;
                self.update_irq(ctx);
                return;
            }
            0x188 => {
                self.regs.write(off, val);
                self.update_irq(ctx);
                return;
            }
            _ => {}
        }
        self.regs.write(off, val);
        if off < 0x140 {
            let ch = if off < 0xA0 { (off / 0x14) as usize } else { 8 + ((off - 0xA0) / 0x14) as usize };
            let r = if off < 0xA0 { off % 0x14 } else { (off - 0xA0) % 0x14 };
            if r == 0x0C && val & (1 << 31) != 0 {
                // duty_start: begin update / fade from DUTY register
                let duty = self.regs.read(Self::ch_base(ch) + 0x08) & 0x1FF_FFFF;
                let num = (val >> 20) & 0x3FF;
                let cycle = ((val >> 10) & 0x3FF).max(1);
                let scale = val & 0x3FF;
                let inc = val & (1 << 30) != 0;
                let period = self.period(ch, ctx.apb_hz).map(|p| p.0).unwrap_or(1000.0);
                // settle the previous fade
                self.duty_applied[ch] = self.current_duty(ch, ctx.now);
                if scale == 0 || num == 0 {
                    self.fades[ch] = None;
                    self.duty_applied[ch] = duty;
                    // immediate update completes after one period
                    self.fades[ch] = Some(Fade { start_ns: ctx.now, step_ns: period as u64, num: 1, scale: 0, inc, duty0: duty });
                } else {
                    self.fades[ch] = Some(Fade {
                        start_ns: ctx.now,
                        step_ns: (period * cycle as f64) as u64,
                        num,
                        scale,
                        inc,
                        duty0: duty,
                    });
                }
                self.regs.write(off, val & !(1 << 31));
            }
        }
        self.changed = true;
    }

    fn update_irq(&mut self, ctx: &mut Ctx) {
        let lvl = self.int_raw & self.regs.read(0x188) != 0;
        ctx.irq.set(src::LEDC, lvl);
    }

    /// Advance fades; returns next event time.
    pub fn tick(&mut self, ctx: &mut Ctx) -> Option<u64> {
        let mut next: Option<u64> = None;
        let mut fired = 0u32;
        for ch in 0..16 {
            if let Some(f) = self.fades[ch] {
                let end = f.start_ns + f.step_ns * f.num as u64;
                if ctx.now >= end {
                    self.duty_applied[ch] = self.current_duty(ch, end);
                    self.fades[ch] = None;
                    fired |= 1 << (8 + ch);
                    self.changed = true;
                } else {
                    if f.scale != 0 {
                        self.changed = true;
                        let step_next = ctx.now + f.step_ns.max(1_000_000); // UI refresh at most every ms
                        next = Some(next.map_or(step_next.min(end), |n: u64| n.min(step_next.min(end))));
                    } else {
                        next = Some(next.map_or(end, |n: u64| n.min(end)));
                    }
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

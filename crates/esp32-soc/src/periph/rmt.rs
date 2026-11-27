//! RMT (remote control) transmitter — used for WS2812 / NeoPixel strips and IR.
//!
//! Transmission is simulated in time so that drivers refilling the channel
//! memory from the TX-threshold interrupt (ping-pong mode) work.

use super::{src, Ctx, RegFile};
use crate::io::RmtItem;

#[derive(Clone, Default)]
struct TxState {
    active: bool,
    pos: u32,
    sent: u32,
    t_ns: u64,
    items: Vec<RmtItem>,
    half: u8,
}

pub struct Rmt {
    pub regs: RegFile,
    pub ram: Vec<u32>,
    pub int_raw: u32,
    tx: [TxState; 8],
    /// completed transmissions waiting to be delivered: (channel, items)
    pub done: Vec<(usize, Vec<RmtItem>)>,
}

impl Rmt {
    pub fn new() -> Rmt {
        let mut regs = RegFile::default();
        for ch in 0..8u32 {
            regs.write(0x20 + ch * 8, (1 << 24) | (1 << 31) | 0x0010_0000 | 2);
            regs.write(0x24 + ch * 8, 0x0000_0000);
        }
        Rmt { regs, ram: vec![0; 512], int_raw: 0, tx: Default::default(), done: Vec::new() }
    }

    fn conf0(&self, ch: usize) -> u32 {
        self.regs.read(0x20 + ch as u32 * 8)
    }
    fn conf1(&self, ch: usize) -> u32 {
        self.regs.read(0x24 + ch as u32 * 8)
    }

    fn tick_ns(&self, ch: usize, apb: u64) -> f64 {
        let div = (self.conf0(ch) & 0xFF) as f64;
        let div = if div == 0.0 { 256.0 } else { div };
        let clk = if self.conf1(ch) & (1 << 17) != 0 { apb as f64 } else { 1_000_000.0 };
        div * 1e9 / clk
    }

    fn update_irq(&mut self, ctx: &mut Ctx) {
        let lvl = self.int_raw & self.regs.read(0xA8) != 0;
        ctx.irq.set(src::RMT, lvl);
    }

    pub fn ram_read(&self, off: u32) -> u32 {
        self.ram[((off >> 2) & 511) as usize]
    }
    pub fn ram_write(&mut self, off: u32, v: u32) {
        self.ram[((off >> 2) & 511) as usize] = v;
    }

    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0xA0 => self.int_raw,
            0xA4 => self.int_raw & self.regs.read(0xA8),
            o if (0x60..0x80).contains(&o) => {
                let ch = ((o - 0x60) / 4) as usize;
                let st = &self.tx[ch];
                let state = if st.active { 1 } else { 0 };
                ((st.pos & 0x3FF) << 12) | (state << 24)
            }
            0xFC => 0x1606230,
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, val: u32, ctx: &mut Ctx) {
        match off {
            0xAC => {
                self.int_raw &= !val;
                self.update_irq(ctx);
            }
            0xA8 => {
                self.regs.write(off, val);
                self.update_irq(ctx);
            }
            o if (0x20..0x60).contains(&o) && (o % 8) == 4 => {
                let ch = ((o - 0x24) / 8) as usize;
                if val & (1 << 3) != 0 {
                    // mem_rd_rst
                    self.tx[ch].pos = 0;
                }
                self.regs.write(off, val & !((1 << 0) | (1 << 2) | (1 << 3) | (1 << 4)));
                if val & 1 != 0 {
                    self.tx[ch] = TxState { active: true, pos: 0, sent: 0, t_ns: ctx.now, items: Vec::new(), half: 0 };
                    self.process(ch, ctx);
                }
            }
            _ => self.regs.write(off, val),
        }
    }

    fn process(&mut self, ch: usize, ctx: &mut Ctx) {
        let tick = self.tick_ns(ch, ctx.apb_hz);
        let mem_blocks = ((self.conf0(ch) >> 24) & 0xF).max(1);
        let block_len = mem_blocks * 64;
        let wrap = self.regs.read(0xF0) & 2 != 0;
        let lim = self.regs.read(0xD0 + ch as u32 * 4) & 0x1FF;
        let base = ch as u32 * 64;
        let mut guard = 0;
        while self.tx[ch].active && self.tx[ch].t_ns <= ctx.now {
            guard += 1;
            if guard > 100_000 {
                break;
            }
            let st = &mut self.tx[ch];
            let word = self.ram[((base + st.pos) & 511) as usize];
            let (dur, level) = if st.half == 0 { (word & 0x7FFF, word & 0x8000 != 0) } else { ((word >> 16) & 0x7FFF, word & 0x8000_0000 != 0) };
            if dur == 0 {
                st.active = false;
                let items = std::mem::take(&mut st.items);
                self.done.push((ch, items));
                self.int_raw |= 1 << (3 * ch);
                if self.conf1(ch) & (1 << 6) != 0 {
                    // continuous mode: restart
                    self.tx[ch] = TxState { active: true, pos: 0, sent: 0, t_ns: ctx.now + 1000, items: Vec::new(), half: 0 };
                }
                break;
            }
            let ns = (dur as f64 * tick) as u32;
            st.items.push(RmtItem { level, duration_ns: ns });
            st.t_ns += ns as u64;
            if st.half == 0 {
                st.half = 1;
            } else {
                st.half = 0;
                st.pos += 1;
                st.sent += 1;
                if lim > 0 && st.sent % lim == 0 {
                    self.int_raw |= 1 << (24 + ch);
                }
                if st.pos >= block_len {
                    if wrap {
                        st.pos = 0;
                    } else {
                        st.active = false;
                        let items = std::mem::take(&mut st.items);
                        self.done.push((ch, items));
                        self.int_raw |= 1 << (3 * ch);
                        break;
                    }
                }
            }
        }
        self.update_irq(ctx);
    }

    pub fn tick(&mut self, ctx: &mut Ctx) -> Option<u64> {
        let mut next = None;
        for ch in 0..8 {
            if self.tx[ch].active {
                self.process(ch, ctx);
                if self.tx[ch].active {
                    let t = self.tx[ch].t_ns;
                    next = Some(next.map_or(t, |n: u64| n.min(t)));
                }
            }
        }
        next
    }
}

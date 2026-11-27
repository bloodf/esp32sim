//! UART0/1/2.

use super::{Ctx, RegFile};
use std::collections::VecDeque;

pub const FIFO: u32 = 0x00;
pub const INT_RAW: u32 = 0x04;
pub const INT_ST: u32 = 0x08;
pub const INT_ENA: u32 = 0x0C;
pub const INT_CLR: u32 = 0x10;
pub const CLKDIV: u32 = 0x14;
pub const STATUS: u32 = 0x1C;
pub const CONF0: u32 = 0x20;
pub const CONF1: u32 = 0x24;
pub const MEM_RX_STATUS: u32 = 0x60;
pub const MEM_CNT_STATUS: u32 = 0x64;

pub const INT_RXFIFO_FULL: u32 = 1 << 0;
pub const INT_TXFIFO_EMPTY: u32 = 1 << 1;
pub const INT_RXFIFO_OVF: u32 = 1 << 4;
pub const INT_RXFIFO_TOUT: u32 = 1 << 8;
pub const INT_TX_DONE: u32 = 1 << 14;

pub struct Uart {
    pub port: u8,
    pub regs: RegFile,
    pub rx: VecDeque<u8>,
    pub int_raw: u32,
    pub rd_ptr: u32,
    pub wr_ptr: u32,
    pub last_rx_ns: u64,
    pub next_rx_ns: u64,
    pub tout_fired: bool,
    pub tx_count: u64,
}

impl Uart {
    pub fn new(port: u8) -> Uart {
        let mut regs = RegFile::default();
        regs.write(CONF0, 0x0800_001C);
        regs.write(CONF1, 0x0000_6060);
        regs.write(CLKDIV, 0x2B6);
        regs.write(0x58, 0x88);
        Uart {
            port,
            regs,
            rx: VecDeque::new(),
            int_raw: 0,
            rd_ptr: 0,
            wr_ptr: 0,
            last_rx_ns: 0,
            next_rx_ns: 0,
            tout_fired: false,
            tx_count: 0,
        }
    }

    fn src(&self) -> usize {
        super::src::UART0 + self.port as usize
    }

    /// Nanoseconds per transmitted byte (10 bits) at the configured baud rate.
    pub fn byte_time_ns(&self, apb_hz: u64) -> u64 {
        let cd = self.regs.read(CLKDIV);
        let div = (cd & 0xFFFFF) as f64 + ((cd >> 20) & 0xF) as f64 / 16.0;
        let clk = if self.regs.read(CONF0) & (1 << 27) != 0 { apb_hz as f64 } else { 1_000_000.0 };
        if div < 1.0 {
            return 87_000;
        }
        let baud = clk / div;
        ((10.0 / baud) * 1e9) as u64
    }

    pub fn baud(&self, apb_hz: u64) -> u32 {
        let cd = self.regs.read(CLKDIV);
        let div = (cd & 0xFFFFF) as f64 + ((cd >> 20) & 0xF) as f64 / 16.0;
        let clk = if self.regs.read(CONF0) & (1 << 27) != 0 { apb_hz as f64 } else { 1_000_000.0 };
        if div < 1.0 {
            0
        } else {
            (clk / div).round() as u32
        }
    }

    fn full_thresh(&self) -> usize {
        let t = (self.regs.read(CONF1) & 0x7F) as usize;
        t.max(1)
    }

    fn update_levels(&mut self, ctx: &mut Ctx) {
        // TX FIFO is always drained instantly -> "empty" condition persists
        self.int_raw |= INT_TXFIFO_EMPTY;
        if self.rx.len() >= self.full_thresh() {
            self.int_raw |= INT_RXFIFO_FULL;
        }
        let ena = self.regs.read(INT_ENA);
        let s = self.src();
        ctx.irq.set(s, self.int_raw & ena != 0);
    }

    pub fn read(&mut self, off: u32, ctx: &mut Ctx) -> u32 {
        match off {
            FIFO => {
                let v = if let Some(b) = self.rx.pop_front() {
                    self.rd_ptr = self.rd_ptr.wrapping_add(1);
                    b as u32
                } else {
                    0
                };
                if self.rx.len() < self.full_thresh() {
                    self.int_raw &= !INT_RXFIFO_FULL;
                }
                self.update_levels(ctx);
                v
            }
            INT_RAW => self.int_raw,
            INT_ST => self.int_raw & self.regs.read(INT_ENA),
            STATUS => {
                let cnt = self.rx.len().min(255) as u32;
                // rxd/txd lines idle high, TX FSM idle
                cnt | (1 << 15) | (1 << 31) | (1 << 13) | (1 << 14) | (1 << 29) | (1 << 30)
            }
            MEM_RX_STATUS => {
                let rd = self.rd_ptr % 128;
                let wr = self.wr_ptr % 128;
                (rd << 2) | (wr << 13)
            }
            MEM_CNT_STATUS => (self.rx.len() as u32 >> 8) & 7,
            0x78 => 0x15122500,
            0x7C => 0x500,
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, val: u32, ctx: &mut Ctx) {
        match off {
            FIFO => {
                ctx.io.uart_tx(ctx.now, self.port, val as u8);
                self.tx_count += 1;
                self.int_raw |= INT_TX_DONE;
                self.update_levels(ctx);
            }
            INT_CLR => {
                self.int_raw &= !val;
                if val & INT_RXFIFO_TOUT != 0 {
                    self.tout_fired = true;
                }
                self.update_levels(ctx);
            }
            INT_ENA => {
                self.regs.write(off, val);
                self.update_levels(ctx);
            }
            INT_RAW | INT_ST | STATUS => {}
            CONF0 => {
                if val & (1 << 17) != 0 {
                    self.rx.clear();
                    self.rd_ptr = self.wr_ptr;
                }
                self.regs.write(off, val & !((1 << 17) | (1 << 18)));
            }
            _ => {
                self.regs.write(off, val);
                self.update_levels(ctx);
            }
        }
    }

    /// Advance RX: pull bytes from the outside world at the line rate and raise timeouts.
    /// Returns the time of the next UART event (if any).
    pub fn tick(&mut self, ctx: &mut Ctx) -> Option<u64> {
        let bt = self.byte_time_ns(ctx.apb_hz);
        let mut next = None;
        while ctx.now >= self.next_rx_ns && self.rx.len() < 128 {
            match ctx.io.uart_rx(ctx.now, self.port) {
                Some(b) => {
                    self.rx.push_back(b);
                    self.wr_ptr = self.wr_ptr.wrapping_add(1);
                    self.last_rx_ns = ctx.now;
                    self.next_rx_ns = self.next_rx_ns.max(ctx.now.saturating_sub(bt)) + bt;
                    self.tout_fired = false;
                    self.int_raw &= !INT_RXFIFO_TOUT;
                }
                None => break,
            }
        }
        if !self.rx.is_empty() {
            let conf1 = self.regs.read(CONF1);
            let tout_en = conf1 & (1 << 31) != 0;
            let thr = ((conf1 >> 24) & 0x7F).max(1) as u64;
            let deadline = self.last_rx_ns + thr * bt;
            if tout_en && !self.tout_fired {
                if ctx.now >= deadline {
                    self.int_raw |= INT_RXFIFO_TOUT;
                    self.tout_fired = true;
                } else {
                    next = Some(deadline);
                }
            }
        }
        self.update_levels(ctx);
        if self.rx.len() < 128 && self.next_rx_ns > ctx.now {
            next = Some(next.map_or(self.next_rx_ns, |n: u64| n.min(self.next_rx_ns)));
        }
        next
    }
}

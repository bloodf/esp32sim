//! I2C controllers (I2C0 / I2C1) — master mode, command-list driven.

use super::{Ctx, RegFile};
use std::collections::VecDeque;

pub const CTR: u32 = 0x04;
pub const SR: u32 = 0x08;
pub const FIFO_CONF: u32 = 0x18;
pub const DATA: u32 = 0x1C;
pub const INT_RAW: u32 = 0x20;
pub const INT_CLR: u32 = 0x24;
pub const INT_ENA: u32 = 0x28;
pub const INT_STATUS: u32 = 0x2C;
pub const COMD0: u32 = 0x58;

pub const INT_RXFIFO_FULL: u32 = 1 << 0;
pub const INT_TXFIFO_EMPTY: u32 = 1 << 1;
pub const INT_END_DETECT: u32 = 1 << 3;
pub const INT_TRANS_COMPLETE: u32 = 1 << 7;
pub const INT_TIME_OUT: u32 = 1 << 8;
pub const INT_TRANS_START: u32 = 1 << 9;
pub const INT_ACK_ERR: u32 = 1 << 10;

#[derive(Debug, Clone, Default)]
pub struct I2cLogEntry {
    pub time_ns: u64,
    pub addr: u8,
    pub read: bool,
    pub data: Vec<u8>,
    pub ack: bool,
}

pub struct I2c {
    pub idx: u8,
    pub regs: RegFile,
    pub cmd: [u32; 16],
    pub tx: VecDeque<u8>,
    pub rx: VecDeque<u8>,
    pub int_raw: u32,
    pub pending_int: u32,
    pub pending_at: u64,
    pub busy_until: u64,
    pub cmd_pos: usize,
    pub after_start: bool,
    pub in_transaction: bool,
    pub cur_read: bool,
    pub ack_rec: bool,
    /// recent transactions (for the UI / debugging)
    pub log: VecDeque<I2cLogEntry>,
    cur_log: Option<I2cLogEntry>,
}

impl I2c {
    pub fn new(idx: u8) -> I2c {
        let mut regs = RegFile::default();
        regs.write(0x00, 0);
        regs.write(FIFO_CONF, 0x0000_550B);
        regs.write(0x0C, 0x10);
        Self {
            idx,
            regs,
            cmd: [0; 16],
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            int_raw: 0,
            pending_int: 0,
            pending_at: 0,
            busy_until: 0,
            cmd_pos: 0,
            after_start: false,
            in_transaction: false,
            cur_read: false,
            ack_rec: false,
            log: VecDeque::new(),
            cur_log: None,
        }
    }

    fn src(&self) -> usize {
        if self.idx == 0 {
            super::src::I2C_EXT0
        } else {
            super::src::I2C_EXT1
        }
    }

    fn update_irq(&mut self, ctx: &mut Ctx) {
        let ena = self.regs.read(INT_ENA);
        let s = self.src();
        ctx.irq.set(s, self.int_raw & ena != 0);
    }

    /// SCL period in ns from the configured low/high periods.
    fn bit_time_ns(&self, apb_hz: u64) -> u64 {
        let low = (self.regs.read(0x00) & 0x3FFF) as u64;
        let high = (self.regs.read(0x38) & 0x3FFF) as u64;
        let cycles = (low + high).max(40);
        cycles * 1_000_000_000 / apb_hz.max(1)
    }

    pub fn read(&mut self, off: u32, ctx: &mut Ctx) -> u32 {
        match off {
            SR => {
                let busy = ctx.now < self.busy_until;
                (if self.ack_rec { 0 } else { 1 }) // ack_rec: 1 means NACK received
                    | ((busy as u32) << 4)
                    | ((self.rx.len().min(63) as u32) << 8)
                    | ((self.tx.len().min(63) as u32) << 18)
            }
            DATA => {
                let v = self.rx.pop_front().unwrap_or(0) as u32;
                v
            }
            INT_RAW => self.int_raw,
            INT_STATUS => self.int_raw & self.regs.read(INT_ENA),
            o if (COMD0..COMD0 + 64).contains(&o) => self.cmd[((o - COMD0) / 4) as usize],
            0xF8 => 0x1510_1400,
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, val: u32, ctx: &mut Ctx, scl: Option<u8>, sda: Option<u8>) {
        match off {
            DATA => {
                if self.tx.len() < 32 {
                    self.tx.push_back(val as u8);
                }
            }
            INT_CLR => {
                self.int_raw &= !val;
                self.update_irq(ctx);
            }
            INT_ENA => {
                self.regs.write(off, val);
                self.update_irq(ctx);
            }
            FIFO_CONF => {
                if val & (1 << 12) != 0 {
                    self.rx.clear();
                }
                if val & (1 << 13) != 0 {
                    self.tx.clear();
                }
                self.regs.write(off, val & !((1 << 12) | (1 << 13)));
            }
            CTR => {
                self.regs.write(off, val & !(1 << 5));
                if val & (1 << 5) != 0 {
                    self.execute(ctx, scl, sda);
                }
            }
            o if (COMD0..COMD0 + 64).contains(&o) => self.cmd[((o - COMD0) / 4) as usize] = val,
            _ => self.regs.write(off, val),
        }
    }

    fn finish_log(&mut self) {
        if let Some(l) = self.cur_log.take() {
            if self.log.len() >= 256 {
                self.log.pop_front();
            }
            self.log.push_back(l);
        }
    }

    /// Run the command list from `cmd_pos` until END or STOP.
    fn execute(&mut self, ctx: &mut Ctx, scl: Option<u8>, sda: Option<u8>) {
        let bt = self.bit_time_ns(ctx.apb_hz);
        let mut bits: u64 = 0;
        let mut result_int = 0u32;
        let mut pos = self.cmd_pos;
        while pos < 16 {
            let c = self.cmd[pos];
            let op = (c >> 11) & 7;
            let n = (c & 0xFF) as usize;
            let ack_check = c & (1 << 8) != 0;
            let ack_exp = c & (1 << 9) != 0; // expected ack value (0 = ACK)
            let ack_val = c & (1 << 10) != 0;
            match op {
                0 => {
                    // RSTART
                    if self.in_transaction {
                        self.finish_log();
                    }
                    self.after_start = true;
                    self.in_transaction = true;
                    bits += 2;
                    self.cmd[pos] |= 1 << 31;
                }
                1 => {
                    // WRITE
                    let mut nack = false;
                    for _ in 0..n {
                        let b = self.tx.pop_front().unwrap_or(0xFF);
                        bits += 9;
                        let acked = if self.after_start {
                            self.after_start = false;
                            let addr = b >> 1;
                            let rd = b & 1 != 0;
                            self.cur_read = rd;
                            let a = ctx.io.i2c_start(ctx.now, scl, sda, addr, rd);
                            self.cur_log = Some(I2cLogEntry { time_ns: ctx.now, addr, read: rd, data: vec![], ack: a });
                            a
                        } else {
                            let a = ctx.io.i2c_write(ctx.now, scl, sda, b);
                            if let Some(l) = self.cur_log.as_mut() {
                                l.data.push(b);
                            }
                            a
                        };
                        self.ack_rec = acked;
                        // ack bit value on the wire: 0 = ACK
                        let ack_bit = !acked;
                        if ack_check && ack_bit != ack_exp {
                            nack = true;
                            break;
                        }
                    }
                    self.cmd[pos] |= 1 << 31;
                    if nack {
                        result_int |= INT_ACK_ERR;
                        ctx.io.i2c_stop(ctx.now, scl, sda);
                        self.finish_log();
                        self.in_transaction = false;
                        self.cmd_pos = 0;
                        break;
                    }
                }
                2 => {
                    // READ
                    for _ in 0..n {
                        // ack_val: 0 => master ACKs (more bytes), 1 => NACK
                        let b = ctx.io.i2c_read(ctx.now, scl, sda, !ack_val);
                        if let Some(l) = self.cur_log.as_mut() {
                            l.data.push(b);
                        }
                        if self.rx.len() < 32 {
                            self.rx.push_back(b);
                        }
                        bits += 9;
                    }
                    self.cmd[pos] |= 1 << 31;
                }
                3 => {
                    // STOP
                    ctx.io.i2c_stop(ctx.now, scl, sda);
                    self.finish_log();
                    self.in_transaction = false;
                    self.cmd[pos] |= 1 << 31;
                    bits += 2;
                    result_int |= INT_TRANS_COMPLETE;
                    self.cmd_pos = 0;
                    break;
                }
                4 => {
                    // END: pause; software refills the command list and the next
                    // trans_start executes again from COMD0
                    self.cmd[pos] |= 1 << 31;
                    result_int |= INT_END_DETECT;
                    self.cmd_pos = 0;
                    break;
                }
                _ => {
                    self.cmd[pos] |= 1 << 31;
                }
            }
            pos += 1;
            if pos >= 16 {
                self.cmd_pos = 0;
            }
        }
        let dur = bits * bt;
        if std::env::var("DEBUG_I2C").is_ok() {
            let cmds: Vec<String> = self.cmd.iter().take(8).map(|c| format!("{}:{}{}", ["RST", "WR", "RD", "STP", "END", "?", "?", "?"][((c >> 11) & 7) as usize], c & 0xFF, if c & (1 << 8) != 0 { "c" } else { "" })).collect();
            eprintln!("i2c{} exec from {} -> result_int={:x} ack={} cmds={:?} int_ena={:x} raw={:x}", self.idx, pos, result_int, self.ack_rec, cmds, self.regs.read(INT_ENA), self.int_raw);
        }
        self.busy_until = ctx.now + dur;
        self.int_raw |= INT_TRANS_START;
        if result_int != 0 {
            self.pending_int |= result_int;
            self.pending_at = ctx.now + dur.max(1);
        }
        self.update_irq(ctx);
    }

    pub fn tick(&mut self, ctx: &mut Ctx) -> Option<u64> {
        if self.pending_int != 0 {
            if ctx.now >= self.pending_at {
                self.int_raw |= self.pending_int;
                self.pending_int = 0;
                self.update_irq(ctx);
                None
            } else {
                Some(self.pending_at)
            }
        } else {
            None
        }
    }
}

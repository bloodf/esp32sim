//! The ESP32 system bus: memories + peripherals.

use crate::flash::Flash;
use crate::io::{IoBridge, NullIo, PwmState, SpiPins, NUM_GPIO};
use crate::periph::{
    dport::Dport, efuse::Efuse, gpio::Gpio, i2c::I2c, ledc::Ledc, rmt::Rmt, rtc::Rtc, spi::DmaMem, spi::FlashChip, spi::Spi,
    timg::Timg, uart::Uart, Ctx, Irq, RegFile,
};
use std::collections::HashSet;
use xtensa_core::{Bus, MemFault};

pub const DRAM_BASE: u32 = 0x3FFA_E000;
pub const DRAM_END: u32 = 0x4000_0000;
pub const IRAM_BASE: u32 = 0x4007_0000;
pub const IRAM_END: u32 = 0x400A_0000;

pub struct Sys {
    pub dram: Vec<u8>,
    pub iram: Vec<u8>,
    pub rtc_fast: Vec<u8>,
    pub rtc_slow: Vec<u8>,
    pub flash: Flash,
    pub chip: FlashChip,
    pub cur_core: usize,
    /// Last RAM word loaded by each core.
    pub last_load: [u32; 2],
    /// How many consecutive loads of each core hit `last_load` (spin detection).
    pub load_repeat: [u8; 2],
    /// Set when the running core stored to the word the other core last loaded
    /// (typically a flag the other core is spinning on): the scheduler then lets
    /// the other core run before this one continues, so short-lived handshakes
    /// between the cores (set a flag, clear it again soon after) are not missed.
    pub yield_core: bool,
    /// Time at the start of the current quantum (ns)
    pub now: u64,
    /// cycle counter of the running core at the start of the quantum
    pub q_cyc0: u64,
    pub cpu_hz: u64,
    pub irq: Irq,
    pub io: Box<dyn IoBridge>,
    pub dport: Dport,
    pub uart: [Uart; 3],
    pub gpio: Gpio,
    pub rtc: Rtc,
    pub timg: [Timg; 2],
    pub frc: crate::periph::frc::FrcTimers,
    pub efuse: Efuse,
    pub i2c: [I2c; 2],
    pub spi: [Spi; 4],
    pub ledc: Ledc,
    pub rmt: Rmt,
    pub misc: RegFile,
    pub unknown: HashSet<u32>,
    pub log_unknown: bool,
    pub pwm: [Option<PwmState>; NUM_GPIO],
    rng: u64,
    pub fault_log: Vec<String>,
    pub code_gen: u64,
    /// g_ticks_per_us values maintained by the ROM HLE
    pub ticks_per_us: [u32; 2],
    pub events: Vec<String>,
    /// the firmware accessed Wi-Fi / Bluetooth radio registers
    pub radio_touched: bool,
    /// state of the ROM rand()
    pub rom_rand: u64,
    /// optional trace of recent peripheral reads (address, value) for debugging
    pub read_trace: Option<std::collections::VecDeque<(u32, u32)>>,
    /// poll detector for undocumented radio registers: (address, repeat count)
    radio_poll: (u32, u32),
}

struct DramDma<'a> {
    dram: &'a mut [u8],
}

impl<'a> DmaMem for DramDma<'a> {
    fn rd8(&mut self, a: u32) -> u8 {
        if (DRAM_BASE..DRAM_END).contains(&a) {
            self.dram[(a - DRAM_BASE) as usize]
        } else {
            0
        }
    }
    fn wr8(&mut self, a: u32, v: u8) {
        if (DRAM_BASE..DRAM_END).contains(&a) {
            self.dram[(a - DRAM_BASE) as usize] = v;
        }
    }
}

impl Sys {
    pub fn new(flash: Flash, mac: [u8; 6]) -> Sys {
        let size = flash.size();
        Sys {
            dram: vec![0; (DRAM_END - DRAM_BASE) as usize],
            iram: vec![0; (IRAM_END - IRAM_BASE) as usize],
            rtc_fast: vec![0; 0x2000],
            rtc_slow: vec![0; 0x2000],
            flash,
            chip: FlashChip::new(size),
            cur_core: 0,
            last_load: [1, 1],
            radio_touched: false,
            load_repeat: [0, 0],
            yield_core: false,
            now: 0,
            q_cyc0: 0,
            cpu_hz: 80_000_000,
            irq: Irq::new(),
            io: Box::new(NullIo::default()),
            dport: Dport::new(),
            uart: [Uart::new(0), Uart::new(1), Uart::new(2)],
            gpio: Gpio::new(),
            rtc: Rtc::new(),
            timg: [Timg::new(0), Timg::new(1)],
            frc: crate::periph::frc::FrcTimers::new(),
            efuse: Efuse::new(mac),
            i2c: [I2c::new(0), I2c::new(1)],
            spi: [Spi::new(0), Spi::new(1), Spi::new(2), Spi::new(3)],
            ledc: Ledc::new(),
            rmt: Rmt::new(),
            misc: RegFile::default(),
            unknown: HashSet::new(),
            log_unknown: false,
            pwm: [None; NUM_GPIO],
            rng: 0x1234_5678_9ABC_DEF1,
            fault_log: Vec::new(),
            code_gen: 0,
            ticks_per_us: [80, 80],
            events: Vec::new(),
            rom_rand: 1,
            read_trace: if std::env::var("PERIPH_TRACE").is_ok() { Some(Default::default()) } else { None },
            radio_poll: (0, 0),
        }
    }

    /// Reset peripherals (keeps flash and RTC slow memory).
    pub fn reset_peripherals(&mut self) {
        let mac = self.efuse.mac;
        self.irq = Irq::new();
        self.dport = Dport::new();
        self.uart = [Uart::new(0), Uart::new(1), Uart::new(2)];
        self.gpio = Gpio::new();
        let reason = self.rtc.reset_reason;
        self.rtc = Rtc::new();
        self.rtc.reset_reason = reason;
        self.timg = [Timg::new(0), Timg::new(1)];
        self.frc = crate::periph::frc::FrcTimers::new();
        self.efuse = Efuse::new(mac);
        self.i2c = [I2c::new(0), I2c::new(1)];
        self.spi = [Spi::new(0), Spi::new(1), Spi::new(2), Spi::new(3)];
        self.ledc = Ledc::new();
        self.rmt = Rmt::new();
        self.misc = RegFile::default();
        self.pwm = [None; NUM_GPIO];
        self.cpu_hz = 80_000_000;
        self.ticks_per_us = [80, 80];
        self.dram.fill(0);
        self.iram.fill(0);
    }

    #[inline]
    pub fn precise_now(&self, cyc: u64) -> u64 {
        let d = cyc.saturating_sub(self.q_cyc0);
        self.now + ((d as u128 * 1_000_000_000u128) / self.cpu_hz as u128) as u64
    }

    pub fn apb_hz(&self) -> u64 {
        if self.rtc.soc_clk_sel() == 1 {
            80_000_000
        } else {
            self.cpu_hz.min(80_000_000)
        }
    }

    /// Compute the CPU clock from the clock configuration registers.
    pub fn compute_cpu_hz(&self) -> u64 {
        match self.rtc.soc_clk_sel() {
            1 => match self.dport.cpu_period_sel() {
                0 => 80_000_000,
                1 => 160_000_000,
                _ => 240_000_000,
            },
            0 => {
                let div = (self.misc.read(0x3FF6_6000) & 0x3FF) as u64 + 1;
                40_000_000 / div
            }
            2 => 8_000_000,
            _ => 40_000_000,
        }
    }

    // ------------------------------------------------------------------ raw memory helpers
    #[inline]
    fn ram_slice(&mut self, addr: u32) -> Option<(&mut Vec<u8>, usize)> {
        if (DRAM_BASE..DRAM_END).contains(&addr) {
            Some((&mut self.dram, (addr - DRAM_BASE) as usize))
        } else if (IRAM_BASE..IRAM_END).contains(&addr) {
            Some((&mut self.iram, (addr - IRAM_BASE) as usize))
        } else if (0x400A_0000..0x400C_0000).contains(&addr) {
            // SRAM1 instruction-bus alias (word-reversed)
            let off = addr - 0x400A_0000;
            let d = 0x1FFFC - (off & !3) + (off & 3);
            Some((&mut self.dram, (0x3FFE_0000 - DRAM_BASE + d) as usize))
        } else if (0x3FF8_0000..0x3FF8_2000).contains(&addr) {
            Some((&mut self.rtc_fast, (addr - 0x3FF8_0000) as usize))
        } else if (0x400C_0000..0x400C_2000).contains(&addr) {
            Some((&mut self.rtc_fast, (addr - 0x400C_0000) as usize))
        } else if (0x5000_0000..0x5000_2000).contains(&addr) {
            Some((&mut self.rtc_slow, (addr - 0x5000_0000) as usize))
        } else {
            None
        }
    }

    #[inline]
    fn flash_addr(&self, addr: u32) -> Option<u32> {
        if (0x3F40_0000..0x3F80_0000).contains(&addr) || (0x400C_2000..0x40C0_0000).contains(&addr) {
            self.dport.translate(self.cur_core, addr)
        } else {
            None
        }
    }

    /// Load bytes directly into memory (used by the loader / HLE), ignoring peripherals.
    pub fn load(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        for (i, &b) in data.iter().enumerate() {
            let a = addr + i as u32;
            match self.ram_slice(a) {
                Some((v, o)) => v[o] = b,
                None => return Err(format!("cannot load to 0x{:08x}", a)),
            }
        }
        Ok(())
    }

    fn is_periph(addr: u32) -> bool {
        (0x3FF0_0000..0x3FF8_0000).contains(&addr) || (0x6000_0000..0x6004_0000).contains(&addr)
    }

    pub fn peek8(&mut self, addr: u32) -> u8 {
        if let Some((v, o)) = self.ram_slice(addr) {
            return v[o];
        }
        if let Some(f) = self.flash_addr(addr) {
            return self.flash.byte(f);
        }
        if Self::is_periph(addr) {
            let c = self.q_cyc0;
            return Bus::read(self, addr, 1, c).unwrap_or(0) as u8;
        }
        0
    }
    pub fn peek32(&mut self, addr: u32) -> u32 {
        if addr & 3 == 0 && Self::is_periph(addr) {
            let c = self.q_cyc0;
            return Bus::read(self, addr, 4, c).unwrap_or(0);
        }
        u32::from_le_bytes([self.peek8(addr), self.peek8(addr + 1), self.peek8(addr + 2), self.peek8(addr + 3)])
    }
    pub fn poke8(&mut self, addr: u32, v: u8) {
        if let Some((m, o)) = self.ram_slice(addr) {
            m[o] = v;
        } else if Self::is_periph(addr) {
            let c = self.q_cyc0;
            let _ = Bus::write(self, addr, 1, v as u32, c);
        }
    }
    pub fn poke32(&mut self, addr: u32, v: u32) {
        if addr & 3 == 0 && Self::is_periph(addr) {
            let c = self.q_cyc0;
            let _ = Bus::write(self, addr, 4, v, c);
            return;
        }
        for (i, b) in v.to_le_bytes().iter().enumerate() {
            self.poke8(addr + i as u32, *b);
        }
    }
    pub fn read_cstr(&mut self, addr: u32, max: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..max as u32 {
            let b = self.peek8(addr + i);
            if b == 0 {
                break;
            }
            out.push(b);
        }
        out
    }
    pub fn read_bytes(&mut self, addr: u32, n: usize) -> Vec<u8> {
        if Self::is_periph(addr) && addr & 3 == 0 {
            // peripheral FIFOs / buffers: access word by word
            let mut out = Vec::with_capacity(n);
            let mut a = addr;
            while out.len() < n {
                let w = self.peek32(a).to_le_bytes();
                for b in w {
                    if out.len() < n {
                        out.push(b);
                    }
                }
                a += 4;
            }
            return out;
        }
        (0..n as u32).map(|i| self.peek8(addr + i)).collect()
    }
    pub fn write_bytes(&mut self, addr: u32, data: &[u8]) {
        if Self::is_periph(addr) && addr & 3 == 0 && data.len() % 4 == 0 {
            for (i, ch) in data.chunks(4).enumerate() {
                self.poke32(addr + 4 * i as u32, u32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]));
            }
            return;
        }
        for (i, &b) in data.iter().enumerate() {
            self.poke8(addr + i as u32, b);
        }
    }

    fn next_rand(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 16) as u32
    }

    // ------------------------------------------------------------------ pads / routing helpers
    /// Find the pad a GPIO-matrix output signal is routed to.
    pub fn pad_for_signal(&self, sig: u32) -> Option<u8> {
        (0..NUM_GPIO).find(|&g| self.gpio.out_sel[g] & 0x1FF == sig && (self.gpio.iomux[g] >> 12) & 7 == 2).map(|g| g as u8)
    }
    /// Find the pad feeding a GPIO-matrix input signal.
    pub fn pad_for_in_signal(&self, sig: u32) -> Option<u8> {
        let v = self.gpio.in_sel.get(sig as usize).copied().unwrap_or(0);
        if v & (1 << 7) != 0 {
            let p = v & 0x3F;
            if (p as usize) < NUM_GPIO {
                return Some(p as u8);
            }
        }
        None
    }

    fn i2c_pins(&self, n: usize) -> (Option<u8>, Option<u8>) {
        let (scl, sda) = if n == 0 { (29, 30) } else { (95, 96) };
        (self.pad_for_signal(scl).or(self.pad_for_in_signal(scl)), self.pad_for_signal(sda).or(self.pad_for_in_signal(sda)))
    }

    fn spi_pins(&self, n: usize) -> SpiPins {
        let direct = |g: u8, func: u32| (self.gpio.iomux[g as usize] >> 12) & 7 == func;
        match n {
            2 => {
                let pin = self.spi[2].regs.read(crate::periph::spi::PIN);
                let cs = if pin & 1 == 0 {
                    self.pad_for_signal(11).or(if direct(15, 1) { Some(15) } else { None })
                } else if pin & 2 == 0 {
                    self.pad_for_signal(61)
                } else if pin & 4 == 0 {
                    self.pad_for_signal(62)
                } else {
                    None
                };
                SpiPins {
                    sclk: self.pad_for_signal(8).or(if direct(14, 1) { Some(14) } else { None }),
                    mosi: self.pad_for_signal(10).or(if direct(13, 1) { Some(13) } else { None }),
                    miso: self.pad_for_in_signal(9).or(if direct(12, 1) { Some(12) } else { None }),
                    cs,
                }
            }
            3 => {
                let pin = self.spi[3].regs.read(crate::periph::spi::PIN);
                let cs = if pin & 1 == 0 {
                    self.pad_for_signal(68).or(if direct(5, 1) { Some(5) } else { None })
                } else if pin & 2 == 0 {
                    self.pad_for_signal(69)
                } else if pin & 4 == 0 {
                    self.pad_for_signal(70)
                } else {
                    None
                };
                SpiPins {
                    sclk: self.pad_for_signal(63).or(if direct(18, 1) { Some(18) } else { None }),
                    mosi: self.pad_for_signal(65).or(if direct(23, 1) { Some(23) } else { None }),
                    miso: self.pad_for_in_signal(64).or(if direct(19, 1) { Some(19) } else { None }),
                    cs,
                }
            }
            _ => SpiPins::default(),
        }
    }

    /// Re-evaluate PWM outputs of pads routed to LEDC channels.
    pub fn refresh_pwm(&mut self, now: u64) {
        let apb = self.apb_hz();
        for g in 0..NUM_GPIO {
            let sig = self.gpio.out_sel[g] & 0x1FF;
            let func = (self.gpio.iomux[g] >> 12) & 7;
            let st = if func == 2 && (71..=86).contains(&sig) {
                let ch = (sig - 71) as usize;
                let mut s = self.ledc.channel_state(ch, now, apb);
                if self.gpio.out_sel[g] & (1 << 9) != 0 {
                    s.duty = 1.0 - s.duty;
                }
                Some(s)
            } else {
                None
            };
            if st != self.pwm[g] {
                if let Some(s) = st {
                    self.io.pwm_update(now, g as u8, s);
                } else if self.pwm[g].is_some() {
                    self.io.pwm_update(now, g as u8, PwmState { freq_hz: 0.0, duty: 0.0, running: false });
                }
                self.pwm[g] = st;
            }
        }
        self.ledc.changed = false;
    }

    // ------------------------------------------------------------------ peripheral dispatch
    fn periph_read(&mut self, addr: u32, cyc: u64) -> u32 {
        let now = self.precise_now(cyc);
        let apb_hz = self.apb_hz();
        let cpu_hz = self.cpu_hz;
        let off = addr & 0xFFF;
        let page = (addr - 0x3FF0_0000) >> 12;
        macro_rules! ctx {
            () => {
                &mut Ctx { now, apb_hz, cpu_hz, io: &mut *self.io, irq: &mut self.irq }
            };
        }
        match page {
            0x00 => self.dport.read(off, &self.irq),
            0x10..=0x13 => self.dport.mmu_read(addr - 0x3FF1_0000),
            0x40 => self.uart[0].read(off, ctx!()),
            0x50 => self.uart[1].read(off, ctx!()),
            0x6E => self.uart[2].read(off, ctx!()),
            0x42 => self.spi[1].read(off, ctx!()),
            0x43 => self.spi[0].read(off, ctx!()),
            0x64 => self.spi[2].read(off, ctx!()),
            0x65 => self.spi[3].read(off, ctx!()),
            0x44 => self.gpio.read(off, ctx!()),
            0x48 => match off {
                0x000..=0x3FF => self.rtc.read(off, ctx!()),
                0x400..=0x7FF => self.gpio.rtc_read(off - 0x400, ctx!()),
                // capacitive touch: SENS_SAR_TOUCH_OUT1..5 hold the counts of pads 0..9
                // (high when untouched, low when a finger is on the pad)
                0x870..=0x880 => {
                    let pair = (off - 0x870) / 4;
                    let now = self.precise_now(cyc);
                    let mut v = 0u32;
                    for k in 0..2 {
                        let pad = (pair * 2 + k) as usize;
                        let gpio = [4u8, 0, 2, 15, 13, 12, 14, 27, 33, 32][pad];
                        let count = if self.io.touch_read(now, gpio) { 15 } else { 80 };
                        // even pads live in the upper half (IDF touch_ll_read_raw_data)
                        v |= count << (16 * (1 - k));
                    }
                    v
                }
                // SAR_TOUCH_CTRL2: measurement always done
                0x884 => self.rtc.sens_read(0x84) | (1 << 10),
                0x800..=0xBFF => self.rtc.sens_read(off - 0x800),
                _ => self.rtc.rtc_i2c.read(off),
            },
            0x49 => self.gpio.iomux_read(off),
            0x53 => self.i2c[0].read(off, ctx!()),
            0x67 => self.i2c[1].read(off, ctx!()),
            0x56 => {
                if off >= 0x800 {
                    self.rmt.ram_read(off - 0x800)
                } else {
                    self.rmt.read(off)
                }
            }
            0x59 => self.ledc.read(off, ctx!()),
            0x5A => self.efuse.read(off),
            0x47 => self.frc.read(off, ctx!()),
            0x5F => self.timg[0].read(off, ctx!()),
            0x60 => self.timg[1].read(off, ctx!()),
            0x66 => match off {
                0x7C => 0x8000_0000 | 0x1604_2000,
                _ => self.misc.read(addr),
            },
            0x75 if off == 0x144 => self.next_rand(),
            0x46 if off == 0x07C => self.misc.read(addr) | (1 << 31),
            0x4E => {
                // RF front-end / analog I2C master (undocumented): report completed
                // operations so that PHY calibration code does not spin forever.
                let v = self.misc.read(addr);
                match off {
                    0x04C => v | (1 << 24),
                    0x0C4 => v | (1 << 31) | (1 << 8),
                    0x168 => v | (1 << 31),
                    _ => v,
                }
            }
            _ => {
                if self.log_unknown && self.unknown.insert(addr) {
                    self.events.push(format!("read of unmodelled register 0x{:08x}", addr));
                }
                let v = self.misc.read(addr);
                if Self::is_radio(addr) {
                    self.radio_touched = true;
                    return self.radio_poll_read(addr, v);
                }
                v
            }
        }
    }

    /// Undocumented WiFi/BT/RF register blocks.
    fn is_radio(addr: u32) -> bool {
        matches!(addr >> 12, 0x3FF46 | 0x3FF4E | 0x3FF5C | 0x3FF5D | 0x3FF71 | 0x3FF72 | 0x3FF73 | 0x3FF74)
    }

    /// Break infinite polls on undocumented radio registers: after many identical
    /// consecutive reads, the "hardware" flips the register state.
    fn radio_poll_read(&mut self, addr: u32, v: u32) -> u32 {
        if self.radio_poll.0 == addr {
            self.radio_poll.1 += 1;
            if self.radio_poll.1 >= 2000 {
                self.radio_poll.1 = 0;
                let nv = !v;
                self.misc.write(addr, nv);
                return nv;
            }
        } else {
            self.radio_poll = (addr, 1);
        }
        v
    }

    fn periph_write(&mut self, addr: u32, val: u32, cyc: u64) {
        let now = self.precise_now(cyc);
        let apb_hz = self.apb_hz();
        let cpu_hz = self.cpu_hz;
        let off = addr & 0xFFF;
        let page = (addr - 0x3FF0_0000) >> 12;
        macro_rules! ctx {
            () => {
                &mut Ctx { now, apb_hz, cpu_hz, io: &mut *self.io, irq: &mut self.irq }
            };
        }
        match page {
            0x00 => {
                self.dport.write(off, val, &mut self.irq);
                if off == crate::periph::dport::CPU_PER_CONF {
                    self.cpu_hz = self.compute_cpu_hz();
                }
            }
            0x10..=0x13 => {
                self.dport.mmu_write(addr - 0x3FF1_0000, val);
            }
            0x40 => self.uart[0].write(off, val, ctx!()),
            0x50 => self.uart[1].write(off, val, ctx!()),
            0x6E => self.uart[2].write(off, val, ctx!()),
            0x42 | 0x43 | 0x64 | 0x65 => {
                let n = match page {
                    0x43 => 0,
                    0x42 => 1,
                    0x64 => 2,
                    _ => 3,
                };
                let pins = self.spi_pins(n);
                let mut dma = DramDma { dram: &mut self.dram };
                let ctx = &mut Ctx { now, apb_hz, cpu_hz, io: &mut *self.io, irq: &mut self.irq };
                self.spi[n].write(off, val, ctx, &mut self.flash, &mut self.chip, pins, &mut dma);
            }
            0x44 => {
                self.gpio.write(off, val, ctx!());
                self.gpio.flush(ctx!());
                if (0x530..0x5D0).contains(&off) {
                    self.refresh_pwm(now);
                }
            }
            0x48 => match off {
                0x000..=0x3FF => {
                    self.rtc.write(off, val, ctx!());
                    if off == crate::periph::rtc::CLK_CONF {
                        self.cpu_hz = self.compute_cpu_hz();
                    }
                }
                0x400..=0x7FF => {
                    self.gpio.rtc_write(off - 0x400, val);
                    self.gpio.flush(ctx!());
                }
                0x800..=0xBFF => self.rtc.sens_write(off - 0x800, val, ctx!()),
                _ => self.rtc.rtc_i2c.write(off, val),
            },
            0x49 => {
                self.gpio.iomux_write(off, val);
                self.gpio.flush(ctx!());
                self.refresh_pwm(now);
            }
            0x53 | 0x67 => {
                let n = if page == 0x53 { 0 } else { 1 };
                let (scl, sda) = self.i2c_pins(n);
                self.i2c[n].write(off, val, ctx!(), scl, sda);
            }
            0x56 => {
                if off >= 0x800 {
                    self.rmt.ram_write(off - 0x800, val)
                } else {
                    self.rmt.write(off, val, ctx!());
                    self.deliver_rmt(now);
                }
            }
            0x59 => {
                self.ledc.write(off, val, ctx!());
                self.refresh_pwm(now);
            }
            0x5A => self.efuse.write(off, val),
            0x47 => self.frc.write(off, val, ctx!()),
            0x5F => self.timg[0].write(off, val, ctx!()),
            0x60 => self.timg[1].write(off, val, ctx!()),
            0x66 => {
                self.misc.write(addr, val);
                if off == 0 {
                    self.cpu_hz = self.compute_cpu_hz();
                }
            }
            _ => {
                if self.log_unknown && self.unknown.insert(addr) {
                    self.events.push(format!("write of unmodelled register 0x{:08x} = 0x{:08x}", addr, val));
                }
                if self.radio_poll.0 == addr {
                    self.radio_poll = (0, 0);
                }
                if Self::is_radio(addr) {
                    self.radio_touched = true;
                }
                self.misc.write(addr, val)
            }
        }
    }

    fn deliver_rmt(&mut self, now: u64) {
        if self.rmt.done.is_empty() {
            return;
        }
        let done = std::mem::take(&mut self.rmt.done);
        for (ch, items) in done {
            let sig = 87 + ch as u32;
            for g in 0..NUM_GPIO {
                if self.gpio.out_sel[g] & 0x1FF == sig {
                    self.io.rmt_tx(now, g as u8, &items);
                }
            }
        }
    }

    /// Periodic peripheral processing.  Returns the next time a peripheral needs attention.
    pub fn tick(&mut self) -> Option<u64> {
        let now = self.now;
        let apb_hz = self.apb_hz();
        let cpu_hz = self.cpu_hz;
        let mut next: Option<u64> = None;
        let upd = |t: Option<u64>, next: &mut Option<u64>| {
            if let Some(t) = t {
                *next = Some(next.map_or(t, |n| n.min(t)));
            }
        };
        macro_rules! ctx {
            () => {
                &mut Ctx { now, apb_hz, cpu_hz, io: &mut *self.io, irq: &mut self.irq }
            };
        }
        for i in 0..3 {
            let t = self.uart[i].tick(ctx!());
            upd(t, &mut next);
        }
        let t = self.frc.tick(ctx!());
        upd(t, &mut next);
        for i in 0..2 {
            let t = self.timg[i].tick(ctx!());
            upd(t, &mut next);
            let t = self.i2c[i].tick(ctx!());
            upd(t, &mut next);
        }
        let t = self.rmt.tick(ctx!());
        upd(t, &mut next);
        self.deliver_rmt(now);
        let t = self.ledc.tick(ctx!());
        upd(t, &mut next);
        if self.ledc.changed {
            self.refresh_pwm(now);
        }
        self.gpio.tick(ctx!());
        let t = self.io.poll(now);
        upd(t, &mut next);
        next
    }
}

impl Bus for Sys {
    #[inline(always)]
    fn wants_break(&self) -> bool {
        self.irq.dirty || self.yield_core
    }
    #[inline]
    fn read(&mut self, addr: u32, size: u8, cyc: u64) -> Result<u32, MemFault> {
        // remembered so that a store by the other core to the word this core is
        // polling ends the storing core's time slice (see `yield_core`)
        let cc = self.cur_core & 1;
        let w = addr & !3;
        if self.last_load[cc] == w {
            self.load_repeat[cc] = self.load_repeat[cc].saturating_add(1);
        } else {
            self.last_load[cc] = w;
            self.load_repeat[cc] = 0;
        }
        if let Some((m, o)) = self.ram_slice(addr) {
            return Ok(match size {
                1 => m[o] as u32,
                2 => u16::from_le_bytes([m[o], m[o + 1]]) as u32,
                _ => u32::from_le_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]]),
            });
        }
        if let Some(f) = self.flash_addr(addr) {
            let d = &self.flash.data;
            let fo = f as usize;
            if fo + size as usize > d.len() {
                return Ok(0xFFFF_FFFF >> (32 - 8 * size as u32));
            }
            return Ok(match size {
                1 => d[fo] as u32,
                2 => u16::from_le_bytes([d[fo], d[fo + 1]]) as u32,
                _ => u32::from_le_bytes([d[fo], d[fo + 1], d[fo + 2], d[fo + 3]]),
            });
        }
        let pa = if (0x6000_0000..0x6004_0000).contains(&addr) { addr - 0x6000_0000 + 0x3FF4_0000 } else { addr };
        if (0x3FF0_0000..0x3FF8_0000).contains(&pa) {
            let w = self.periph_read(pa & !3, cyc);
            if let Some(t) = self.read_trace.as_mut() {
                if t.len() >= 64 {
                    t.pop_front();
                }
                t.push_back((pa & !3, w));
            }
            let sh = (pa & 3) * 8;
            return Ok(match size {
                1 => (w >> sh) & 0xFF,
                2 => (w >> sh) & 0xFFFF,
                _ => w,
            });
        }
        if (0x3FF9_0000..0x3FFA_0000).contains(&addr) || (0x4000_0000..0x4007_0000).contains(&addr) {
            // mask ROM: not available (functions are emulated)
            return Ok(0);
        }
        self.fault_log.push(format!("read{} from unmapped 0x{:08x}", size * 8, addr));
        Err(MemFault)
    }

    #[inline]
    fn write(&mut self, addr: u32, size: u8, val: u32, cyc: u64) -> Result<(), MemFault> {
        let other = (self.cur_core & 1) ^ 1;
        if (addr & !3) == self.last_load[other] && self.load_repeat[other] >= 2 {
            // the other core keeps re-reading this word: it is spinning on it
            self.yield_core = true;
        }
        if let Some((m, o)) = self.ram_slice(addr) {
            match size {
                1 => m[o] = val as u8,
                2 => m[o..o + 2].copy_from_slice(&(val as u16).to_le_bytes()),
                _ => m[o..o + 4].copy_from_slice(&val.to_le_bytes()),
            }
            return Ok(());
        }
        let pa = if (0x6000_0000..0x6004_0000).contains(&addr) { addr - 0x6000_0000 + 0x3FF4_0000 } else { addr };
        if (0x3FF0_0000..0x3FF8_0000).contains(&pa) {
            let v = match size {
                4 => val,
                _ => val << ((pa & 3) * 8),
            };
            self.periph_write(pa & !3, v, cyc);
            return Ok(());
        }
        if self.flash_addr(addr).is_some() {
            self.fault_log.push(format!("write to flash-mapped address 0x{:08x}", addr));
            return Err(MemFault);
        }
        self.fault_log.push(format!("write{} to unmapped 0x{:08x} = 0x{:x}", size * 8, addr, val));
        Err(MemFault)
    }

    #[inline]
    fn fetch(&mut self, addr: u32) -> Result<u32, MemFault> {
        if (IRAM_BASE..IRAM_END - 4).contains(&addr) {
            let o = (addr - IRAM_BASE) as usize;
            let m = &self.iram;
            return Ok(u32::from_le_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]]));
        }
        let mut b = [0u8; 4];
        for (i, x) in b.iter_mut().enumerate() {
            let a = addr + i as u32;
            if let Some((m, o)) = self.ram_slice(a) {
                *x = m[o];
            } else if let Some(f) = self.flash_addr(a) {
                *x = self.flash.byte(f);
            } else if i == 0 {
                self.fault_log.push(format!("instruction fetch from unmapped 0x{:08x}", addr));
                return Err(MemFault);
            }
        }
        Ok(u32::from_le_bytes(b))
    }
}

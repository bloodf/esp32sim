//! SPI controllers.
//!
//! * SPI1 talks to the emulated SPI NOR flash (used by esp_flash / NVS / filesystems).
//! * SPI2 (HSPI) and SPI3 (VSPI) are general-purpose masters connected to the circuit.
//! * SPI0 (cache controller) registers are stored but flash reads go through the MMU model.

use super::{Ctx, RegFile};
use crate::flash::Flash;
use crate::io::SpiPins;

pub const CMD: u32 = 0x00;
pub const ADDR: u32 = 0x04;
pub const CTRL: u32 = 0x08;
pub const RD_STATUS: u32 = 0x10;
pub const CLOCK: u32 = 0x18;
pub const USER: u32 = 0x1C;
pub const USER1: u32 = 0x20;
pub const USER2: u32 = 0x24;
pub const MOSI_DLEN: u32 = 0x28;
pub const MISO_DLEN: u32 = 0x2C;
pub const SLV_WR_STATUS: u32 = 0x30;
pub const PIN: u32 = 0x34;
pub const SLAVE: u32 = 0x38;
pub const W0: u32 = 0x80;
pub const EXT2: u32 = 0xF8;
pub const DMA_CONF: u32 = 0x100;
pub const DMA_OUT_LINK: u32 = 0x104;
pub const DMA_IN_LINK: u32 = 0x108;
pub const DMA_INT_ENA: u32 = 0x110;
pub const DMA_INT_RAW: u32 = 0x114;
pub const DMA_INT_ST: u32 = 0x118;
pub const DMA_INT_CLR: u32 = 0x11C;

/// Emulated SPI NOR flash chip state.
pub struct FlashChip {
    pub status: u32,
    pub jedec_id: u32,
}

impl FlashChip {
    pub fn new(size: u32) -> FlashChip {
        // GigaDevice-like id, capacity byte = log2(size)
        let cap = 31 - size.leading_zeros();
        FlashChip { status: 0, jedec_id: 0xC8_40_00 | cap }
    }
}

/// Memory access for DMA engines.
pub trait DmaMem {
    fn rd8(&mut self, addr: u32) -> u8;
    fn wr8(&mut self, addr: u32, v: u8);
    fn rd32(&mut self, addr: u32) -> u32 {
        u32::from_le_bytes([self.rd8(addr), self.rd8(addr + 1), self.rd8(addr + 2), self.rd8(addr + 3)])
    }
    fn wr32(&mut self, addr: u32, v: u32) {
        for (i, b) in v.to_le_bytes().iter().enumerate() {
            self.wr8(addr + i as u32, *b);
        }
    }
}

pub struct Spi {
    pub idx: u8,
    pub regs: RegFile,
    pub w: [u32; 16],
    pub out_link_pending: Option<u32>,
    pub in_link_pending: Option<u32>,
    pub dma_int_raw: u32,
    pub busy_until: u64,
    pub transfers: u64,
}

impl Spi {
    pub fn new(idx: u8) -> Spi {
        let mut regs = RegFile::default();
        regs.write(CLOCK, 0x8000_3043);
        regs.write(USER, 0x8000_0040);
        regs.write(USER1, 0x5C00_0007);
        regs.write(USER2, 0x7000_0000);
        regs.write(PIN, 0x6);
        regs.write(0x3FC, 0x1604270);
        Spi { idx, regs, w: [0; 16], out_link_pending: None, in_link_pending: None, dma_int_raw: 0, busy_until: 0, transfers: 0 }
    }

    fn w_bytes(&self) -> [u8; 64] {
        let mut b = [0u8; 64];
        for i in 0..16 {
            b[i * 4..i * 4 + 4].copy_from_slice(&self.w[i].to_le_bytes());
        }
        b
    }
    fn set_w_bytes(&mut self, data: &[u8], start: usize) {
        let mut b = self.w_bytes();
        for (i, &x) in data.iter().enumerate() {
            if start + i < 64 {
                b[start + i] = x;
            }
        }
        for i in 0..16 {
            self.w[i] = u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        }
    }

    pub fn clock_hz(&self, apb: u64) -> u64 {
        let c = self.regs.read(CLOCK);
        if c & (1 << 31) != 0 {
            return apb;
        }
        let pre = ((c >> 18) & 0x1FFF) as u64 + 1;
        let n = ((c >> 12) & 0x3F) as u64 + 1;
        apb / (pre * n)
    }

    fn src(&self) -> usize {
        super::src::SPI0 + self.idx as usize
    }

    fn update_irq(&mut self, ctx: &mut Ctx) {
        let slave = self.regs.read(SLAVE);
        let lvl = slave & (1 << 4) != 0 && slave & (1 << 9) != 0;
        let s = self.src();
        ctx.irq.set(s, lvl);
        let dma = self.dma_int_raw & self.regs.read(DMA_INT_ENA) != 0;
        let ds = match self.idx {
            1 => super::src::SPI1_DMA,
            2 => super::src::SPI2_DMA,
            _ => super::src::SPI3_DMA,
        };
        if self.idx >= 1 {
            ctx.irq.set(ds, dma);
        }
    }

    pub fn read(&mut self, off: u32, ctx: &mut Ctx) -> u32 {
        match off {
            CMD => {
                // operations complete instantly, except a timed user transfer on SPI2/3
                if self.idx >= 2 && ctx.now < self.busy_until {
                    1 << 18
                } else {
                    0
                }
            }
            o if (W0..W0 + 64).contains(&o) => self.w[((o - W0) / 4) as usize],
            EXT2 => 0,
            DMA_INT_RAW => self.dma_int_raw,
            DMA_INT_ST => self.dma_int_raw & self.regs.read(DMA_INT_ENA),
            0x10C => 0, // dma_status: idle
            _ => self.regs.read(off),
        }
    }

    /// Register write.  `flash` is only used by SPI0/1, `pins` describe the routing for SPI2/3.
    pub fn write(
        &mut self,
        off: u32,
        val: u32,
        ctx: &mut Ctx,
        flash: &mut Flash,
        chip: &mut FlashChip,
        pins: SpiPins,
        mem: &mut dyn DmaMem,
    ) {
        match off {
            CMD => {
                if val != 0 {
                    if self.idx <= 1 {
                        self.flash_cmd(val, flash, chip);
                    } else if val & (1 << 18) != 0 {
                        self.user_transfer(ctx, pins, mem);
                    }
                }
            }
            o if (W0..W0 + 64).contains(&o) => self.w[((o - W0) / 4) as usize] = val,
            DMA_OUT_LINK => {
                if val & (1 << 29) != 0 {
                    self.out_link_pending = Some(0x3FF0_0000 | (val & 0xFFFFF));
                }
                if val & (1 << 28) != 0 {
                    self.out_link_pending = None;
                }
                self.regs.write(off, val & 0xFFFFF);
            }
            DMA_IN_LINK => {
                if val & (1 << 29) != 0 {
                    self.in_link_pending = Some(0x3FF0_0000 | (val & 0xFFFFF));
                }
                if val & (1 << 28) != 0 {
                    self.in_link_pending = None;
                }
                self.regs.write(off, val & 0xFFFFF);
            }
            DMA_INT_CLR => {
                self.dma_int_raw &= !val;
                self.update_irq(ctx);
            }
            DMA_INT_ENA | SLAVE => {
                self.regs.write(off, val);
                self.update_irq(ctx);
            }
            _ => self.regs.write(off, val),
        }
    }

    // ------------------------------------------------------------------ flash (SPI0/1)
    fn flash_cmd(&mut self, cmd: u32, flash: &mut Flash, chip: &mut FlashChip) {
        let addr = self.regs.read(ADDR);
        if std::env::var("DEBUG_SPIFLASH").is_ok() && cmd & !(1 << 18) != 0 {
            eprintln!("spiflash cmd=0x{:08x} addr=0x{:x}", cmd, addr);
        }
        let wel = chip.status & 2 != 0;
        if cmd & (1 << 30) != 0 {
            chip.status |= 2;
        }
        if cmd & (1 << 29) != 0 {
            chip.status &= !2;
        }
        if cmd & (1 << 28) != 0 {
            self.w[0] = ((chip.jedec_id >> 16) & 0xFF) | (chip.jedec_id & 0xFF00) | ((chip.jedec_id & 0xFF) << 16);
        }
        if cmd & (1 << 27) != 0 {
            self.regs.write(RD_STATUS, chip.status & 0xFFFF);
        }
        if cmd & (1 << 26) != 0 && wel {
            chip.status = self.regs.read(RD_STATUS) & 0xFFFC;
        }
        if cmd & (1 << 25) != 0 && wel {
            let a = addr & 0xFF_FFFF;
            let len = ((addr >> 24) & 0xFF).min(64) as usize;
            let data = self.w_bytes();
            program_page(flash, a, &data[..len]);
            chip.status &= !2;
        }
        if cmd & (1 << 24) != 0 && wel {
            flash.erase(addr & 0xFF_F000, 0x1000);
            chip.status &= !2;
        }
        if cmd & (1 << 23) != 0 && wel {
            flash.erase(addr & 0xFF_0000, 0x10000);
            chip.status &= !2;
        }
        if cmd & (1 << 22) != 0 && wel {
            let n = flash.size();
            flash.erase(0, n);
            chip.status &= !2;
        }
        if cmd & (1 << 31) != 0 {
            // legacy read: length from MISO_DLEN, address in ADDR[23:0]
            let len = ((self.regs.read(MISO_DLEN) & 0xFFFFFF) + 1).div_ceil(8).min(64) as usize;
            let mut buf = vec![0u8; len];
            flash.read(addr & 0xFF_FFFF, &mut buf);
            self.set_w_bytes(&buf, 0);
        }
        if cmd & (1 << 18) != 0 {
            self.flash_user(flash, chip);
        }
    }

    fn flash_user(&mut self, flash: &mut Flash, chip: &mut FlashChip) {
        let user = self.regs.read(USER);
        let user2 = self.regs.read(USER2);
        let pin = self.regs.read(PIN);
        let cs0 = pin & 1 == 0;
        let command = if user & (1 << 31) != 0 { Some((user2 & 0xFF) as u8) } else { None };
        let addr_bits = if user & (1 << 30) != 0 { ((self.regs.read(USER1) >> 26) & 0x3F) + 1 } else { 0 };
        let addr_raw = self.regs.read(ADDR);
        let mut addr = if addr_bits == 0 {
            0
        } else if addr_bits >= 32 {
            addr_raw
        } else {
            addr_raw >> (32 - addr_bits)
        };
        // dual/quad I/O reads append mode bits after the 24-bit address
        if addr_bits > 24 && addr_bits < 32 {
            addr >>= addr_bits - 24;
        }
        let mosi_len = if user & (1 << 27) != 0 { ((self.regs.read(MOSI_DLEN) & 0xFFFFFF) + 1).div_ceil(8) } else { 0 } as usize;
        let miso_len = if user & (1 << 28) != 0 { ((self.regs.read(MISO_DLEN) & 0xFFFFFF) + 1).div_ceil(8) } else { 0 } as usize;
        let mosi = self.w_bytes();
        let mut miso = vec![0xFFu8; miso_len.min(64)];
        if !cs0 {
            // another chip select (e.g. PSRAM probe on CS1): nothing there
            if miso_len > 0 {
                let off = if user & (1 << 24) != 0 { 32 } else { 0 };
                self.set_w_bytes(&miso, off);
            }
            return;
        }
        let wel = chip.status & 2 != 0;
        if std::env::var("DEBUG_SPIFLASH").is_ok() {
            eprintln!(
                "spiflash usr cmd={:?} addr=0x{:x} (bits {}) mosi={} miso={} user=0x{:08x} cs0={}",
                command.map(|c| format!("0x{:02x}", c)),
                addr,
                addr_bits,
                mosi_len,
                miso_len,
                user,
                cs0
            );
        }
        if let Some(c) = command {
            match c {
                0x9F => {
                    let id = [(chip.jedec_id >> 16) as u8, (chip.jedec_id >> 8) as u8, chip.jedec_id as u8];
                    for (i, b) in miso.iter_mut().enumerate() {
                        *b = *id.get(i).unwrap_or(&0xFF);
                    }
                }
                0x05 => {
                    for b in miso.iter_mut() {
                        *b = chip.status as u8;
                    }
                }
                0x35 => {
                    for b in miso.iter_mut() {
                        *b = (chip.status >> 8) as u8;
                    }
                }
                0x15 => {
                    for b in miso.iter_mut() {
                        *b = 0;
                    }
                }
                0x06 => chip.status |= 2,
                0x04 => chip.status &= !2,
                0x01 => {
                    if wel {
                        let v = mosi[0] as u32 | if mosi_len > 1 { (mosi[1] as u32) << 8 } else { chip.status & 0xFF00 };
                        chip.status = v & 0xFFFC;
                    }
                }
                0x31 => {
                    if wel {
                        chip.status = (chip.status & 0xFF) | ((mosi[0] as u32) << 8);
                    }
                }
                0x03 | 0x0B | 0x3B | 0xBB | 0x6B | 0xEB => {
                    flash.read(addr & 0xFF_FFFF, &mut miso);
                    if std::env::var("DEBUG_SPIFLASH").is_ok() {
                        eprintln!("   read @0x{:x}: {:02x?}", addr, &miso[..miso.len().min(8)]);
                    }
                }
                0x02 | 0x32 | 0x38 => {
                    if wel {
                        program_page(flash, addr & 0xFF_FFFF, &mosi[..mosi_len.min(64)]);
                        chip.status &= !2;
                    }
                }
                0x20 => {
                    if wel {
                        flash.erase(addr & 0xFF_F000, 0x1000);
                        chip.status &= !2;
                    }
                }
                0x52 => {
                    if wel {
                        flash.erase(addr & 0xFF_8000, 0x8000);
                        chip.status &= !2;
                    }
                }
                0xD8 => {
                    if wel {
                        flash.erase(addr & 0xFF_0000, 0x10000);
                        chip.status &= !2;
                    }
                }
                0x60 | 0xC7 => {
                    if wel {
                        let n = flash.size();
                        flash.erase(0, n);
                        chip.status &= !2;
                    }
                }
                0x4B => {
                    for (i, b) in miso.iter_mut().enumerate() {
                        *b = 0x10 + i as u8;
                    }
                }
                0x5A => {
                    // SFDP: not supported -> 0xFF
                }
                _ => {}
            }
        }
        if miso_len > 0 {
            let off = if user & (1 << 24) != 0 { 32 } else { 0 };
            self.set_w_bytes(&miso, off);
        }
    }

    // ------------------------------------------------------------------ general purpose master (SPI2/3)
    fn user_transfer(&mut self, ctx: &mut Ctx, pins: SpiPins, mem: &mut dyn DmaMem) {
        let user = self.regs.read(USER);
        let user2 = self.regs.read(USER2);
        let mut bits_total: u64 = 0;
        let mut tx: Vec<u8> = Vec::new();
        // command phase
        if user & (1 << 31) != 0 {
            let bitlen = ((user2 >> 28) & 0xF) + 1;
            let v = user2 & 0xFFFF;
            // ESP32 sends bits 7..0 then 15..8
            let nbytes = bitlen.div_ceil(8);
            for i in 0..nbytes {
                tx.push((v >> (8 * i)) as u8);
            }
            bits_total += bitlen as u64;
        }
        // address phase
        if user & (1 << 30) != 0 {
            let bitlen = ((self.regs.read(USER1) >> 26) & 0x3F) + 1;
            let a = self.regs.read(ADDR);
            let nbytes = bitlen.div_ceil(8);
            for i in 0..nbytes.min(4) {
                tx.push((a >> (24 - 8 * i)) as u8);
            }
            if nbytes > 4 {
                let b = self.regs.read(SLV_WR_STATUS);
                for i in 0..(nbytes - 4).min(4) {
                    tx.push((b >> (24 - 8 * i)) as u8);
                }
            }
            bits_total += bitlen as u64;
        }
        if user & (1 << 29) != 0 {
            bits_total += ((self.regs.read(USER1) & 0xFF) + 1) as u64;
        }
        let header_len = tx.len();
        let mosi_on = user & (1 << 27) != 0;
        let miso_on = user & (1 << 28) != 0;
        let mosi_bits = if mosi_on { (self.regs.read(MOSI_DLEN) & 0xFFFFFF) + 1 } else { 0 };
        let miso_bits = if miso_on { (self.regs.read(MISO_DLEN) & 0xFFFFFF) + 1 } else { 0 };
        let full_duplex = user & 1 != 0;
        let data_bits = if full_duplex { mosi_bits.max(miso_bits) } else { mosi_bits };
        let mosi_bytes = mosi_bits.div_ceil(8) as usize;
        let miso_bytes = miso_bits.div_ceil(8) as usize;
        // gather MOSI data
        if mosi_on {
            if let Some(link) = self.out_link_pending.take() {
                let mut d = dma_read_chain(mem, link);
                d.resize(mosi_bytes, 0);
                tx.extend_from_slice(&d);
                self.dma_int_raw |= (1 << 6) | (1 << 7) | (1 << 8);
            } else {
                let w = self.w_bytes();
                let off = if user & (1 << 25) != 0 { 32 } else { 0 };
                for i in 0..mosi_bytes.min(64 - off) {
                    tx.push(w[off + i]);
                }
            }
        }
        let mut buf = tx.clone();
        let total_len = header_len + mosi_bytes.max(if full_duplex || !mosi_on { miso_bytes } else { 0 });
        buf.resize(total_len, 0xFF);
        // half duplex read phase follows the write phase
        let half_read = miso_on && !full_duplex && mosi_on;
        if half_read {
            buf.resize(header_len + mosi_bytes + miso_bytes, 0xFF);
        }
        bits_total += (buf.len() - header_len) as u64 * 8;
        let _ = data_bits;
        let nbits = (buf.len() * 8) as u32;
        ctx.io.spi_transfer(ctx.now, self.idx, pins, &mut buf, nbits);
        self.transfers += 1;
        if miso_on {
            let start = if half_read { header_len + mosi_bytes } else { header_len };
            let rx: Vec<u8> = buf[start.min(buf.len())..(start + miso_bytes).min(buf.len())].to_vec();
            if let Some(link) = self.in_link_pending.take() {
                dma_write_chain(mem, link, &rx);
                self.dma_int_raw |= (1 << 3) | (1 << 5);
            } else {
                let off = if user & (1 << 24) != 0 { 32 } else { 0 };
                self.set_w_bytes(&rx, off);
            }
        }
        let hz = self.clock_hz(ctx.apb_hz).max(1);
        let dur = bits_total * 1_000_000_000 / hz;
        self.busy_until = ctx.now + dur;
        let slave = self.regs.read(SLAVE) | (1 << 4);
        self.regs.write(SLAVE, slave);
        self.update_irq(ctx);
    }
}

fn program_page(flash: &mut Flash, addr: u32, data: &[u8]) {
    // page program wraps within a 256-byte page
    let page = addr & !0xFF;
    for (i, &b) in data.iter().enumerate() {
        let a = page | ((addr + i as u32) & 0xFF);
        flash.program(a, &[b]);
    }
}

/// Read a DMA descriptor chain (lldesc_t) and return the concatenated data.
pub fn dma_read_chain(mem: &mut dyn DmaMem, mut desc: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..1024 {
        if desc == 0 || desc < 0x3FF0_0000 {
            break;
        }
        let w0 = mem.rd32(desc);
        let buf = mem.rd32(desc + 4);
        let next = mem.rd32(desc + 8);
        let length = (w0 >> 12) & 0xFFF;
        for i in 0..length {
            out.push(mem.rd8(buf + i));
        }
        let eof = w0 & (1 << 30) != 0;
        // clear owner bit
        mem.wr32(desc, w0 & !(1 << 31));
        if eof || next == 0 {
            break;
        }
        desc = next;
    }
    out
}

/// Write data into a DMA receive descriptor chain.
pub fn dma_write_chain(mem: &mut dyn DmaMem, mut desc: u32, data: &[u8]) {
    let mut pos = 0usize;
    for _ in 0..1024 {
        if desc == 0 || desc < 0x3FF0_0000 || pos >= data.len() {
            break;
        }
        let w0 = mem.rd32(desc);
        let buf = mem.rd32(desc + 4);
        let next = mem.rd32(desc + 8);
        let size = (w0 & 0xFFF) as usize;
        let n = size.min(data.len() - pos);
        for i in 0..n {
            mem.wr8(buf + i as u32, data[pos + i]);
        }
        pos += n;
        let eof = pos >= data.len();
        let nw0 = (w0 & !(0xFFF << 12) & !(1 << 31)) | ((n as u32) << 12) | if eof { 1 << 30 } else { 0 };
        mem.wr32(desc, nw0);
        if eof || next == 0 {
            break;
        }
        desc = next;
    }
}

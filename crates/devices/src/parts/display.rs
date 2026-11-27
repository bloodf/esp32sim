//! Displays: SSD1306 / SH1106 OLEDs, HD44780 character LCDs, SPI TFTs, MAX7219, TM1637, 7-segment.

use crate::parts::{hex, prop_f64, prop_str, prop_u8, Props};
use crate::{pin, Device, PinInfo, PinKind::*, PinView, Rail, Stamp};
use serde_json::{json, Value};

// ===================================================================== SSD1306 / SH1106
pub struct OledCore {
    pub w: usize,
    pub h: usize,
    sh1106: bool,
    ram: Vec<u8>,
    ram_w: usize,
    mode: u8,
    col_start: usize,
    col_end: usize,
    page_start: usize,
    page_end: usize,
    col: usize,
    page: usize,
    pub on: bool,
    pub invert: bool,
    pub contrast: u8,
    seg_remap: bool,
    com_flip: bool,
    start_line: usize,
    cmd: Vec<u8>,
    pub version: u64,
}

impl OledCore {
    pub fn new(w: usize, h: usize, sh1106: bool) -> OledCore {
        let ram_w = if sh1106 { 132 } else { 128 };
        OledCore {
            w,
            h,
            sh1106,
            ram: vec![0; ram_w * 8],
            ram_w,
            mode: if sh1106 { 2 } else { 2 },
            col_start: 0,
            col_end: 127,
            page_start: 0,
            page_end: 7,
            col: 0,
            page: 0,
            on: false,
            invert: false,
            contrast: 0x7F,
            seg_remap: false,
            com_flip: false,
            start_line: 0,
            cmd: Vec::new(),
            version: 0,
        }
    }

    fn args_needed(c: u8) -> usize {
        match c {
            0x81 | 0x20 | 0xA8 | 0xD3 | 0xD5 | 0xD9 | 0xDA | 0xDB | 0x8D | 0xAD => 1,
            0x21 | 0x22 | 0xA3 => 2,
            0x26 | 0x27 => 6,
            0x29 | 0x2A => 5,
            _ => 0,
        }
    }

    pub fn command(&mut self, b: u8) {
        self.cmd.push(b);
        let c = self.cmd[0];
        if self.cmd.len() <= Self::args_needed(c) {
            return;
        }
        let a = self.cmd.clone();
        self.cmd.clear();
        match c {
            0xAE => self.on = false,
            0xAF => self.on = true,
            0x81 => self.contrast = a[1],
            0xA6 => self.invert = false,
            0xA7 => self.invert = true,
            0x20 => self.mode = a[1] & 3,
            0x21 => {
                self.col_start = a[1] as usize & 127;
                self.col_end = a[2] as usize & 127;
                self.col = self.col_start;
            }
            0x22 => {
                self.page_start = a[1] as usize & 7;
                self.page_end = a[2] as usize & 7;
                self.page = self.page_start;
            }
            0xA0 => self.seg_remap = false,
            0xA1 => self.seg_remap = true,
            0xC0 => self.com_flip = false,
            0xC8 => self.com_flip = true,
            0x40..=0x7F => self.start_line = (c & 0x3F) as usize,
            0xB0..=0xB7 => self.page = (c & 7) as usize,
            0x00..=0x0F => self.col = (self.col & 0xF0) | (c & 0xF) as usize,
            0x10..=0x1F => self.col = (self.col & 0x0F) | (((c & 0xF) as usize) << 4),
            _ => {}
        }
        self.version += 1;
    }

    pub fn data(&mut self, b: u8) {
        let col = self.col.min(self.ram_w - 1);
        self.ram[self.page * self.ram_w + col] = b;
        self.version += 1;
        match self.mode {
            0 => {
                // horizontal addressing
                if self.col >= self.col_end {
                    self.col = self.col_start;
                    self.page = if self.page >= self.page_end { self.page_start } else { self.page + 1 };
                } else {
                    self.col += 1;
                }
            }
            1 => {
                if self.page >= self.page_end {
                    self.page = self.page_start;
                    self.col = if self.col >= self.col_end { self.col_start } else { self.col + 1 };
                } else {
                    self.page += 1;
                }
            }
            _ => {
                if self.col + 1 < self.ram_w {
                    self.col += 1;
                }
            }
        }
    }

    /// Displayed pixel at (x, y), taking remap/flip/start-line into account.
    pub fn pixel(&self, x: usize, y: usize) -> bool {
        let xoff = if self.sh1106 { 2 } else { 0 };
        let col = if self.seg_remap { x + xoff } else { self.ram_w - 1 - x - xoff.min(self.ram_w - 1 - x) };
        let rows = 64;
        let yy = if self.com_flip { y } else { (self.h - 1) - y };
        let row = (yy + self.start_line) % rows;
        let v = self.ram[(row / 8) * self.ram_w + col.min(self.ram_w - 1)] & (1 << (row % 8)) != 0;
        v != self.invert
    }

    /// Packed 1bpp bitmap (row-major, MSB first) as hex.
    pub fn bitmap_hex(&self) -> String {
        let mut out = vec![0u8; self.w * self.h / 8];
        if self.on {
            for y in 0..self.h {
                for x in 0..self.w {
                    if self.pixel(x, y) {
                        let i = y * self.w + x;
                        out[i / 8] |= 0x80 >> (i % 8);
                    }
                }
            }
        }
        hex(&out)
    }

    pub fn state(&self) -> Value {
        json!({"width": self.w, "height": self.h, "on": self.on, "contrast": self.contrast, "bitmap": self.bitmap_hex(), "version": self.version})
    }
}

pub struct Oled {
    kind: &'static str,
    pub core: OledCore,
    addr: u8,
    spi: bool,
    /// I2C control-byte state: None = expecting control byte
    ctrl: Option<(bool, bool)>,
    first: bool,
}

impl Oled {
    pub fn new(p: &Props, sh1106: bool, spi: bool) -> Oled {
        let w = prop_f64(p, &["width"], 128.0) as usize;
        let h = prop_f64(p, &["height"], 64.0) as usize;
        Oled {
            kind: match (sh1106, spi) {
                (false, false) => "ssd1306",
                (true, false) => "sh1106",
                (false, true) => "ssd1306-spi",
                (true, true) => "sh1106-spi",
            },
            core: OledCore::new(w.min(128), h.min(64), sh1106),
            addr: prop_u8(p, &["address", "addr"], 0x3C),
            spi,
            ctrl: None,
            first: true,
        }
    }
}

impl Device for Oled {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn pins(&self) -> Vec<PinInfo> {
        if self.spi {
            vec![pin("GND", Ground), pin("VCC", Power), pin("SCK", Input), pin("MOSI", Input), pin("RES", Input), pin("DC", Input), pin("CS", Input)]
        } else {
            vec![pin("GND", Ground), pin("VCC", Power), pin("SCL", Input), pin("SDA", Io)]
        }
    }
    fn i2c_pins(&self) -> Option<(usize, usize)> {
        if self.spi {
            None
        } else {
            Some((2, 3))
        }
    }
    fn i2c_start(&mut self, addr: u8, read: bool) -> bool {
        if addr != self.addr {
            return false;
        }
        if !read {
            self.ctrl = None;
            self.first = true;
        }
        true
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        match self.ctrl {
            None => {
                // control byte: Co (bit7), D/C# (bit6)
                self.ctrl = Some((b & 0x80 != 0, b & 0x40 != 0));
            }
            Some((co, dc)) => {
                if dc {
                    self.core.data(b);
                } else {
                    self.core.command(b);
                }
                if co {
                    self.ctrl = None;
                }
            }
        }
        true
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        // status: display on/off in bit 6 (inverted)
        if self.core.on {
            0x00
        } else {
            0x40
        }
    }
    fn spi_pins(&self) -> Option<(usize, usize, Option<usize>, usize)> {
        if self.spi {
            Some((2, 3, None, 6))
        } else {
            None
        }
    }
    fn spi_transfer(&mut self, _now: u64, data: &mut [u8], lv: &dyn Fn(usize) -> bool) {
        let dc = lv(5);
        for b in data.iter_mut() {
            if dc {
                self.core.data(*b);
            } else {
                self.core.command(*b);
            }
            *b = 0xFF;
        }
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        if self.spi && v.level(4) == Some(false) {
            // held in reset
            let (w, h) = (self.core.w, self.core.h);
            let sh = self.kind.starts_with("sh");
            if self.core.version != 0 {
                self.core = OledCore::new(w, h, sh);
            }
        }
    }
    fn state(&self) -> Value {
        self.core.state()
    }
    fn summary(&self) -> String {
        let lit = (0..self.core.h).map(|y| (0..self.core.w).filter(|&x| self.core.pixel(x, y)).count()).sum::<usize>();
        format!("{} {}x{}, {} pixels lit", if self.core.on { "ON" } else { "off" }, self.core.w, self.core.h, if self.core.on { lit } else { 0 })
    }
}

// ===================================================================== HD44780 character LCD
pub struct Hd44780 {
    pub cols: usize,
    pub rows: usize,
    ddram: [u8; 128],
    cgram: [u8; 64],
    ac: u8,
    to_cgram: bool,
    inc: bool,
    shift_display: bool,
    pub display_on: bool,
    cursor: bool,
    blink: bool,
    eight_bit: bool,
    pending_hi: Option<u8>,
    offset: i32,
    pub version: u64,
}

impl Hd44780 {
    pub fn new(cols: usize, rows: usize) -> Hd44780 {
        Hd44780 {
            cols,
            rows,
            ddram: [0x20; 128],
            cgram: [0; 64],
            ac: 0,
            to_cgram: false,
            inc: true,
            shift_display: false,
            display_on: false,
            cursor: false,
            blink: false,
            eight_bit: true,
            pending_hi: None,
            offset: 0,
            version: 0,
        }
    }

    /// 4-bit interface: feed the nibble present on D4..D7 at an E falling edge.
    pub fn nibble(&mut self, rs: bool, n: u8) {
        if self.eight_bit {
            // in 8-bit mode with only D4..D7 wired, D0..D3 read as 0
            self.write(rs, n << 4);
            return;
        }
        match self.pending_hi.take() {
            None => self.pending_hi = Some(n),
            Some(hi) => self.write(rs, (hi << 4) | (n & 0xF)),
        }
    }

    pub fn write(&mut self, rs: bool, b: u8) {
        self.version += 1;
        if rs {
            if self.to_cgram {
                self.cgram[(self.ac & 0x3F) as usize] = b & 0x1F;
                self.ac = (self.ac + 1) & 0x3F;
            } else {
                self.ddram[(self.ac & 0x7F) as usize] = b;
                self.ac = if self.inc { self.ac.wrapping_add(1) } else { self.ac.wrapping_sub(1) } & 0x7F;
                if self.shift_display {
                    self.offset += if self.inc { 1 } else { -1 };
                }
            }
            return;
        }
        if b & 0x80 != 0 {
            self.ac = b & 0x7F;
            self.to_cgram = false;
        } else if b & 0x40 != 0 {
            self.ac = b & 0x3F;
            self.to_cgram = true;
        } else if b & 0x20 != 0 {
            self.eight_bit = b & 0x10 != 0;
            self.pending_hi = None;
        } else if b & 0x10 != 0 {
            let display = b & 0x08 != 0;
            let right = b & 0x04 != 0;
            if display {
                self.offset += if right { -1 } else { 1 };
            } else {
                self.ac = if right { self.ac.wrapping_add(1) } else { self.ac.wrapping_sub(1) } & 0x7F;
            }
        } else if b & 0x08 != 0 {
            self.display_on = b & 0x04 != 0;
            self.cursor = b & 0x02 != 0;
            self.blink = b & 0x01 != 0;
        } else if b & 0x04 != 0 {
            self.inc = b & 0x02 != 0;
            self.shift_display = b & 0x01 != 0;
        } else if b & 0x02 != 0 {
            self.ac = 0;
            self.offset = 0;
            self.to_cgram = false;
        } else if b & 0x01 != 0 {
            self.ddram = [0x20; 128];
            self.ac = 0;
            self.inc = true;
            self.offset = 0;
            self.to_cgram = false;
        }
    }

    fn row_addr(&self, r: usize) -> usize {
        match r {
            0 => 0x00,
            1 => 0x40,
            2 => self.cols,
            _ => 0x40 + self.cols,
        }
    }

    /// Character codes visible on each row.
    pub fn lines(&self) -> Vec<Vec<u8>> {
        (0..self.rows)
            .map(|r| {
                let base = self.row_addr(r);
                (0..self.cols)
                    .map(|c| {
                        let line_len: i32 = 40;
                        let pos = ((c as i32 + self.offset).rem_euclid(line_len)) as usize;
                        let line_start = if base >= 0x40 { 0x40 } else { 0 };
                        let a = line_start + (base - line_start + pos) % 40;
                        self.ddram[a & 0x7F]
                    })
                    .collect()
            })
            .collect()
    }

    pub fn text_lines(&self) -> Vec<String> {
        self.lines()
            .iter()
            .map(|l| {
                l.iter()
                    .map(|&c| match c {
                        0..=7 => char::from_u32(0x2460 + c as u32).unwrap_or('?'),
                        0x20..=0x7D => c as char,
                        0x7E => '→',
                        0x7F => '←',
                        0xDF => '°',
                        _ => '▯',
                    })
                    .collect()
            })
            .collect()
    }

    pub fn state(&self, backlight: bool) -> Value {
        let cur_row = if self.ac >= 0x40 { 1 } else { 0 } + if (self.ac & 0x3F) as usize >= self.cols && self.rows > 2 { 2 } else { 0 };
        let codes: Vec<String> = self.lines().iter().map(|l| hex(l)).collect();
        json!({
            "cols": self.cols, "rows": self.rows, "on": self.display_on, "backlight": backlight,
            "text": self.text_lines(), "codes": codes, "cgram": hex(&self.cgram),
            "cursor": {"visible": self.cursor, "blink": self.blink, "row": cur_row.min(self.rows - 1), "col": (self.ac & 0x3F) as usize % self.cols.max(1)},
            "version": self.version
        })
    }
}

/// LCD with PCF8574 I2C backpack (P0=RS P1=RW P2=E P3=BL P4..P7=D4..D7).
pub struct LcdI2c {
    pub lcd: Hd44780,
    addr: u8,
    port: u8,
    pub backlight: bool,
}
impl LcdI2c {
    pub fn new(p: &Props, default_rows: usize) -> LcdI2c {
        let cols = prop_f64(p, &["cols", "columns"], if default_rows == 4 { 20.0 } else { 16.0 }) as usize;
        let rows = prop_f64(p, &["rows"], default_rows as f64) as usize;
        LcdI2c { lcd: Hd44780::new(cols, rows), addr: prop_u8(p, &["address", "addr"], 0x27), port: 0xFF, backlight: false }
    }
}
impl Device for LcdI2c {
    fn kind(&self) -> &'static str {
        if self.lcd.rows == 4 {
            "lcd2004-i2c"
        } else {
            "lcd1602-i2c"
        }
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("GND", Ground), pin("VCC", Power), pin("SDA", Io), pin("SCL", Input)]
    }
    fn i2c_pins(&self) -> Option<(usize, usize)> {
        Some((3, 2))
    }
    fn i2c_start(&mut self, addr: u8, _read: bool) -> bool {
        addr == self.addr
    }
    fn i2c_write(&mut self, b: u8) -> bool {
        let old = self.port;
        self.port = b;
        self.backlight = b & 0x08 != 0;
        // E falling edge latches
        if old & 0x04 != 0 && b & 0x04 == 0 {
            let rs = old & 0x01 != 0;
            let rw = old & 0x02 != 0;
            if !rw {
                self.lcd.nibble(rs, old >> 4);
            }
        }
        true
    }
    fn i2c_read(&mut self, _ack: bool) -> u8 {
        self.port
    }
    fn state(&self) -> Value {
        self.lcd.state(self.backlight)
    }
    fn summary(&self) -> String {
        format!("|{}|", self.lcd.text_lines().join("|"))
    }
}

/// HD44780 wired directly to GPIOs (4-bit or 8-bit).
pub struct LcdParallel {
    pub lcd: Hd44780,
    last_e: bool,
}
impl LcdParallel {
    pub fn new(p: &Props) -> LcdParallel {
        let cols = prop_f64(p, &["cols", "columns"], 16.0) as usize;
        let rows = prop_f64(p, &["rows"], 2.0) as usize;
        LcdParallel { lcd: Hd44780::new(cols, rows), last_e: false }
    }
}
impl Device for LcdParallel {
    fn kind(&self) -> &'static str {
        "lcd-parallel"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![
            pin("VSS", Ground),
            pin("VDD", Power),
            pin("V0", Analog),
            pin("RS", Input),
            pin("RW", Input),
            pin("E", Input),
            pin("D0", Input),
            pin("D1", Input),
            pin("D2", Input),
            pin("D3", Input),
            pin("D4", Input),
            pin("D5", Input),
            pin("D6", Input),
            pin("D7", Input),
            pin("A", Power),
            pin("K", Ground),
        ]
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        let e = v.level(5).unwrap_or(false);
        if self.last_e && !e {
            let rs = v.level(3).unwrap_or(false);
            let rw = v.level(4).unwrap_or(false);
            if !rw {
                let hi: u8 = (0..4).map(|i| (v.level(10 + i).unwrap_or(false) as u8) << i).sum();
                let d03 = (0..4).any(|i| v.v(6 + i).is_some());
                if d03 && self.lcd.eight_bit {
                    let lo: u8 = (0..4).map(|i| (v.level(6 + i).unwrap_or(false) as u8) << i).sum();
                    self.lcd.write(rs, (hi << 4) | lo);
                } else {
                    self.lcd.nibble(rs, hi);
                }
            }
        }
        self.last_e = e;
    }
    fn state(&self) -> Value {
        self.lcd.state(true)
    }
    fn summary(&self) -> String {
        format!("|{}|", self.lcd.text_lines().join("|"))
    }
}

// ===================================================================== SPI TFT (ILI9341 / ST7735 / ST7789)
pub struct Tft {
    kind: &'static str,
    pub w: usize,
    pub h: usize,
    pub fb: Vec<u16>,
    cmd: u8,
    args: Vec<u8>,
    xs: usize,
    xe: usize,
    ys: usize,
    ye: usize,
    x: usize,
    y: usize,
    madctl: u8,
    pub on: bool,
    invert: bool,
    pix_hi: Option<u8>,
    colmod16: bool,
    /// Physical panel wiring relative to the controller's default scan: real
    /// modules are mounted so that the common libraries' "rotation 0" MADCTL
    /// value (ILI9341: MX|BGR, ST7735: MX|MY) shows an upright image.
    flip_x: bool,
    flip_y: bool,
    panel_bgr: bool,
    pub version: u64,
}
impl Tft {
    pub fn new(p: &Props, kind: &'static str) -> Tft {
        let (dw, dh) = match kind {
            "ili9341" => (240.0, 320.0),
            "st7789" => (240.0, 240.0),
            _ => (128.0, 160.0),
        };
        let w = prop_f64(p, &["width"], dw) as usize;
        let h = prop_f64(p, &["height"], dh) as usize;
        let (fx, fy, bgr) = match kind {
            "ili9341" => (true, false, true),
            "st7735" => (true, true, false),
            _ => (false, false, false),
        };
        let flag = |k: &str, d: bool| p.get(k).and_then(|v| v.as_bool()).unwrap_or(d);
        Tft {
            flip_x: flag("panel_flip_x", fx),
            flip_y: flag("panel_flip_y", fy),
            panel_bgr: flag("panel_bgr", bgr),
            kind,
            w,
            h,
            fb: vec![0; w * h],
            cmd: 0,
            args: Vec::new(),
            xs: 0,
            xe: w - 1,
            ys: 0,
            ye: h - 1,
            x: 0,
            y: 0,
            madctl: 0,
            on: false,
            invert: false,
            pix_hi: None,
            colmod16: true,
            version: 0,
        }
    }
    fn command(&mut self, c: u8) {
        self.cmd = c;
        self.args.clear();
        self.pix_hi = None;
        match c {
            0x01 => {
                self.fb.iter_mut().for_each(|p| *p = 0);
                self.on = false;
            }
            0x28 => self.on = false,
            0x29 => self.on = true,
            0x20 => self.invert = false,
            0x21 => self.invert = true,
            0x2C => {
                self.x = self.xs;
                self.y = self.ys;
            }
            _ => {}
        }
        self.version += 1;
    }
    fn data(&mut self, b: u8) {
        match self.cmd {
            0x2A => {
                self.args.push(b);
                if self.args.len() == 4 {
                    self.xs = ((self.args[0] as usize) << 8) | self.args[1] as usize;
                    self.xe = ((self.args[2] as usize) << 8) | self.args[3] as usize;
                }
            }
            0x2B => {
                self.args.push(b);
                if self.args.len() == 4 {
                    self.ys = ((self.args[0] as usize) << 8) | self.args[1] as usize;
                    self.ye = ((self.args[2] as usize) << 8) | self.args[3] as usize;
                }
            }
            0x36 => self.madctl = b,
            0x3A => self.colmod16 = b & 0x7 != 0x6,
            0x2C | 0x3C => match self.pix_hi.take() {
                None => self.pix_hi = Some(b),
                Some(hi) => {
                    let c = ((hi as u16) << 8) | b as u16;
                    self.put(c);
                }
            },
            _ => {}
        }
    }
    fn put(&mut self, c: u16) {
        let (mut px, mut py) = (self.x, self.y);
        let mv = self.madctl & 0x20 != 0;
        let mx = self.madctl & 0x40 != 0;
        let my = self.madctl & 0x80 != 0;
        if mv {
            std::mem::swap(&mut px, &mut py);
        }
        if mx {
            px = self.w.saturating_sub(1).saturating_sub(px);
        }
        if my {
            py = self.h.saturating_sub(1).saturating_sub(py);
        }
        if self.flip_x {
            px = self.w.saturating_sub(1).saturating_sub(px);
        }
        if self.flip_y {
            py = self.h.saturating_sub(1).saturating_sub(py);
        }
        // ST7735 style offsets are commonly compensated by the driver; clip to the panel
        if px < self.w && py < self.h {
            // the BGR bit must match the panel's colour filter order, else red and blue swap
            let swap = (self.madctl & 0x08 != 0) != self.panel_bgr;
            let c = if swap { (c & 0x07E0) | (c >> 11) | ((c & 0x1F) << 11) } else { c };
            self.fb[py * self.w + px] = c;
        }
        self.x += 1;
        if self.x > self.xe {
            self.x = self.xs;
            self.y += 1;
            if self.y > self.ye {
                self.y = self.ys;
            }
        }
        self.version += 1;
    }
    /// RGB888 framebuffer as raw bytes (for the GUI).
    pub fn rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.w * self.h * 3);
        for &c in &self.fb {
            let c = if self.invert { !c } else { c };
            let r = ((c >> 11) & 0x1F) as u8;
            let g = ((c >> 5) & 0x3F) as u8;
            let b = (c & 0x1F) as u8;
            let (r, g, b) = if self.on { (r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2) } else { (0, 0, 0) };
            out.extend_from_slice(&[r, g, b]);
        }
        out
    }
}
impl Device for Tft {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("GND", Ground), pin("CS", Input), pin("RESET", Input), pin("DC", Input), pin("MOSI", Input), pin("SCK", Input), pin("LED", Power), pin("MISO", Output)]
    }
    fn spi_pins(&self) -> Option<(usize, usize, Option<usize>, usize)> {
        Some((6, 5, Some(8), 2))
    }
    fn spi_transfer(&mut self, _now: u64, data: &mut [u8], lv: &dyn Fn(usize) -> bool) {
        let dc = lv(4);
        for b in data.iter_mut() {
            if dc {
                self.data(*b);
            } else {
                self.command(*b);
            }
            *b = 0;
        }
    }
    fn state(&self) -> Value {
        // `lit`: pixels that are not black (lets tests check that something was drawn)
        let lit = self.fb.iter().filter(|&&p| p != 0).count();
        json!({"width": self.w, "height": self.h, "on": self.on, "version": self.version, "framebuffer": true, "lit": lit})
    }
    fn summary(&self) -> String {
        let lit = self.fb.iter().filter(|&&p| p != 0).count();
        format!("{} {}x{} {} · {} pixels lit", self.kind, self.w, self.h, if self.on { "on" } else { "off" }, lit)
    }
    fn framebuffer(&self) -> Option<(usize, usize, Vec<u8>)> {
        Some((self.w, self.h, self.rgb()))
    }
}

// ===================================================================== MAX7219 (8x8 matrix / 7-seg driver)
pub struct Max7219 {
    chips: usize,
    regs: Vec<[u8; 16]>,
    shift: Vec<u8>,
    last_cs: bool,
    pub version: u64,
}
impl Max7219 {
    pub fn new(p: &Props) -> Max7219 {
        let chips = prop_f64(p, &["chips", "count", "modules"], 1.0).clamp(1.0, 16.0) as usize;
        Max7219 { chips, regs: vec![[0; 16]; chips], shift: Vec::new(), last_cs: true, version: 0 }
    }
    fn latch(&mut self) {
        // last 2*chips bytes: first pair belongs to the farthest chip
        let n = self.chips * 2;
        if self.shift.len() >= 2 {
            let take = self.shift.len().min(n);
            let bytes = self.shift[self.shift.len() - take..].to_vec();
            let pairs = bytes.len() / 2;
            for k in 0..pairs {
                let chip = pairs - 1 - k;
                let (reg, val) = (bytes[2 * k] & 0xF, bytes[2 * k + 1]);
                if chip < self.chips {
                    self.regs[chip][reg as usize] = val;
                }
            }
            self.version += 1;
        }
        self.shift.clear();
    }
}
impl Device for Max7219 {
    fn kind(&self) -> &'static str {
        "max7219"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("VCC", Power), pin("GND", Ground), pin("DIN", Input), pin("CS", Input), pin("CLK", Input)]
    }
    fn spi_pins(&self) -> Option<(usize, usize, Option<usize>, usize)> {
        Some((4, 2, None, 3))
    }
    fn spi_transfer(&mut self, _now: u64, data: &mut [u8], _lv: &dyn Fn(usize) -> bool) {
        self.shift.extend_from_slice(data);
        for b in data.iter_mut() {
            *b = 0;
        }
    }
    /// LOAD/CS rising edge latches the shifted frames (GPIO or hardware CS).
    fn spi_select(&mut self, selected: bool) {
        if !selected && !self.last_cs {
            self.latch();
        }
        self.last_cs = !selected;
    }
    fn state(&self) -> Value {
        let chips: Vec<Value> = self
            .regs
            .iter()
            .map(|r| json!({"rows": hex(&r[1..9]), "decode": r[9], "intensity": r[10], "scan_limit": r[11], "on": r[12] & 1 != 0, "test": r[15] & 1 != 0}))
            .collect();
        json!({"chips": chips, "version": self.version})
    }
    fn summary(&self) -> String {
        format!("{} module(s), {}", self.chips, if self.regs[0][12] & 1 != 0 { "on" } else { "shutdown" })
    }
}

// ===================================================================== TM1637 4-digit 7-segment
pub struct Tm1637 {
    digits: usize,
    pub seg: [u8; 6],
    pub brightness: u8,
    pub on: bool,
    last_clk: bool,
    last_dio: bool,
    active: bool,
    bits: u8,
    byte: u8,
    bytes: Vec<u8>,
    ack: bool,
    dirty: bool,
}
impl Tm1637 {
    pub fn new(p: &Props) -> Tm1637 {
        Tm1637 {
            digits: prop_f64(p, &["digits"], 4.0) as usize,
            seg: [0; 6],
            brightness: 0,
            on: false,
            last_clk: true,
            last_dio: true,
            active: false,
            bits: 0,
            byte: 0,
            bytes: Vec::new(),
            ack: false,
            dirty: false,
        }
    }
    fn process(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let c = self.bytes[0];
        match c & 0xC0 {
            0x40 => {}
            0xC0 => {
                let mut a = (c & 7) as usize;
                for &d in &self.bytes[1..] {
                    if a < 6 {
                        self.seg[a] = d;
                    }
                    a += 1;
                }
            }
            0x80 => {
                self.on = c & 0x08 != 0;
                self.brightness = c & 7;
            }
            _ => {}
        }
        self.bytes.clear();
    }
}
impl Device for Tm1637 {
    fn kind(&self) -> &'static str {
        "tm1637"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("CLK", Input), pin("DIO", Io), pin("VCC", Power), pin("GND", Ground)]
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::ToRail { a: 1, rail: Rail::V3, ohms: 10_000.0 });
        out.push(Stamp::ToRail { a: 0, rail: Rail::V3, ohms: 10_000.0 });
        if self.ack {
            out.push(Stamp::ToRail { a: 1, rail: Rail::Gnd, ohms: 50.0 });
        }
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        let clk = v.level(0).unwrap_or(true);
        let dio = v.level(1).unwrap_or(true);
        if clk && self.last_clk && dio != self.last_dio && !self.ack {
            if !dio {
                self.active = true;
                self.bits = 0;
                self.byte = 0;
                self.bytes.clear();
            } else {
                self.active = false;
                self.process();
            }
        } else if self.active && clk && !self.last_clk {
            if self.bits < 8 {
                self.byte |= (dio as u8) << self.bits;
            }
            self.bits += 1;
        } else if self.active && !clk && self.last_clk {
            if self.bits == 8 {
                self.ack = true;
                self.dirty = true;
                self.bytes.push(self.byte);
            } else if self.bits == 9 {
                self.ack = false;
                self.dirty = true;
                self.bits = 0;
                self.byte = 0;
            }
        }
        self.last_clk = clk;
        self.last_dio = dio;
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn state(&self) -> Value {
        json!({"digits": self.digits, "segments": hex(&self.seg[..self.digits]), "brightness": self.brightness, "on": self.on})
    }
    fn summary(&self) -> String {
        let s: String = self.seg[..self.digits].iter().map(|&b| seg_char(b & 0x7F)).collect();
        format!("[{}]{}", s, if self.on { "" } else { " (off)" })
    }
}

pub fn seg_char(b: u8) -> char {
    match b {
        0x3F => '0',
        0x06 => '1',
        0x5B => '2',
        0x4F => '3',
        0x66 => '4',
        0x6D => '5',
        0x7D => '6',
        0x07 => '7',
        0x7F => '8',
        0x6F => '9',
        0x77 => 'A',
        0x7C => 'b',
        0x39 => 'C',
        0x5E => 'd',
        0x79 => 'E',
        0x71 => 'F',
        0x40 => '-',
        0x00 => ' ',
        _ => '?',
    }
}

// ===================================================================== 7-segment display (LEDs, optional multiplexing)
pub struct SevenSeg {
    digits: usize,
    anode: bool,
    /// last time each digit/segment was lit (ns)
    lit_at: Vec<[u64; 8]>,
    now: u64,
}
impl SevenSeg {
    pub fn new(p: &Props) -> SevenSeg {
        let digits = prop_f64(p, &["digits"], 1.0).clamp(1.0, 4.0) as usize;
        SevenSeg { digits, anode: prop_str(p, &["common"], "cathode").starts_with('a'), lit_at: vec![[0; 8]; digits], now: 0 }
    }
}
const SEG_PINS: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "DP"];
const DIG_PINS: [&str; 4] = ["D1", "D2", "D3", "D4"];
impl Device for SevenSeg {
    fn kind(&self) -> &'static str {
        "7segment"
    }
    fn pins(&self) -> Vec<PinInfo> {
        let mut v: Vec<PinInfo> = SEG_PINS.iter().map(|n| pin(n, Passive)).collect();
        if self.digits == 1 {
            v.push(pin("COM", Passive));
        } else {
            for d in DIG_PINS.iter().take(self.digits) {
                v.push(pin(d, Passive));
            }
        }
        v
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        for d in 0..self.digits {
            for s in 0..8 {
                let tag = (d * 8 + s) as u32;
                if self.anode {
                    out.push(Stamp::Diode { a: 8 + d, k: s, vf: 1.9, ron: 15.0, tag });
                } else {
                    out.push(Stamp::Diode { a: s, k: 8 + d, vf: 1.9, ron: 15.0, tag });
                }
            }
        }
    }
    fn on_solved(&mut self, now: u64, v: &PinView) {
        self.now = now;
        for d in 0..self.digits {
            for s in 0..8 {
                if (v.diode_current)((d * 8 + s) as u32) > 0.0005 {
                    self.lit_at[d][s] = now.max(1);
                }
            }
        }
    }
    fn state(&self) -> Value {
        // persistence of vision: segments lit within the last 30 ms
        let segs: Vec<u8> = self
            .lit_at
            .iter()
            .map(|d| (0..8).filter(|&s| d[s] != 0 && self.now.saturating_sub(d[s]) < 30_000_000).map(|s| 1u8 << s).sum())
            .collect();
        json!({"digits": self.digits, "segments": hex(&segs)})
    }
    fn summary(&self) -> String {
        let segs: Vec<u8> = self
            .lit_at
            .iter()
            .map(|d| (0..8).filter(|&s| d[s] != 0 && self.now.saturating_sub(d[s]) < 30_000_000).map(|s| 1u8 << s).sum())
            .collect();
        format!("[{}]", segs.iter().map(|&b| seg_char(b & 0x7F)).collect::<String>())
    }
}

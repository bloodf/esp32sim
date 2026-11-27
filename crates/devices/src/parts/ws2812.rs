//! WS2812 / WS2812B / SK6812 addressable RGB LEDs (strip, ring or matrix).

use crate::parts::{prop_f64, prop_str, Props};
use crate::{pin, Device, PinInfo, PinKind::*, PinView, RmtItem};
use serde_json::{json, Value};

pub struct Ws2812 {
    pub count: usize,
    pub layout: String,
    pub width: usize,
    rgbw: bool,
    order: String,
    pub pixels: Vec<[u8; 4]>,
    bits: Vec<bool>,
    last_level: bool,
    rise_at: u64,
    fall_at: u64,
    pub frames: u64,
}

impl Ws2812 {
    pub fn new(p: &Props) -> Ws2812 {
        let count = prop_f64(p, &["count", "pixels", "leds"], 8.0).clamp(1.0, 4096.0) as usize;
        let layout = prop_str(p, &["layout"], "strip").to_string();
        let width = prop_f64(p, &["width", "columns"], (count as f64).sqrt().ceil()) as usize;
        let rgbw = prop_str(p, &["type"], "ws2812") == "sk6812-rgbw";
        Ws2812 {
            count,
            layout,
            width: width.max(1),
            rgbw,
            order: prop_str(p, &["order"], "GRB").to_ascii_uppercase(),
            pixels: vec![[0; 4]; count],
            bits: Vec::new(),
            last_level: false,
            rise_at: 0,
            fall_at: 0,
            frames: 0,
        }
    }

    fn push_bit(&mut self, b: bool) {
        self.bits.push(b);
    }

    /// Latch accumulated bits into the pixel buffer.
    fn latch(&mut self) {
        let per = if self.rgbw { 32 } else { 24 };
        if self.bits.len() < per {
            self.bits.clear();
            return;
        }
        let n = (self.bits.len() / per).min(self.count);
        for i in 0..n {
            let mut bytes = [0u8; 4];
            for (k, byte) in bytes.iter_mut().enumerate().take(per / 8) {
                let mut v = 0u8;
                for bit in 0..8 {
                    v = (v << 1) | self.bits[i * per + k * 8 + bit] as u8;
                }
                *byte = v;
            }
            let mut px = [0u8; 4];
            for (k, ch) in self.order.chars().take(3).enumerate() {
                match ch {
                    'R' => px[0] = bytes[k],
                    'G' => px[1] = bytes[k],
                    'B' => px[2] = bytes[k],
                    _ => {}
                }
            }
            px[3] = if self.rgbw { bytes[3] } else { 0 };
            self.pixels[i] = px;
        }
        self.frames += 1;
        self.bits.clear();
    }
}

impl Device for Ws2812 {
    fn kind(&self) -> &'static str {
        "ws2812"
    }
    fn pins(&self) -> Vec<PinInfo> {
        vec![pin("DIN", Input), pin("VCC", Power), pin("GND", Ground), pin("DOUT", Output)]
    }
    fn waveform(&mut self, _now: u64, pin: usize, items: &[RmtItem]) {
        if pin != 0 {
            return;
        }
        let mut i = 0;
        while i < items.len() {
            let it = items[i];
            if it.level {
                let one = it.duration_ns > 550;
                self.push_bit(one);
            } else if it.duration_ns > 40_000 {
                self.latch();
            }
            i += 1;
        }
        // an RMT transmission ends with the line idle low: latch
        self.latch();
    }
    fn on_solved(&mut self, now: u64, v: &PinView) {
        // bit-banged driving: measure high pulse widths on DIN
        let level = v.level(0).unwrap_or(false);
        if level && !self.last_level {
            if now.saturating_sub(self.fall_at) > 50_000 && !self.bits.is_empty() {
                self.latch();
            }
            self.rise_at = now;
        } else if !level && self.last_level {
            let w = now.saturating_sub(self.rise_at);
            if w < 5_000 {
                self.push_bit(w > 550);
            }
            self.fall_at = now;
        }
        self.last_level = level;
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        if !self.bits.is_empty() && now.saturating_sub(self.fall_at) > 50_000 && !self.last_level {
            self.latch();
        }
        if self.bits.is_empty() {
            None
        } else {
            Some(self.fall_at + 60_000)
        }
    }
    fn state(&self) -> Value {
        let px: Vec<String> = self.pixels.iter().map(|p| format!("{:02x}{:02x}{:02x}", p[0], p[1], p[2])).collect();
        json!({"count": self.count, "layout": self.layout, "width": self.width, "pixels": px, "frames": self.frames})
    }
    fn summary(&self) -> String {
        let lit = self.pixels.iter().filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 0).count();
        let first: Vec<String> = self.pixels.iter().take(4).map(|p| format!("#{:02x}{:02x}{:02x}", p[0], p[1], p[2])).collect();
        format!("{} of {} lit [{}{}]", lit, self.count, first.join(" "), if self.count > 4 { " ..." } else { "" })
    }
}

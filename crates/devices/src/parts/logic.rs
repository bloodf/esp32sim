//! Logic ICs: 74HC595 shift register.

use crate::parts::Props;
use crate::{pin, Device, PinInfo, PinKind::*, PinView, Rail, Stamp};
use serde_json::{json, Value};

pub struct Hc595 {
    sr: u8,
    pub latch: u8,
    oe: bool,
    last_sh: bool,
    last_st: bool,
    dirty: bool,
}
impl Hc595 {
    pub fn new(_p: &Props) -> Hc595 {
        Hc595 { sr: 0, latch: 0, oe: true, last_sh: false, last_st: false, dirty: false }
    }
}
const Q: [&str; 8] = ["Q0", "Q1", "Q2", "Q3", "Q4", "Q5", "Q6", "Q7"];
impl Device for Hc595 {
    fn kind(&self) -> &'static str {
        "74hc595"
    }
    fn pins(&self) -> Vec<PinInfo> {
        let mut v = vec![pin("VCC", Power), pin("GND", Ground), pin("DS", Input), pin("SHCP", Input), pin("STCP", Input), pin("OE", Input), pin("MR", Input), pin("Q7S", Output)];
        for q in Q {
            v.push(pin(q, Output));
        }
        v
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        out.push(Stamp::ToRail { a: 7, rail: if self.sr & 0x80 != 0 { Rail::V3 } else { Rail::Gnd }, ohms: 50.0 });
        if self.oe {
            for i in 0..8 {
                out.push(Stamp::ToRail { a: 8 + i, rail: if self.latch & (1 << i) != 0 { Rail::V3 } else { Rail::Gnd }, ohms: 50.0 });
            }
        }
    }
    fn on_solved(&mut self, _now: u64, v: &PinView) {
        let sh = v.level(3).unwrap_or(false);
        let st = v.level(4).unwrap_or(false);
        let oe = !v.level(5).unwrap_or(false);
        let mr = v.level(6).unwrap_or(true);
        if !mr {
            if self.sr != 0 {
                self.sr = 0;
                self.dirty = true;
            }
        } else if sh && !self.last_sh {
            let ds = v.level(2).unwrap_or(false);
            self.sr = (self.sr << 1) | ds as u8;
            self.dirty = true;
        }
        if st && !self.last_st {
            self.latch = self.sr;
            self.dirty = true;
        }
        if oe != self.oe {
            self.oe = oe;
            self.dirty = true;
        }
        self.last_sh = sh;
        self.last_st = st;
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn state(&self) -> Value {
        json!({"outputs": self.latch, "shift": self.sr, "enabled": self.oe})
    }
    fn summary(&self) -> String {
        format!("Q7..Q0 = {:08b}", self.latch)
    }
}

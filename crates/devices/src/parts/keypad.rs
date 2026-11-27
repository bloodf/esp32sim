//! Matrix keypads (4x3, 4x4).

use crate::parts::{prop_f64, Props};
use crate::{pin, Device, PinInfo, PinKind::*, Stamp};
use serde_json::{json, Value};
use std::collections::VecDeque;

pub struct Keypad {
    rows: Vec<&'static str>,
    cols: usize,
    layout: Vec<Vec<char>>,
    pressed: Vec<(usize, usize)>,
    /// scheduled (time, key, press?)
    queue: VecDeque<(u64, char, bool)>,
    press_ms: u64,
    dirty: bool,
    pub history: String,
}

const ROW_PINS: [&str; 4] = ["R1", "R2", "R3", "R4"];
const COL_PINS: [&str; 4] = ["C1", "C2", "C3", "C4"];

impl Keypad {
    pub fn new(p: &Props, cols: usize) -> Keypad {
        let layout: Vec<Vec<char>> = if let Some(arr) = p.get("keys").and_then(|v| v.as_array()) {
            arr.iter().map(|r| r.as_str().unwrap_or("").chars().collect()).collect()
        } else if cols == 4 {
            vec!["123A".chars().collect(), "456B".chars().collect(), "789C".chars().collect(), "*0#D".chars().collect()]
        } else {
            vec!["123".chars().collect(), "456".chars().collect(), "789".chars().collect(), "*0#".chars().collect()]
        };
        Keypad {
            rows: ROW_PINS.to_vec(),
            cols,
            layout,
            pressed: Vec::new(),
            queue: VecDeque::new(),
            press_ms: prop_f64(p, &["press_ms"], 120.0) as u64,
            dirty: false,
            history: String::new(),
        }
    }
    fn find(&self, k: char) -> Option<(usize, usize)> {
        for (r, row) in self.layout.iter().enumerate() {
            for (c, ch) in row.iter().enumerate() {
                if ch.eq_ignore_ascii_case(&k) {
                    return Some((r, c));
                }
            }
        }
        None
    }
    fn set(&mut self, k: char, down: bool) {
        if let Some(rc) = self.find(k) {
            if down {
                if !self.pressed.contains(&rc) {
                    self.pressed.push(rc);
                    self.history.push(k);
                    if self.history.len() > 64 {
                        self.history.remove(0);
                    }
                }
            } else {
                self.pressed.retain(|x| *x != rc);
            }
            self.dirty = true;
        }
    }
}

impl Device for Keypad {
    fn kind(&self) -> &'static str {
        if self.cols == 4 {
            "keypad-4x4"
        } else {
            "keypad-4x3"
        }
    }
    fn pins(&self) -> Vec<PinInfo> {
        let mut v: Vec<PinInfo> = self.rows.iter().map(|r| pin(r, Passive)).collect();
        for c in COL_PINS.iter().take(self.cols) {
            v.push(pin(c, Passive));
        }
        v
    }
    fn stamps(&self, out: &mut Vec<Stamp>) {
        for &(r, c) in &self.pressed {
            out.push(Stamp::Resistor { a: r, b: 4 + c, ohms: 1.0 });
        }
    }
    fn poll(&mut self, now: u64) -> Option<u64> {
        while let Some(&(t, k, down)) = self.queue.front() {
            if t > now {
                break;
            }
            self.queue.pop_front();
            self.set(k, down);
        }
        self.queue.front().map(|x| x.0)
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    fn input(&mut self, now: u64, a: &Value) -> Result<(), String> {
        // {"key":"5","action":"press|release|click"}  or {"type":"1234#"}
        if let Some(seq) = a.get("type").and_then(|v| v.as_str()) {
            let mut t = now.max(self.queue.back().map(|x| x.0).unwrap_or(0));
            for ch in seq.chars() {
                self.queue.push_back((t, ch, true));
                t += self.press_ms * 1_000_000;
                self.queue.push_back((t, ch, false));
                t += self.press_ms * 1_000_000;
            }
            return Ok(());
        }
        let key = a.get("key").and_then(|v| v.as_str()).and_then(|s| s.chars().next()).ok_or("expected {\"key\":\"5\"}")?;
        if self.find(key).is_none() {
            return Err(format!("keypad has no key '{}'", key));
        }
        match a.get("action").and_then(|v| v.as_str()).unwrap_or("click") {
            "press" | "down" => self.set(key, true),
            "release" | "up" => self.set(key, false),
            _ => {
                self.queue.push_back((now, key, true));
                self.queue.push_back((now + self.press_ms * 1_000_000, key, false));
            }
        }
        Ok(())
    }
    fn state(&self) -> Value {
        let pressed: Vec<String> = self.pressed.iter().map(|&(r, c)| self.layout[r][c].to_string()).collect();
        let layout: Vec<String> = self.layout.iter().map(|r| r.iter().collect()).collect();
        json!({"pressed": pressed, "layout": layout, "history": self.history})
    }
    fn summary(&self) -> String {
        let p: String = self.pressed.iter().map(|&(r, c)| self.layout[r][c]).collect();
        if p.is_empty() {
            format!("typed: {}", self.history)
        } else {
            format!("holding: {}", p)
        }
    }
}

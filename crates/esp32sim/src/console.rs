//! Interactive terminal: serial monitor + simulator commands (lines starting with '/').

use crate::sim::{Cmd, Sim, SimHandle};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Duration;

pub const HELP: &str = "\
Console commands (everything else is sent to the ESP32 serial port):
  /help                       this help
  /parts                      list components and their state
  /show <id>                  render a component (OLED, LCD, LEDs, NeoPixels, ...)
  /press <id> | /release <id> | /click <id> [ms]   push buttons
  /toggle <id>                switches, relays, PIR sensors...
  /set <id> <value>           potentiometer (0..1), signal volts, sensor value
  /set <id> key=value ...     e.g. /set imu roll=30 pitch=10, /set dht temperature=30
  /key <id> <key>             keypad: tap a key;  /type <id> 1234#  types a sequence
  /tap <id> [uid] [ntag]      NFC: present a card;  /remove <id>  takes it away
  /turn <id> <steps>          rotary encoder
  /input <id> <json>          send any JSON action to a component
  /send <file>                send a file to the serial port
  /paste <file>               MicroPython: paste a .py file at the REPL (paste mode)
  /pins                       ESP32 pin voltages and modes
  /bus [n]                    last n I2C/SPI/UART/RMT transactions (bus monitor)
  /status                     emulator status
  /pause  /resume  /reset     control execution
  /speed <x>                  real-time factor (0 = unlimited)
  /quit                       exit";

pub fn run(target: &Path, speed: Option<f64>, seconds: Option<f64>, eol: &str) -> Result<i32, String> {
    let mut cfg = crate::project::load(target)?;
    if let Some(s) = speed {
        cfg.speed = Some(s);
    }
    println!("esp32sim: {} ({} components) — type /help for commands", if cfg.name.is_empty() { "project" } else { &cfg.name }, cfg.components.len());
    let sim = Sim::start(cfg, true);
    let auto_eol = eol == "auto";
    let mut eol = match eol {
        "crlf" => "\r\n",
        "cr" => "\r",
        "none" => "",
        _ => "\n",
    }
    .to_string();
    // stdin reader thread
    let (ltx, lrx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if ltx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = ltx.send("/quit-eof".into());
    });
    let mut serial_pos = 0u64;
    let mut msg_seq = 0u64;
    let mut out = std::io::stdout();
    loop {
        // output
        {
            let s = sim.shared.lock().unwrap();
            let (bytes, total) = s.serial.since(serial_pos);
            if !bytes.is_empty() {
                let _ = out.write_all(bytes);
                // MicroPython's REPL executes on CR only
                if auto_eol && eol == "\n" && String::from_utf8_lossy(bytes).contains("MicroPython") {
                    eol = "\r".into();
                }
            }
            serial_pos = total;
            for (seq, m) in s.messages.iter() {
                if *seq > msg_seq {
                    let _ = writeln!(out, "\x1b[36m[sim] {}\x1b[0m", m);
                    msg_seq = *seq;
                }
            }
            if let Some(limit) = seconds {
                let t = s.status.get("time_ns").and_then(|v| v.as_u64()).unwrap_or(0) as f64 / 1e9;
                if t >= limit {
                    let _ = out.flush();
                    drop(s);
                    drop(sim);
                    return Ok(0);
                }
            }
        }
        let _ = out.flush();
        match lrx.recv_timeout(Duration::from_millis(30)) {
            Ok(line) => {
                if line == "/quit-eof" {
                    if seconds.is_none() {
                        // stdin closed (e.g. piped): keep running until killed
                        continue;
                    }
                    continue;
                }
                if let Some(cmd) = line.strip_prefix('/') {
                    if !command(&sim, cmd, &mut out) {
                        drop(sim);
                        return Ok(0);
                    }
                } else {
                    sim.send(Cmd::Serial(format!("{}{}", line, eol).into_bytes()));
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => {}
        }
    }
}

fn kv_value(s: &str) -> Value {
    if let Ok(f) = s.parse::<f64>() {
        json!(f)
    } else if s == "true" || s == "false" {
        json!(s == "true")
    } else {
        json!(s)
    }
}

/// Execute a console command. Returns false to quit.
pub fn command(sim: &SimHandle, line: &str, out: &mut dyn Write) -> bool {
    let mut it = line.split_whitespace();
    let cmd = it.next().unwrap_or("");
    let args: Vec<&str> = it.collect();
    let arg = |i: usize| args.get(i).copied().unwrap_or("");
    // arguments after the first one (empty if the command was typed bare)
    let rest_args: &[&str] = args.get(1..).unwrap_or(&[]);
    let report = |r: Result<(), String>, out: &mut dyn Write| {
        if let Err(e) = r {
            let _ = writeln!(out, "\x1b[31m{}\x1b[0m", e);
        }
    };
    match cmd {
        "help" | "h" | "?" => {
            let _ = writeln!(out, "{}", HELP);
        }
        "quit" | "exit" | "q" => return false,
        "pause" => sim.send(Cmd::Pause),
        "resume" | "run" | "continue" => sim.send(Cmd::Resume),
        "reset" => sim.send(Cmd::Reset),
        "speed" => sim.send(Cmd::Speed(arg(0).parse().unwrap_or(1.0))),
        "status" => {
            let s = sim.shared.lock().unwrap();
            let _ = writeln!(out, "{}", serde_json::to_string_pretty(&s.status).unwrap_or_default());
        }
        "parts" | "ls" => {
            let s = sim.shared.lock().unwrap();
            if let Some(cs) = s.board.get("components").and_then(|v| v.as_array()) {
                for c in cs {
                    let id = c["id"].as_str().unwrap_or("");
                    if id.starts_with("boot_") || id == "board_led_r" {
                        continue;
                    }
                    let _ = writeln!(out, "  {:<14} {:<16} {}", id, c["kind"].as_str().unwrap_or(""), c["summary"].as_str().unwrap_or(""));
                }
            }
        }
        "pins" => {
            let s = sim.shared.lock().unwrap();
            if let Some(p) = s.board.get("pads").and_then(|v| v.as_object()) {
                let mut keys: Vec<(u32, &Value)> = p.iter().map(|(k, v)| (k.parse().unwrap_or(0), v)).collect();
                keys.sort_by_key(|x| x.0);
                for (g, v) in keys {
                    let volts = v["v"].as_f64().map(|x| format!("{:.2} V", x)).unwrap_or_else(|| "floating".into());
                    let _ = writeln!(out, "  GPIO{:<3} {:<4} {}", g, v["mode"].as_str().unwrap_or(""), volts);
                }
            }
        }
        "show" => {
            let s = sim.shared.lock().unwrap();
            let found = s.board.get("components").and_then(|v| v.as_array()).and_then(|cs| cs.iter().find(|c| c["id"].as_str().map(|x| x.eq_ignore_ascii_case(arg(0))).unwrap_or(false)).cloned());
            match found {
                Some(c) => {
                    let _ = writeln!(out, "{}", render(&c));
                }
                None => {
                    let _ = writeln!(out, "no component '{}'", arg(0));
                }
            }
        }
        "press" => report(sim.input(arg(0), json!({"action": "press"})), out),
        "release" => report(sim.input(arg(0), json!({"action": "release"})), out),
        "click" => report(sim.input(arg(0), json!({"action": "click", "ms": arg(1).parse::<u64>().unwrap_or(150)})), out),
        "toggle" => report(sim.input(arg(0), json!({"action": "toggle"})), out),
        "key" => report(sim.input(arg(0), json!({"key": arg(1)})), out),
        "type" => report(sim.input(arg(0), json!({"type": rest_args.join(" ")})), out),
        "turn" => report(sim.input(arg(0), json!({"steps": arg(1).parse::<i64>().unwrap_or(1)})), out),
        "remove" => report(sim.input(arg(0), json!({"action": "remove"})), out),
        "tap" => {
            let mut a = json!({"action": "tap"});
            for x in rest_args {
                if x.contains("ntag") {
                    a["card"] = json!("ntag213");
                } else {
                    a["uid"] = json!(x);
                }
            }
            report(sim.input(arg(0), a), out)
        }
        "set" => {
            let id = arg(0);
            let rest = rest_args;
            let action = if rest.len() == 1 && !rest[0].contains('=') {
                let v = kv_value(rest[0]);
                json!({"value": v.clone(), "volts": v.clone(), "lux": v.clone(), "distance": v.clone(), "weight": v.clone(), "temperature": v})
            } else {
                let mut m = serde_json::Map::new();
                for kv in rest {
                    if let Some((k, v)) = kv.split_once('=') {
                        m.insert(k.to_string(), kv_value(v));
                    }
                }
                Value::Object(m)
            };
            report(sim.input(id, action), out)
        }
        "input" => {
            let js = rest_args.join(" ");
            match serde_json::from_str::<Value>(&js) {
                Ok(v) => report(sim.input(arg(0), v), out),
                Err(e) => {
                    let _ = writeln!(out, "bad JSON: {}", e);
                }
            }
        }
        "bus" => {
            // recent I2C / SPI / UART / RMT transactions
            let n: usize = arg(0).parse().unwrap_or(20);
            let v = sim.exec(move |_m, b| {
                let total = b.buslog.total;
                let (ev, _) = b.buslog.since(total.saturating_sub(n as u64), n);
                json!(ev)
            });
            for e in v.as_array().cloned().unwrap_or_default() {
                let bus = e["bus"].as_str().unwrap_or("");
                let what = match bus {
                    "I2C" => format!("{} {} {}  {}", e["addr"].as_str().unwrap_or(""), e["dir"].as_str().unwrap_or(""), if e["ack"].as_bool() == Some(true) { "ACK" } else { "NACK" }, e["data"].as_str().unwrap_or("")),
                    "SPI" => format!("MOSI {}  MISO {}", e["mosi"].as_str().unwrap_or(""), e["miso"].as_str().unwrap_or("")),
                    _ => e["text"].as_str().unwrap_or("").to_string(),
                };
                let devs = e["devices"].as_array().map(|d| d.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default();
                let _ = writeln!(out, "  {:>12.6}s {:<6} {:<10} {}", e["t"].as_u64().unwrap_or(0) as f64 / 1e9, bus, devs, what);
            }
        }
        "send" | "paste" => {
            // send a file to the serial port; /paste wraps it in MicroPython paste mode
            match std::fs::read(arg(0)) {
                Ok(mut data) => {
                    if cmd == "paste" {
                        data.retain(|&b| b != b'\r');
                        data.insert(0, 0x05);
                        data.push(0x04);
                    }
                    let n = data.len();
                    sim.send(Cmd::Serial(data));
                    let _ = writeln!(out, "sent {} bytes", n);
                }
                Err(e) => {
                    let _ = writeln!(out, "{}: {}", arg(0), e);
                }
            }
        }
        "" => {}
        other => {
            let _ = writeln!(out, "unknown command '/{}' (try /help)", other);
        }
    }
    true
}

fn hexbytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

/// Text rendering of a component state.
pub fn render(c: &Value) -> String {
    let kind = c["kind"].as_str().unwrap_or("");
    let st = &c["state"];
    match kind {
        "ssd1306" | "sh1106" | "ssd1306-spi" | "sh1106-spi" => {
            let w = st["width"].as_u64().unwrap_or(128) as usize;
            let h = st["height"].as_u64().unwrap_or(64) as usize;
            let bm = hexbytes(st["bitmap"].as_str().unwrap_or(""));
            let px = |x: usize, y: usize| -> bool {
                let i = y * w + x;
                bm.get(i / 8).map(|b| b & (0x80 >> (i % 8)) != 0).unwrap_or(false)
            };
            let mut s = format!("┌{}┐\n", "─".repeat(w));
            for y in (0..h).step_by(2) {
                s.push('│');
                for x in 0..w {
                    let (a, b) = (px(x, y), px(x, y + 1));
                    s.push(match (a, b) {
                        (true, true) => '█',
                        (true, false) => '▀',
                        (false, true) => '▄',
                        _ => ' ',
                    });
                }
                s.push_str("│\n");
            }
            s.push_str(&format!("└{}┘", "─".repeat(w)));
            s
        }
        "lcd1602-i2c" | "lcd2004-i2c" | "lcd-parallel" => {
            let lines: Vec<String> = st["text"].as_array().map(|a| a.iter().map(|l| l.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
            let w = st["cols"].as_u64().unwrap_or(16) as usize;
            let mut s = format!("┌{}┐ backlight {}\n", "─".repeat(w), if st["backlight"].as_bool().unwrap_or(false) { "on" } else { "off" });
            for l in lines {
                s.push_str(&format!("│{}│\n", l));
            }
            s.push_str(&format!("└{}┘", "─".repeat(w)));
            s
        }
        "ws2812" => {
            let mut s = String::new();
            if let Some(px) = st["pixels"].as_array() {
                let w = if st["layout"] == "matrix" { st["width"].as_u64().unwrap_or(8) as usize } else { px.len() };
                for (i, p) in px.iter().enumerate() {
                    let b = hexbytes(p.as_str().unwrap_or("000000"));
                    if b.len() == 3 {
                        s.push_str(&format!("\x1b[38;2;{};{};{}m●\x1b[0m", b[0], b[1], b[2]));
                    }
                    if (i + 1) % w.max(1) == 0 {
                        s.push('\n');
                    }
                }
            }
            s
        }
        "led" => {
            let b = st["brightness"].as_f64().unwrap_or(0.0);
            format!("{} {}", if b > 0.05 { "●" } else { "○" }, c["summary"].as_str().unwrap_or(""))
        }
        _ => format!("{}\n{}", c["summary"].as_str().unwrap_or(""), serde_json::to_string_pretty(st).unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_typed_at_the_console() {
        assert_eq!(kv_value("3.5"), json!(3.5));
        assert_eq!(kv_value("-2"), json!(-2.0));
        assert_eq!(kv_value("true"), json!(true));
        assert_eq!(kv_value("false"), json!(false));
        assert_eq!(kv_value("hello"), json!("hello"));
        assert_eq!(hexbytes("00ff10"), vec![0, 255, 16]);
        assert_eq!(hexbytes("abc"), vec![0xab], "odd length ignores the tail");
        assert_eq!(hexbytes("zz"), Vec::<u8>::new());
    }

    #[test]
    fn renders_displays_as_text() {
        // 8x2 OLED with the top-left and bottom-right pixels lit
        let oled = json!({"kind": "ssd1306", "state": {"width": 8, "height": 2, "bitmap": "8001"}});
        let r = render(&oled);
        assert!(r.starts_with("┌────────┐"), "{}", r);
        assert!(r.contains("│▀      ▄│"), "{}", r);
        let lcd = json!({"kind": "lcd1602-i2c", "state": {"cols": 4, "text": ["Hi  ", "  :)"], "backlight": true}});
        let r = render(&lcd);
        assert!(r.contains("backlight on") && r.contains("│Hi  │") && r.contains("│  :)│"), "{}", r);
        let strip = json!({"kind": "ws2812", "state": {"pixels": ["ff0000", "00ff00"], "layout": "strip"}});
        assert!(render(&strip).contains("\x1b[38;2;255;0;0m●"));
        let led = json!({"kind": "led", "state": {"brightness": 0.9}, "summary": "on"});
        assert_eq!(render(&led), "● on");
        let other = json!({"kind": "servo", "state": {"angle": 90}, "summary": "90°"});
        assert!(render(&other).starts_with("90°\n"));
    }
}

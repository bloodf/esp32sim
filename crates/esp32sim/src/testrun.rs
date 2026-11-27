//! Headless scenario runner: executes the `test:` steps of a project.

use crate::sim::{Cmd, Sim};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::{Duration, Instant};

fn parse_duration_ns(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_f64().map(|ms| (ms * 1e6) as u64),
        Value::String(s) => {
            let s = s.trim();
            let (num, mult) = if let Some(x) = s.strip_suffix("ms") {
                (x, 1e6)
            } else if let Some(x) = s.strip_suffix("us") {
                (x, 1e3)
            } else if let Some(x) = s.strip_suffix('s') {
                (x, 1e9)
            } else {
                (s, 1e6)
            };
            num.trim().parse::<f64>().ok().map(|x| (x * mult) as u64)
        }
        _ => None,
    }
}

/// Backslash escapes in `send:` text (on top of YAML's own): \n \r \t and \\.
/// Anything else (e.g. Python's \x41) is passed through unchanged.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.peek().copied() {
            Some('n') => {
                it.next();
                out.push('\n');
            }
            Some('r') => {
                it.next();
                out.push('\r');
            }
            Some('t') => {
                it.next();
                out.push('\t');
            }
            Some('\\') => {
                it.next();
                out.push('\\');
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// Compare a state value with an expectation like ">0.5", "<=3", "ON", true.
pub fn matches(actual: &Value, expect: &Value) -> bool {
    if let Some(e) = expect.as_str() {
        let e = e.trim();
        for op in [">=", "<=", "!=", "==", ">", "<"] {
            if let Some(rhs) = e.strip_prefix(op) {
                if let (Some(a), Ok(b)) = (actual.as_f64(), rhs.trim().parse::<f64>()) {
                    return match op {
                        ">=" => a >= b,
                        "<=" => a <= b,
                        "!=" => (a - b).abs() > 1e-9,
                        "==" => (a - b).abs() < 1e-9,
                        ">" => a > b,
                        _ => a < b,
                    };
                }
            }
        }
        if let Some(a) = actual.as_str() {
            return a == e || a.contains(e);
        }
        if let Some(a) = actual.as_f64() {
            return e.parse::<f64>().map(|b| (a - b).abs() < 1e-6).unwrap_or(false);
        }
        return false;
    }
    if let (Some(a), Some(b)) = (actual.as_f64(), expect.as_f64()) {
        return (a - b).abs() < 1e-6;
    }
    actual == expect
}

/// Resolve "on[0]" / "pixels.3" style paths inside a state object.
fn lookup<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in path.split('.') {
        let (name, idx) = match part.split_once('[') {
            Some((n, rest)) => (n, rest.trim_end_matches(']').parse::<usize>().ok()),
            None => (part, None),
        };
        if !name.is_empty() {
            cur = cur.get(name).or_else(|| name.parse::<usize>().ok().and_then(|i| cur.get(i)))?;
        }
        if let Some(i) = idx {
            cur = cur.get(i)?;
        }
    }
    Some(cur)
}

pub fn run(target: &Path, timeout_s: f64, verbose: bool) -> Result<i32, String> {
    let mut cfg = crate::project::load(target)?;
    if cfg.test.is_empty() {
        return Err("project has no `test:` steps".into());
    }
    cfg.speed = Some(0.0); // as fast as possible
    // tests start from a fresh, in-memory SD card every time
    for c in cfg.components.iter_mut() {
        c.props.insert("__ephemeral".into(), serde_json::Value::Bool(true));
    }
    let steps = cfg.test.clone();
    let base_dir = cfg.base_dir.clone();
    let sim = Sim::start_with(cfg, true, false);
    // Lock-step: the simulation only advances while a step waits for something,
    // so inputs are applied at well-defined emulated times (deterministic runs).
    sim.send(Cmd::RunUntil(0));
    let mut serial_seen = 0usize; // index in the serial log already matched
    let mut serial_pos = 0u64;
    let mut log = String::new();
    let deadline_emu = (timeout_s * 1e9) as u64;
    let wall0 = Instant::now();
    let emu_now = |sim: &crate::sim::SimHandle| -> u64 { sim.shared.lock().unwrap().status.get("time_ns").and_then(|v| v.as_u64()).unwrap_or(0) };
    // Advance emulated time to `t` and wait until the simulation has paused there.
    // Returns the state string ("paused", "halted: ...", "breakpoint ...").
    let advance = |sim: &crate::sim::SimHandle, t: u64| -> String {
        sim.send(Cmd::RunUntil(t));
        let w0 = Instant::now();
        loop {
            {
                let s = sim.shared.lock().unwrap();
                let st = s.status.get("state").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let now = s.status.get("time_ns").and_then(|v| v.as_u64()).unwrap_or(0);
                if st != "running" && st != "" && (now >= t || st != "paused") {
                    return st;
                }
            }
            if w0.elapsed() > Duration::from_secs(120) {
                return "halted: simulation stopped responding".into();
            }
            std::thread::sleep(Duration::from_micros(30));
        }
    };
    const TICK: u64 = 1_000_000; // lock-step granularity: 1 ms
    let pull = |log: &mut String, serial_pos: &mut u64| {
        let s = sim.shared.lock().unwrap();
        let (b, total) = s.serial.since(*serial_pos);
        let t = String::from_utf8_lossy(b).to_string();
        if verbose && !t.is_empty() {
            print!("{}", t);
        }
        log.push_str(&t);
        *serial_pos = total;
        let st = s.status.get("state").and_then(|v| v.as_str()).unwrap_or("").to_string();
        st
    };
    let mut failures = 0;
    for (i, step) in steps.iter().enumerate() {
        let (key, val) = match step.iter().next() {
            Some((k, v)) => (k.clone(), v.clone()),
            None => continue,
        };
        let opts: &Map<String, Value> = step;
        let step_timeout = opts.get("timeout").and_then(parse_duration_ns).unwrap_or(10_000_000_000);
        let desc = format!("step {}: {} {}", i + 1, key, val);
        let result: Result<(), String> = match key.as_str() {
            "wait" | "sleep" => {
                let ns = parse_duration_ns(&val).ok_or("bad duration")?;
                let t0 = emu_now(&sim);
                let st = advance(&sim, t0 + ns);
                pull(&mut log, &mut serial_pos);
                if st.starts_with("halted") {
                    Err(st)
                } else {
                    Ok(())
                }
            }
            "expect_serial" | "wait_serial" => {
                let text = unescape(val.as_str().unwrap_or(""));
                let t0 = emu_now(&sim);
                loop {
                    let st = pull(&mut log, &mut serial_pos);
                    if let Some(pos) = log[serial_seen.min(log.len())..].find(&text) {
                        serial_seen += pos + text.len();
                        break Ok(());
                    }
                    if st.starts_with("halted") {
                        break Err(format!("firmware halted: {}", st));
                    }
                    let now = emu_now(&sim);
                    if now > t0 + step_timeout {
                        break Err(format!("timeout waiting for serial text {:?}", text));
                    }
                    if wall0.elapsed() > Duration::from_secs(600) {
                        break Err("wall clock timeout".into());
                    }
                    advance(&sim, now + TICK);
                }
            }
            "send" => {
                sim.send(Cmd::Serial(unescape(val.as_str().unwrap_or("")).into_bytes()));
                Ok(())
            }
            "send_file" | "paste" => {
                // send a file from the project folder; `paste` wraps it in MicroPython's
                // paste mode (Ctrl-E ... Ctrl-D) so a whole module can be defined at the REPL
                let rel = val.as_str().unwrap_or("");
                match std::fs::read(base_dir.join(rel)) {
                    Ok(mut data) => {
                        if key == "paste" {
                            data.retain(|&b| b != b'\r');
                            let mut v = vec![0x05];
                            v.extend(data);
                            v.push(0x04);
                            data = v;
                        }
                        sim.send(Cmd::Serial(data));
                        Ok(())
                    }
                    Err(e) => Err(format!("{}: {}", rel, e)),
                }
            }
            "press" | "release" | "click" | "toggle" => sim.input(val.as_str().unwrap_or(""), json!({"action": key})),
            "input" | "set" => {
                let id = val.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mut a = val.clone();
                if let Some(o) = a.as_object_mut() {
                    o.remove("id");
                }
                sim.input(&id, a)
            }
            "expect" | "expect_text" => {
                let id = val.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let t0 = emu_now(&sim);
                // expectations are retried until they hold or the step times out
                let within = val
                    .get("within")
                    .or(opts.get("within"))
                    .and_then(parse_duration_ns)
                    .unwrap_or(if key == "expect" { 0 } else { 2_000_000_000 });
                loop {
                    pull(&mut log, &mut serial_pos);
                    let (have_board, comp) = {
                        let s = sim.shared.lock().unwrap();
                        let cs = s.board.get("components").and_then(|v| v.as_array());
                        (cs.is_some(), cs.and_then(|cs| cs.iter().find(|c| c["id"].as_str() == Some(&id)).cloned()))
                    };
                    if !have_board {
                        // simulation still starting: no circuit snapshot published yet
                        if wall0.elapsed() > Duration::from_secs(60) {
                            break Err("simulation did not start".into());
                        }
                        let now = emu_now(&sim);
                        advance(&sim, now + TICK);
                        continue;
                    }
                    let Some(c) = comp else { break Err(format!("no component '{}'", id)) };
                    let mut bad = Vec::new();
                    for (k, e) in val.as_object().unwrap() {
                        if k == "id" || k == "within" || k == "timeout" {
                            continue;
                        }
                        if k == "contains" {
                            let text = format!("{} {}", c["summary"], c["state"]);
                            if !text.contains(e.as_str().unwrap_or("")) {
                                bad.push(format!("'{}' does not contain {}", c["summary"].as_str().unwrap_or(""), e));
                            }
                            continue;
                        }
                        match lookup(&c["state"], k) {
                            Some(a) if matches(a, e) => {}
                            Some(a) => bad.push(format!("{} = {} (expected {})", k, a, e)),
                            None => bad.push(format!("{} not found in state", k)),
                        }
                    }
                    if bad.is_empty() {
                        break Ok(());
                    }
                    let now = emu_now(&sim);
                    if now >= t0 + within {
                        break Err(format!("{}: {}", id, bad.join(", ")));
                    }
                    advance(&sim, (now + TICK).min(t0 + within));
                }
            }
            "log" | "echo" => {
                println!("  # {}", val.as_str().unwrap_or(""));
                Ok(())
            }
            other => Err(format!("unknown step '{}'", other)),
        };
        if emu_now(&sim) > deadline_emu {
            println!("✗ {} — overall timeout", desc);
            failures += 1;
            break;
        }
        match result {
            Ok(()) => println!("✓ {}", desc),
            Err(e) => {
                println!("✗ {} — {}", desc, e);
                failures += 1;
                break;
            }
        }
    }
    let t = emu_now(&sim) as f64 / 1e9;
    sim.send(Cmd::Quit);
    if failures == 0 {
        println!("PASS ({} steps, {:.3} s emulated, {:.2} s wall)", steps.len(), t, wall0.elapsed().as_secs_f64());
        Ok(0)
    } else {
        if !verbose {
            let tail: String = log.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect();
            println!("--- serial output (tail) ---\n{}", tail);
        }
        let msgs: Vec<String> = sim.shared.lock().unwrap().messages.iter().rev().take(12).map(|(_, m)| m.clone()).collect();
        if !msgs.is_empty() {
            println!("--- emulator messages ---");
            for m in msgs.iter().rev() {
                println!("{}", m);
            }
        }
        println!("FAIL");
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ns(&json!(250)), Some(250_000_000), "bare numbers are ms");
        assert_eq!(parse_duration_ns(&json!(0.5)), Some(500_000));
        assert_eq!(parse_duration_ns(&json!("2s")), Some(2_000_000_000));
        assert_eq!(parse_duration_ns(&json!("1.5 s")), Some(1_500_000_000));
        assert_eq!(parse_duration_ns(&json!("20ms")), Some(20_000_000));
        assert_eq!(parse_duration_ns(&json!("750us")), Some(750_000));
        assert_eq!(parse_duration_ns(&json!("12")), Some(12_000_000));
        assert_eq!(parse_duration_ns(&json!("soon")), None);
        assert_eq!(parse_duration_ns(&json!(null)), None);
    }

    #[test]
    fn send_text_escapes() {
        assert_eq!(unescape(r"a\nb\r\tc\\d"), "a\nb\r\tc\\d");
        assert_eq!(unescape(r"print('\x41')"), r"print('\x41')", "unknown escapes pass through");
        assert_eq!(unescape("trailing\\"), "trailing\\");
    }

    #[test]
    fn expectation_matching() {
        assert!(matches(&json!(0.7), &json!(">0.5")));
        assert!(!matches(&json!(0.3), &json!(">0.5")));
        assert!(matches(&json!(3), &json!("<=3")));
        assert!(matches(&json!(2), &json!("!=3")));
        assert!(matches(&json!(3.0), &json!("==3")));
        assert!(matches(&json!(1.0), &json!(1)));
        assert!(matches(&json!(true), &json!(true)));
        assert!(!matches(&json!(false), &json!(true)));
        assert!(matches(&json!("Hello world"), &json!("world")), "substring");
        assert!(matches(&json!(42), &json!("42")));
        assert!(!matches(&json!("abc"), &json!(">1")));
        assert!(matches(&json!([1, 2]), &json!([1, 2])));
    }

    #[test]
    fn state_paths() {
        let st = json!({"on": [false, true], "pixels": ["ff0000", "00ff00"], "cursor": {"row": 1}, "text": ["a", "b"]});
        assert_eq!(lookup(&st, "on[1]"), Some(&json!(true)));
        assert_eq!(lookup(&st, "pixels.1"), Some(&json!("00ff00")));
        assert_eq!(lookup(&st, "cursor.row"), Some(&json!(1)));
        assert_eq!(lookup(&st, "text[0]"), Some(&json!("a")));
        assert_eq!(lookup(&st, "missing"), None);
        assert_eq!(lookup(&st, "on[5]"), None);
    }
}

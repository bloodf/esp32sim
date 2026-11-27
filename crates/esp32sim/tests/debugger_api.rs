//! Source-level debugging through the GUI API, on the hand-assembled
//! `debug-demo` ELF (DWARF line info for blink.c, no compiler needed):
//!
//! ```c
//!  4 int step(int x) {
//!  5   return x + 3;
//!  6 }
//!  8 void app_main(void) {
//!  9   for (;;) {
//! 10     counter = step(counter);
//! 11     *(volatile unsigned *)0x3FF40000 = '.';
//! ```
mod common;
use common::*;
use esp32sim::testing::TempDir;
use serde_json::{json, Value};

fn debug_project() -> TempDir {
    let d = TempDir::new("dbg");
    let f = fixture("debug-demo");
    for n in ["blink.elf", "blink.c"] {
        std::fs::copy(f.join(n), d.path().join(n)).unwrap();
    }
    write(&d.path().join("esp32sim.yaml"), "name: debug-demo\nfirmware: blink.elf\ncomponents: []\nconnections: []\n");
    d
}

async fn wait_state(ws: &mut Ws, prefix: &str) {
    let p = prefix.to_string();
    ws.until(15, prefix, move |w| w.status["state"].as_str().map(|s| s.starts_with(&p)).unwrap_or(false)).await;
}

async fn line_of(s: &Srv) -> (u64, String) {
    let (_, r) = s.get("/api/debug/regs?core=0").await;
    (r["location"]["line"].as_u64().unwrap_or(0), r["location"]["function"].as_str().unwrap_or("").to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn debug_info_files_and_address_lookup() {
    let s = Srv::start_with(debug_project(), None);
    let v = s.wait_for("/api/debug/files", 10, |v| v["available"] == json!(true)).await;
    assert!(v["files"].to_string().contains("blink.c"), "{}", v);
    let (_, l) = s.get("/api/debug/lookup?addr=0x40080432").await;
    assert_eq!(l["location"]["line"], json!(11));
    let (_, l) = s.get("/api/debug/lookup?addr=0x40080413").await;
    assert_eq!((l["location"]["line"].clone(), l["location"]["function"].clone()), (json!(5), json!("step")));
    let (_, l) = s.get("/api/debug/lookup?addr=0x10").await;
    assert_eq!(l["location"], Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn breakpoints_stepping_and_watches() {
    let s = Srv::start_with(debug_project(), None);
    let mut ws = Ws::connect(&s.ws_url()).await;
    ws.wait_serial("...", 10).await; // the program prints dots
    // file:line breakpoints are resolved to addresses
    let (_, r) = s.post("/api/debug/breakpoints", json!({"breakpoints": [{"file": "blink.c", "line": 11}, {"file": "blink.c", "line": 99}]})).await;
    let bps = r["breakpoints"].as_array().unwrap();
    assert_eq!(bps[0]["verified"], json!(true), "{}", r);
    assert_eq!(bps[0]["addresses"], json!(["0x40080432"]));
    assert_eq!(bps[1]["verified"], json!(false), "no code on line 99");
    wait_state(&mut ws, "breakpoint").await;
    assert_eq!(ws.status["location"]["line"], json!(11), "{}", ws.status);
    let (_, regs) = s.get("/api/debug/regs?core=0").await;
    assert_eq!(regs["pc"], json!("0x40080432"));
    assert_eq!(regs["a"].as_array().unwrap().len(), 16);
    assert!(regs["disassembly"].to_string().contains("s32i") || regs["disassembly"].to_string().contains("s8i") || !regs["disassembly"].is_null());
    assert!(regs["backtrace"].to_string().contains("app_main"), "{}", regs["backtrace"]);
    // step over: 11 -> 9 -> 10, step into the call: 4/5 in step()
    s.post("/api/debug/step", json!({"core": 0, "kind": "over"})).await;
    assert_eq!(line_of(&s).await.0, 9);
    s.post("/api/debug/step", json!({"core": 0, "kind": "over"})).await;
    assert_eq!(line_of(&s).await.0, 10);
    s.post("/api/debug/step", json!({"core": 0, "kind": "into"})).await;
    let (line, func) = line_of(&s).await;
    assert!((4..=5).contains(&line) && func == "step", "{} {}", line, func);
    // single instructions
    let (_, before) = s.get("/api/debug/regs?core=0").await;
    s.post("/api/debug/step", json!({"core": 0, "kind": "instruction", "count": 2})).await;
    let (_, after) = s.get("/api/debug/regs?core=0").await;
    assert_ne!(before["pc"], after["pc"]);
    // watch the global `counter`: +3 per loop iteration
    let (_, w) = s.get("/api/debug/symbol?name=counter").await;
    assert!(w["address"].as_str().unwrap().starts_with("0x3ffb"), "{}", w);
    let c0 = w["u32"].as_u64().unwrap();
    s.post("/api/control", json!({"cmd": "resume"})).await;
    ws.until(10, "next hit", |w| w.status["state"].as_str().map(|s| s.starts_with("breakpoint")).unwrap_or(false) && w.status["instructions"].as_u64().is_some()).await;
    let w2 = s.wait_for("/api/debug/symbol?name=counter", 10, move |v| v["u32"].as_u64().map(|x| x != c0).unwrap_or(false)).await;
    assert_eq!(w2["u32"].as_u64().unwrap(), c0 + 3, "one more iteration");
    let (_, miss) = s.get("/api/debug/symbol?name=nope").await;
    assert_eq!(miss["error"], json!("symbol not found"));
    // memory view of the counter
    let (_, mem) = s.get(&format!("/api/debug/mem?addr={}&len=8", w2["address"].as_str().unwrap())).await;
    assert!(mem.to_string().contains(&format!("{:02x}", (c0 + 3) & 0xFF)), "{}", mem);
    // clearing breakpoints lets it run freely
    s.post("/api/debug/breakpoints", json!({"breakpoints": []})).await;
    s.post("/api/control", json!({"cmd": "resume"})).await;
    wait_state(&mut ws, "running").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn address_breakpoints_survive_reload() {
    let s = Srv::start_with(debug_project(), None);
    let mut ws = Ws::connect(&s.ws_url()).await;
    ws.wait_serial(".", 10).await;
    let (_, r) = s.post("/api/debug/breakpoints", json!({"breakpoints": [{"address": "0x40080413"}]})).await;
    assert_eq!(r["breakpoints"][0]["verified"], json!(true));
    wait_state(&mut ws, "breakpoint core 0 at 0x40080413").await;
    let t_hit = ws.status["time_ns"].as_u64().unwrap();
    // reloading the firmware (time restarts at 0) keeps the breakpoint
    s.post("/api/project/reload", json!({})).await;
    ws.until(15, "reloaded", move |w| w.status["time_ns"].as_u64().map(|t| t < t_hit).unwrap_or(false) || w.status["state"] == "loading").await;
    wait_state(&mut ws, "breakpoint core 0 at 0x40080413").await;
    let (_, regs) = s.get("/api/debug/regs?core=0").await;
    assert_eq!(regs["pc"], json!("0x40080413"));
}

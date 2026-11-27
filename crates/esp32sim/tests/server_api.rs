//! GUI / IDE server end to end: the real HTTP + WebSocket API, in-process,
//! on a temporary copy of the mock-firmware project (no browser, no
//! toolchain, no network: AI providers are mocked).
mod common;
use common::*;
use esp32sim::server::NativeHooks;
use esp32sim::testing::{write_mock_project, TempDir};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

// ------------------------------------------------------------- basics

#[tokio::test(flavor = "multi_thread")]
async fn static_assets_and_info() {
    let s = Srv::mock();
    let (st, html) = s.get_text("/").await;
    assert_eq!(st, 200);
    assert!(html.contains("<title>") && html.contains("js/main.js"), "index.html");
    for asset in ["/js/main.js", "/js/circuit.js", "/js/parts.js", "/css/app.css"] {
        assert_eq!(s.get_text(asset).await.0, 200, "{}", asset);
    }
    assert_eq!(s.get("/does/not/exist").await.0, 404);
    let (_, info) = s.get("/api/info").await;
    assert_eq!(info["root"].as_str().map(PathBuf::from), Some(std::fs::canonicalize(s.dir.path()).unwrap()));
    assert_eq!(info["desktop"], json!(false));
    assert_eq!(info["shell"], json!("browser"));
    assert_eq!(info["platformio_project"], json!(false));
    assert!(info["version"].as_str().unwrap().contains('.'));
}

#[tokio::test(flavor = "multi_thread")]
async fn catalog_boards_and_templates() {
    let s = Srv::mock();
    let (_, cat) = s.get("/api/catalog").await;
    let kinds: Vec<&str> = cat.as_array().unwrap().iter().map(|p| p["type"].as_str().unwrap()).collect();
    for k in ["ssd1306", "pn532", "keypad-4x3", "pcf8575", "ads1115", "mcp3421", "gy-86", "ws2812", "buzzer"] {
        assert!(kinds.contains(&k), "{} in catalog", k);
    }
    let (_, boards) = s.get("/api/boards").await;
    assert_eq!(boards.as_array().unwrap().len(), 3);
    let (_, t) = s.get("/api/templates").await;
    let names: Vec<&str> = t.as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"blink") && names.contains(&"oled") && names.contains(&"keypad"), "{:?}", names);
}

// -------------------------------------------------------------- project

#[tokio::test(flavor = "multi_thread")]
async fn project_get_and_put() {
    let s = Srv::mock();
    let (_, p) = s.get("/api/project").await;
    assert_eq!(p["name"], json!("mockfw"));
    assert_eq!(p["components"].as_array().unwrap().len(), 6);
    assert!(p["yaml"].as_str().unwrap().contains("firmware: mockfw.bin"));
    // add a buzzer through the YAML
    let yaml = p["yaml"].as_str().unwrap().replace("components:\n", "components:\n  - {id: bz, type: buzzer}\n");
    let (st, r) = s.put("/api/project", json!({"yaml": yaml})).await;
    assert_eq!((st, r["ok"].clone()), (200, json!(true)), "{}", r);
    let on_disk = std::fs::read_to_string(s.path("esp32sim.yaml")).unwrap();
    assert!(on_disk.contains("{id: bz, type: buzzer}"), "{}", on_disk);
    // invalid YAML and invalid wiring are rejected without touching the file
    let (st, r) = s.put("/api/project", json!({"yaml": "components: [ {id: x"})).await;
    assert_eq!(st, 400, "{}", r);
    let bad = yaml.replace("[esp32.D5, r1.1]", "[esp32.D5, r1.9]");
    let (st, r) = s.put("/api/project", json!({"yaml": bad})).await;
    assert_eq!(st, 400);
    assert!(r["error"].as_str().unwrap().contains("r1.9"), "{}", r);
    assert_eq!(std::fs::read_to_string(s.path("esp32sim.yaml")).unwrap(), on_disk);
    // the structured form works too
    let (_, mut p) = s.get("/api/project").await;
    p["name"] = json!("renamed");
    let (st, _) = s.put("/api/project", p).await;
    assert_eq!(st, 200);
    assert!(std::fs::read_to_string(s.path("esp32sim.yaml")).unwrap().starts_with("# Mock") || std::fs::read_to_string(s.path("esp32sim.yaml")).unwrap().contains("name: renamed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn files_crud_and_path_safety() {
    let s = Srv::mock();
    let (_, f) = s.get("/api/files").await;
    let tree = f["tree"].to_string();
    assert!(tree.contains("esp32sim.yaml") && tree.contains("mockfw.bin"), "{}", tree);
    let (st, _) = s.put("/api/file", json!({"path": "src/main.cpp", "content": "void setup() {}\nvoid loop() {}\n"})).await;
    assert_eq!(st, 200);
    let (_, r) = s.get("/api/file?path=src/main.cpp").await;
    assert!(r["content"].as_str().unwrap().contains("void loop"));
    let (st, _) = s.put("/api/file", json!({"path": "lib/mylib", "content": "", "dir": true})).await;
    assert_eq!(st, 200);
    assert!(s.path("lib/mylib").is_dir());
    let (st, _) = s.post("/api/file/rename", json!({"from": "src/main.cpp", "to": "src/app.cpp"})).await;
    assert_eq!(st, 200);
    assert!(s.path("src/app.cpp").exists() && !s.path("src/main.cpp").exists());
    let (st, _) = s.delete("/api/file?path=src/app.cpp").await;
    assert_eq!(st, 200);
    assert!(!s.path("src/app.cpp").exists());
    // no escaping the project folder
    // absolute paths are taken relative to the project
    let (st, _) = s.put("/api/file", json!({"path": "/abs/inside.txt", "content": "x"})).await;
    assert_eq!(st, 200);
    assert!(s.path("abs/inside.txt").exists());
    for bad in ["../outside.txt", "src/../../x", "a/../../../etc/x"] {
        let (st, _) = s.put("/api/file", json!({"path": bad, "content": "x"})).await;
        assert_eq!(st, 400, "{}", bad);
        let (st, _) = s.get(&format!("/api/file?path={}", bad)).await;
        assert_ne!(st, 200, "{}", bad);
    }
    assert_eq!(s.get("/api/file?path=nope.txt").await.0, 404);
}

// ----------------------------------------------------------- simulation

#[tokio::test(flavor = "multi_thread")]
async fn live_simulation_over_websocket() {
    let s = Srv::mock();
    let mut ws = Ws::connect(&s.ws_url()).await;
    ws.wait_serial("mockfw ready", 10).await;
    assert_eq!(ws.status["state"], json!("running"), "{}", ws.status);
    // serial input from the monitor
    ws.send(json!({"type": "serial", "data": "ping\n"})).await;
    ws.wait_serial("pong", 10).await;
    ws.send(json!({"type": "serial", "data": "led on\n"})).await;
    ws.until(10, "LED on", |w| w.component("led")["state"]["brightness"].as_f64().unwrap_or(0.0) > 0.2).await;
    // clicking the button in the GUI (WebSocket input) lights the board LED
    ws.send(json!({"type": "input", "id": "btn", "action": {"action": "press"}})).await;
    ws.until(10, "board LED on", |w| w.component("board_led")["state"]["brightness"].as_f64().unwrap_or(0.0) > 0.05).await;
    ws.send(json!({"type": "input", "id": "btn", "action": {"action": "release"}})).await;
    ws.until(10, "board LED off", |w| w.component("board_led")["state"]["brightness"].as_f64().unwrap_or(1.0) < 0.01).await;
    // the REST input endpoint does the same and validates ids; a click is a
    // 150 ms press
    let (st, _) = s.post("/api/input", json!({"id": "btn", "action": {"action": "click"}})).await;
    assert_eq!(st, 200);
    ws.until(10, "click pressed", |w| w.component("btn")["state"]["pressed"] == json!(true)).await;
    ws.until(10, "click released", |w| w.component("btn")["state"]["pressed"] == json!(false)).await;
    let (st, r) = s.post("/api/input", json!({"id": "ghost", "action": {"action": "press"}})).await;
    assert_eq!(st, 400, "{}", r);
    ws.send(json!({"type": "serial", "data": "count\n"})).await;
    ws.until(10, "count", |w| w.serial.contains("count=2")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn run_controls_pause_reset_speed() {
    let s = Srv::mock();
    let mut ws = Ws::connect(&s.ws_url()).await;
    ws.wait_serial("mockfw ready", 10).await;
    assert_eq!(s.post("/api/control", json!({"cmd": "pause"})).await.0, 200);
    ws.until(10, "paused", |w| w.status["state"] == "paused").await;
    let t = ws.status["time_ns"].as_u64().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ws.until(2, "status", |_| true).await;
    assert_eq!(ws.status["time_ns"].as_u64().unwrap(), t, "time stands still while paused");
    assert_eq!(s.post("/api/control", json!({"cmd": "speed", "value": 4.0})).await.0, 200);
    assert_eq!(s.post("/api/control", json!({"cmd": "resume"})).await.0, 200);
    ws.until(10, "running", |w| w.status["state"] == "running").await;
    // reset (EN button): firmware boots again
    let before = ws.serial.matches("mockfw ready").count();
    assert_eq!(s.post("/api/control", json!({"cmd": "reset"})).await.0, 200);
    ws.until(10, "second boot", move |w| w.serial.matches("mockfw ready").count() > before).await;
    // the firmware's own software reset is reported as well
    ws.send(json!({"type": "serial", "data": "reboot\n"})).await;
    ws.until(10, "third boot", move |w| w.serial.matches("mockfw ready").count() > before + 1).await;
    assert_eq!(s.post("/api/control", json!({"cmd": "erase-flash"})).await.0, 200);
    assert_eq!(s.post("/api/control", json!({"cmd": "bogus"})).await.0, 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn logic_trace_and_bus_monitor() {
    let s = Srv::mock();
    let mut ws = Ws::connect(&s.ws_url()).await;
    ws.wait_serial("mockfw ready", 10).await;
    ws.send(json!({"type": "serial", "data": "ping\n"})).await;
    ws.wait_serial("pong", 10).await;
    // heartbeat edges on GPIO18 appear in the logic analyser trace
    let tr = s.wait_for("/api/trace?since=0", 15, |v| v["events"].as_array().map(|e| e.iter().any(|x| x[1] == 18)).unwrap_or(false)).await;
    assert!(tr["now"].as_u64().unwrap() > 0);
    // UART traffic is in the bus monitor, both directions
    let bl = s.wait_for("/api/buslog?since=0", 10, |v| v["events"].as_array().map(|e| e.len() > 2).unwrap_or(false)).await;
    let txt = bl["events"].to_string();
    assert!(txt.contains("UART"), "{}", txt);
    let next = bl["next"].as_u64().unwrap();
    let (_, later) = s.get(&format!("/api/buslog?since={}", next)).await;
    assert!(later["events"].as_array().unwrap().len() < bl["events"].as_array().unwrap().len() + 50);
}

#[tokio::test(flavor = "multi_thread")]
async fn display_framebuffer_endpoint() {
    let s = Srv::mock();
    let (_, p) = s.get("/api/project").await;
    let yaml = p["yaml"].as_str().unwrap().replace("components:\n", "components:\n  - {id: tft, type: st7789}\n");
    assert_eq!(s.put("/api/project", json!({"yaml": yaml})).await.0, 200);
    let t0 = std::time::Instant::now();
    let fb = loop {
        let (st, b) = s.get_bytes("/api/fb/tft").await;
        if st == 200 {
            break b;
        }
        assert!(t0.elapsed().as_secs() < 10, "framebuffer never available");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let w = u32::from_le_bytes(fb[0..4].try_into().unwrap()) as usize;
    let h = u32::from_le_bytes(fb[4..8].try_into().unwrap()) as usize;
    assert_eq!((w, h), (240, 240));
    assert_eq!(fb.len(), 8 + w * h * 3);
    assert_eq!(s.get("/api/fb/led").await.0, 404, "LEDs have no framebuffer");
}

// ------------------------------------------------------------- settings

#[tokio::test(flavor = "multi_thread")]
async fn settings_never_return_api_keys() {
    let s = Srv::mock();
    let (st, v) = s.put("/api/settings", json!({"ai": {"provider": "anthropic", "model": "m1", "keys": {"anthropic": "sk-secret"}, "inline": true}})).await;
    assert_eq!(st, 200);
    assert_eq!(v["ai"]["provider"], json!("anthropic"));
    assert_eq!(v["ai"]["keys_set"]["anthropic"], json!(true));
    assert!(!v.to_string().contains("sk-secret"));
    let (_, v) = s.get("/api/settings").await;
    assert!(!v.to_string().contains("sk-secret"));
    assert_eq!(v["ai"]["inline"], json!(true));
    // empty string removes the key; tools paths can be set and cleared
    let (_, v) = s.put("/api/settings", json!({"ai": {"keys": {"anthropic": ""}}, "pio": "/opt/pio/bin/pio"})).await;
    assert_eq!(v["ai"]["keys_set"].get("anthropic"), None);
    assert_eq!(v["pio"], json!("/opt/pio/bin/pio"));
    let (_, v) = s.put("/api/settings", json!({"pio": ""})).await;
    assert_eq!(v["pio"], Value::Null);
    assert!(v["tools"]["git"].is_string(), "git is detected: {}", v["tools"]);
}

// ------------------------------------------------------------------- AI

async fn configure_ai(s: &Srv, provider: &str, base: &str) {
    let (st, _) = s.put("/api/settings", json!({"ai": {"provider": provider, "model": "", "base_url": base, "keys": {provider: "test-key"}}})).await;
    assert_eq!(st, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn ai_chat_streams_from_every_provider() {
    let (mock, seen) = mock_ai().await;
    let s = Srv::mock();
    for (provider, prefix) in [("openai", "/openai/v1"), ("anthropic", "/anthropic"), ("gemini", "/gemini"), ("ollama", "/ollama"), ("openai-compatible", "/openai/v1")] {
        configure_ai(&s, provider, &format!("{}{}", mock, prefix)).await;
        let (st, text) = s
            .post_text(
                "/api/ai/chat",
                json!({"messages": [{"role": "user", "content": "why is my LED off?"}], "context": {"file": "src/main.cpp", "content": "int led = 5;", "selection": "led"}}),
            )
            .await;
        assert_eq!(st, 200);
        assert_eq!(text, "Hello from mock!", "{}", provider);
    }
    let reqs = seen.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 5);
    // the assistant gets the circuit, the open file and the selection
    let (path, body, hdrs) = &reqs[0];
    assert!(path.starts_with("/openai/v1/chat/completions"), "{}", path);
    let sys = body["messages"][0]["content"].as_str().unwrap();
    assert!(sys.contains("esp32sim.yaml") && sys.contains("[esp32.D5, r1.1]"), "circuit in the prompt");
    assert!(sys.contains("int led = 5;") && sys.contains("## Selected code"));
    assert_eq!(body["messages"][1]["content"], json!("why is my LED off?"));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["model"], json!("gpt-4o-mini"), "default model");
    assert!(hdrs.iter().any(|(k, v)| k == "authorization" && v == "Bearer test-key"));
    let (path, body, hdrs) = &reqs[1];
    assert!(path.ends_with("/v1/messages?"), "{}", path);
    assert!(body["system"].as_str().unwrap().contains("esp32sim"));
    assert!(hdrs.iter().any(|(k, v)| k == "x-api-key" && v == "test-key"));
    assert!(hdrs.iter().any(|(k, _)| k == "anthropic-version"));
    let (path, body, _) = &reqs[2];
    assert!(path.contains("gemini-2.5-flash:streamGenerateContent") && path.contains("key=test-key"), "{}", path);
    assert_eq!(body["contents"][0]["parts"][0]["text"], json!("why is my LED off?"));
    let (path, body, _) = &reqs[3];
    assert!(path.starts_with("/ollama/api/chat"));
    assert_eq!(body["messages"][0]["role"], json!("system"));
}

#[tokio::test(flavor = "multi_thread")]
async fn ai_errors_are_reported_in_the_stream() {
    let (mock, _seen) = mock_ai().await;
    let s = Srv::mock();
    // no key configured
    let (_, _) = s.put("/api/settings", json!({"ai": {"provider": "openai", "base_url": "", "keys": {"openai": ""}}})).await;
    let (_, text) = s.post_text("/api/ai/chat", json!({"messages": [{"role": "user", "content": "hi"}]})).await;
    assert!(text.contains("**Error:**") && text.contains("no API key"), "{}", text);
    // upstream rejects the key
    configure_ai(&s, "openai", &format!("{}/fail", mock)).await;
    let (_, text) = s.post_text("/api/ai/chat", json!({"messages": [{"role": "user", "content": "hi"}]})).await;
    assert!(text.contains("401"), "{}", text);
    // provider unreachable
    configure_ai(&s, "openai", "http://127.0.0.1:9").await;
    let (_, text) = s.post_text("/api/ai/chat", json!({"messages": [{"role": "user", "content": "hi"}]})).await;
    assert!(text.contains("request failed"), "{}", text);
}

#[tokio::test(flavor = "multi_thread")]
async fn ai_inline_completion_and_model_list() {
    let (mock, seen) = mock_ai().await;
    let s = Srv::mock();
    configure_ai(&s, "openai", &format!("{}/openai/v1", mock)).await;
    let (st, v) = s.post("/api/ai/complete", json!({"prefix": "void setup() {\n  Serial.", "suffix": "\n}", "path": "src/main.cpp"})).await;
    assert_eq!(st, 200);
    assert_eq!(v["text"], json!("Hello from mock!"));
    let req = seen.requests.lock().unwrap().last().cloned().unwrap();
    assert!(req.1["messages"][1]["content"].as_str().unwrap().contains("Serial.<CURSOR>"));
    assert_eq!(req.1["temperature"], json!(0.1));
    let (st, v) = s.get("/api/ai/models").await;
    assert_eq!(st, 200);
    assert_eq!(v["models"], json!(["mock-a", "mock-b"]), "sorted");
    configure_ai(&s, "ollama", &format!("{}/ollama", mock)).await;
    assert_eq!(s.get("/api/ai/models").await.1["models"], json!(["llama-mock"]));
    configure_ai(&s, "gemini", &format!("{}/gemini", mock)).await;
    assert_eq!(s.get("/api/ai/models").await.1["models"], json!(["gemini-mock"]));
}

// ------------------------------------------------------------------ git

#[tokio::test(flavor = "multi_thread")]
async fn git_workflow() {
    let s = Srv::mock();
    let (st, _) = s.post("/api/git/action", json!({"action": "init"})).await;
    assert_eq!(st, 200);
    assert!(std::fs::read_to_string(s.path(".gitignore")).unwrap().contains(".esp32sim/"));
    let (_, status) = s.get("/api/git/status").await;
    assert!(status.to_string().contains("esp32sim.yaml"), "{}", status);
    assert_eq!(s.post("/api/git/action", json!({"action": "stage_all"})).await.0, 200);
    let (st, r) = s.post("/api/git/action", json!({"action": "commit", "message": ""})).await;
    assert_eq!(st, 400, "{}", r);
    let (st, r) = s.post("/api/git/action", json!({"action": "commit", "message": "first"})).await;
    assert_eq!(st, 200, "{}", r);
    let (_, log) = s.get("/api/git/log").await;
    assert!(log.to_string().contains("first"), "{}", log);
    // modify, diff, show, branch
    write(&s.path("notes.txt"), "hello\n");
    std::fs::write(s.path("esp32sim.yaml"), std::fs::read_to_string(s.path("esp32sim.yaml")).unwrap().replace("name: mockfw", "name: changed")).unwrap();
    let (_, d) = s.get("/api/git/diff?path=esp32sim.yaml").await;
    assert!(d["diff"].as_str().unwrap().contains("+name: changed"), "{}", d);
    let (_, sh) = s.get("/api/git/show?path=esp32sim.yaml").await;
    assert!(sh["content"].as_str().unwrap().contains("name: mockfw"));
    assert_eq!(s.post("/api/git/action", json!({"action": "branch", "name": "feature"})).await.0, 200);
    let (_, status) = s.get("/api/git/status").await;
    assert!(status.to_string().contains("feature"), "{}", status);
    assert_eq!(s.post("/api/git/action", json!({"action": "discard", "paths": ["esp32sim.yaml"]})).await.0, 200);
    assert!(std::fs::read_to_string(s.path("esp32sim.yaml")).unwrap().contains("name: mockfw"));
    assert_eq!(s.post("/api/git/action", json!({"action": "nope"})).await.0, 400);
}

// ---------------------------------------------------------- build / IDE

#[tokio::test(flavor = "multi_thread")]
async fn build_reports_missing_project_files() {
    let s = Srv::mock();
    let (st, r) = s.post("/api/build", json!({})).await;
    assert_eq!(st, 400);
    assert!(r["error"].as_str().unwrap().contains("platformio.ini"), "{}", r);
    let (_, b) = s.get("/api/build").await;
    assert_eq!((b["running"].clone(), b["ok"].clone()), (json!(false), json!(false)));
    // a sketch without arduino-cli suggests converting it
    write(&s.path("blink.ino"), "void setup(){}\nvoid loop(){}\n");
    let (st, r) = s.post("/api/build", json!({})).await;
    if st == 400 {
        assert!(r["error"].as_str().unwrap().contains("Convert to PlatformIO"), "{}", r);
    }
    let (st, _) = s.post("/api/convert-sketch", json!({})).await;
    assert_eq!(st, 200);
    let ini = std::fs::read_to_string(s.path("platformio.ini")).unwrap();
    assert!(ini.contains("framework = arduino") && ini.contains("board = esp32dev"), "{}", ini);
    assert_eq!(s.get("/api/info").await.1["platformio_project"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn new_project_from_template_then_reopen() {
    let s = Srv::mock();
    let other = TempDir::new("newproj");
    let target = other.path().join("my-oled");
    let (st, r) = s.post("/api/project/new", json!({"path": target.display().to_string(), "template": "oled"})).await;
    assert_eq!(st, 200, "{}", r);
    assert!(target.join("platformio.ini").exists() && target.join("esp32sim.yaml").exists());
    let (_, p) = s.get("/api/project").await;
    assert!(p["yaml"].as_str().unwrap().contains("ssd1306"), "{}", p["yaml"]);
    let (_, settings) = s.get("/api/settings").await;
    assert!(settings["recent"].to_string().contains("my-oled"));
    // back to the mock project
    let (st, _) = s.post("/api/project/open", json!({"path": s.dir.path().display().to_string()})).await;
    assert_eq!(st, 200);
    assert_eq!(s.get("/api/project").await.1["name"], json!("mockfw"));
    let (st, r) = s.post("/api/project/open", json!({"path": "/definitely/not/here"})).await;
    assert_eq!(st, 400, "{}", r);
    assert_eq!(s.post("/api/project/reload", json!({})).await.0, 200);
}

// ---------------------------------------------------------- native shell

struct FakeShell(PathBuf);
impl NativeHooks for FakeShell {
    fn pick_folder(&self, _title: &str) -> Option<PathBuf> {
        Some(self.0.clone())
    }
    fn pick_file(&self, _title: &str, ext: &[&str]) -> Option<PathBuf> {
        if ext.contains(&"bin") {
            Some(self.0.join("mockfw.bin"))
        } else {
            None
        }
    }
    fn shell(&self) -> &str {
        "tauri"
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn native_dialogs_only_in_the_desktop_shell() {
    let s = Srv::mock();
    assert_eq!(s.post("/api/native/pick-folder", json!({})).await.0, 501);
    let dir = TempDir::new("native");
    write_mock_project(dir.path()).unwrap();
    let p = dir.path().to_path_buf();
    let d = Srv::start_with(dir, Some(Arc::new(FakeShell(p.clone()))));
    let (_, info) = d.get("/api/info").await;
    assert_eq!((info["desktop"].clone(), info["shell"].clone()), (json!(true), json!("tauri")));
    let (st, r) = d.post("/api/native/pick-folder", json!({"title": "Open"})).await;
    assert_eq!(st, 200);
    assert_eq!(r["path"].as_str().map(PathBuf::from), Some(p.clone()));
    let (_, r) = d.post("/api/native/pick-file", json!({"extensions": ["bin"]})).await;
    assert!(r["path"].as_str().unwrap().ends_with("mockfw.bin"));
    let (_, r) = d.post("/api/native/pick-file", json!({"extensions": ["txt"]})).await;
    assert_eq!(r["path"], Value::Null, "cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn server_handle_shuts_down_cleanly() {
    let s = Srv::mock();
    let url = s.base.clone();
    assert_eq!(s.get("/api/info").await.0, 200);
    drop(s);
    let c = reqwest::Client::new();
    assert!(c.get(format!("{}/api/info", url)).send().await.is_err(), "port closed after drop");
}

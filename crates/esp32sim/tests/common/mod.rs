//! Shared helpers for the esp32sim integration tests: an isolated config
//! directory, an in-process GUI server on a free port, HTTP / WebSocket
//! clients and a mock AI provider speaking the four streaming formats.
#![allow(dead_code)]
use esp32sim::testing::{write_mock_project, TempDir};
use esp32sim::{server, ServerOptions};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

/// Keep settings / recent projects of the tests away from the user's config.
pub fn isolate_config() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let d = std::env::temp_dir().join(format!("esp32sim-test-home-{}", std::process::id()));
        let _ = std::fs::create_dir_all(d.join(".config"));
        std::env::set_var("XDG_CONFIG_HOME", d.join(".config"));
        std::env::set_var("XDG_CACHE_HOME", d.join(".cache"));
    });
}

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// An in-process GUI server on a mock project in a temp dir.
pub struct Srv {
    pub h: esp32sim::ServerHandle,
    pub base: String,
    pub c: reqwest::Client,
    pub dir: TempDir,
}

impl Srv {
    pub fn start_with(dir: TempDir, native: Option<Arc<dyn server::NativeHooks>>) -> Srv {
        isolate_config();
        let h = server::start(ServerOptions { target: Some(dir.path().to_path_buf()), port: 0, native, no_recent: true, ..Default::default() }).expect("server");
        let base = h.url.trim_end_matches('/').to_string();
        Srv { h, base, c: reqwest::Client::new(), dir }
    }
    /// Server on a fresh copy of the mock firmware project.
    pub fn mock() -> Srv {
        let dir = TempDir::new("srv");
        write_mock_project(dir.path()).unwrap();
        Srv::start_with(dir, None)
    }
    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    pub async fn get(&self, p: &str) -> (u16, Value) {
        let r = self.c.get(format!("{}{}", self.base, p)).send().await.expect("GET");
        let st = r.status().as_u16();
        (st, r.json().await.unwrap_or(Value::Null))
    }
    pub async fn get_text(&self, p: &str) -> (u16, String) {
        let r = self.c.get(format!("{}{}", self.base, p)).send().await.expect("GET");
        (r.status().as_u16(), r.text().await.unwrap_or_default())
    }
    pub async fn get_bytes(&self, p: &str) -> (u16, Vec<u8>) {
        let r = self.c.get(format!("{}{}", self.base, p)).send().await.expect("GET");
        (r.status().as_u16(), r.bytes().await.map(|b| b.to_vec()).unwrap_or_default())
    }
    pub async fn post(&self, p: &str, body: Value) -> (u16, Value) {
        let r = self.c.post(format!("{}{}", self.base, p)).json(&body).send().await.expect("POST");
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
    pub async fn post_text(&self, p: &str, body: Value) -> (u16, String) {
        let r = self.c.post(format!("{}{}", self.base, p)).json(&body).send().await.expect("POST");
        (r.status().as_u16(), r.text().await.unwrap_or_default())
    }
    pub async fn put(&self, p: &str, body: Value) -> (u16, Value) {
        let r = self.c.put(format!("{}{}", self.base, p)).json(&body).send().await.expect("PUT");
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
    pub async fn delete(&self, p: &str) -> (u16, Value) {
        let r = self.c.delete(format!("{}{}", self.base, p)).send().await.expect("DELETE");
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
    pub fn ws_url(&self) -> String {
        format!("{}/ws", self.base.replace("http://", "ws://"))
    }
    /// Poll a GET endpoint until `pred` holds (or panic after `secs`).
    pub async fn wait_for(&self, p: &str, secs: u64, pred: impl Fn(&Value) -> bool) -> Value {
        let t0 = Instant::now();
        loop {
            let (_, v) = self.get(p).await;
            if pred(&v) {
                return v;
            }
            assert!(t0.elapsed() < Duration::from_secs(secs), "timeout waiting on {}: last {}", p, v);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// WebSocket session with the GUI server: collects serial output and the
/// latest board snapshot from the `update` messages.
pub struct Ws {
    pub tx: futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, tokio_tungstenite::tungstenite::Message>,
    pub rx: futures::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>,
    pub serial: String,
    pub board: Value,
    pub status: Value,
    pub messages: Vec<String>,
    pub resets: usize,
}

impl Ws {
    pub async fn connect(url: &str) -> Ws {
        let (s, _) = tokio_tungstenite::connect_async(url).await.expect("ws connect");
        let (tx, rx) = s.split();
        Ws { tx, rx, serial: String::new(), board: Value::Null, status: Value::Null, messages: vec![], resets: 0 }
    }
    pub async fn send(&mut self, v: Value) {
        self.tx.send(tokio_tungstenite::tungstenite::Message::Text(v.to_string())).await.expect("ws send");
    }
    async fn pump_once(&mut self, wait: Duration) {
        use base64::Engine;
        if let Ok(Some(Ok(m))) = tokio::time::timeout(wait, self.rx.next()).await {
            if let tokio_tungstenite::tungstenite::Message::Text(t) = m {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == "reset" {
                    self.resets += 1;
                }
                if let Some(s) = v.get("serial").and_then(|s| s.as_str()) {
                    let b = base64::engine::general_purpose::STANDARD.decode(s).unwrap();
                    self.serial.push_str(&String::from_utf8_lossy(&b));
                }
                if let Some(b) = v.get("board") {
                    self.board = b.clone();
                }
                if let Some(s) = v.get("status") {
                    self.status = s.clone();
                }
                if let Some(ms) = v.get("messages").and_then(|m| m.as_array()) {
                    self.messages.extend(ms.iter().map(|m| m.to_string()));
                }
            }
        }
    }
    /// Read messages until `pred(self)` holds.
    pub async fn until(&mut self, secs: u64, what: &str, pred: impl Fn(&Ws) -> bool) {
        let t0 = Instant::now();
        while !pred(self) {
            if t0.elapsed() >= Duration::from_secs(secs) {
                let tail: String = self.serial.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
                panic!("timeout waiting for {}; status {}; serial tail: {:?}", what, self.status, tail);
            }
            self.pump_once(Duration::from_millis(200)).await;
        }
    }
    pub async fn wait_serial(&mut self, text: &str, secs: u64) {
        let t = text.to_string();
        self.until(secs, &format!("serial {:?}", text), move |w| w.serial.contains(&t)).await;
    }
    pub fn component(&self, id: &str) -> Value {
        self.board["components"].as_array().and_then(|a| a.iter().find(|c| c["id"] == id).cloned()).unwrap_or(Value::Null)
    }
}

// ------------------------------------------------------------ mock AI provider

/// Requests seen by the mock provider: (path, JSON body, headers of interest).
#[derive(Default, Clone)]
pub struct Seen {
    pub requests: Arc<Mutex<Vec<(String, Value, Vec<(String, String)>)>>>,
}

/// Start a mock of OpenAI / Anthropic / Gemini / Ollama streaming APIs.
/// Every chat answers "Hello from <provider> mock!" split into chunks.
pub async fn mock_ai() -> (String, Seen) {
    use axum::body::Body;
    use axum::extract::{Request, State};
    use axum::response::{IntoResponse, Response};
    let seen = Seen::default();
    async fn handle(State(seen): State<Seen>, req: Request) -> Response {
        let path = req.uri().path().to_string();
        let query = req.uri().query().unwrap_or("").to_string();
        let hdrs: Vec<(String, String)> = req
            .headers()
            .iter()
            .filter(|(k, _)| ["authorization", "x-api-key", "anthropic-version"].contains(&k.as_str()))
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let bytes = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap_or_default();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        seen.requests.lock().unwrap().push((format!("{}?{}", path, query), body, hdrs));
        let words = ["Hello", " from", " mock", "!"];
        let sse = |f: &dyn Fn(&str) -> Value| -> String {
            let mut s: String = words.iter().map(|w| format!("data: {}\n\n", f(w))).collect();
            s.push_str("data: [DONE]\n\n");
            s
        };
        // each provider lives under its own prefix: /openai, /anthropic, /gemini, /ollama
        let (ctype, text) = if path.starts_with("/fail") {
            return (axum::http::StatusCode::UNAUTHORIZED, "invalid api key").into_response();
        } else if path.starts_with("/openai") && path.ends_with("/chat/completions") {
            ("text/event-stream", sse(&|w| json!({"choices": [{"delta": {"content": w}}]})))
        } else if path.starts_with("/openai") && path.ends_with("/models") {
            ("application/json", json!({"data": [{"id": "mock-b"}, {"id": "mock-a"}]}).to_string())
        } else if path.starts_with("/anthropic") && path.ends_with("/v1/messages") {
            let mut s = String::from("event: message_start\ndata: {\"type\":\"message_start\"}\n\n");
            for w in words {
                s.push_str(&format!("event: content_block_delta\ndata: {}\n\n", json!({"type": "content_block_delta", "delta": {"type": "text_delta", "text": w}})));
            }
            s.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
            ("text/event-stream", s)
        } else if path.starts_with("/anthropic") && path.ends_with("/v1/models") {
            ("application/json", json!({"data": [{"id": "claude-mock"}]}).to_string())
        } else if path.starts_with("/gemini") && path.contains(":streamGenerateContent") {
            ("text/event-stream", words.iter().map(|w| format!("data: {}\n\n", json!({"candidates": [{"content": {"parts": [{"text": w}]}}]}))).collect())
        } else if path.starts_with("/gemini") && path.contains("/v1beta/models") {
            ("application/json", json!({"models": [{"name": "models/gemini-mock"}]}).to_string())
        } else if path.starts_with("/ollama") && path.ends_with("/api/chat") {
            ("application/x-ndjson", words.iter().map(|w| format!("{}\n", json!({"message": {"content": w}, "done": false}))).collect::<String>() + &format!("{}\n", json!({"done": true})))
        } else if path.starts_with("/ollama") && path.ends_with("/api/tags") {
            ("application/json", json!({"models": [{"name": "llama-mock"}]}).to_string())
        } else {
            return (axum::http::StatusCode::NOT_FOUND, "no such mock route").into_response();
        };
        ([(axum::http::header::CONTENT_TYPE, ctype)], Body::from(text)).into_response()
    }
    let app = axum::Router::new().fallback(handle).with_state(seen.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    (url, seen)
}

pub fn write(p: &Path, s: &str) {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, s).unwrap();
}

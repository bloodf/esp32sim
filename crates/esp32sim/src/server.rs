//! Web GUI / IDE server.

use crate::ide::{self, Settings};
use crate::sim::{Cmd, Sim, SimHandle};
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use devices::ProjectConfig;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(RustEmbed)]
#[folder = "../../web/"]
struct Assets;

#[derive(Default)]
pub struct BuildState {
    pub running: bool,
    pub lines: Vec<String>,
    pub seq: u64,
    pub ok: Option<bool>,
    pub diagnostics: Vec<ide::Diagnostic>,
}

pub struct App {
    pub sim: Mutex<Option<SimHandle>>,
    pub project: Mutex<ProjectConfig>,
    pub root: Mutex<PathBuf>,
    pub build: Mutex<BuildState>,
    pub settings: Mutex<Settings>,
    pub native: Option<Arc<dyn NativeHooks>>,
}

type S = State<Arc<App>>;

fn err(code: StatusCode, m: impl ToString) -> Response {
    (code, Json(json!({"error": m.to_string()}))).into_response()
}

impl App {
    pub fn root(&self) -> PathBuf {
        self.root.lock().unwrap().clone()
    }
    fn with_sim<R>(&self, f: impl FnOnce(&SimHandle) -> R) -> Option<R> {
        self.sim.lock().unwrap().as_ref().map(f)
    }
    /// (Re)start the simulator for the current project.
    pub fn restart_sim(&self) {
        let cfg = self.project.lock().unwrap().clone();
        let mut g = self.sim.lock().unwrap();
        if let Some(old) = g.take() {
            old.send(Cmd::Quit);
        }
        *g = Some(Sim::start(cfg, true));
    }
    pub fn reload_sim(&self) {
        let cfg = self.project.lock().unwrap().clone();
        // a simulator thread that died (internal error) is replaced instead
        let have = self.sim.lock().unwrap().as_ref().map(|s| s.thread.as_ref().map(|t| !t.is_finished()).unwrap_or(false)).unwrap_or(false);
        if have {
            self.with_sim(|s| s.send(Cmd::Reload(Box::new(cfg), None)));
        } else {
            self.restart_sim();
        }
    }
    pub fn open(&self, target: &Path) -> Result<(), String> {
        let cfg = crate::project::load(target)?;
        let root = cfg.source.as_ref().map(|s| cfg.resolve(s)).unwrap_or_else(|| cfg.base_dir.clone());
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        {
            let mut st = self.settings.lock().unwrap();
            st.add_recent(&std::fs::canonicalize(target).unwrap_or(target.to_path_buf()).display().to_string());
            let _ = st.save();
        }
        *self.project.lock().unwrap() = cfg;
        *self.root.lock().unwrap() = root;
        self.restart_sim();
        Ok(())
    }
}

/// Services the embedding application can provide (the desktop shell offers
/// native file dialogs; the browser build has none).
pub trait NativeHooks: Send + Sync {
    /// Let the user pick a folder; `None` if cancelled.
    fn pick_folder(&self, title: &str) -> Option<PathBuf>;
    /// Let the user pick a file (e.g. firmware); `None` if cancelled.
    fn pick_file(&self, title: &str, extensions: &[&str]) -> Option<PathBuf> {
        let _ = (title, extensions);
        None
    }
    /// Name shown in the UI ("desktop", "tauri", ...).
    fn shell(&self) -> &str {
        "desktop"
    }
}

/// How to start the GUI server.
#[derive(Clone)]
pub struct ServerOptions {
    /// project file or directory to open (default: current folder or most recent project)
    pub target: Option<PathBuf>,
    /// TCP port; 0 picks a free one
    pub port: u16,
    pub host: String,
    pub open_browser: bool,
    /// native services of an embedding shell
    pub native: Option<Arc<dyn NativeHooks>>,
    /// do not fall back to the most recent project when `target` is None
    pub no_recent: bool,
}

impl Default for ServerOptions {
    fn default() -> Self {
        ServerOptions { target: None, port: 8732, host: "127.0.0.1".into(), open_browser: false, native: None, no_recent: false }
    }
}

/// A running GUI server.  Dropping it stops the server and the simulator.
pub struct ServerHandle {
    pub port: u16,
    pub url: String,
    pub app: Arc<App>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ServerHandle {
    /// Block until the server stops (it normally never does).
    pub fn wait(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
    /// Open another project (as the File menu of a desktop shell would).
    pub fn open_project(&self, target: &Path) -> Result<(), String> {
        self.app.open(target)
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(sim) = self.app.sim.lock().unwrap().take() {
            sim.send(Cmd::Quit);
        }
    }
}

/// Create the shared application state and open the initial project.
pub fn new_app(opts: &ServerOptions) -> Result<Arc<App>, String> {
    let app = Arc::new(App {
        sim: Mutex::new(None),
        project: Mutex::new(ProjectConfig::default()),
        root: Mutex::new(std::env::current_dir().unwrap_or_default()),
        build: Mutex::new(BuildState::default()),
        settings: Mutex::new(Settings::load()),
        native: opts.native.clone(),
    });
    // no project given: the current folder if it is one, else the most recent project
    let target = opts.target.clone().or_else(|| {
        let cwd = std::env::current_dir().ok()?;
        if ["esp32sim.yaml", "esp32sim.yml", "platformio.ini"].iter().any(|f| cwd.join(f).exists()) {
            return Some(cwd);
        }
        if opts.no_recent {
            return None;
        }
        let recent = app.settings.lock().unwrap().recent.clone();
        recent.into_iter().map(PathBuf::from).find(|p| p.exists())
    });
    if let Some(t) = &target {
        app.open(t)?;
    } else {
        // nothing to open: work in ~/esp32-projects rather than an arbitrary folder
        let dir = ide::home().join("esp32-projects");
        let _ = std::fs::create_dir_all(&dir);
        *app.root.lock().unwrap() = dir;
    }
    Ok(app)
}

/// All HTTP / WebSocket routes of the GUI.
pub fn router(app: Arc<App>) -> Router {
Router::new()
        .route("/ws", get(ws_handler))
        .route("/lsp", get(lsp_handler))
        .route("/api/info", get(info))
        .route("/api/catalog", get(|| async { Json(devices::registry::catalog_json()) }))
        .route("/api/boards", get(|| async { Json(devices::registry::boards_json()) }))
        .route("/api/templates", get(templates))
        .route("/api/project", get(get_project).put(put_project))
        .route("/api/project/open", post(open_project))
        .route("/api/project/new", post(new_project))
        .route("/api/project/reload", post(reload_project))
        .route("/api/input", post(post_input))
        .route("/api/control", post(control))
        .route("/api/fb/:id", get(framebuffer))
        .route("/api/files", get(files))
        .route("/api/file", get(read_file).put(write_file).delete(delete_file))
        .route("/api/file/rename", post(rename_file))
        .route("/api/build", post(start_build).get(build_status))
        .route("/api/convert-sketch", post(convert_sketch))
        .route("/api/git/status", get(git_status))
        .route("/api/git/log", get(git_log))
        .route("/api/git/diff", get(git_diff))
        .route("/api/git/show", get(git_show))
        .route("/api/git/action", post(git_action))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/ai/models", get(ai_models))
        .route("/api/ai/chat", post(ai_chat))
        .route("/api/ai/complete", post(ai_complete))
        .route("/api/debug/files", get(debug_files))
        .route("/api/debug/breakpoints", post(set_breakpoints))
        .route("/api/debug/regs", get(debug_regs))
        .route("/api/debug/step", post(debug_step))
        .route("/api/debug/mem", get(debug_mem))
        .route("/api/debug/symbol", get(debug_symbol))
        .route("/api/trace", get(pad_trace))
        .route("/api/buslog", get(bus_log))
        .route("/api/debug/lookup", get(debug_lookup))
        .fallback(get(static_file))
        .route("/api/native/pick-folder", post(native_pick_folder))
        .route("/api/native/pick-file", post(native_pick_file))
        .fallback(get(static_file))
        .with_state(app)
}

/// Start the server on a background thread; returns once it is listening.
pub fn start(opts: ServerOptions) -> Result<ServerHandle, String> {
    let app = new_app(&opts)?;
    let addr: std::net::SocketAddr = format!("{}:{}", opts.host, opts.port).parse().map_err(|e| format!("bad address {}:{}: {}", opts.host, opts.port, e))?;
    let std_listener = std::net::TcpListener::bind(addr).map_err(|e| format!("cannot listen on {}: {}", addr, e))?;
    std_listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = std_listener.local_addr().map_err(|e| e.to_string())?.port();
    let host = if opts.host == "0.0.0.0" { "127.0.0.1".to_string() } else { opts.host.clone() };
    let url = format!("http://{}:{}/", host, port);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let router = router(app.clone());
    let thread = std::thread::Builder::new()
        .name("esp32sim-http".into())
        .spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("error: {}", e);
                    return;
                }
            };
            rt.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(std_listener) {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("error: {}", e);
                        return;
                    }
                };
                let _ = axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let _ = rx.await;
                    })
                    .await;
            });
        })
        .map_err(|e| e.to_string())?;
    if opts.open_browser {
        open_url(&url);
    }
    Ok(ServerHandle { port, url, app, shutdown: Some(tx), thread: Some(thread) })
}

/// `esp32sim gui`: start and serve until killed.
pub fn run(opts: ServerOptions) -> Result<i32, String> {
    let h = start(opts)?;
    println!("esp32sim GUI running at {}", h.url);
    h.wait();
    Ok(0)
}

async fn native_pick_folder(State(app): S, body: Option<Json<Value>>) -> Response {
    let title = body.as_ref().and_then(|b| b.get("title").and_then(|t| t.as_str()).map(String::from)).unwrap_or_else(|| "Open project folder".into());
    match app.native.clone() {
        None => err(StatusCode::NOT_IMPLEMENTED, "native dialogs are only available in the desktop app"),
        Some(n) => {
            let r = tokio::task::spawn_blocking(move || n.pick_folder(&title)).await.ok().flatten();
            Json(json!({"path": r.map(|p| p.display().to_string())})).into_response()
        }
    }
}

async fn native_pick_file(State(app): S, body: Option<Json<Value>>) -> Response {
    let title = body.as_ref().and_then(|b| b.get("title").and_then(|t| t.as_str()).map(String::from)).unwrap_or_else(|| "Choose a file".into());
    let exts: Vec<String> = body.as_ref().and_then(|b| b.get("extensions").and_then(|e| e.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())).unwrap_or_default();
    match app.native.clone() {
        None => err(StatusCode::NOT_IMPLEMENTED, "native dialogs are only available in the desktop app"),
        Some(n) => {
            let r = tokio::task::spawn_blocking(move || {
                let e: Vec<&str> = exts.iter().map(|s| s.as_str()).collect();
                n.pick_file(&title, &e)
            })
            .await
            .ok()
            .flatten();
            Json(json!({"path": r.map(|p| p.display().to_string())})).into_response()
        }
    }
}

fn open_url(url: &str) {
    for cmd in ["taiga-browser", "xdg-open", "open"] {
        if std::process::Command::new(cmd).arg(url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_ok() {
            return;
        }
    }
}

async fn static_file(uri: axum::http::Uri) -> Response {
    let mut path = uri.path().trim_start_matches('/').to_string();
    if path.is_empty() {
        path = "index.html".into();
    }
    match Assets::get(&path) {
        Some(f) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            let mut h = HeaderMap::new();
            h.insert(header::CONTENT_TYPE, mime.as_ref().parse().unwrap());
            if path.starts_with("vendor/") {
                h.insert(header::CACHE_CONTROL, "max-age=86400".parse().unwrap());
            } else {
                h.insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
            }
            (h, Body::from(f.data.into_owned())).into_response()
        }
        None => err(StatusCode::NOT_FOUND, "not found"),
    }
}

async fn info(State(app): S) -> Json<Value> {
    let root = app.root();
    let has_pio = root.join("platformio.ini").exists();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "root": root.display().to_string(),
        "project_dir": app.project.lock().unwrap().base_dir.display().to_string(),
        "platformio_project": has_pio,
        "settings": app.settings.lock().unwrap().public(),
        "desktop": app.native.is_some(),
        "shell": app.native.as_ref().map(|n| n.shell().to_string()).unwrap_or_else(|| "browser".into()),
    }))
}

async fn templates() -> Json<Value> {
    Json(json!(crate::project::TEMPLATES.iter().map(|t| json!({"name": t.name, "description": t.description})).collect::<Vec<_>>()))
}

// ------------------------------------------------------------------ project
async fn get_project(State(app): S) -> Json<Value> {
    let p = app.project.lock().unwrap().clone();
    let mut v = serde_json::to_value(&p).unwrap_or(Value::Null);
    v["yaml"] = json!(p.to_yaml());
    v["path"] = json!(p.base_dir.join(crate::project::PROJECT_FILE).display().to_string());
    Json(v)
}

async fn put_project(State(app): S, Json(body): Json<Value>) -> Response {
    // accepts either {"yaml": "..."} or a full config object
    let parsed = if let Some(y) = body.get("yaml").and_then(|v| v.as_str()) {
        ProjectConfig::parse(y)
    } else {
        serde_json::from_value::<ProjectConfig>(body.clone()).map_err(|e| e.to_string())
    };
    let mut cfg = match parsed {
        Ok(c) => c,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    cfg.base_dir = app.project.lock().unwrap().base_dir.clone();
    if cfg.base_dir.as_os_str().is_empty() {
        cfg.base_dir = app.root();
    }
    // validate wiring before saving
    if let Err(e) = cfg.build_board() {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if let Err(e) = crate::project::save(&cfg) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    *app.project.lock().unwrap() = cfg;
    if body.get("reload").and_then(|v| v.as_bool()).unwrap_or(true) {
        app.reload_sim();
    }
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct PathReq {
    path: String,
    #[serde(default)]
    template: Option<String>,
}

async fn open_project(State(app): S, Json(r): Json<PathReq>) -> Response {
    let p = PathBuf::from(shellexpand(&r.path));
    match app.open(&p) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

fn shellexpand(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        return ide::home().join(rest).display().to_string();
    }
    p.to_string()
}

async fn new_project(State(app): S, Json(r): Json<PathReq>) -> Response {
    let dir = PathBuf::from(shellexpand(&r.path));
    match crate::project::create(&dir, r.template.as_deref().unwrap_or("blink")) {
        Ok(p) => match app.open(&p) {
            Ok(()) => Json(json!({"ok": true, "path": p.display().to_string()})).into_response(),
            Err(e) => err(StatusCode::BAD_REQUEST, e),
        },
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

async fn reload_project(State(app): S) -> Response {
    let path = app.project.lock().unwrap().base_dir.join(crate::project::PROJECT_FILE);
    if path.exists() {
        match ProjectConfig::load(&path) {
            Ok(c) => *app.project.lock().unwrap() = c,
            Err(e) => return err(StatusCode::BAD_REQUEST, e),
        }
    }
    app.reload_sim();
    Json(json!({"ok": true})).into_response()
}

// ------------------------------------------------------------------ simulation
async fn post_input(State(app): S, Json(v): Json<Value>) -> Response {
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let action = v.get("action").cloned().unwrap_or(Value::Null);
    let r = tokio::task::spawn_blocking(move || app.with_sim(|s| s.input(&id, action)).unwrap_or(Err("no simulation".into()))).await.unwrap();
    match r {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

async fn control(State(app): S, Json(v): Json<Value>) -> Response {
    let cmd = v.get("cmd").and_then(|x| x.as_str()).unwrap_or("");
    let c = match cmd {
        "pause" => Cmd::Pause,
        "resume" | "run" => Cmd::Resume,
        "reset" => Cmd::Reset,
        "erase-flash" => Cmd::EraseFlashData,
        "speed" => Cmd::Speed(v.get("value").and_then(|x| x.as_f64()).unwrap_or(1.0)),
        "serial" => Cmd::Serial(v.get("data").and_then(|x| x.as_str()).unwrap_or("").as_bytes().to_vec()),
        "start" => {
            app.restart_sim();
            return Json(json!({"ok": true})).into_response();
        }
        _ => return err(StatusCode::BAD_REQUEST, "unknown command"),
    };
    app.with_sim(|s| s.send(c));
    Json(json!({"ok": true})).into_response()
}

async fn framebuffer(State(app): S, AxPath(id): AxPath<String>) -> Response {
    let r = tokio::task::spawn_blocking(move || {
        app.with_sim(|s| {
            let (tx, rx) = std::sync::mpsc::channel();
            s.send(Cmd::Framebuffer { id, reply: tx });
            rx.recv_timeout(Duration::from_secs(2)).ok().flatten()
        })
        .flatten()
    })
    .await
    .unwrap();
    match r {
        Some((w, h, rgb)) => {
            let mut body = Vec::with_capacity(8 + rgb.len());
            body.extend_from_slice(&(w as u32).to_le_bytes());
            body.extend_from_slice(&(h as u32).to_le_bytes());
            body.extend_from_slice(&rgb);
            ([(header::CONTENT_TYPE, "application/octet-stream")], body).into_response()
        }
        None => err(StatusCode::NOT_FOUND, "no framebuffer"),
    }
}

async fn ws_handler(State(app): S, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| ws_session(app, socket))
}

async fn ws_session(app: Arc<App>, mut socket: WebSocket) {
    let mut serial_pos = 0u64;
    let mut msg_seq = 0u64;
    let mut last_version = u64::MAX;
    let mut build_seq = 0u64;
    // start from the tail of the serial log
    if let Some(total) = app.with_sim(|s| s.shared.lock().unwrap().serial.base) {
        serial_pos = total;
    }
    let mut sim_ptr: usize = 0;
    loop {
        // current simulator (it may be replaced when a project is opened)
        let cur = app.sim.lock().unwrap().as_ref().map(|s| Arc::as_ptr(&s.shared) as usize).unwrap_or(0);
        if cur != sim_ptr {
            sim_ptr = cur;
            serial_pos = 0;
            msg_seq = 0;
            last_version = u64::MAX;
            let _ = socket.send(Message::Text(json!({"type": "reset"}).to_string().into())).await;
        }
        let update = app.with_sim(|s| {
            let sh = s.shared.lock().unwrap();
            let mut out = serde_json::Map::new();
            if sh.version != last_version {
                out.insert("status".into(), sh.status.clone());
                out.insert("board".into(), sh.board.clone());
                last_version = sh.version;
            }
            let (bytes, total) = sh.serial.since(serial_pos);
            if !bytes.is_empty() {
                out.insert("serial".into(), json!(base64::engine::general_purpose::STANDARD.encode(bytes)));
            }
            serial_pos = total;
            let msgs: Vec<Value> = sh.messages.iter().filter(|(q, _)| *q > msg_seq).map(|(_, m)| json!(m)).collect();
            if let Some((q, _)) = sh.messages.last() {
                msg_seq = msg_seq.max(*q);
            }
            if !msgs.is_empty() {
                out.insert("messages".into(), json!(msgs));
            }
            out
        });
        let mut out = update.unwrap_or_default();
        {
            let b = app.build.lock().unwrap();
            if b.seq != build_seq {
                let new_lines: Vec<&String> = b.lines.iter().skip((build_seq as usize).min(b.lines.len())).collect();
                out.insert("build".into(), json!({"lines": new_lines, "running": b.running, "ok": b.ok, "diagnostics": b.diagnostics}));
                build_seq = b.seq;
            }
        }
        if !out.is_empty() {
            out.insert("type".into(), json!("update"));
            if socket.send(Message::Text(Value::Object(out).to_string().into())).await.is_err() {
                return;
            }
        }
        // incoming commands
        match tokio::time::timeout(Duration::from_millis(50), socket.recv()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    handle_ws_cmd(&app, v);
                }
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return,
            _ => {}
        }
    }
}

fn handle_ws_cmd(app: &Arc<App>, v: Value) {
    match v.get("type").and_then(|x| x.as_str()).unwrap_or("") {
        "serial" => {
            let data = v.get("data").and_then(|x| x.as_str()).unwrap_or("").as_bytes().to_vec();
            app.with_sim(|s| s.send(Cmd::Serial(data)));
        }
        "input" => {
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let action = v.get("action").cloned().unwrap_or(Value::Null);
            app.with_sim(|s| s.send(Cmd::Input { id, action, reply: None }));
        }
        "control" => {
            let c = match v.get("cmd").and_then(|x| x.as_str()).unwrap_or("") {
                "pause" => Some(Cmd::Pause),
                "resume" => Some(Cmd::Resume),
                "reset" => Some(Cmd::Reset),
                "erase-flash" => Some(Cmd::EraseFlashData),
                "speed" => Some(Cmd::Speed(v.get("value").and_then(|x| x.as_f64()).unwrap_or(1.0))),
                _ => None,
            };
            if let Some(c) = c {
                app.with_sim(|s| s.send(c));
            }
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ files
#[derive(Deserialize)]
struct FileQ {
    path: String,
}

async fn files(State(app): S) -> Json<Value> {
    let root = app.root();
    Json(json!({"root": root.display().to_string(), "tree": ide::list_tree(&root)}))
}

async fn read_file(State(app): S, Query(q): Query<FileQ>) -> Response {
    let p = match ide::safe_path(&app.root(), &q.path) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match std::fs::read(&p) {
        Ok(b) => match String::from_utf8(b) {
            Ok(s) => Json(json!({"path": q.path, "content": s})).into_response(),
            Err(_) => err(StatusCode::UNSUPPORTED_MEDIA_TYPE, "binary file"),
        },
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

#[derive(Deserialize)]
struct WriteReq {
    path: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    dir: bool,
}

async fn write_file(State(app): S, Json(r): Json<WriteReq>) -> Response {
    let root = app.root();
    let p = match ide::safe_path(&root, &r.path) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let res = if r.dir {
        std::fs::create_dir_all(&p)
    } else {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&p, r.content.as_bytes())
    };
    // saving the project file reloads the simulation
    if res.is_ok() && p.file_name().map(|n| n == crate::project::PROJECT_FILE).unwrap_or(false) {
        if let Ok(c) = ProjectConfig::load(&p) {
            *app.project.lock().unwrap() = c;
            app.reload_sim();
        }
    }
    match res {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn delete_file(State(app): S, Query(q): Query<FileQ>) -> Response {
    let p = match ide::safe_path(&app.root(), &q.path) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    match r {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
struct RenameReq {
    from: String,
    to: String,
}

async fn rename_file(State(app): S, Json(r): Json<RenameReq>) -> Response {
    let root = app.root();
    let (a, b) = match (ide::safe_path(&root, &r.from), ide::safe_path(&root, &r.to)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return err(StatusCode::BAD_REQUEST, "bad path"),
    };
    match std::fs::rename(&a, &b) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

// ------------------------------------------------------------------ build
async fn start_build(State(app): S) -> Response {
    {
        let mut b = app.build.lock().unwrap();
        if b.running {
            return err(StatusCode::CONFLICT, "a build is already running");
        }
        *b = BuildState { running: true, lines: vec![], seq: 1, ok: None, diagnostics: vec![] };
    }
    let root = app.root();
    let settings = app.settings.lock().unwrap().clone();
    let (tool, args) = match ide::build_command(&root, &settings) {
        Ok(x) => x,
        Err(e) => {
            let mut b = app.build.lock().unwrap();
            b.lines.push(format!("error: {}", e));
            b.running = false;
            b.ok = Some(false);
            b.seq += 1;
            return err(StatusCode::BAD_REQUEST, e);
        }
    };
    let app2 = app.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let (exe, is_pio) = match &tool {
            ide::BuildTool::Pio(p) => (p.clone(), true),
            ide::BuildTool::ArduinoCli(p) => (p.clone(), false),
        };
        let push = |app: &Arc<App>, l: String| {
            let mut b = app.build.lock().unwrap();
            if let Some(d) = ide::parse_diagnostic(&l, &root) {
                b.diagnostics.push(d);
            }
            b.lines.push(l);
            b.seq += 1;
        };
        push(&app2, format!("$ {} {}", exe, args.join(" ")));
        let child = tokio::process::Command::new(&exe)
            .args(&args)
            .current_dir(&root)
            .env("PLATFORMIO_FORCE_ANSI", "false")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                push(&app2, format!("error: cannot start {}: {}", exe, e));
                let mut b = app2.build.lock().unwrap();
                b.running = false;
                b.ok = Some(false);
                b.seq += 1;
                return;
            }
        };
        let out = child.stdout.take().unwrap();
        let errp = child.stderr.take().unwrap();
        let a1 = app2.clone();
        let root1 = root.clone();
        let t1 = tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut b = a1.build.lock().unwrap();
                if let Some(d) = ide::parse_diagnostic(&l, &root1) {
                    b.diagnostics.push(d);
                }
                b.lines.push(l);
                b.seq += 1;
            }
        });
        let a2 = app2.clone();
        let root2 = root.clone();
        let t2 = tokio::spawn(async move {
            let mut lines = BufReader::new(errp).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut b = a2.build.lock().unwrap();
                if let Some(d) = ide::parse_diagnostic(&l, &root2) {
                    b.diagnostics.push(d);
                }
                b.lines.push(l);
                b.seq += 1;
            }
        });
        let status = child.wait().await;
        let _ = t1.await;
        let _ = t2.await;
        let ok = status.map(|s| s.success()).unwrap_or(false);
        if ok && !is_pio {
            if let Some((bin, elf)) = ide::arduino_cli_outputs(&root) {
                let mut p = app2.project.lock().unwrap();
                p.firmware = Some(bin.display().to_string());
                p.elf = elf.map(|e| e.display().to_string());
            }
        }
        if ok && is_pio {
            // compile database for clangd (best effort)
            let _ = tokio::process::Command::new(&exe).args(["run", "-t", "compiledb"]).current_dir(&root).output().await;
        }
        {
            let mut b = app2.build.lock().unwrap();
            b.lines.push(if ok { "✔ build succeeded — loading firmware into the simulator".into() } else { "✘ build failed".into() });
            b.running = false;
            b.ok = Some(ok);
            b.seq += 1;
        }
        if ok {
            app2.reload_sim();
        }
    });
    Json(json!({"ok": true})).into_response()
}

async fn build_status(State(app): S) -> Json<Value> {
    let b = app.build.lock().unwrap();
    Json(json!({"running": b.running, "ok": b.ok, "lines": b.lines, "diagnostics": b.diagnostics}))
}

async fn convert_sketch(State(app): S) -> Response {
    match ide::convert_sketch_to_pio(&app.root()) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

// ------------------------------------------------------------------ git
async fn git_status(State(app): S) -> Json<Value> {
    let root = app.root();
    Json(tokio::task::spawn_blocking(move || ide::git_status(&root)).await.unwrap())
}
async fn git_log(State(app): S) -> Json<Value> {
    let root = app.root();
    Json(tokio::task::spawn_blocking(move || ide::git_log(&root, 50)).await.unwrap())
}
async fn git_diff(State(app): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let root = app.root();
    let mut args = vec!["diff".to_string()];
    if q.get("staged").map(|s| s == "true").unwrap_or(false) {
        args.push("--cached".into());
    }
    if let Some(p) = q.get("path") {
        args.push("--".into());
        args.push(p.clone());
    }
    let out = tokio::task::spawn_blocking(move || ide::git(&root, &args.iter().map(|s| s.as_str()).collect::<Vec<_>>())).await.unwrap();
    Json(json!({"diff": out.unwrap_or_else(|e| e)}))
}
async fn git_show(State(app): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    // content of a file at HEAD (for the diff editor)
    let root = app.root();
    let path = q.get("path").cloned().unwrap_or_default();
    let rev = q.get("rev").cloned().unwrap_or_else(|| "HEAD".into());
    let out = tokio::task::spawn_blocking(move || ide::git(&root, &["show", &format!("{}:{}", rev, path)])).await.unwrap();
    Json(json!({"content": out.ok()}))
}
async fn git_action(State(app): S, Json(v): Json<Value>) -> Response {
    let root = app.root();
    let action = v.get("action").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let paths: Vec<String> = v.get("paths").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|p| p.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let message = v.get("message").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let r = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let g = |args: Vec<&str>| ide::git(&root, &args);
        match action.as_str() {
            "init" => {
                let r = g(vec!["init"])?;
                if !root.join(".gitignore").exists() {
                    let _ = std::fs::write(root.join(".gitignore"), ".pio/\n.esp32sim/\nbuild/\n");
                }
                Ok(r)
            }
            "stage" => g([vec!["add", "--"], paths.iter().map(|s| s.as_str()).collect()].concat()),
            "stage_all" => g(vec!["add", "-A"]),
            "unstage" => g([vec!["reset", "-q", "HEAD", "--"], paths.iter().map(|s| s.as_str()).collect()].concat()),
            "discard" => g([vec!["checkout", "--"], paths.iter().map(|s| s.as_str()).collect()].concat()),
            "commit" => {
                if message.trim().is_empty() {
                    return Err("commit message is empty".into());
                }
                let r = g(vec!["commit", "-m", &message]);
                match r {
                    Err(e) if e.contains("user.email") || e.contains("Please tell me who you are") => {
                        ide::git(&root, &["-c", "user.name=esp32sim user", "-c", "user.email=user@esp32sim.local", "commit", "-m", &message])
                    }
                    other => other,
                }
            }
            "checkout" => g(vec!["checkout", &name]),
            "branch" => g(vec!["checkout", "-b", &name]),
            "pull" => g(vec!["pull", "--ff-only"]),
            "push" => g(vec!["push"]),
            "fetch" => g(vec!["fetch", "--all"]),
            "remote_add" => g(vec!["remote", "add", "origin", &name]),
            _ => Err(format!("unknown git action '{}'", action)),
        }
    })
    .await
    .unwrap();
    match r {
        Ok(out) => Json(json!({"ok": true, "output": out})).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

// ------------------------------------------------------------------ settings & AI
async fn get_settings(State(app): S) -> Json<Value> {
    Json(app.settings.lock().unwrap().public())
}

async fn put_settings(State(app): S, Json(v): Json<Value>) -> Response {
    let mut guard = app.settings.lock().unwrap();
    let s: &mut Settings = &mut guard;
    if let Some(ai) = v.get("ai") {
        for (k, field) in [("provider", &mut s.ai.provider), ("model", &mut s.ai.model), ("base_url", &mut s.ai.base_url)] {
            if let Some(x) = ai.get(k).and_then(|x| x.as_str()) {
                *field = x.to_string();
            }
        }
        if let Some(t) = ai.get("temperature").and_then(|x| x.as_f64()) {
            s.ai.temperature = Some(t);
        }
        if let Some(b) = ai.get("inline").and_then(|x| x.as_bool()) {
            s.ai.inline = b;
        }
        // {"keys": {"openai": "sk-..."}} — empty string removes a key
        if let Some(keys) = ai.get("keys").and_then(|x| x.as_object()) {
            for (k, val) in keys {
                if let Some(x) = val.as_str() {
                    if x.is_empty() {
                        s.ai.keys.remove(k);
                    } else {
                        s.ai.keys.insert(k.clone(), x.to_string());
                    }
                }
            }
        }
    }
    for (k, field) in [("pio", &mut s.pio), ("arduino_cli", &mut s.arduino_cli), ("clangd", &mut s.clangd)] {
        if let Some(x) = v.get(k).and_then(|x| x.as_str()) {
            *field = if x.is_empty() { None } else { Some(x.to_string()) };
        }
    }
    if let Some(e) = v.get("editor") {
        s.editor = e.clone();
    }
    match s.save() {
        Ok(()) => Json(s.public()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn ai_models(State(app): S) -> Response {
    let s = app.settings.lock().unwrap().ai.clone();
    match crate::ai::list_models(&s).await {
        Ok(m) => Json(json!({"models": m})).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

/// Inline completion: the code to insert at the cursor (no explanations).
async fn ai_complete(State(app): S, Json(v): Json<Value>) -> Response {
    let mut s = app.settings.lock().unwrap().ai.clone();
    s.temperature = Some(0.1);
    let prefix = v.get("prefix").and_then(|x| x.as_str()).unwrap_or("");
    let suffix = v.get("suffix").and_then(|x| x.as_str()).unwrap_or("");
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
    // keep the request small: the last ~3000 chars before and ~1000 after the cursor
    let pre: String = prefix.chars().rev().take(3000).collect::<Vec<_>>().into_iter().rev().collect();
    let suf: String = suffix.chars().take(1000).collect();
    let system = "You are a code completion engine for Arduino / ESP32 C++ firmware in the esp32sim IDE. \
Reply with ONLY the text to insert at <CURSOR> (no explanations, no markdown fences). \
Complete the current statement or the next few lines at most; match the existing style. If nothing fits, reply with nothing.";
    let user = format!("File: {}\n```\n{}<CURSOR>{}\n```", path, pre, suf);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
    let msgs = vec![json!({"role": "user", "content": user})];
    let job = tokio::spawn(async move { crate::ai::chat(&s, system, msgs, tx).await });
    let mut out = String::new();
    while let Some(t) = rx.recv().await {
        out.push_str(&t);
        if out.len() > 2000 {
            break;
        }
    }
    let r = job.await.ok().unwrap_or(Ok(()));
    if let Err(e) = r {
        return err(StatusCode::BAD_GATEWAY, e);
    }
    // strip accidental fences
    let mut t = out.trim_end().to_string();
    if t.starts_with("```") {
        t = t.lines().skip(1).filter(|l| !l.starts_with("```")).collect::<Vec<_>>().join("\n");
    }
    Json(json!({"text": t})).into_response()
}

async fn ai_chat(State(app): S, Json(v): Json<Value>) -> Response {
    let s = app.settings.lock().unwrap().ai.clone();
    let messages: Vec<Value> = v.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    // project context for the assistant
    let mut system = crate::ai::SYSTEM_PROMPT.to_string();
    let ctx = v.get("context").cloned().unwrap_or(Value::Null);
    let project_yaml = app.project.lock().unwrap().to_yaml();
    system.push_str(&format!("\n\n## Circuit (esp32sim.yaml)\n```yaml\n{}\n```", project_yaml));
    if let Some(f) = ctx.get("file").and_then(|x| x.as_str()) {
        let content = ctx.get("content").and_then(|x| x.as_str()).unwrap_or("");
        system.push_str(&format!("\n\n## Current file: {}\n```cpp\n{}\n```", f, content.chars().take(60_000).collect::<String>()));
    }
    if let Some(sel) = ctx.get("selection").and_then(|x| x.as_str()) {
        if !sel.is_empty() {
            system.push_str(&format!("\n\n## Selected code\n```cpp\n{}\n```", sel));
        }
    }
    {
        let b = app.build.lock().unwrap();
        if b.ok == Some(false) {
            let errs: Vec<String> = b.diagnostics.iter().filter(|d| d.severity == "error").take(20).map(|d| format!("{}:{}:{}: {}", d.file, d.line, d.col, d.message)).collect();
            if !errs.is_empty() {
                system.push_str(&format!("\n\n## Last build errors\n{}", errs.join("\n")));
            }
        }
    }
    if let Some(serial) = app.with_sim(|sh| {
        let s = sh.shared.lock().unwrap();
        let d = &s.serial.data;
        String::from_utf8_lossy(&d[d.len().saturating_sub(3000)..]).to_string()
    }) {
        if !serial.is_empty() {
            system.push_str(&format!("\n\n## Serial monitor (tail)\n```\n{}\n```", serial));
        }
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
    let (btx, brx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(64);
    tokio::spawn(async move {
        let s2 = s.clone();
        let fwd = tokio::spawn(async move {
            while let Some(t) = rx.recv().await {
                if btx.send(Ok(t)).await.is_err() {
                    break;
                }
            }
            btx
        });
        let r = crate::ai::chat(&s2, &system, messages, tx).await;
        let btx = fwd.await.ok();
        if let (Err(e), Some(btx)) = (r, btx) {
            let _ = btx.send(Ok(format!("\n\n**Error:** {}", e))).await;
        }
    });
    let stream = tokio_stream_from(brx);
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], Body::from_stream(stream)).into_response()
}

fn tokio_stream_from(mut rx: tokio::sync::mpsc::Receiver<Result<String, std::io::Error>>) -> impl futures::Stream<Item = Result<String, std::io::Error>> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

// ------------------------------------------------------------------ debugger
async fn debug_files(State(app): S) -> Json<Value> {
    let d = app.with_sim(|s| s.shared.lock().unwrap().debug.clone()).flatten();
    Json(json!({"available": d.is_some(), "files": d.map(|d| d.files().to_vec()).unwrap_or_default()}))
}

async fn set_breakpoints(State(app): S, Json(v): Json<Value>) -> Json<Value> {
    // {"breakpoints": [{"file": "src/main.cpp", "line": 12}, {"address": "0x400d1234"}]}
    let specs = v.get("breakpoints").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let root = app.root();
    let r = tokio::task::spawn_blocking(move || {
        app.with_sim(|s| {
            let (tx, rx) = std::sync::mpsc::channel();
            s.send(Cmd::SetBreakpointSpecs { specs: specs.clone(), root, reply: Some(tx) });
            rx.recv_timeout(Duration::from_secs(5)).ok()
        })
        .flatten()
        .unwrap_or_else(|| json!(specs.iter().map(|b| json!({"file": b["file"], "line": b["line"], "verified": false})).collect::<Vec<_>>()))
    })
    .await
    .unwrap_or(Value::Null);
    Json(json!({"breakpoints": r}))
}

async fn debug_regs(State(app): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let core: usize = q.get("core").and_then(|c| c.parse().ok()).unwrap_or(1);
    let v = tokio::task::spawn_blocking(move || {
        app.with_sim(|s| {
            let (tx, rx) = std::sync::mpsc::channel();
            s.send(Cmd::Regs { core, reply: tx });
            rx.recv_timeout(Duration::from_secs(3)).unwrap_or(Value::Null)
        })
        .unwrap_or(Value::Null)
    })
    .await
    .unwrap();
    Json(v)
}

async fn debug_step(State(app): S, Json(v): Json<Value>) -> Json<Value> {
    let core = v.get("core").and_then(|x| x.as_u64()).unwrap_or(1) as usize;
    let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("instruction").to_string();
    let count = v.get("count").and_then(|x| x.as_u64()).unwrap_or(1);
    let r = tokio::task::spawn_blocking(move || {
        app.with_sim(|s| {
            let (tx, rx) = std::sync::mpsc::channel();
            match kind.as_str() {
                "into" => s.send(Cmd::StepLine { core, over: false, reply: tx }),
                "over" => s.send(Cmd::StepLine { core, over: true, reply: tx }),
                _ => s.send(Cmd::Step { core, count, reply: tx }),
            }
            rx.recv_timeout(Duration::from_secs(30)).unwrap_or(Value::Null)
        })
        .unwrap_or(Value::Null)
    })
    .await
    .unwrap();
    Json(r)
}

async fn debug_mem(State(app): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let addr = q.get("addr").and_then(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
    let len: u32 = q.get("len").and_then(|l| l.parse().ok()).unwrap_or(256);
    let bytes = tokio::task::spawn_blocking(move || {
        app.with_sim(|s| {
            let (tx, rx) = std::sync::mpsc::channel();
            s.send(Cmd::ReadMem { addr, len, reply: tx });
            rx.recv_timeout(Duration::from_secs(3)).unwrap_or_default()
        })
        .unwrap_or_default()
    })
    .await
    .unwrap();
    Json(json!({"addr": format!("0x{:08x}", addr), "hex": bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>()}))
}

/// Bus monitor: I2C / SPI / UART / RMT transactions since sequence number `since`.
async fn bus_log(State(app): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let since: u64 = q.get("since").and_then(|v| v.parse().ok()).unwrap_or(0);
    let v = tokio::task::spawn_blocking(move || {
        app.with_sim(|s| {
            s.exec(move |m, b| {
                let (ev, next) = b.buslog.since(since, 2000);
                json!({"now": m.now_ns(), "next": next, "events": ev})
            })
        })
        .unwrap_or(Value::Null)
    })
    .await
    .unwrap_or(Value::Null);
    Json(v)
}

/// Logic analyzer: pad level changes since sequence number `since`.
async fn pad_trace(State(app): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let since: u64 = q.get("since").and_then(|v| v.parse().ok()).unwrap_or(0);
    let v = tokio::task::spawn_blocking(move || {
        app.with_sim(|s| {
            s.exec(move |m, b| {
                let (ev, next) = b.trace.since(since, 50_000);
                let events: Vec<Value> = ev.iter().map(|(t, p, l)| json!([t, p, l.map(|x| x as u8)])).collect();
                let pads: Vec<u8> = (0..40u8).filter(|&g| b.pad_net[g as usize].is_some()).collect();
                json!({"now": m.now_ns(), "next": next, "events": events, "pads": pads,
                       "levels": b.trace.levels().iter().map(|l| l.map(|x| x as u8)).collect::<Vec<_>>()})
            })
        })
        .unwrap_or(Value::Null)
    })
    .await
    .unwrap_or(Value::Null);
    Json(v)
}

async fn debug_symbol(State(app): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    // read a global variable by symbol name: value as u32/i32/f32 plus raw bytes
    let name = q.get("name").cloned().unwrap_or_default();
    let v = tokio::task::spawn_blocking(move || {
        app.with_sim(|s| {
            s.exec(move |m, _b| {
                let addr = m.symbols.by_name.get(&name).copied();
                match addr {
                    Some(a) => {
                        let bytes = m.sys.read_bytes(a, 8);
                        let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                        json!({"name": name, "address": format!("0x{:08x}", a), "u32": w, "i32": w as i32, "f32": f32::from_bits(w), "bytes": bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")})
                    }
                    None => json!({"name": name, "error": "symbol not found"}),
                }
            })
        })
        .unwrap_or(Value::Null)
    })
    .await
    .unwrap();
    Json(v)
}

async fn debug_lookup(State(app): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let addr = q.get("addr").and_then(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
    let d = app.with_sim(|s| s.shared.lock().unwrap().debug.clone()).flatten();
    let loc = d.and_then(|d| d.location(addr));
    Json(json!({"location": loc.map(|l| json!({"file": l.file, "line": l.line, "function": l.function}))}))
}

// ------------------------------------------------------------------ LSP bridge (clangd)
async fn lsp_handler(State(app): S, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| lsp_session(app, socket))
}

async fn lsp_session(app: Arc<App>, mut socket: WebSocket) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
    let settings = app.settings.lock().unwrap().clone();
    let Some(clangd) = ide::find_tool("clangd", settings.clangd.as_deref()) else {
        let _ = socket.send(Message::Text(json!({"esp32sim": "no-clangd"}).to_string().into())).await;
        return;
    };
    let root = app.root();
    let cdb = ide::lsp_compile_dir(&root);
    let child = tokio::process::Command::new(&clangd)
        .args(["--background-index", "--clang-tidy=false", "--header-insertion=never", "--log=error"])
        .arg(format!("--compile-commands-dir={}", cdb.display()))
        .current_dir(&root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let Ok(mut child) = child else { return };
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let (mut ws_tx, mut ws_rx) = futures::StreamExt::split(socket);
    let reader = tokio::spawn(async move {
        use futures::SinkExt;
        loop {
            // read headers
            let mut header = Vec::new();
            let mut len: usize = 0;
            loop {
                let mut b = [0u8; 1];
                if stdout.read_exact(&mut b).await.is_err() {
                    return;
                }
                header.push(b[0]);
                if header.ends_with(b"\r\n\r\n") {
                    let h = String::from_utf8_lossy(&header).to_string();
                    for line in h.lines() {
                        if let Some(v) = line.strip_prefix("Content-Length:") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                    }
                    break;
                }
            }
            let mut body = vec![0u8; len];
            if stdout.read_exact(&mut body).await.is_err() {
                return;
            }
            if ws_tx.send(Message::Text(String::from_utf8_lossy(&body).to_string().into())).await.is_err() {
                return;
            }
        }
    });
    while let Some(Ok(msg)) = futures::StreamExt::next(&mut ws_rx).await {
        if let Message::Text(t) = msg {
            let frame = format!("Content-Length: {}\r\n\r\n{}", t.len(), t);
            if stdin.write_all(frame.as_bytes()).await.is_err() {
                break;
            }
        }
    }
    reader.abort();
    let _ = child.kill().await;
}

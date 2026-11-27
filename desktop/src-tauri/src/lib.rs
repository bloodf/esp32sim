//! esp32sim desktop app.
//!
//! The emulator, circuit editor and IDE run in-process (the same server the
//! `esp32sim gui` command starts, on a free loopback port) and are shown in
//! a native window.  The desktop shell adds native menus and file dialogs:
//! the web UI asks for them through `/api/native/*`, which calls the
//! `NativeHooks` implemented here.
//!
//! Usage: `esp32sim-desktop [project folder | esp32sim.yaml | firmware.bin]`
//!
//! Everything is generic over the Tauri runtime so the shell can be tested
//! on Tauri's mock runtime (`cargo test --no-default-features --features mock`).

use esp32sim::server::{self, NativeHooks, ServerHandle, ServerOptions};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Manager, RunEvent, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, FilePath, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

/// Label of the main window.
pub const MAIN: &str = "main";

/// Native dialogs for the web UI (called from the server's worker threads,
/// so the blocking dialog variants are fine here).
pub struct DesktopHooks<R: Runtime> {
    app: AppHandle<R>,
}

fn to_path(p: Option<FilePath>) -> Option<PathBuf> {
    p.and_then(|f| f.into_path().ok())
}

impl<R: Runtime> NativeHooks for DesktopHooks<R> {
    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        to_path(self.app.dialog().file().set_title(title).blocking_pick_folder())
    }
    fn pick_file(&self, title: &str, extensions: &[&str]) -> Option<PathBuf> {
        let mut d = self.app.dialog().file().set_title(title);
        if !extensions.is_empty() {
            d = d.add_filter("Files", extensions);
        }
        to_path(d.blocking_pick_file())
    }
    fn shell(&self) -> &str {
        "tauri"
    }
}

/// The running engine, shared with the menu handlers.
pub struct Engine(pub Mutex<Option<ServerHandle>>);

impl Engine {
    pub fn with<T>(&self, f: impl FnOnce(&ServerHandle) -> T) -> Option<T> {
        self.0.lock().unwrap().as_ref().map(f)
    }
}

pub fn main_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(MAIN)
}

pub fn project_title<R: Runtime>(app: &AppHandle<R>) -> String {
    let name = app.state::<Engine>().with(|h| h.app.project.lock().unwrap().name.clone()).unwrap_or_default();
    if name.is_empty() {
        "esp32sim".into()
    } else {
        format!("{} — esp32sim", name)
    }
}

/// Open a project (folder, esp32sim.yaml or firmware image) and refresh the UI.
pub fn open_target<R: Runtime>(app: &AppHandle<R>, path: PathBuf) {
    let r = app.state::<Engine>().with(|h| h.open_project(&path)).unwrap_or(Err("engine not running".into()));
    match r {
        Ok(()) => {
            if let Some(w) = main_window(app) {
                let _ = w.eval("location.reload()");
                let _ = w.set_title(&project_title(app));
            }
        }
        Err(e) => {
            app.dialog().message(format!("Could not open {}:\n{}", path.display(), e)).title("esp32sim").kind(MessageDialogKind::Error).show(|_| {});
        }
    }
}

/// Run a GUI action implemented by the web UI (dialogs it already has).
fn ui_action<R: Runtime>(app: &AppHandle<R>, js: &str) {
    if let Some(w) = main_window(app) {
        let _ = w.eval(js);
    }
}

/// Current zoom factor of the main window.
pub struct Zoom(pub Mutex<f64>);

fn zoom<R: Runtime>(app: &AppHandle<R>, factor: Option<f64>) {
    let st = app.state::<Zoom>();
    let mut z = st.0.lock().unwrap();
    *z = match factor {
        Some(f) => (*z * f).clamp(0.5, 2.5),
        None => 1.0,
    };
    if let Some(w) = main_window(app) {
        let _ = w.set_zoom(*z);
    }
}

/// Menu entry ids (also used by the tests).
pub const MENU_IDS: &[&str] = &[
    "new", "open", "open-firmware", "save-all", "settings", "build", "resume", "pause", "reset", "reload", "erase-flash", "zoom-in", "zoom-out", "zoom-reset", "reload-ui", "browser", "about",
];

pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let item = |id: &str, label: &str, accel: Option<&str>| {
        let b = MenuItemBuilder::with_id(id, label);
        match accel {
            Some(a) => b.accelerator(a).build(app),
            None => b.build(app),
        }
    };
    let file = SubmenuBuilder::new(app, "File")
        .item(&item("new", "New Project…", Some("CmdOrCtrl+N"))?)
        .item(&item("open", "Open Project Folder…", Some("CmdOrCtrl+O"))?)
        .item(&item("open-firmware", "Open Firmware (.bin / .elf)…", Some("CmdOrCtrl+Shift+O"))?)
        .separator()
        .item(&item("save-all", "Save All", Some("CmdOrCtrl+Shift+S"))?)
        .item(&item("settings", "Settings…", Some("CmdOrCtrl+,"))?)
        .separator()
        .item(&PredefinedMenuItem::quit(app, Some("Quit esp32sim"))?)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit").undo().redo().separator().cut().copy().paste().select_all().build()?;
    let sim = SubmenuBuilder::new(app, "Simulation")
        .item(&item("build", "Build && Run", Some("CmdOrCtrl+B"))?)
        .item(&item("resume", "Run / Continue", Some("F5"))?)
        .item(&item("pause", "Pause", Some("F6"))?)
        .item(&item("reset", "Reset Board (EN)", None)?)
        .item(&item("reload", "Reload Simulation", Some("CmdOrCtrl+R"))?)
        .separator()
        .item(&item("erase-flash", "Erase Flash Data…", None)?)
        .build()?;
    let mut view = SubmenuBuilder::new(app, "View")
        .item(&item("zoom-in", "Zoom In", Some("CmdOrCtrl+="))?)
        .item(&item("zoom-out", "Zoom Out", Some("CmdOrCtrl+-"))?)
        .item(&item("zoom-reset", "Actual Size", Some("CmdOrCtrl+0"))?)
        .separator()
        .item(&item("reload-ui", "Reload Interface", Some("CmdOrCtrl+Shift+R"))?)
        .item(&item("browser", "Open in Web Browser", None)?);
    if cfg!(any(debug_assertions, feature = "devtools")) {
        view = view.item(&item("devtools", "Developer Tools", Some("CmdOrCtrl+Alt+I"))?);
    }
    let view = view.build()?;
    let help = SubmenuBuilder::new(app, "Help").item(&item("about", "About esp32sim", None)?).build()?;
    let menu = MenuBuilder::new(app).items(&[&file, &edit, &sim, &view, &help]).build()?;
    app.set_menu(menu)?;
    Ok(())
}

pub fn on_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        "open" => {
            let a = app.clone();
            app.dialog().file().set_title("Open project folder").pick_folder(move |p| {
                if let Some(p) = to_path(p) {
                    open_target(&a, p);
                }
            });
        }
        "open-firmware" => {
            let a = app.clone();
            app.dialog().file().set_title("Open firmware").add_filter("ESP32 firmware", &["bin", "elf"]).pick_file(move |p| {
                if let Some(p) = to_path(p) {
                    open_target(&a, p);
                }
            });
        }
        "new" => ui_action(app, "document.querySelector('#menu-file [data-act=new]').click()"),
        "save-all" => ui_action(app, "document.querySelector('#menu-file [data-act=\"save-all\"]').click()"),
        "settings" => ui_action(app, "document.querySelector('#activity [data-act=settings]').click()"),
        "build" => ui_action(app, "document.querySelector('#btn-build').click()"),
        "resume" => ui_action(app, "document.querySelector('#btn-run').click()"),
        "pause" => ui_action(app, "document.querySelector('#btn-pause').click()"),
        "reset" => ui_action(app, "document.querySelector('#btn-reset').click()"),
        "erase-flash" => ui_action(app, "document.querySelector('#menu-file [data-act=\"erase-flash\"]').click()"),
        "reload" => {
            app.state::<Engine>().with(|h| h.app.reload_sim());
        }
        "reload-ui" => ui_action(app, "location.reload()"),
        "zoom-in" => zoom(app, Some(1.1)),
        "zoom-out" => zoom(app, Some(1.0 / 1.1)),
        "zoom-reset" => zoom(app, None),
        "browser" => {
            if let Some(url) = app.state::<Engine>().with(|h| h.url.clone()) {
                let _ = app.opener().open_url(url, None::<&str>);
            }
        }
        "devtools" => {
            #[cfg(any(debug_assertions, feature = "devtools"))]
            if let Some(w) = main_window(app) {
                if w.is_devtools_open() {
                    w.close_devtools();
                } else {
                    w.open_devtools();
                }
            }
        }
        "about" => {
            app.dialog()
                .message(format!(
                    "esp32sim {}\n\nESP32 emulator with circuit simulation, displays, a PlatformIO / Arduino IDE, debugger and AI assistant.\n\nEngine: {}",
                    env!("CARGO_PKG_VERSION"),
                    app.state::<Engine>().with(|h| h.url.clone()).unwrap_or_default()
                ))
                .title("About esp32sim")
                .kind(MessageDialogKind::Info)
                .show(|_| {});
        }
        _ => {}
    }
}

/// Start the engine (GUI server with native hooks), the menu and the main window.
pub fn start<R: Runtime>(app: &AppHandle<R>, target: Option<PathBuf>) -> Result<url::Url, Box<dyn std::error::Error>> {
    let srv = server::start(ServerOptions {
        target,
        port: 0,
        host: "127.0.0.1".into(),
        open_browser: false,
        native: Some(Arc::new(DesktopHooks { app: app.clone() })),
        no_recent: false,
    })?;
    let url: url::Url = srv.url.parse()?;
    *app.state::<Engine>().0.lock().unwrap() = Some(srv);
    build_menu(app)?;
    WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(url.clone()))
        .title(project_title(app))
        .inner_size(1440.0, 900.0)
        .min_inner_size(960.0, 600.0)
        .build()?;
    Ok(url)
}

/// The application builder: plugins, state, setup and menu handling.
pub fn builder<R: Runtime>(b: tauri::Builder<R>, target: Option<PathBuf>) -> tauri::Builder<R> {
    b.plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Engine(Mutex::new(None)))
        .manage(Zoom(Mutex::new(1.0)))
        .setup(move |app| {
            start(app.handle(), target.clone())?;
            Ok(())
        })
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
}

/// Event-loop callback: stop the engine when the app exits.
pub fn on_run_event<R: Runtime>(app: &AppHandle<R>, event: RunEvent) {
    if let RunEvent::Exit = event {
        shutdown(app);
    }
}

/// Stop the simulator (saving flash data) and the HTTP server.
pub fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    let h = app.state::<Engine>().0.lock().unwrap().take();
    drop(h);
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    //! The desktop shell on Tauri's mock runtime: the real engine starts, the
    //! window points at it, menus exist and their actions work.
    use super::*;
    use esp32sim::testing::{write_mock_project, TempDir};
    use std::io::{Read, Write};
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    fn http_get(url: &url::Url, path: &str) -> Option<String> {
        let mut s = std::net::TcpStream::connect((url.host_str()?, url.port()?)).ok()?;
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok()?;
        write!(s, "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n", path).ok()?;
        let mut b = String::new();
        s.read_to_string(&mut b).ok()?;
        Some(b)
    }

    fn app_on(project: &std::path::Path) -> tauri::App<MockRuntime> {
        let cfg = std::env::temp_dir().join(format!("esp32sim-desktop-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &cfg);
        let mut app = builder(mock_builder(), Some(project.to_path_buf())).build(mock_context(noop_assets())).expect("build app");
        #[allow(deprecated)] // one iteration is exactly what the test needs: it runs setup
        app.run_iteration(|_, _| {});
        app
    }

    fn engine_url(app: &tauri::App<MockRuntime>) -> url::Url {
        app.state::<Engine>().with(|h| h.url.clone()).expect("engine running").parse().unwrap()
    }

    #[test]
    fn engine_window_and_menus() {
        let dir = TempDir::new("desktop");
        write_mock_project(dir.path()).unwrap();
        let app = app_on(dir.path());
        let h = app.handle().clone();
        // the engine serves the GUI and knows it runs inside the desktop shell
        let url = engine_url(&app);
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        let info = http_get(&url, "/api/info").expect("engine answers");
        assert!(info.contains("\"desktop\":true") && info.contains("\"shell\":\"tauri\""), "{}", info);
        assert!(http_get(&url, "/").unwrap().contains("js/main.js"));
        // one main window, on the engine, titled after the project
        let w = main_window(&h).expect("main window");
        assert_eq!(w.url().unwrap().origin(), url.origin());
        assert_eq!(project_title(&h), "mockfw — esp32sim");
        // every menu entry is present
        let menu = h.menu().expect("app menu");
        for id in MENU_IDS {
            assert!(menu.get(*id).is_some() || menu.items().unwrap().iter().any(|i| i.as_submenu().and_then(|s| s.get(*id)).is_some()), "menu entry {}", id);
        }
        // menu actions that do not open dialogs
        for id in ["reload", "zoom-in", "zoom-in", "zoom-out", "reload-ui", "build", "pause", "resume", "reset", "settings", "new", "unknown-id"] {
            on_menu(&h, id);
        }
        assert!((*h.state::<Zoom>().0.lock().unwrap() - 1.1).abs() < 1e-9);
        on_menu(&h, "zoom-reset");
        assert_eq!(*h.state::<Zoom>().0.lock().unwrap(), 1.0);
        // opening another project switches the engine (and the title)
        let other = TempDir::new("desktop-other");
        write_mock_project(other.path()).unwrap();
        std::fs::write(other.path().join("esp32sim.yaml"), esp32sim::testing::MOCK_PROJECT_YAML.replace("name: mockfw", "name: second")).unwrap();
        open_target(&h, other.path().to_path_buf());
        assert_eq!(project_title(&h), "second — esp32sim");
        assert!(http_get(&url, "/api/project").unwrap().contains("\"name\":\"second\""));
        // a bad path is reported (dialog) without stopping the engine
        open_target(&h, PathBuf::from("/no/such/project"));
        assert!(http_get(&url, "/api/info").is_some());
        // shutting down closes the engine's port
        shutdown(&h);
        assert!(h.state::<Engine>().with(|_| ()).is_none());
        assert!(http_get(&url, "/api/info").is_none(), "port closed");
    }
}

//! The `esp32sim` command line, end to end (the compiled binary, with the
//! mock firmware project in a temp dir and a private config directory).
use esp32sim::testing::{write_mock_project, TempDir};
use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_esp32sim"));
    let home = std::env::temp_dir().join(format!("esp32sim-cli-home-{}", std::process::id()));
    c.env("XDG_CONFIG_HOME", home.join(".config")).env("XDG_CACHE_HOME", home.join(".cache")).env("HOME", &home);
    c
}

fn run(args: &[&str]) -> (i32, String) {
    let o: Output = bin().args(args).output().expect("run esp32sim");
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr))
}

#[test]
fn help_and_version() {
    let (code, out) = run(&["--help"]);
    assert_eq!(code, 0);
    for sub in ["run", "test", "gui", "new", "parts", "info"] {
        assert!(out.contains(sub), "help lists {}: {}", sub, out);
    }
    let (code, out) = run(&["--version"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("esp32sim "), "{}", out);
    let (code, _) = run(&["frobnicate"]);
    assert_eq!(code, 2, "unknown sub-command");
}

#[test]
fn parts_listing_and_details() {
    let (code, out) = run(&["parts"]);
    assert_eq!(code, 0);
    assert!(out.lines().count() >= 85);
    assert!(out.lines().any(|l| l.starts_with("pn532 ")));
    let (code, out) = run(&["parts", "ADS1115"]);
    assert_eq!(code, 0);
    assert!(out.contains("pins:") && out.contains("A0") && out.contains("address: \"0x48\""), "{}", out);
    assert!(out.contains("- {id: ads1115_1, type: ads1115}"));
    let (code, out) = run(&["parts", "flux-capacitor"]);
    assert_eq!(code, 2);
    assert!(out.contains("unknown part type"));
}

#[test]
fn new_project_from_every_template() {
    let d = TempDir::new("cli-new");
    for t in ["blink", "button", "oled", "lcd", "keypad", "neopixel", "sensors", "nfc", "rfid", "sdlogger", "gps"] {
        let dir = d.path().join(t);
        let (code, out) = run(&["new", dir.to_str().unwrap(), "--template", t]);
        assert_eq!(code, 0, "{}: {}", t, out);
        for f in ["platformio.ini", "esp32sim.yaml", "src/main.cpp"] {
            assert!(dir.join(f).exists(), "{} has {}", t, f);
        }
        let y = std::fs::read_to_string(dir.join("esp32sim.yaml")).unwrap();
        let cfg = devices::ProjectConfig::parse(&y).unwrap_or_else(|e| panic!("{}: {}", t, e));
        cfg.build_board().unwrap_or_else(|e| panic!("{} wiring: {}", t, e));
    }
    let (code, out) = run(&["new", d.path().join("x").to_str().unwrap(), "--template", "nope"]);
    assert_eq!(code, 2, "{}", out);
    // refuses to overwrite an existing project
    let (code, _) = run(&["new", d.path().join("blink").to_str().unwrap()]);
    assert_eq!(code, 2);
}

#[test]
fn info_describes_firmware_images() {
    let d = TempDir::new("cli-info");
    write_mock_project(d.path()).unwrap();
    let (code, out) = run(&["info", d.path().join("mockfw.bin").to_str().unwrap()]);
    assert_eq!(code, 0, "{}", out);
    assert!(out.contains("format: AppImage") && out.contains("entry: 0x40080400") && out.contains("app0"), "{}", out);
    let (code, out) = run(&["info", d.path().join("esp32sim.yaml").to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(out.contains("unrecognised firmware format"), "{}", out);
    let (code, _) = run(&["info", "/no/such/file.bin"]);
    assert_eq!(code, 2);
}

#[test]
fn scenario_runner_pass_and_fail() {
    let d = TempDir::new("cli-test");
    write_mock_project(d.path()).unwrap();
    let t0 = Instant::now();
    let (code, out) = run(&["test", d.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{}", out);
    assert!(out.contains("PASS (24 steps"), "{}", out);
    assert!(t0.elapsed() < Duration::from_secs(20), "fast: {:?}", t0.elapsed());
    // a failing expectation: exit code 1, the failing step and the serial tail
    let y = std::fs::read_to_string(d.path().join("esp32sim.yaml")).unwrap();
    let y = y.replace("expect_serial: \"pong\"", "expect_serial: \"pang\"\n    timeout: 200ms");
    std::fs::write(d.path().join("esp32sim.yaml"), y).unwrap();
    let (code, out) = run(&["test", d.path().to_str().unwrap(), "--timeout", "5"]);
    assert_eq!(code, 1, "{}", out);
    assert!(out.contains("✗ step 3") && out.contains("pang") && out.contains("FAIL"), "{}", out);
    assert!(out.contains("--- serial output (tail) ---") && out.contains("pong"), "{}", out);
    // no test section: usage error
    let e = TempDir::new("cli-test-empty");
    write_mock_project(e.path()).unwrap();
    let y = std::fs::read_to_string(e.path().join("esp32sim.yaml")).unwrap();
    std::fs::write(e.path().join("esp32sim.yaml"), &y[..y.find("test:").unwrap()]).unwrap();
    let (code, out) = run(&["test", e.path().to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(out.contains("no `test:` steps"), "{}", out);
}

#[test]
fn scenario_runner_is_deterministic() {
    let d = TempDir::new("cli-det");
    write_mock_project(d.path()).unwrap();
    let emu = |out: &str| out.lines().find(|l| l.starts_with("PASS")).map(|l| l.split(',').nth(1).unwrap_or("").trim().to_string());
    let a = run(&["test", d.path().to_str().unwrap()]).1;
    let b = run(&["test", d.path().to_str().unwrap()]).1;
    assert_eq!(emu(&a), emu(&b), "same emulated duration:\n{}\n{}", a, b);
}

#[test]
fn interactive_console_session() {
    let d = TempDir::new("cli-run");
    write_mock_project(d.path()).unwrap();
    let mut child = bin()
        .args(["run", d.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let out = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let o2 = out.clone();
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            o2.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    let wait_for = |needle: &str| {
        let t0 = Instant::now();
        while !out.lock().unwrap().contains(needle) {
            assert!(t0.elapsed() < Duration::from_secs(15), "timeout waiting for {:?}; output:\n{}", needle, out.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait_for("mockfw ready");
    for line in ["ping", "/press btn", "/show board_led", "/release btn", "/parts", "/pins", "/status", "/type", "/tap", "/set", "/show nothing", "/bogus", "count"] {
        writeln!(stdin, "{}", line).unwrap();
        std::thread::sleep(Duration::from_millis(60));
    }
    wait_for("pong");
    wait_for("count=1");
    wait_for("GPIO4");
    wait_for("no component 'nothing'");
    writeln!(stdin, "/quit").unwrap();
    let t0 = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "console did not quit");
        std::thread::sleep(Duration::from_millis(20));
    };
    reader.join().unwrap();
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(status.success(), "exit {:?}\nstderr: {}\nout: {}", status, err, out.lock().unwrap());
    assert!(!err.contains("panicked"), "{}", err);
    let o = out.lock().unwrap();
    assert!(o.contains("btn") && o.contains("pushbutton"), "/parts lists the circuit:\n{}", o);
}

#[test]
fn run_with_time_limit_and_bare_firmware() {
    let d = TempDir::new("cli-limit");
    write_mock_project(d.path()).unwrap();
    // a firmware file without a project: bare board
    let o = bin().args(["run", d.path().join("mockfw.bin").to_str().unwrap(), "--seconds", "0.2", "--speed", "0"]).stdin(Stdio::null()).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{}", out);
    assert!(out.contains("mockfw ready"), "{}", out);
}

#[test]
fn gui_server_starts_and_serves() {
    let d = TempDir::new("cli-gui");
    write_mock_project(d.path()).unwrap();
    // find a free port
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut child = bin().args(["gui", d.path().to_str().unwrap(), "--port", &port.to_string(), "--no-browser"]).stdout(Stdio::piped()).spawn().unwrap();
    let get = |path: &str| -> Option<(u16, String)> {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        write!(s, "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n", path).ok()?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).ok()?;
        let t = String::from_utf8_lossy(&buf).to_string();
        Some((t.split_whitespace().nth(1)?.parse().ok()?, t))
    };
    let t0 = Instant::now();
    let mut ok = None;
    while t0.elapsed() < Duration::from_secs(10) {
        if let Some(r) = get("/api/info") {
            ok = Some(r);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    let (code, body) = ok.expect("GUI server answered");
    assert_eq!(code, 200);
    assert!(body.contains("\"desktop\":false"), "{}", body);
    // port already in use -> clear error
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = l.local_addr().unwrap().port();
    let (code, out) = run(&["gui", d.path().to_str().unwrap(), "--port", &busy.to_string(), "--no-browser"]);
    assert_eq!(code, 2);
    assert!(out.contains("cannot listen"), "{}", out);
}

//! IDE backend: project files, build tools (PlatformIO / arduino-cli), git, settings.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

// ------------------------------------------------------------------ settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiSettings {
    /// openai | anthropic | gemini | ollama | openai-compatible
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    /// custom endpoint (ollama / LM Studio / OpenRouter / Groq ...)
    #[serde(default)]
    pub base_url: String,
    /// provider -> API key (never sent back to the browser)
    #[serde(default)]
    pub keys: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    /// AI inline code completions (ghost text) in the editor
    #[serde(default)]
    pub inline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub ai: AiSettings,
    #[serde(default)]
    pub pio: Option<String>,
    #[serde(default)]
    pub arduino_cli: Option<String>,
    #[serde(default)]
    pub clangd: Option<String>,
    #[serde(default)]
    pub recent: Vec<String>,
    #[serde(default)]
    pub editor: Value,
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".config"));
    base.join("esp32sim")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

impl Settings {
    pub fn load() -> Settings {
        std::fs::read_to_string(config_dir().join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) -> Result<(), String> {
        let d = config_dir();
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let p = d.join("settings.json");
        std::fs::write(&p, serde_json::to_string_pretty(self).unwrap()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
    /// Settings as shown to the browser (API keys masked).
    pub fn public(&self) -> Value {
        let keys: Value = self.ai.keys.iter().map(|(k, v)| (k.clone(), json!(!v.is_empty()))).collect::<serde_json::Map<_, _>>().into();
        json!({
            "ai": {"provider": self.ai.provider, "model": self.ai.model, "base_url": self.ai.base_url, "keys_set": keys, "temperature": self.ai.temperature, "inline": self.ai.inline},
            "pio": self.pio, "arduino_cli": self.arduino_cli, "clangd": self.clangd, "recent": self.recent, "editor": self.editor,
            "tools": {"pio": find_tool("pio", self.pio.as_deref()), "arduino_cli": find_tool("arduino-cli", self.arduino_cli.as_deref()), "clangd": find_tool("clangd", self.clangd.as_deref()), "git": find_tool("git", None)},
        })
    }
    pub fn add_recent(&mut self, p: &str) {
        self.recent.retain(|x| x != p);
        self.recent.insert(0, p.to_string());
        self.recent.truncate(12);
    }
}

/// Locate an external tool: explicit setting, PATH, then well known locations.
pub fn find_tool(name: &str, configured: Option<&str>) -> Option<String> {
    if let Some(c) = configured {
        if !c.is_empty() && Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    // executable names to try (Windows adds .exe / .cmd)
    let names: Vec<String> = if cfg!(windows) { vec![format!("{}.exe", name), format!("{}.cmd", name), name.to_string()] } else { vec![name.to_string()] };
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            for n in &names {
                let p = d.join(n);
                if p.is_file() {
                    return Some(p.display().to_string());
                }
            }
        }
    }
    let h = home();
    let dirs = [
        h.join(".platformio/penv/bin"),
        h.join(".platformio/penv/Scripts"),
        h.join(".local/bin"),
        h.join(".pio-venv/bin"),
        h.join("bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
    ];
    for d in dirs {
        for n in &names {
            let p = d.join(n);
            if p.is_file() {
                return Some(p.display().to_string());
            }
        }
    }
    None
}

// ------------------------------------------------------------------ files
/// Resolve a project-relative path, refusing to escape the project root.
pub fn safe_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim_start_matches('/');
    let p = Path::new(rel);
    for c in p.components() {
        match c {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return Err("path outside the project".into()),
            _ => {}
        }
    }
    Ok(root.join(p))
}

const HIDDEN: &[&str] = &[".git", ".pio", ".esp32sim", "node_modules", ".vscode", "build", "__pycache__"];

pub fn list_tree(root: &Path) -> Value {
    /// at most this many entries, so opening a huge folder stays responsive
    const MAX_ENTRIES: usize = 4000;
    fn walk(root: &Path, dir: &Path, depth: usize, budget: &mut usize) -> Vec<Value> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else { return out };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| (!e.path().is_dir(), e.file_name()));
        for e in entries {
            if *budget == 0 {
                out.push(json!({"name": "… (more files not shown)", "path": "", "dir": false, "size": 0}));
                break;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if HIDDEN.contains(&name.as_str()) {
                continue;
            }
            *budget -= 1;
            let path = e.path();
            let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
            if path.is_dir() {
                let children = if depth < 8 { walk(root, &path, depth + 1, budget) } else { vec![] };
                out.push(json!({"name": name, "path": rel, "dir": true, "children": children}));
            } else {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(json!({"name": name, "path": rel, "dir": false, "size": size}));
            }
        }
        out
    }
    let mut budget = MAX_ENTRIES;
    json!(walk(root, root, 0, &mut budget))
}

// ------------------------------------------------------------------ build
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub severity: String,
    pub message: String,
}

/// Parse gcc-style "file:line:col: error: message" lines.
pub fn parse_diagnostic(line: &str, root: &Path) -> Option<Diagnostic> {
    let l = line.trim();
    for sev in ["error", "warning", "note", "fatal error"] {
        let pat = format!(": {}: ", sev);
        if let Some(idx) = l.find(&pat) {
            let head = &l[..idx];
            let msg = &l[idx + pat.len()..];
            // "file:line:col" or "file:line" (no column)
            let three: Vec<&str> = head.rsplitn(3, ':').collect();
            let num = |s: &str| s.trim().parse::<u32>().ok();
            let (file, line_no, col) = match three.as_slice() {
                [c, l, f] if num(c).is_some() && num(l).is_some() => (f.to_string(), num(l)?, num(c)?),
                [l, f, ..] if num(l).is_some() => {
                    let file = head[..head.len() - l.len() - 1].to_string();
                    let _ = f;
                    (file, num(l)?, 1)
                }
                _ => return None,
            };
            if file.trim().is_empty() {
                return None;
            }
            let fp = PathBuf::from(&file);
            let rel = fp.strip_prefix(root).map(|p| p.display().to_string()).unwrap_or(file);
            return Some(Diagnostic { file: rel, line: line_no, col, severity: if sev == "fatal error" { "error".into() } else { sev.into() }, message: msg.to_string() });
        }
    }
    None
}

pub enum BuildTool {
    Pio(String),
    ArduinoCli(String),
}

/// Determine how to build the project at `root`.
pub fn build_command(root: &Path, settings: &Settings) -> Result<(BuildTool, Vec<String>), String> {
    if root.join("platformio.ini").exists() {
        let pio = find_tool("pio", settings.pio.as_deref()).or_else(|| find_tool("platformio", None)).ok_or(
            "PlatformIO CLI not found. Install it (pip install platformio) or set its path in Settings.",
        )?;
        return Ok((BuildTool::Pio(pio), vec!["run".into()]));
    }
    let has_ino = std::fs::read_dir(root).map(|rd| rd.flatten().any(|e| e.path().extension().map(|x| x == "ino").unwrap_or(false))).unwrap_or(false);
    if has_ino {
        if let Some(cli) = find_tool("arduino-cli", settings.arduino_cli.as_deref()) {
            return Ok((
                BuildTool::ArduinoCli(cli),
                vec!["compile".into(), "--fqbn".into(), "esp32:esp32:esp32".into(), "--output-dir".into(), "build".into(), "--build-property".into(), "compiler.optimization_flags=-Og -g".into(), ".".into()],
            ));
        }
        return Err("Arduino sketch found but neither arduino-cli nor a platformio.ini is available. Use 'Convert to PlatformIO' or install arduino-cli.".into());
    }
    Err("no platformio.ini or .ino sketch in the project".into())
}

/// Create a platformio.ini for an Arduino sketch folder.
pub fn convert_sketch_to_pio(root: &Path) -> Result<(), String> {
    let ini = "[platformio]\nsrc_dir = .\n\n[env:esp32dev]\nplatform = espressif32\nboard = esp32dev\nframework = arduino\nmonitor_speed = 115200\nbuild_flags = -g\n";
    if root.join("platformio.ini").exists() {
        return Err("platformio.ini already exists".into());
    }
    std::fs::write(root.join("platformio.ini"), ini).map_err(|e| e.to_string())
}

/// After arduino-cli builds, point the project at its output.
pub fn arduino_cli_outputs(root: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    let rd = std::fs::read_dir(root.join("build")).ok()?;
    let mut bin = None;
    let mut elf = None;
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.ends_with(".ino.merged.bin") || (n.ends_with(".ino.bin") && bin.is_none()) {
            bin = Some(e.path());
        }
        if n.ends_with(".ino.elf") {
            elf = Some(e.path());
        }
    }
    bin.map(|b| (b, elf))
}

// ------------------------------------------------------------------ git
pub fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").args(args).current_dir(root).output().map_err(|e| format!("git: {}", e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        Err(if err.trim().is_empty() { String::from_utf8_lossy(&out.stdout).to_string() } else { err })
    }
}

pub fn git_status(root: &Path) -> Value {
    if git(root, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return json!({"repo": false});
    }
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default().trim().to_string();
    let porcelain = git(root, &["status", "--porcelain=v1", "-uall"]).unwrap_or_default();
    let files: Vec<Value> = porcelain
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let (x, y) = (&l[0..1], &l[1..2]);
            let path = l[3..].trim().trim_matches('"').to_string();
            let path = path.split(" -> ").last().unwrap_or(&path).to_string();
            json!({"path": path, "index": x, "worktree": y, "staged": x != " " && x != "?", "untracked": x == "?"})
        })
        .collect();
    let ahead_behind = git(root, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).ok();
    let (ahead, behind) = ahead_behind
        .and_then(|s| {
            let mut it = s.split_whitespace();
            Some((it.next()?.parse::<u32>().ok()?, it.next()?.parse::<u32>().ok()?))
        })
        .unwrap_or((0, 0));
    let branches: Vec<String> = git(root, &["branch", "--format=%(refname:short)"]).unwrap_or_default().lines().map(|s| s.to_string()).collect();
    let remotes: Vec<String> = git(root, &["remote"]).unwrap_or_default().lines().map(|s| s.to_string()).collect();
    json!({"repo": true, "branch": branch, "files": files, "ahead": ahead, "behind": behind, "branches": branches, "remotes": remotes})
}

pub fn git_log(root: &Path, n: usize) -> Value {
    let out = git(root, &["log", &format!("-{}", n), "--pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s"]).unwrap_or_default();
    json!(out
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\x1f').collect();
            if f.len() < 5 {
                return None;
            }
            Some(json!({"hash": f[0], "short": f[1], "author": f[2], "when": f[3], "subject": f[4]}))
        })
        .collect::<Vec<_>>())
}

// ------------------------------------------------------------------ IntelliSense before the first build
const STUBS: &[(&str, &str)] = &[
    ("Arduino.h", include_str!("../stubs/Arduino.h")),
    ("WString.h", include_str!("../stubs/WString.h")),
    ("Print.h", include_str!("../stubs/Print.h")),
    ("Stream.h", include_str!("../stubs/Stream.h")),
    ("HardwareSerial.h", include_str!("../stubs/HardwareSerial.h")),
    ("Esp.h", include_str!("../stubs/Esp.h")),
    ("freertos/FreeRTOS.h", include_str!("../stubs/freertos/FreeRTOS.h")),
    ("Wire.h", include_str!("../stubs/Wire.h")),
    ("SPI.h", include_str!("../stubs/SPI.h")),
    ("WiFi.h", include_str!("../stubs/WiFi.h")),
    ("Preferences.h", include_str!("../stubs/Preferences.h")),
];

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache")).join("esp32sim")
}

/// Directory holding the compile database clangd should use for `root`.
///
/// A project built with PlatformIO has its own `compile_commands.json`
/// (`pio run -t compiledb`, done after every build).  Before that, we
/// synthesise one in the cache that points at bundled Arduino-ESP32 API
/// stubs plus the project's include/, src/ and library folders, so code
/// completion and diagnostics work straight away.
pub fn lsp_compile_dir(root: &Path) -> PathBuf {
    if root.join("compile_commands.json").exists() {
        return root.to_path_buf();
    }
    let stubs = cache_dir().join("arduino-stubs");
    for (name, text) in STUBS {
        let p = stubs.join(name);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(*text) {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(&p, text);
        }
    }
    let mut incs: Vec<PathBuf> = vec![stubs.clone(), root.join("include"), root.join("src")];
    // project libraries and libraries PlatformIO already downloaded
    let mut lib_roots = vec![root.join("lib")];
    if let Ok(envs) = std::fs::read_dir(root.join(".pio/libdeps")) {
        lib_roots.extend(envs.flatten().map(|e| e.path()));
    }
    for lr in lib_roots {
        if let Ok(rd) = std::fs::read_dir(&lr) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    incs.push(if p.join("src").is_dir() { p.join("src") } else { p });
                }
            }
        }
    }
    let mut flags: Vec<String> = vec!["-std=gnu++17".into(), "-DARDUINO=10819".into(), "-DESP32=1".into(), "-DARDUINO_ARCH_ESP32=1".into(), "-DESP_PLATFORM".into(), "-Wno-unknown-attributes".into()];
    flags.extend(incs.iter().filter(|p| p.is_dir()).map(|p| format!("-I{}", p.display())));
    let mut files = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > 6 {
            return;
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if p.is_dir() {
                    walk(&p, out, depth + 1);
                } else if matches!(p.extension().and_then(|x| x.to_str()), Some("cpp" | "c" | "cc" | "ino" | "h" | "hpp")) {
                    out.push(p);
                }
            }
        }
    }
    for d in ["src", "include", "lib", "test"] {
        walk(&root.join(d), &mut files, 0);
    }
    // Arduino-IDE style sketch: .ino files in the root
    if let Ok(rd) = std::fs::read_dir(root) {
        files.extend(rd.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some("ino")));
    }
    let entries: Vec<Value> = files
        .iter()
        .map(|f| {
            let is_c = f.extension().and_then(|x| x.to_str()) == Some("c");
            let mut args: Vec<String> = vec![if is_c { "clang".into() } else { "clang++".into() }];
            if f.extension().and_then(|x| x.to_str()) == Some("ino") {
                args.push("-xc++".into());
                args.push("-include".into());
                args.push("Arduino.h".into());
            }
            args.extend(flags.iter().filter(|x| !(is_c && x.starts_with("-std="))).cloned());
            args.push("-c".into());
            args.push(f.display().to_string());
            json!({"directory": root.display().to_string(), "file": f.display().to_string(), "arguments": args})
        })
        .collect();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in root.display().to_string().bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    let dir = cache_dir().join("lsp").join(format!("{:016x}", h));
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("compile_commands.json"), serde_json::to_string_pretty(&Value::Array(entries)).unwrap_or_default());
    dir
}

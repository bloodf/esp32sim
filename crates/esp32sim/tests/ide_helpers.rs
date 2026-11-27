//! IDE backend helpers: diagnostics parsing, project file access, build tool
//! selection, sketch conversion, git status parsing and the synthesised
//! clangd compile database (IntelliSense before the first build).
use esp32sim::ide::{self, Settings};
use esp32sim::testing::TempDir;
use serde_json::Value;
use std::path::Path;

fn w(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

#[test]
fn gcc_and_clang_diagnostics() {
    let root = Path::new("/work/proj");
    let d = ide::parse_diagnostic("/work/proj/src/main.cpp:12:5: error: 'ledPin' was not declared in this scope", root).unwrap();
    assert_eq!((d.file.as_str(), d.line, d.col, d.severity.as_str()), ("src/main.cpp", 12, 5, "error"));
    assert_eq!(d.message, "'ledPin' was not declared in this scope");
    let d = ide::parse_diagnostic("src/app.cpp:3: warning: unused variable 'x'", root).unwrap();
    assert_eq!((d.line, d.col, d.severity.as_str()), (3, 1, "warning"));
    let d = ide::parse_diagnostic("src/main.cpp:1:10: fatal error: Adafruit_SSD1306.h: No such file or directory", root).unwrap();
    assert_eq!(d.severity, "error");
    assert!(d.message.starts_with("Adafruit_SSD1306.h"));
    let d = ide::parse_diagnostic("  lib/x/x.h:7:2: note: declared here", root).unwrap();
    assert_eq!((d.file.as_str(), d.severity.as_str()), ("lib/x/x.h", "note"));
    for junk in ["Compiling .pio/build/esp32dev/src/main.cpp.o", "error: linker failed", "*** [firmware.elf] Error 1", ""] {
        assert!(ide::parse_diagnostic(junk, root).is_none(), "{}", junk);
    }
}

#[test]
fn safe_paths_stay_in_the_project() {
    let root = Path::new("/p");
    assert_eq!(ide::safe_path(root, "src/main.cpp").unwrap(), Path::new("/p/src/main.cpp"));
    assert_eq!(ide::safe_path(root, "/src/main.cpp").unwrap(), Path::new("/p/src/main.cpp"));
    assert!(ide::safe_path(root, "../etc/passwd").is_err());
    assert!(ide::safe_path(root, "src/../../x").is_err());
    assert!(ide::safe_path(root, "./ok.txt").is_ok());
}

#[test]
fn file_tree_hides_build_folders_and_sorts_dirs_first() {
    let d = TempDir::new("tree");
    for f in ["src/main.cpp", "include/pins.h", ".pio/build/x.o", ".git/HEAD", ".esp32sim/flash-data.bin", "platformio.ini", "a.txt"] {
        w(&d.path().join(f), "x");
    }
    let t = ide::list_tree(d.path());
    let names: Vec<&str> = t.as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["include", "src", "a.txt", "platformio.ini"]);
    let src = &t.as_array().unwrap()[1];
    assert_eq!(src["children"][0]["path"], Value::String("src/main.cpp".into()));
    assert_eq!(src["children"][0]["size"], Value::from(1));
}

#[test]
fn file_tree_is_capped_for_huge_folders() {
    let d = TempDir::new("bigtree");
    for i in 0..4100 {
        std::fs::write(d.path().join(format!("f{:05}.txt", i)), "").unwrap();
    }
    let t = ide::list_tree(d.path());
    let a = t.as_array().unwrap();
    assert_eq!(a.len(), 4001);
    assert!(a.last().unwrap()["name"].as_str().unwrap().contains("more files not shown"));
}

#[test]
fn build_tool_selection() {
    let d = TempDir::new("buildsel");
    let s = Settings::default();
    let e = ide::build_command(d.path(), &s).err().unwrap();
    assert!(e.contains("no platformio.ini or .ino"), "{}", e);
    w(&d.path().join("blink.ino"), "void setup(){}\nvoid loop(){}\n");
    if ide::find_tool("arduino-cli", None).is_none() {
        let e = ide::build_command(d.path(), &s).err().unwrap();
        assert!(e.contains("Convert to PlatformIO"), "{}", e);
    }
    ide::convert_sketch_to_pio(d.path()).unwrap();
    assert!(ide::convert_sketch_to_pio(d.path()).is_err(), "only once");
    // a configured PlatformIO executable is used as-is
    let fake = d.path().join("fake-pio");
    w(&fake, "#!/bin/sh\n");
    let s = Settings { pio: Some(fake.display().to_string()), ..Default::default() };
    match ide::build_command(d.path(), &s).unwrap() {
        (ide::BuildTool::Pio(p), args) => {
            assert_eq!(p, fake.display().to_string());
            assert_eq!(args, vec!["run"]);
        }
        _ => panic!("expected PlatformIO"),
    }
}

#[test]
fn arduino_cli_output_discovery() {
    let d = TempDir::new("acli");
    assert!(ide::arduino_cli_outputs(d.path()).is_none());
    w(&d.path().join("build/blink.ino.bin"), "");
    w(&d.path().join("build/blink.ino.elf"), "");
    w(&d.path().join("build/blink.ino.merged.bin"), "");
    let (bin, elf) = ide::arduino_cli_outputs(d.path()).unwrap();
    assert!(bin.ends_with("blink.ino.merged.bin"), "merged image preferred: {:?}", bin);
    assert!(elf.unwrap().ends_with("blink.ino.elf"));
}

#[test]
fn git_status_and_log_parsing() {
    let d = TempDir::new("gitparse");
    assert_eq!(ide::git_status(d.path())["repo"], Value::Bool(false));
    ide::git(d.path(), &["init", "-q", "-b", "main"]).unwrap();
    w(&d.path().join("a.txt"), "1");
    w(&d.path().join("dir/b.txt"), "2");
    let st = ide::git_status(d.path());
    assert_eq!(st["repo"], Value::Bool(true));
    let files = st["files"].as_array().unwrap();
    assert!(files.iter().all(|f| f["untracked"] == Value::Bool(true)));
    assert!(files.iter().any(|f| f["path"] == "dir/b.txt"), "-uall lists nested files");
    ide::git(d.path(), &["add", "a.txt"]).unwrap();
    let st = ide::git_status(d.path());
    let a = st["files"].as_array().unwrap().iter().find(|f| f["path"] == "a.txt").unwrap().clone();
    assert_eq!(a["staged"], Value::Bool(true));
    ide::git(d.path(), &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "subject with | pipes"]).unwrap();
    let log = ide::git_log(d.path(), 10);
    assert_eq!(log[0]["subject"], Value::String("subject with | pipes".into()));
    assert_eq!(log[0]["author"], Value::String("t".into()));
    assert_eq!(ide::git_status(d.path())["branch"], Value::String("main".into()));
}

#[test]
fn clangd_compile_database_before_the_first_build() {
    let home = TempDir::new("lspcache");
    std::env::set_var("XDG_CACHE_HOME", home.path());
    let d = TempDir::new("lsp");
    w(&d.path().join("src/main.cpp"), "#include <Arduino.h>\n");
    w(&d.path().join("src/util.c"), "");
    w(&d.path().join("lib/MyLib/src/MyLib.h"), "");
    w(&d.path().join("sketch.ino"), "");
    let dir = ide::lsp_compile_dir(d.path());
    assert!(dir.starts_with(home.path()), "synthesised in the cache");
    let db: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("compile_commands.json")).unwrap()).unwrap();
    let entries = db.as_array().unwrap();
    let find = |suffix: &str| entries.iter().find(|e| e["file"].as_str().unwrap().ends_with(suffix)).cloned().unwrap();
    let cpp = find("main.cpp");
    let args: Vec<&str> = cpp["arguments"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert_eq!(args[0], "clang++");
    assert!(args.contains(&"-std=gnu++17") && args.contains(&"-DESP32=1"));
    assert!(args.iter().any(|a| a.ends_with("arduino-stubs")), "Arduino API stubs on the include path");
    assert!(args.iter().any(|a| a.ends_with("lib/MyLib/src")), "project libraries included");
    let c = find("util.c");
    assert_eq!(c["arguments"][0], "clang");
    assert!(!c["arguments"].to_string().contains("-std=gnu++17"));
    let ino = find("sketch.ino");
    assert!(ino["arguments"].to_string().contains("\"-include\",\"Arduino.h\""), "{}", ino);
    for stub in ["Arduino.h", "Wire.h", "SPI.h", "WiFi.h", "Preferences.h", "freertos/FreeRTOS.h"] {
        assert!(home.path().join("esp32sim/arduino-stubs").join(stub).exists(), "{}", stub);
    }
    // a PlatformIO compile database wins once it exists
    w(&d.path().join("compile_commands.json"), "[]");
    assert_eq!(ide::lsp_compile_dir(d.path()), d.path());
}

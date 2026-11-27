//! Helpers behind the `parts` and `info` sub-commands.
use std::io::Write;
use std::path::Path;

/// `esp32sim parts`: one line per supported part.
pub fn list_parts(out: &mut dyn Write) -> Result<i32, String> {
    for e in devices::registry::CATALOG {
        // stop quietly if the reader goes away (e.g. `esp32sim parts | head`)
        if writeln!(out, "{:<16} {:<12} {:<34} {}", e.kind, e.category, e.name, e.description).is_err() {
            break;
        }
    }
    Ok(0)
}

/// `esp32sim parts <type>`: pins, default properties and a YAML snippet.
pub fn describe_part(p: &str, out: &mut dyn Write) -> Result<i32, String> {
    let cat = devices::registry::catalog_json();
    let found = cat.as_array().and_then(|a| a.iter().find(|x| x["type"].as_str().map(|t| t.eq_ignore_ascii_case(p)).unwrap_or(false)).cloned());
    let x = found.ok_or_else(|| format!("unknown part type '{}' (see `esp32sim parts`)", p))?;
    let w = |out: &mut dyn Write, s: String| {
        let _ = writeln!(out, "{}", s);
    };
    w(out, format!("{} — {}\n{}\n", x["type"].as_str().unwrap_or(""), x["name"].as_str().unwrap_or(""), x["description"].as_str().unwrap_or("")));
    w(out, "pins:".into());
    for pin in x["pins"].as_array().cloned().unwrap_or_default() {
        w(out, format!("  {:<8} {}", pin["name"].as_str().unwrap_or(""), pin["kind"].as_str().unwrap_or("")));
    }
    w(out, "properties (defaults):".into());
    for (k, v) in x["props"].as_object().cloned().unwrap_or_default() {
        w(out, format!("  {}: {}", k, v));
    }
    let t = x["type"].as_str().unwrap_or("part");
    w(out, format!("\nYAML:\n  - {{id: {}_1, type: {}}}", t.replace('-', "_"), t));
    Ok(0)
}

/// `esp32sim info <firmware>`: format, partitions and app segments.
pub fn info(p: &Path) -> Result<i32, String> {
    print!("{}", info_text(p)?);
    Ok(0)
}

pub fn info_text(p: &Path) -> Result<String, String> {
    use std::fmt::Write as _;
    let d = std::fs::read(p).map_err(|e| format!("{}: {}", p.display(), e))?;
    let kind = esp32_soc::flash::detect_kind(&d)?;
    let mut s = String::new();
    let _ = writeln!(s, "format: {:?}", kind);
    let flash = esp32_soc::flash::build_flash(&d, 4 << 20, None, None)?;
    let _ = writeln!(s, "partitions:");
    for part in flash.partitions() {
        let _ = writeln!(s, "  {}", part);
    }
    let bp = flash.boot_partition().ok_or("no app partition")?;
    let img = esp32_soc::flash::parse_app_image(&flash, bp.offset)?;
    let _ = writeln!(s, "app: '{}' version '{}' IDF {}", img.project_name, img.version, img.idf_version);
    let _ = writeln!(s, "entry: 0x{:08x}", img.entry);
    for seg in &img.segments {
        let _ = writeln!(s, "  segment 0x{:08x} len 0x{:06x}", seg.load_addr, seg.len);
    }
    Ok(s)
}

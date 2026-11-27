//! Project files (YAML): board, firmware, components, wiring, GUI layout and test scenarios.

use crate::circuit::Board;
use crate::registry::{board_def, create, pad_number};
use crate::Device;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComponentCfg {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// all other keys are device properties (and GUI placement: x, y, rotate)
    #[serde(flatten)]
    pub props: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConnCfg {
    Pair(Vec<String>),
    Text(String),
    Wire {
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        points: Option<Vec<[f64; 2]>>,
    },
}

impl ConnCfg {
    pub fn endpoints(&self) -> Result<Vec<(String, String)>, String> {
        match self {
            ConnCfg::Pair(v) => {
                if v.len() < 2 {
                    return Err(format!("connection needs at least two endpoints: {:?}", v));
                }
                // a list of endpoints all connected together
                Ok(v.windows(2).map(|w| (w[0].clone(), w[1].clone())).collect())
            }
            ConnCfg::Text(s) => {
                let parts: Vec<String> = s.split(['-', '>', '<', '=']).map(|x| x.trim()).filter(|x| !x.is_empty()).map(|x| x.to_string()).collect();
                // allow "esp32.D21 -> oled.SDA" but do not split names containing '-' (e.g. "rgb-led")
                if parts.len() < 2 {
                    return Err(format!("cannot parse connection '{}'", s));
                }
                let joined: Vec<String> = s.split("->").flat_map(|x| x.split(" - ")).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                let v = if joined.len() >= 2 { joined } else { parts };
                Ok(v.windows(2).map(|w| (w[0].clone(), w[1].clone())).collect())
            }
            ConnCfg::Wire { from, to, .. } => Ok(vec![(from.clone(), to.clone())]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TestStep {
    Map(Map<String, Value>),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_board")]
    pub board: String,
    /// firmware image (.bin / merged .bin / .elf), relative to the project file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    /// ELF with symbols (optional, enables source-level debugging and backtraces)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partitions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootloader: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash_size: Option<String>,
    /// real-time factor: 1.0 = real time, 0 = as fast as possible
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    /// PlatformIO / Arduino project directory (for the IDE)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub components: Vec<ComponentCfg>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub connections: Vec<ConnCfg>,
    /// GUI metadata (positions, view settings)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test: Vec<Map<String, Value>>,
    #[serde(skip)]
    pub base_dir: PathBuf,
}

/// `components:` with nothing under it is YAML null: treat it as an empty list.
fn null_as_empty<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

fn default_board() -> String {
    "esp32-devkit-v1".into()
}

impl ProjectConfig {
    pub fn load(path: &Path) -> Result<ProjectConfig, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let mut c = Self::parse(&text)?;
        c.base_dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        Ok(c)
    }

    pub fn parse(text: &str) -> Result<ProjectConfig, String> {
        let v: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| format!("YAML error: {}", e))?;
        let j: Value = serde_json::to_value(v).map_err(|e| e.to_string())?;
        serde_json::from_value(j).map_err(|e| format!("project file: {}", e))
    }

    /// Serialize in the compact style used by hand-written project files:
    /// one flow mapping per component, one flow sequence per connection.
    pub fn to_yaml(&self) -> String {
        let v = match serde_json::to_value(self) {
            Ok(Value::Object(m)) => m,
            _ => return serde_yaml::to_string(self).unwrap_or_default(),
        };
        let mut out = String::new();
        for key in ["name", "board", "firmware", "elf", "partitions", "bootloader", "flash_size", "speed", "source"] {
            if let Some(x) = v.get(key) {
                if !x.is_null() {
                    out += &format!("{}: {}\n", key, flow(x));
                }
            }
        }
        for key in ["components", "connections"] {
            match v.get(key) {
                Some(Value::Array(cs)) if !cs.is_empty() => {
                    out += &format!("{}:\n", key);
                    for c in cs {
                        // a plain wire (no colour / route points) is written as a pair
                        let simple = c.as_object().filter(|o| o.len() == 2 && o.contains_key("from") && o.contains_key("to"));
                        match simple {
                            Some(o) if key == "connections" => out += &format!("  - [{}, {}]\n", flow(&o["from"]), flow(&o["to"])),
                            _ => out += &format!("  - {}\n", flow(c)),
                        }
                    }
                }
                _ => out += &format!("{}: []\n", key),
            }
        }
        if let Some(l) = v.get("layout") {
            if !l.is_null() && l.as_object().map(|o| !o.is_empty()).unwrap_or(true) {
                out += "layout:\n";
                if let Value::Object(o) = l {
                    for (k, x) in o {
                        out += &format!("  {}: {}\n", flow_key(k), flow(x));
                    }
                }
            }
        }
        if let Some(Value::Array(ts)) = v.get("test") {
            if !ts.is_empty() {
                out += "test:\n";
                for t in ts {
                    match t {
                        Value::Object(o) if o.len() == 1 => {
                            let (k, x) = o.iter().next().unwrap();
                            out += &format!("  - {}: {}\n", flow_key(k), flow(x));
                        }
                        _ => out += &format!("  - {}\n", flow(t)),
                    }
                }
            }
        }
        out
    }

    pub fn resolve(&self, p: &str) -> PathBuf {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            pb
        } else {
            self.base_dir.join(pb)
        }
    }

    pub fn flash_bytes(&self) -> usize {
        let s = self.flash_size.clone().unwrap_or_else(|| "4MB".into()).to_ascii_uppercase();
        let n: usize = s.trim_end_matches("MB").trim_end_matches('M').trim().parse().unwrap_or(4);
        n.clamp(1, 16) << 20
    }

    /// Instantiate all components and wire them into a Board.
    pub fn build_board(&self) -> Result<Board, String> {
        let mut comps: Vec<(String, Box<dyn Device>)> = Vec::new();
        let mut wires: Vec<(String, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for c in &self.components {
            if c.id.is_empty() {
                return Err(format!("component of type '{}' has no id", c.kind));
            }
            if !seen.insert(c.id.to_ascii_lowercase()) {
                return Err(format!("duplicate component id '{}'", c.id));
            }
            if matches!(c.id.to_ascii_lowercase().as_str(), "esp32" | "board" | "gnd" | "3v3" | "5v" | "vin") {
                return Err(format!("component id '{}' is reserved", c.id));
            }
            let mut props = c.props.clone();
            props.insert("__id".into(), Value::String(c.id.clone()));
            props.insert("__base_dir".into(), Value::String(self.base_dir.display().to_string()));
            let dev = create(&c.kind, &props).map_err(|e| format!("component '{}': {}", c.id, e))?;
            comps.push((c.id.clone(), dev));
        }
        // board built-ins: on-board LED (through its resistor) and BOOT button
        let bd = board_def(&self.board);
        if let Some(g) = bd.led_gpio {
            let mut p = Map::new();
            p.insert("color".into(), Value::String("blue".into()));
            comps.push(("board_led".into(), create("led", &p)?));
            let mut r = Map::new();
            r.insert("value".into(), Value::String("1k".into()));
            comps.push(("board_led_r".into(), create("resistor", &r)?));
            wires.push((format!("esp32.GPIO{}", g), "board_led_r.1".into()));
            wires.push(("board_led_r.2".into(), "board_led.A".into()));
            wires.push(("board_led.C".into(), "GND".into()));
        }
        comps.push(("boot_button".into(), create("pushbutton", &Map::new())?));
        wires.push(("esp32.GPIO0".into(), "boot_button.1".into()));
        wires.push(("boot_button.2".into(), "GND".into()));
        let mut r = Map::new();
        r.insert("value".into(), Value::String("10k".into()));
        comps.push(("boot_pullup".into(), create("resistor", &r)?));
        wires.push(("esp32.GPIO0".into(), "boot_pullup.1".into()));
        wires.push(("boot_pullup.2".into(), "3V3".into()));
        for c in &self.connections {
            wires.extend(c.endpoints()?);
        }
        Board::new(comps, &wires, &|s: &str| pad_number(s))
    }
}

/// A YAML flow-style rendering of a JSON value (used for project files).
fn flow(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => flow_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(flow).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, x)| match (k.as_str(), x.as_u64()) {
                    // I2C addresses read better in hex
                    ("address" | "addr", Some(a)) if a < 0x400 => format!("{}: 0x{:02X}", k, a),
                    _ => format!("{}: {}", flow_key(k), flow(x)),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn flow_key(k: &str) -> String {
    flow_str(k)
}

/// Plain scalar when unambiguous, JSON-style double quotes otherwise.
fn flow_str(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "._/+-".contains(c))
        && !s.starts_with(['-', '+'])
        && s.parse::<f64>().is_err()
        && !matches!(s.to_ascii_lowercase().as_str(), "true" | "false" | "null" | "~" | ".inf" | ".nan")
        && !(s.starts_with("0x") || s.starts_with("0o") || s.starts_with("0b"));
    if plain {
        s.to_string()
    } else {
        serde_json::to_string(s).unwrap_or_default()
    }
}

#[cfg(test)]
mod yaml_tests {
    use super::*;

    #[test]
    fn compact_yaml_round_trips() {
        let src = "name: demo\nboard: esp32-devkit-v1\nfirmware: .pio/build/esp32dev/firmware.bin\ncomponents:\n  - {id: led1, type: led, color: red, x: 520, y: 140}\n  - {id: io, type: pcf8574, address: \"0x20\"}\n  - {id: r, type: resistor, value: 4.7k}\nconnections:\n  - [esp32.D4, led1.A]\n  - {from: led1.C, to: GND, color: \"#3a3a3a\", points: [[10.0, 20.0]]}\n  - \"esp32.D5 - r.1\"\ntest:\n  - expect_serial: \"ready: yes\"\n  - press: btn\n  - expect: {id: led1, brightness: \">0.5\"}\n";
        let c = ProjectConfig::parse(src).unwrap();
        let out = c.to_yaml();
        assert!(out.contains("  - {id: led1, type: led, color: red, x: 520, y: 140}"), "{}", out);
        assert!(out.contains("  - [esp32.D4, led1.A]"), "{}", out);
        assert!(out.contains("address: \"0x20\""), "{}", out);
        let again = ProjectConfig::parse(&out).unwrap();
        assert_eq!(serde_json::to_value(&again).unwrap(), serde_json::to_value(&c).unwrap());
    }

    #[test]
    fn empty_lists() {
        let c = ProjectConfig::parse("name: x\ncomponents:\nconnections:\n").unwrap();
        assert!(c.components.is_empty() && c.connections.is_empty());
        let out = c.to_yaml();
        assert!(out.contains("components: []") && out.contains("connections: []"), "{}", out);
        ProjectConfig::parse(&out).unwrap();
    }
}

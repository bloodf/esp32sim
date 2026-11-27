//! Project loading and scaffolding.

use devices::ProjectConfig;
use std::path::{Path, PathBuf};

pub const PROJECT_FILE: &str = "esp32sim.yaml";

/// Load a project from a YAML file, a directory or a firmware image.
pub fn load(target: &Path) -> Result<ProjectConfig, String> {
    let t = if target.as_os_str().is_empty() { Path::new(".") } else { target };
    if t.is_dir() {
        for name in [PROJECT_FILE, "esp32sim.yml", "diagram.yaml"] {
            let p = t.join(name);
            if p.exists() {
                return ProjectConfig::load(&p);
            }
        }
        // bare PlatformIO project: no circuit yet
        let mut c = ProjectConfig { name: dir_name(t), board: "esp32-devkit-v1".into(), ..Default::default() };
        c.base_dir = t.to_path_buf();
        c.source = Some(".".into());
        return Ok(c);
    }
    let mut ext = t.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "yaml" | "yml" | "bin" | "elf") {
        // firmware without an extension (e.g. cargo/esp-hal build output): sniff the header
        let mut head = [0u8; 4];
        if std::fs::File::open(t).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok() {
            if head == *b"\x7fELF" {
                ext = "elf".into();
            } else if head[0] == 0xE9 {
                ext = "bin".into();
            }
        }
    }
    match ext.as_str() {
        "yaml" | "yml" => ProjectConfig::load(t),
        "bin" | "elf" => {
            let abs = std::fs::canonicalize(t).map_err(|e| format!("{}: {}", t.display(), e))?;
            let mut c = ProjectConfig { name: dir_name(&abs), board: "esp32-devkit-v1".into(), ..Default::default() };
            c.firmware = Some(abs.display().to_string());
            c.base_dir = abs.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            Ok(c)
        }
        _ => Err(format!("don't know how to open '{}' (expected project .yaml, directory, .bin or .elf)", t.display())),
    }
}

fn dir_name(p: &Path) -> String {
    std::fs::canonicalize(p).ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string())).unwrap_or_else(|| "project".into())
}

pub fn save(cfg: &ProjectConfig) -> Result<PathBuf, String> {
    let p = cfg.base_dir.join(PROJECT_FILE);
    // keep the comment block at the top of a hand-written file
    let header: String = std::fs::read_to_string(&p)
        .map(|old| old.lines().take_while(|l| l.trim_start().starts_with('#') || l.trim().is_empty()).map(|l| format!("{}\n", l)).collect())
        .unwrap_or_default();
    std::fs::write(&p, header + &cfg.to_yaml()).map_err(|e| e.to_string())?;
    Ok(p)
}

pub struct Template {
    pub name: &'static str,
    pub description: &'static str,
    pub lib_deps: &'static [&'static str],
    pub code: &'static str,
    pub circuit: &'static str,
}

pub const TEMPLATES: &[Template] = &[
    Template {
        name: "blink",
        description: "Blink an LED and print to the serial monitor",
        lib_deps: &[],
        code: r#"#include <Arduino.h>

const int LED_PIN = 4;

void setup() {
  Serial.begin(115200);
  pinMode(LED_PIN, OUTPUT);
  Serial.println("Blink example started");
}

void loop() {
  digitalWrite(LED_PIN, HIGH);
  Serial.println("LED on");
  delay(500);
  digitalWrite(LED_PIN, LOW);
  Serial.println("LED off");
  delay(500);
}
"#,
        circuit: r#"components:
  - {id: led1, type: led, color: red, x: 520, y: 140}
  - {id: r1, type: resistor, value: 220, x: 420, y: 140}
connections:
  - [esp32.D4, r1.1]
  - [r1.2, led1.A]
  - [led1.C, GND]
test:
  - expect_serial: "Blink example started"
  - wait: 300ms
  - expect: {id: led1, brightness: ">0.5"}
  - wait: 500ms
  - expect: {id: led1, brightness: "<0.1"}
"#,
    },
    Template {
        name: "button",
        description: "Push button with internal pull-up toggling an LED",
        lib_deps: &[],
        code: r#"#include <Arduino.h>

const int BUTTON_PIN = 15;
const int LED_PIN = 4;
bool ledOn = false;
int lastState = HIGH;

void setup() {
  Serial.begin(115200);
  pinMode(BUTTON_PIN, INPUT_PULLUP);
  pinMode(LED_PIN, OUTPUT);
  Serial.println("Press the button");
}

void loop() {
  int s = digitalRead(BUTTON_PIN);
  if (s == LOW && lastState == HIGH) {
    ledOn = !ledOn;
    digitalWrite(LED_PIN, ledOn);
    Serial.printf("Button pressed, LED %s\n", ledOn ? "ON" : "OFF");
    delay(30);
  }
  lastState = s;
  delay(5);
}
"#,
        circuit: r#"components:
  - {id: btn1, type: pushbutton, x: 420, y: 260}
  - {id: led1, type: led, color: green, x: 520, y: 140}
  - {id: r1, type: resistor, value: 220, x: 420, y: 140}
connections:
  - [esp32.D15, btn1.1]
  - [btn1.2, GND]
  - [esp32.D4, r1.1]
  - [r1.2, led1.A]
  - [led1.C, GND]
test:
  - expect_serial: "Press the button"
  - click: btn1
  - expect_serial: "LED ON"
  - expect: {id: led1, brightness: ">0.5"}
"#,
    },
    Template {
        name: "oled",
        description: "SSD1306 128x64 OLED over I2C (Adafruit GFX)",
        lib_deps: &["adafruit/Adafruit SSD1306@^2.5.7", "adafruit/Adafruit GFX Library@^1.11.5"],
        code: r#"#include <Arduino.h>
#include <Wire.h>
#include <Adafruit_GFX.h>
#include <Adafruit_SSD1306.h>

Adafruit_SSD1306 display(128, 64, &Wire, -1);

void setup() {
  Serial.begin(115200);
  if (!display.begin(SSD1306_SWITCHCAPVCC, 0x3C)) {
    Serial.println("SSD1306 not found");
    for (;;) delay(1000);
  }
  display.clearDisplay();
  display.setTextSize(2);
  display.setTextColor(SSD1306_WHITE);
  display.setCursor(0, 0);
  display.println("Hello");
  display.println("ESP32!");
  display.display();
  Serial.println("OLED ready");
}

void loop() {
  static int n = 0;
  display.fillRect(0, 40, 128, 24, SSD1306_BLACK);
  display.setTextSize(1);
  display.setCursor(0, 44);
  display.printf("count: %d", n++);
  display.display();
  delay(1000);
}
"#,
        circuit: r#"components:
  - {id: oled, type: ssd1306, address: 0x3C, x: 520, y: 120}
connections:
  - [esp32.D21, oled.SDA]
  - [esp32.D22, oled.SCL]
  - [oled.VCC, 3V3]
  - [oled.GND, GND]
test:
  - expect_serial: "OLED ready"
"#,
    },
    Template {
        name: "lcd",
        description: "16x2 LCD with I2C backpack (LiquidCrystal_I2C)",
        lib_deps: &["marcoschwartz/LiquidCrystal_I2C@^1.1.4"],
        code: r#"#include <Arduino.h>
#include <Wire.h>
#include <LiquidCrystal_I2C.h>

LiquidCrystal_I2C lcd(0x27, 16, 2);

void setup() {
  Serial.begin(115200);
  lcd.init();
  lcd.backlight();
  lcd.setCursor(0, 0);
  lcd.print("Hello, ESP32!");
  Serial.println("LCD ready");
}

void loop() {
  lcd.setCursor(0, 1);
  lcd.printf("Uptime %lus   ", millis() / 1000);
  delay(500);
}
"#,
        circuit: r#"components:
  - {id: lcd, type: lcd1602-i2c, address: 0x27, x: 520, y: 120}
connections:
  - [esp32.D21, lcd.SDA]
  - [esp32.D22, lcd.SCL]
  - [lcd.VCC, 5V]
  - [lcd.GND, GND]
test:
  - expect_serial: "LCD ready"
  - wait: 200ms
  - expect_text: {id: lcd, contains: "Hello, ESP32!"}
"#,
    },
    Template {
        name: "keypad",
        description: "4x4 membrane keypad (Keypad library)",
        lib_deps: &["chris--a/Keypad@^3.1.1"],
        code: r#"#include <Arduino.h>
#include <Keypad.h>

const byte ROWS = 4, COLS = 4;
char keys[ROWS][COLS] = {
  {'1','2','3','A'}, {'4','5','6','B'}, {'7','8','9','C'}, {'*','0','#','D'}
};
byte rowPins[ROWS] = {13, 12, 14, 27};
byte colPins[COLS] = {26, 25, 33, 32};
Keypad keypad = Keypad(makeKeymap(keys), rowPins, colPins, ROWS, COLS);

void setup() {
  Serial.begin(115200);
  Serial.println("Type on the keypad");
}

void loop() {
  char k = keypad.getKey();
  if (k) Serial.printf("Key: %c\n", k);
}
"#,
        circuit: r#"layout:
  board: {x: 360, y: 40}
components:
  - {id: keypad, type: keypad-4x4, x: 60, y: 140}
connections:
  - [esp32.D13, keypad.R1]
  - [esp32.D12, keypad.R2]
  - [esp32.D14, keypad.R3]
  - [esp32.D27, keypad.R4]
  - [esp32.D26, keypad.C1]
  - [esp32.D25, keypad.C2]
  - [esp32.D33, keypad.C3]
  - [esp32.D32, keypad.C4]
test:
  - expect_serial: "Type on the keypad"
  - input: {id: keypad, type: "5#"}
  - expect_serial: "Key: 5"
  - expect_serial: "Key: #"
"#,
    },
    Template {
        name: "neopixel",
        description: "WS2812 LED strip rainbow (Adafruit NeoPixel)",
        lib_deps: &["adafruit/Adafruit NeoPixel@^1.11.0"],
        code: r#"#include <Arduino.h>
#include <Adafruit_NeoPixel.h>

Adafruit_NeoPixel strip(12, 5, NEO_GRB + NEO_KHZ800);

void setup() {
  Serial.begin(115200);
  strip.begin();
  strip.setBrightness(80);
  Serial.println("NeoPixel ready");
}

void loop() {
  static uint16_t hue = 0;
  for (int i = 0; i < strip.numPixels(); i++) {
    strip.setPixelColor(i, strip.gamma32(strip.ColorHSV(hue + i * 65536L / strip.numPixels())));
  }
  strip.show();
  hue += 512;
  delay(20);
}
"#,
        circuit: r#"components:
  - {id: ring, type: ws2812, count: 12, layout: ring, x: 520, y: 120}
connections:
  - [esp32.D5, ring.DIN]
  - [ring.VCC, 5V]
  - [ring.GND, GND]
test:
  - expect_serial: "NeoPixel ready"
"#,
    },
    Template {
        name: "sensors",
        description: "MPU6050 + BMP280 + potentiometer on the ADC",
        lib_deps: &["adafruit/Adafruit MPU6050@^2.2.4", "adafruit/Adafruit BMP280 Library@^2.6.8", "adafruit/Adafruit Unified Sensor@^1.1.9"],
        code: r#"#include <Arduino.h>
#include <Wire.h>
#include <Adafruit_MPU6050.h>
#include <Adafruit_BMP280.h>

Adafruit_MPU6050 mpu;
Adafruit_BMP280 bmp;

void setup() {
  Serial.begin(115200);
  if (!mpu.begin()) Serial.println("MPU6050 not found");
  if (!bmp.begin(0x76)) Serial.println("BMP280 not found");
  Serial.println("Sensors ready");
}

void loop() {
  sensors_event_t a, g, t;
  mpu.getEvent(&a, &g, &t);
  Serial.printf("accel %.2f %.2f %.2f m/s2 | %.1f C %.1f hPa | pot %d\n",
      a.acceleration.x, a.acceleration.y, a.acceleration.z,
      bmp.readTemperature(), bmp.readPressure() / 100.0, analogRead(34));
  delay(1000);
}
"#,
        circuit: r#"components:
  - {id: imu, type: mpu6050, x: 520, y: 60}
  - {id: baro, type: bmp280, address: 0x76, x: 520, y: 220}
  - {id: pot, type: potentiometer, value: 10k, x: 400, y: 320}
connections:
  - [esp32.D21, imu.SDA, baro.SDA]
  - [esp32.D22, imu.SCL, baro.SCL]
  - [imu.VCC, 3V3]
  - [imu.GND, GND]
  - [baro.VCC, 3V3]
  - [baro.GND, GND]
  - [pot.1, GND]
  - [pot.2, 3V3]
  - [pot.W, esp32.D34]
test:
  - expect_serial: "Sensors ready"
"#,
    },
    Template {
        name: "nfc",
        description: "PN532 NFC reader over I2C (Adafruit PN532)",
        lib_deps: &["adafruit/Adafruit PN532@^1.3.1"],
        code: r#"#include <Arduino.h>
#include <Wire.h>
#include <Adafruit_PN532.h>

Adafruit_PN532 nfc(-1, -1);  // I2C, no IRQ/RESET pins

void setup() {
  Serial.begin(115200);
  nfc.begin();
  uint32_t v = nfc.getFirmwareVersion();
  if (!v) { Serial.println("PN532 not found"); while (1) delay(10); }
  Serial.printf("PN5%02x firmware %d.%d\n", (v >> 24) & 0xFF, (v >> 16) & 0xFF, (v >> 8) & 0xFF);
  nfc.SAMConfig();
  Serial.println("Waiting for a card...");
}

void loop() {
  uint8_t uid[7]; uint8_t len;
  if (nfc.readPassiveTargetID(PN532_MIFARE_ISO14443A, uid, &len, 1000)) {
    Serial.print("Card UID:");
    for (int i = 0; i < len; i++) Serial.printf(" %02X", uid[i]);
    Serial.println();
    delay(1000);
  }
}
"#,
        circuit: r#"components:
  - {id: nfc, type: pn532, x: 520, y: 120}
connections:
  - [esp32.D21, nfc.SDA]
  - [esp32.D22, nfc.SCL]
  - [nfc.VCC, 3V3]
  - [nfc.GND, GND]
test:
  - expect_serial: "Waiting for a card"
  - input: {id: nfc, action: tap, card: mifare, uid: "DE:AD:BE:EF"}
  - expect_serial: "Card UID: DE AD BE EF"
"#,
    },
    Template {
        name: "rfid",
        description: "MFRC522 RFID door lock: known card lights the LED (MFRC522 library)",
        lib_deps: &["miguelbalboa/MFRC522@^1.4.11"],
        code: r#"#include <Arduino.h>
#include <SPI.h>
#include <MFRC522.h>

const int SS_PIN = 5;     // RC522 "SDA"
const int RST_PIN = 27;
const int LED_OK = 2;     // on-board LED
const byte ALLOWED[4] = {0xDE, 0xAD, 0xBE, 0xEF};

MFRC522 rfid(SS_PIN, RST_PIN);

void setup() {
  Serial.begin(115200);
  pinMode(LED_OK, OUTPUT);
  SPI.begin();            // SCK 18, MISO 19, MOSI 23
  rfid.PCD_Init();
  Serial.println("Scan a card");
}

void loop() {
  if (!rfid.PICC_IsNewCardPresent() || !rfid.PICC_ReadCardSerial()) return;
  Serial.print("UID:");
  for (byte i = 0; i < rfid.uid.size; i++) Serial.printf(" %02X", rfid.uid.uidByte[i]);
  Serial.println();
  bool ok = rfid.uid.size == 4 && memcmp(rfid.uid.uidByte, ALLOWED, 4) == 0;
  Serial.println(ok ? "Access granted" : "Access denied");
  digitalWrite(LED_OK, ok ? HIGH : LOW);
  rfid.PICC_HaltA();
  delay(1500);
  digitalWrite(LED_OK, LOW);
}
"#,
        circuit: r#"components:
  - {id: rfid, type: mfrc522, x: 520, y: 120}
connections:
  - [esp32.D5, rfid.SDA]
  - [esp32.D18, rfid.SCK]
  - [esp32.D23, rfid.MOSI]
  - [esp32.D19, rfid.MISO]
  - [esp32.D27, rfid.RST]
  - [rfid.3.3V, 3V3]
  - [rfid.GND, GND]
test:
  - expect_serial: "Scan a card"
  - input: {id: rfid, action: tap, card: mifare, uid: "DE:AD:BE:EF"}
  - expect_serial: "Access granted"
  - input: {id: rfid, action: remove}
  - wait: 2s
  - input: {id: rfid, action: tap, card: mifare, uid: "01:02:03:04"}
  - expect_serial: "Access denied"
"#,
    },
    Template {
        name: "sdlogger",
        description: "Data logger: DHT22 readings appended to a CSV on a microSD card",
        lib_deps: &["adafruit/DHT sensor library@^1.4.6", "adafruit/Adafruit Unified Sensor@^1.1.14"],
        code: r#"#include <Arduino.h>
#include <SPI.h>
#include <SD.h>
#include <DHT.h>

const int SD_CS = 5;      // SCK 18, MISO 19, MOSI 23
DHT dht(27, DHT22);

void setup() {
  Serial.begin(115200);
  dht.begin();
  if (!SD.begin(SD_CS)) {
    Serial.println("SD card mount failed");
    while (true) delay(1000);
  }
  Serial.printf("SD card: %u MB\n", (unsigned)(SD.cardSize() / (1024 * 1024)));
  File f = SD.open("/log.csv", FILE_APPEND);
  if (f) {
    if (f.size() == 0) f.println("millis,temp_c,humidity");
    f.close();
  }
}

void loop() {
  float t = dht.readTemperature();
  float h = dht.readHumidity();
  File f = SD.open("/log.csv", FILE_APPEND);
  if (f) {
    f.printf("%lu,%.1f,%.1f\n", millis(), t, h);
    f.close();
    Serial.printf("logged %.1f C %.1f %%\n", t, h);
  } else {
    Serial.println("cannot open /log.csv");
  }
  delay(2000);
}
"#,
        circuit: r#"components:
  - {id: sd, type: sd-card, size_mb: 64, x: 520, y: 100}
  - {id: dht, type: dht22, temperature: 22, humidity: 55, x: 520, y: 260}
connections:
  - [esp32.D5, sd.CS]
  - [esp32.D18, sd.SCK]
  - [esp32.D23, sd.MOSI]
  - [esp32.D19, sd.MISO]
  - [sd.VCC, 3V3]
  - [sd.GND, GND]
  - [esp32.D27, dht.SDA]
  - [dht.VCC, 3V3]
  - [dht.GND, GND]
test:
  - expect_serial: "SD card:"
  - expect_serial: "logged 22.0 C 55.0 %"
  - expect: {id: sd, contains: "log.csv", within: 3s}
"#,
    },
    Template {
        name: "gps",
        description: "GPS tracker: NEO-6M on Serial2 parsed with TinyGPSPlus",
        lib_deps: &["mikalhart/TinyGPSPlus@^1.0.3"],
        code: r#"#include <Arduino.h>
#include <TinyGPSPlus.h>

TinyGPSPlus gps;
HardwareSerial GPSSerial(2);

void setup() {
  Serial.begin(115200);
  GPSSerial.begin(9600, SERIAL_8N1, 16, 17);  // RX = GPIO16 (GPS TX), TX = GPIO17
  Serial.println("Waiting for GPS fix...");
}

void loop() {
  while (GPSSerial.available()) gps.encode(GPSSerial.read());
  static unsigned long last = 0;
  if (millis() - last > 2000) {
    last = millis();
    if (gps.location.isValid()) {
      Serial.printf("lat %.5f lon %.5f alt %.1f m sats %u\n", gps.location.lat(), gps.location.lng(),
                    gps.altitude.meters(), (unsigned)gps.satellites.value());
    } else {
      Serial.println("no fix yet");
    }
  }
}
"#,
        circuit: r#"components:
  - {id: gps, type: gps-neo6m, lat: 40.4168, lon: -3.7038, alt: 650, x: 520, y: 140}
connections:
  - [esp32.D16, gps.TX]
  - [esp32.D17, gps.RX]
  - [gps.VCC, 3V3]
  - [gps.GND, GND]
test:
  - expect_serial: "lat 40.41680 lon -3.70380"
"#,
    },
];

pub fn create(dir: &Path, template: &str) -> Result<PathBuf, String> {
    let t = TEMPLATES.iter().find(|t| t.name == template).ok_or_else(|| {
        format!("unknown template '{}'; available: {}", template, TEMPLATES.iter().map(|t| t.name).collect::<Vec<_>>().join(", "))
    })?;
    if dir.join(PROJECT_FILE).exists() {
        return Err(format!("{} already contains an esp32sim project", dir.display()));
    }
    std::fs::create_dir_all(dir.join("src")).map_err(|e| e.to_string())?;
    let name = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
    let mut ini = String::from("[env:esp32dev]\nplatform = espressif32\nboard = esp32dev\nframework = arduino\nmonitor_speed = 115200\nbuild_flags = -g\n");
    if !t.lib_deps.is_empty() {
        ini.push_str("lib_deps =\n");
        for l in t.lib_deps {
            ini.push_str(&format!("  {}\n", l));
        }
    }
    write_new(&dir.join("platformio.ini"), &ini)?;
    write_new(&dir.join("src").join("main.cpp"), t.code)?;
    let yaml = format!(
        "# esp32sim project: circuit + firmware + tests\nname: {}\nboard: esp32-devkit-v1\nsource: .\n# firmware defaults to the newest .pio/build/*/firmware.bin\nspeed: 1.0\n{}",
        name, t.circuit
    );
    let p = dir.join(PROJECT_FILE);
    write_new(&p, &yaml)?;
    write_new(&dir.join(".gitignore"), ".pio/\n.esp32sim/\n")?;
    Ok(p)
}

fn write_new(p: &Path, content: &str) -> Result<(), String> {
    if p.exists() {
        return Ok(());
    }
    std::fs::write(p, content).map_err(|e| format!("{}: {}", p.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every template's circuit parses and wires up, and its test steps are well-formed.
    #[test]
    fn templates_build() {
        for t in TEMPLATES {
            let cfg = devices::ProjectConfig::parse(t.circuit).unwrap_or_else(|e| panic!("{}: {}", t.name, e));
            cfg.build_board().unwrap_or_else(|e| panic!("{}: {}", t.name, e));
            assert!(!cfg.test.is_empty(), "{} has no test scenario", t.name);
            assert!(t.code.contains("void setup()") && t.code.contains("void loop()"), "{}", t.name);
        }
    }
}

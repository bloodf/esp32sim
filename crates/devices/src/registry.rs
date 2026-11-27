//! Device factory and catalog.

use crate::parts::{basic::*, display::*, i2c_io::*, i2c_sensors::SensorModule, keypad::Keypad, logic::Hc595, motion::*, onewire::Ds18b20, pn532::Pn532, rfid::Mfrc522, sdcard::SdCard, serial::{Gps, Hc05}, timed::*, ws2812::Ws2812, Props};
use crate::Device;
use serde_json::{json, Value};

pub struct CatalogEntry {
    pub kind: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub props: &'static str,
}

macro_rules! e {
    ($k:expr, $n:expr, $c:expr, $d:expr, $p:expr) => {
        CatalogEntry { kind: $k, name: $n, category: $c, description: $d, props: $p }
    };
}

/// All supported parts. `props` is a JSON object with default properties (shown in the editor).
pub const CATALOG: &[CatalogEntry] = &[
    e!("led", "LED", "Output", "Light emitting diode. Brightness follows the current; burns out without a resistor.", r#"{"color":"red"}"#),
    e!("rgb-led", "RGB LED", "Output", "4-pin RGB LED (common cathode or anode).", r#"{"common":"cathode"}"#),
    e!("resistor", "Resistor", "Passive", "Fixed resistor.", r#"{"value":"220"}"#),
    e!("potentiometer", "Potentiometer", "Input", "Rotary potentiometer (1 - W - 2).", r#"{"value":"10k","position":0.5}"#),
    e!("capacitor", "Capacitor", "Passive", "Capacitor (open circuit in DC analysis).", r#"{"value":"100n"}"#),
    e!("pushbutton", "Push button", "Input", "Momentary tactile switch. Optional contact bounce.", r#"{"bounce":0}"#),
    e!("switch", "Switch (SPST)", "Input", "On/off toggle switch.", r#"{"on":false}"#),
    e!("slide-switch", "Slide switch (SPDT)", "Input", "Two-position slide switch, common pin C.", r#"{"on":false}"#),
    e!("keypad-4x4", "Keypad 4x4", "Input", "16-key membrane matrix keypad (R1-R4, C1-C4).", r#"{}"#),
    e!("keypad-4x3", "Keypad 4x3", "Input", "12-key matrix keypad (R1-R4, C1-C3).", r#"{}"#),
    e!("joystick", "Analog joystick", "Input", "Two-axis joystick with push button.", r#"{}"#),
    e!("rotary-encoder", "Rotary encoder (KY-040)", "Input", "Quadrature encoder with push switch.", r#"{}"#),
    e!("ldr", "Photoresistor (LDR)", "Sensor", "Light dependent resistor.", r#"{"lux":300}"#),
    e!("ntc", "NTC thermistor", "Sensor", "10k B3950 thermistor.", r#"{"temperature":25}"#),
    e!("pir", "PIR motion sensor", "Sensor", "HC-SR501 style motion detector (digital output).", r#"{"hold_ms":3000}"#),
    e!("ir-obstacle", "IR obstacle sensor", "Sensor", "Digital obstacle sensor (active low).", r#"{}"#),
    e!("touch-pad", "Touch pad (electrode)", "Input", "Metal pad on an ESP32 touch pin (T0-T9, e.g. GPIO4): touchRead() drops from ~80 to ~15 while touched.", r#"{}"#),
    e!("touch-sensor", "Touch sensor (TTP223)", "Sensor", "Capacitive touch button module, OUT high while touched.", r#"{}"#),
    e!("hall-sensor", "Hall sensor (KY-003)", "Sensor", "Magnetic switch, OUT low near a magnet.", r#"{}"#),
    e!("vibration-sensor", "Vibration sensor (SW-420)", "Sensor", "Vibration switch module, OUT high while shaken.", r#"{}"#),
    e!("tilt-sensor", "Tilt switch (SW-520D)", "Sensor", "Ball tilt switch module, OUT low when tilted.", r#"{}"#),
    e!("soil-moisture", "Soil moisture sensor", "Sensor", "Probe + LM393 board: AO drops as the soil gets wetter, DO low above the threshold.", r#"{"moisture":0.3,"threshold":0.5}"#),
    e!("mq2", "MQ-2 gas sensor", "Sensor", "Smoke / LPG sensor module: AO rises with gas, DO low above the threshold.", r#"{"gas":0.1,"threshold":0.5}"#),
    e!("sound-sensor", "Sound sensor (KY-038)", "Sensor", "Microphone module: AO follows loudness, DO high when loud.", r#"{"sound":0.1,"threshold":0.5}"#),
    e!("flame-sensor", "Flame sensor", "Sensor", "IR flame detector: AO drops near a flame, DO low when detected.", r#"{"flame":0,"threshold":0.5}"#),
    e!("rain-sensor", "Rain sensor", "Sensor", "Rain plate + LM393: AO drops when wet, DO low above the threshold.", r#"{"rain":0,"threshold":0.5}"#),
    e!("water-level", "Water level sensor", "Sensor", "Analog water level strip (S rises with the water level).", r#"{"level":0.2}"#),
    e!("signal", "Signal source", "Input", "Adjustable voltage / logic source for testing.", r#"{"volts":0}"#),
    e!("buzzer", "Passive buzzer", "Output", "Piezo buzzer; plays the PWM frequency (tone()).", r#"{}"#),
    e!("buzzer-active", "Active buzzer", "Output", "Beeps when powered.", r#"{}"#),
    e!("relay", "Relay module", "Output", "GPIO driven relay board (1-8 channels).", r#"{"channels":1,"active_low":false}"#),
    e!("servo", "Servo (SG90)", "Output", "Hobby servo driven by 50 Hz PWM.", r#"{}"#),
    e!("ws2812", "WS2812 / NeoPixel", "Output", "Addressable RGB LEDs (strip, ring or matrix).", r#"{"count":8,"layout":"strip"}"#),
    e!("7segment", "7-segment display", "Display", "LED 7-segment display, 1-4 digits (multiplexing supported).", r#"{"digits":1,"common":"cathode"}"#),
    e!("tm1637", "TM1637 4-digit display", "Display", "2-wire 7-segment display module.", r#"{"digits":4}"#),
    e!("max7219", "MAX7219 LED matrix", "Display", "8x8 LED matrix / 7-seg driver (SPI), chainable.", r#"{"chips":1}"#),
    e!("ssd1306", "OLED SSD1306 (I2C)", "Display", "128x64 / 128x32 monochrome OLED.", r#"{"address":"0x3C","width":128,"height":64}"#),
    e!("sh1106", "OLED SH1106 (I2C)", "Display", "1.3\" 128x64 OLED.", r#"{"address":"0x3C"}"#),
    e!("ssd1306-spi", "OLED SSD1306 (SPI)", "Display", "128x64 OLED with 4-wire SPI.", r#"{"width":128,"height":64}"#),
    e!("lcd1602-i2c", "LCD 16x2 (I2C)", "Display", "HD44780 character LCD with PCF8574 backpack.", r#"{"address":"0x27"}"#),
    e!("lcd2004-i2c", "LCD 20x4 (I2C)", "Display", "20x4 character LCD with PCF8574 backpack.", r#"{"address":"0x27"}"#),
    e!("lcd-parallel", "LCD 16x2 (parallel)", "Display", "HD44780 LCD wired directly (4-bit or 8-bit).", r#"{"cols":16,"rows":2}"#),
    e!("ili9341", "TFT ILI9341 (SPI)", "Display", "2.4\"/2.8\" 240x320 colour TFT.", r#"{}"#),
    e!("st7735", "TFT ST7735 (SPI)", "Display", "1.8\" 128x160 colour TFT.", r#"{}"#),
    e!("st7789", "TFT ST7789 (SPI)", "Display", "240x240 colour IPS TFT.", r#"{}"#),
    e!("pcf8574", "PCF8574 I/O expander", "I2C", "8-bit quasi-bidirectional I/O (0x20-0x27).", r#"{"address":"0x20"}"#),
    e!("pcf8574a", "PCF8574A I/O expander", "I2C", "8-bit I/O expander (0x38-0x3F).", r#"{"address":"0x38"}"#),
    e!("pcf8575", "PCF8575 I/O expander", "I2C", "16-bit quasi-bidirectional I/O.", r#"{"address":"0x20"}"#),
    e!("mcp23017", "MCP23017 I/O expander", "I2C", "16-bit I/O expander with interrupts.", r#"{"address":"0x20"}"#),
    e!("i2c-relay", "I2C relay board", "I2C", "PCF8574 based 4/8 channel relay module.", r#"{"address":"0x20","channels":4,"active_low":true}"#),
    e!("ads1115", "ADS1115 16-bit ADC", "I2C", "4-channel precision ADC with PGA.", r#"{"address":"0x48"}"#),
    e!("ads1015", "ADS1015 12-bit ADC", "I2C", "4-channel ADC with PGA.", r#"{"address":"0x48"}"#),
    e!("mcp3421", "MCP3421 18-bit ADC", "I2C", "Delta-sigma ADC, differential input.", r#"{"address":"0x68"}"#),
    e!("at24c", "AT24C EEPROM", "I2C", "Serial EEPROM (size in kbit).", r#"{"address":"0x50","kbit":32}"#),
    e!("mpu6050", "MPU6050 (GY-521)", "Sensor", "6-axis accelerometer + gyroscope.", r#"{"address":"0x68"}"#),
    e!("mpu9250", "MPU9250 9DOF", "Sensor", "9-axis IMU (accel, gyro, AK8963 magnetometer).", r#"{"address":"0x68"}"#),
    e!("adxl345", "ADXL345 accelerometer", "Sensor", "3-axis digital accelerometer.", r#"{"address":"0x53"}"#),
    e!("hmc5883l", "HMC5883L compass", "Sensor", "3-axis magnetometer (GY-271).", r#"{}"#),
    e!("qmc5883l", "QMC5883L compass", "Sensor", "3-axis magnetometer (GY-271 clone).", r#"{}"#),
    e!("gy-86", "GY-86 10DOF flight controller", "Sensor", "MPU6050 + HMC5883L + MS5611 barometer.", r#"{}"#),
    e!("gy-87", "GY-87 10DOF", "Sensor", "MPU6050 + HMC5883L + BMP180.", r#"{}"#),
    e!("gy-91", "GY-91 10DOF", "Sensor", "MPU9250 + BMP280.", r#"{}"#),
    e!("ms5611", "MS5611 barometer", "Sensor", "High resolution pressure sensor.", r#"{"address":"0x77"}"#),
    e!("bmp280", "BMP280 barometer", "Sensor", "Pressure + temperature.", r#"{"address":"0x76"}"#),
    e!("bme280", "BME280", "Sensor", "Pressure + temperature + humidity.", r#"{"address":"0x76"}"#),
    e!("bmp180", "BMP180 barometer", "Sensor", "Pressure + temperature.", r#"{}"#),
    e!("dht11", "DHT11", "Sensor", "Temperature / humidity (single-wire protocol).", r#"{"temperature":24,"humidity":45}"#),
    e!("dht22", "DHT22 / AM2302", "Sensor", "Temperature / humidity (single-wire protocol).", r#"{"temperature":24,"humidity":45}"#),
    e!("ir-receiver", "IR receiver + remote", "Input", "VS1838B 38 kHz receiver with a virtual 21-key remote (NEC codes).", r#"{}"#),
    e!("hc-sr04", "HC-SR04 ultrasonic", "Sensor", "Distance sensor (trigger / echo).", r#"{"distance":100}"#),
    e!("bh1750", "BH1750 light sensor", "Sensor", "Digital ambient light sensor.", r#"{"address":"0x23"}"#),
    e!("sht31", "SHT31", "Sensor", "Temperature / humidity.", r#"{"address":"0x44"}"#),
    e!("aht20", "AHT20", "Sensor", "Temperature / humidity.", r#"{}"#),
    e!("ds3231", "DS3231 RTC", "Time", "Precision real time clock.", r#"{}"#),
    e!("ds1307", "DS1307 RTC", "Time", "Real time clock.", r#"{}"#),
    e!("pn532", "PN532 NFC reader", "RFID/NFC", "NFC reader/writer (I2C). Tap virtual MIFARE/NTAG cards.", r#"{"address":"0x24"}"#),
    e!("gps-neo6m", "GPS module (NEO-6M)", "Sensor", "NMEA GPS on UART (9600 baud): RMC/GGA/VTG/GSA/GLL once per second. Set the position in the inspector.", r#"{"lat":40.4168,"lon":-3.7038,"alt":650}"#),
    e!("hc-05", "HC-05 Bluetooth serial", "Communication", "Bluetooth SPP bridge on UART: AT commands until a (virtual) phone connects, then a transparent link to a phone terminal in the inspector.", r#"{"name":"HC-05"}"#),
    e!("sd-card", "microSD card module", "Storage", "SPI microSD card (SDHC, FAT32 image in .esp32sim/). Files written by the firmware are listed in the inspector.", r#"{"size_mb":64}"#),
    e!("mfrc522", "MFRC522 RFID reader (RC522)", "RFID/NFC", "13.56 MHz RFID reader on SPI. Tap virtual MIFARE Classic / NTAG cards.", r#"{}"#),
    e!("ds18b20", "DS18B20 temperature", "Sensor", "1-Wire digital thermometer (needs a 4.7k pull-up on DQ). Several can share one pin.", r#"{"temperature":22.5}"#),
    e!("hx711", "HX711 + load cell", "Sensor", "24-bit load cell amplifier (scale). Set the weight in grams.", r#"{"weight":0,"scale":420,"offset":8400}"#),
    e!("ina219", "INA219 current sensor", "I2C", "High-side current / power monitor with 0.1 Ω shunt between VIN+ and VIN-.", r#"{"address":"0x40"}"#),
    e!("l298n", "L298N motor driver", "Output", "Dual H-bridge driving two DC motors (ENA/IN1/IN2, ENB/IN3/IN4; PWM on EN sets speed).", r#"{"rpm":200}"#),
    e!("a4988", "A4988 stepper driver", "Output", "STEP/DIR stepper driver with a NEMA17 motor (200 steps/rev).", r#"{"microsteps":1}"#),
    e!("uln2003", "28BYJ-48 + ULN2003", "Output", "Geared 5 V stepper on a ULN2003 board (4096 half-steps/rev).", r#"{}"#),
    e!("74hc595", "74HC595 shift register", "Logic", "8-bit serial-in parallel-out shift register.", r#"{}"#),
];

pub fn create(kind: &str, p: &Props) -> Result<Box<dyn Device>, String> {
    let k = kind.to_ascii_lowercase().replace('_', "-");
    let d: Box<dyn Device> = match k.as_str() {
        "led" => Box::new(Led::new(p)),
        "rgb-led" | "rgbled" => Box::new(RgbLed::new(p)),
        "resistor" => Box::new(Resistor::new(p)),
        "potentiometer" | "pot" => Box::new(Potentiometer::new(p)),
        "capacitor" => Box::new(Capacitor::new(p)),
        "pushbutton" | "button" | "push-button" => Box::new(PushButton::new(p)),
        "switch" | "spst" => Box::new(Switch::new(p, false)),
        "slide-switch" | "spdt" => Box::new(Switch::new(p, true)),
        "keypad-4x4" | "keypad4x4" | "keypad" => Box::new(Keypad::new(p, 4)),
        "keypad-4x3" | "keypad4x3" => Box::new(Keypad::new(p, 3)),
        "joystick" => Box::new(Joystick::new(p)),
        "rotary-encoder" | "encoder" | "ky-040" => Box::new(Encoder::new(p)),
        "ldr" | "photoresistor" => Box::new(Ldr::new(p)),
        "ntc" | "thermistor" => Box::new(Ntc::new(p)),
        "pir" => Box::new(DigitalSensor::new(p, "pir", true, 3000)),
        "ir-obstacle" => Box::new(DigitalSensor::new(p, "ir-obstacle", false, 500)),
        "touch-pad" | "touch-electrode" => Box::new(TouchPad::new(p)),
        "touch-sensor" | "ttp223" => Box::new(DigitalSensor::new(p, "touch-sensor", true, 300)),
        "hall-sensor" | "ky-003" => Box::new(DigitalSensor::new(p, "hall-sensor", false, 500)),
        "vibration-sensor" | "sw-420" => Box::new(DigitalSensor::new(p, "vibration-sensor", true, 200)),
        "tilt-sensor" | "sw-520d" => Box::new(DigitalSensor::new(p, "tilt-sensor", false, 500)),
        "soil-moisture" => Box::new(AnalogModule::new(p, "soil-moisture")),
        "mq2" | "gas-sensor" | "mq-2" => Box::new(AnalogModule::new(p, "mq2")),
        "sound-sensor" | "ky-038" => Box::new(AnalogModule::new(p, "sound-sensor")),
        "flame-sensor" => Box::new(AnalogModule::new(p, "flame-sensor")),
        "rain-sensor" => Box::new(AnalogModule::new(p, "rain-sensor")),
        "water-level" => Box::new(AnalogModule::new(p, "water-level")),
        "signal" => Box::new(SignalSource::new(p)),
        "buzzer" => Box::new(Buzzer::new(p, false)),
        "buzzer-active" => Box::new(Buzzer::new(p, true)),
        "relay" | "relay-module" => Box::new(RelayModule::new(p)),
        "servo" => Box::new(Servo::new(p)),
        "ws2812" | "neopixel" | "ws2812b" => Box::new(Ws2812::new(p)),
        "7segment" | "seven-segment" => Box::new(SevenSeg::new(p)),
        "tm1637" => Box::new(Tm1637::new(p)),
        "max7219" => Box::new(Max7219::new(p)),
        "ssd1306" => Box::new(Oled::new(p, false, false)),
        "sh1106" => Box::new(Oled::new(p, true, false)),
        "ssd1306-spi" => Box::new(Oled::new(p, false, true)),
        "sh1106-spi" => Box::new(Oled::new(p, true, true)),
        "lcd1602-i2c" | "lcd-i2c" | "lcd1602" => Box::new(LcdI2c::new(p, 2)),
        "lcd2004-i2c" | "lcd2004" => Box::new(LcdI2c::new(p, 4)),
        "lcd-parallel" => Box::new(LcdParallel::new(p)),
        "ili9341" => Box::new(Tft::new(p, "ili9341")),
        "st7735" => Box::new(Tft::new(p, "st7735")),
        "st7789" => Box::new(Tft::new(p, "st7789")),
        "pcf8574" => Box::new(Pcf857x::new(p, 8, false)),
        "pcf8574a" => Box::new(Pcf857x::new(p, 8, true)),
        "pcf8574t" => Box::new(Pcf857x::new(p, 8, false)),
        "pcf8575" => Box::new(Pcf857x::new(p, 16, false)),
        "mcp23017" => Box::new(Mcp23017::new(p)),
        "i2c-relay" => Box::new(I2cRelay::new(p)),
        "ads1115" => Box::new(Ads1x15::new(p, false)),
        "ads1015" => Box::new(Ads1x15::new(p, true)),
        "mcp3421" => Box::new(Mcp3421::new(p)),
        "at24c" | "eeprom" | "at24c32" | "at24c256" => Box::new(At24c::new(p)),
        "dht11" => Box::new(Dht::new(p, false)),
        "dht22" | "am2302" => Box::new(Dht::new(p, true)),
        "hc-sr04" | "hcsr04" | "ultrasonic" => Box::new(HcSr04::new(p)),
        "ir-receiver" | "vs1838b" | "ir-remote" => Box::new(IrReceiver::new(p)),
        "pn532" => Box::new(Pn532::new(p)),
        "mfrc522" | "rc522" | "rfid-rc522" => Box::new(Mfrc522::new(p)),
        "sd-card" | "sdcard" | "microsd" => Box::new(SdCard::new(p)),
        "gps-neo6m" | "gps" | "neo-6m" => Box::new(Gps::new(p)),
        "hc-05" | "hc05" | "bluetooth-serial" => Box::new(Hc05::new(p)),
        "ds18b20" => {
            // distinct ROM codes per component id (stable across runs)
            let seed = p.get("__id").and_then(|v| v.as_str()).map(|s| s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))).unwrap_or(0);
            Box::new(Ds18b20::new(p, seed & 0xFFFF_FFFF_FFFF))
        }
        "hx711" | "load-cell" => Box::new(Hx711::new(p)),
        "ina219" => Box::new(Ina219::new(p)),
        "l298n" | "motor-driver" => Box::new(L298n::new(p)),
        "a4988" | "drv8825" | "stepper" => Box::new(A4988::new(p)),
        "uln2003" | "28byj-48" => Box::new(Uln2003::new(p)),
        "74hc595" | "hc595" => Box::new(Hc595::new(p)),
        other => match SensorModule::new(static_kind(other), p) {
            Some(m) => Box::new(m),
            None => return Err(format!("unknown component type '{}'", kind)),
        },
    };
    Ok(d)
}

fn static_kind(k: &str) -> &'static str {
    for e in CATALOG {
        if e.kind == k {
            return e.kind;
        }
    }
    match k {
        "mpu6500" => "mpu6500",
        "gy-521" => "mpu6050",
        "gy-271" => "hmc5883l",
        _ => "",
    }
}

/// Catalog as JSON (with pin lists) for the GUI.
pub fn catalog_json() -> Value {
    let items: Vec<Value> = CATALOG
        .iter()
        .map(|e| {
            let props: Props = serde_json::from_str(e.props).unwrap_or_default();
            let pins = create(e.kind, &props).map(|d| d.pins()).unwrap_or_default();
            json!({"type": e.kind, "name": e.name, "category": e.category, "description": e.description, "props": props, "pins": pins})
        })
        .collect();
    json!(items)
}

// ------------------------------------------------------------------ ESP32 boards
pub struct BoardDef {
    pub id: &'static str,
    pub name: &'static str,
    /// (label, gpio or -1 for power pins) in physical order: left column then right column
    pub left: &'static [(&'static str, i32)],
    pub right: &'static [(&'static str, i32)],
    pub led_gpio: Option<u8>,
}

pub const DEVKIT_V1: BoardDef = BoardDef {
    id: "esp32-devkit-v1",
    name: "ESP32 DevKit V1 (DOIT, 30 pins)",
    left: &[
        ("EN", -2), ("VP", 36), ("VN", 39), ("D34", 34), ("D35", 35), ("D32", 32), ("D33", 33), ("D25", 25), ("D26", 26), ("D27", 27),
        ("D14", 14), ("D12", 12), ("D13", 13), ("GND", -1), ("VIN", -5),
    ],
    right: &[
        ("D23", 23), ("D22", 22), ("TX0", 1), ("RX0", 3), ("D21", 21), ("D19", 19), ("D18", 18), ("D5", 5), ("TX2", 17), ("RX2", 16),
        ("D4", 4), ("D2", 2), ("D15", 15), ("GND", -1), ("3V3", -3),
    ],
    led_gpio: Some(2),
};

pub const DEVKITC: BoardDef = BoardDef {
    id: "esp32-devkitc",
    name: "ESP32-DevKitC V4 (38 pins)",
    left: &[
        ("3V3", -3), ("EN", -2), ("SVP", 36), ("SVN", 39), ("IO34", 34), ("IO35", 35), ("IO32", 32), ("IO33", 33), ("IO25", 25), ("IO26", 26),
        ("IO27", 27), ("IO14", 14), ("IO12", 12), ("GND", -1), ("IO13", 13), ("SD2", 9), ("SD3", 10), ("CMD", 11), ("5V", -5),
    ],
    right: &[
        ("GND", -1), ("IO23", 23), ("IO22", 22), ("TXD0", 1), ("RXD0", 3), ("IO21", 21), ("GND", -1), ("IO19", 19), ("IO18", 18), ("IO5", 5),
        ("IO17", 17), ("IO16", 16), ("IO4", 4), ("IO0", 0), ("IO2", 2), ("IO15", 15), ("SD1", 8), ("SD0", 7), ("CLK", 6),
    ],
    led_gpio: None,
};

pub const NODEMCU_32S: BoardDef = BoardDef {
    id: "nodemcu-32s",
    name: "NodeMCU-32S (38 pins)",
    left: &[
        ("3V3", -3), ("EN", -2), ("SVP", 36), ("SVN", 39), ("P34", 34), ("P35", 35), ("P32", 32), ("P33", 33), ("P25", 25), ("P26", 26),
        ("P27", 27), ("P14", 14), ("P12", 12), ("GND", -1), ("P13", 13), ("SD2", 9), ("SD3", 10), ("CMD", 11), ("5V", -5),
    ],
    right: &[
        ("GND", -1), ("P23", 23), ("P22", 22), ("TX", 1), ("RX", 3), ("P21", 21), ("GND", -1), ("P19", 19), ("P18", 18), ("P5", 5),
        ("P17", 17), ("P16", 16), ("P4", 4), ("P0", 0), ("P2", 2), ("P15", 15), ("SD1", 8), ("SD0", 7), ("CLK", 6),
    ],
    led_gpio: Some(2),
};

pub const BOARDS: [&BoardDef; 3] = [&DEVKIT_V1, &DEVKITC, &NODEMCU_32S];

pub fn board_def(id: &str) -> &'static BoardDef {
    match id.to_ascii_lowercase().as_str() {
        "esp32-devkitc" | "esp32-devkitc-v4" | "devkitc" | "esp32-wroom-32" => &DEVKITC,
        "nodemcu-32s" | "nodemcu32s" | "nodemcu" => &NODEMCU_32S,
        _ => &DEVKIT_V1,
    }
}

pub fn boards_json() -> Value {
    json!(BOARDS.iter().map(|b| json!({
        "id": b.id, "name": b.name,
        "left": b.left.iter().map(|(l, g)| json!({"label": l, "gpio": g})).collect::<Vec<_>>(),
        "right": b.right.iter().map(|(l, g)| json!({"label": l, "gpio": g})).collect::<Vec<_>>(),
        "led": b.led_gpio
    })).collect::<Vec<_>>())
}

/// Map an ESP32 pin label to a GPIO number.
pub fn pad_number(label: &str) -> Option<u8> {
    let s = label.trim().to_ascii_uppercase();
    let named = match s.as_str() {
        "TX" | "TX0" | "TXD0" | "U0TXD" => Some(1),
        "RX" | "RX0" | "RXD0" | "U0RXD" => Some(3),
        "TX2" | "U2TXD" => Some(17),
        "RX2" | "U2RXD" => Some(16),
        "VP" | "SVP" | "SENSOR_VP" => Some(36),
        "VN" | "SVN" | "SENSOR_VN" => Some(39),
        "SDA" => Some(21),
        "SCL" => Some(22),
        "MOSI" => Some(23),
        "MISO" => Some(19),
        "SCK" | "SCLK" => Some(18),
        "SS" | "CS" => Some(5),
        "LED" | "BUILTIN_LED" | "LED_BUILTIN" => Some(2),
        "BOOT" => Some(0),
        "DAC1" => Some(25),
        "DAC2" => Some(26),
        "CLK" => Some(6),
        "SD0" => Some(7),
        "SD1" => Some(8),
        "SD2" => Some(9),
        "SD3" => Some(10),
        "CMD" => Some(11),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let adc: &[(&str, u8)] = &[
        ("A0", 36), ("A3", 39), ("A4", 32), ("A5", 33), ("A6", 34), ("A7", 35), ("A10", 4), ("A11", 0), ("A12", 2), ("A13", 15),
        ("A14", 13), ("A15", 12), ("A16", 14), ("A17", 27), ("A18", 25), ("A19", 26), ("T0", 4), ("T1", 0), ("T2", 2), ("T3", 15),
        ("T4", 13), ("T5", 12), ("T6", 14), ("T7", 27), ("T8", 33), ("T9", 32),
    ];
    for (n, g) in adc {
        if s == *n {
            return Some(*g);
        }
    }
    let num = s.trim_start_matches("GPIO").trim_start_matches("IO").trim_start_matches('D').trim_start_matches('G').trim_start_matches('P');
    match num.parse::<u8>() {
        Ok(n) if n < 40 && !(28..=31).contains(&n) && n != 20 && n != 24 => Some(n),
        _ => None,
    }
}

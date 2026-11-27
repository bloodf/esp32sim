//! I2C modules driven register-by-register like Arduino / IDF drivers do.
//! Expected values come from the datasheets (conversion formulas, register
//! maps, CRCs), not from the implementation.
mod common;
use common::*;
use serde_json::json;

// ------------------------------------------------------------ displays

/// Pixel from the display's packed 1bpp state bitmap (row-major, MSB first).
fn oled_px(b: &devices::circuit::Board, id: &str, x: usize, y: usize) -> bool {
    let st = state(b, id);
    let w = st["width"].as_u64().unwrap() as usize;
    let hex = st["bitmap"].as_str().unwrap();
    let i = y * w + x;
    let byte = u8::from_str_radix(&hex[(i / 8) * 2..(i / 8) * 2 + 2], 16).unwrap();
    byte & (0x80 >> (i % 8)) != 0
}

#[test]
fn ssd1306_draws_pixels_from_page_data() {
    let mut b = i2c_board("oled", "{id: oled, type: ssd1306}");
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.probe(0x3C));
    assert!(!i2c.probe(0x3D));
    // Adafruit-style init: off, horizontal addressing, segment remap + COM
    // scan direction (normal orientation), full window, charge pump, on
    assert!(i2c.write(0x3C, &[0x00, 0xAE, 0x20, 0x00, 0xA1, 0xC8, 0x21, 0, 127, 0x22, 0, 7, 0x8D, 0x14, 0xAF]));
    // one column byte = 8 vertical pixels of page 0, then a 2nd column with 1 pixel
    assert!(i2c.write(0x3C, &[0x40, 0xFF, 0x01]));
    let st = state(&b, "oled");
    assert_eq!(st["on"], json!(true));
    assert_eq!((st["width"].as_u64(), st["height"].as_u64()), (Some(128), Some(64)));
    assert!(summary(&b, "oled").contains("9 pixels lit"), "{}", summary(&b, "oled"));
    assert!(oled_px(&b, "oled", 0, 0) && oled_px(&b, "oled", 0, 7) && oled_px(&b, "oled", 1, 0));
    assert!(!oled_px(&b, "oled", 1, 1));
    assert!(!oled_px(&b, "oled", 0, 8));
    // inverse video, then display off hides everything
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x3C, &[0x00, 0xA7]));
    assert!(!oled_px(&b, "oled", 0, 0) && oled_px(&b, "oled", 5, 5));
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x3C, &[0x00, 0xA6, 0xAE]));
    assert_eq!(state(&b, "oled")["on"], json!(false));
    assert!(summary(&b, "oled").contains("0 pixels lit"));
}

#[test]
fn ssd1306_without_remap_is_mirrored_like_the_real_panel() {
    let mut b = i2c_board("oled", "{id: oled, type: ssd1306}");
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x3C, &[0x00, 0x20, 0x00, 0xAF]));
    assert!(i2c.write(0x3C, &[0x40, 0x01]));
    assert!(oled_px(&b, "oled", 127, 63), "column 0 / row 0 appear at the opposite corner");
}

#[test]
fn ssd1306_contrast_and_custom_address() {
    let mut b = i2c_board("oled", "{id: oled, type: ssd1306, address: 0x3D, height: 32}");
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.probe(0x3D) && !i2c.probe(0x3C));
    assert!(i2c.write(0x3D, &[0x00, 0x81, 0x42]));
    let st = state(&b, "oled");
    assert_eq!(st["contrast"], json!(0x42));
    assert_eq!(st["height"], json!(32));
}

#[test]
fn sh1106_uses_page_addressing_with_column_offset() {
    let mut b = i2c_board("oled", "{id: oled, type: sh1106}");
    let mut i2c = I2c::new(&mut b);
    // page 0, column 2 (the first visible column of the 132-column RAM)
    assert!(i2c.write(0x3C, &[0x00, 0xA1, 0xC8, 0xAF, 0xB0, 0x02, 0x10]));
    assert!(i2c.write(0x3C, &[0x40, 0x81]));
    assert!(oled_px(&b, "oled", 0, 0) && oled_px(&b, "oled", 0, 7), "bits 0 and 7 of column 0");
    assert!(!oled_px(&b, "oled", 0, 1));
    assert!(summary(&b, "oled").contains("2 pixels lit"));
}

/// HD44780 behind a PCF8574 backpack: P0=RS P1=RW P2=E P3=backlight P4..7=D4..7
fn lcd_nibble(i2c: &mut I2c, addr: u8, nib: u8, rs: bool) {
    let base = (nib << 4) | 0x08 | rs as u8;
    assert!(i2c.write(addr, &[base | 0x04]));
    assert!(i2c.write(addr, &[base]));
}
fn lcd_byte(i2c: &mut I2c, addr: u8, v: u8, rs: bool) {
    lcd_nibble(i2c, addr, v >> 4, rs);
    lcd_nibble(i2c, addr, v & 0xF, rs);
}
fn lcd_init(i2c: &mut I2c, addr: u8) {
    for _ in 0..3 {
        lcd_nibble(i2c, addr, 0x3, false);
        i2c.wait(5 * MS);
    }
    lcd_nibble(i2c, addr, 0x2, false);
    for c in [0x28, 0x0C, 0x01, 0x06] {
        lcd_byte(i2c, addr, c, false);
        i2c.wait(2 * MS);
    }
}
fn lcd_print(i2c: &mut I2c, addr: u8, s: &str) {
    for c in s.bytes() {
        lcd_byte(i2c, addr, c, true);
    }
}

#[test]
fn lcd1602_i2c_backpack_4bit_protocol() {
    let mut b = i2c_board("lcd", "{id: lcd, type: lcd1602-i2c}");
    let mut i2c = I2c::new(&mut b);
    lcd_init(&mut i2c, 0x27);
    lcd_print(&mut i2c, 0x27, "Hello");
    lcd_byte(&mut i2c, 0x27, 0x80 | 0x40, false); // row 2
    lcd_print(&mut i2c, 0x27, "ESP32 sim");
    let st = state(&b, "lcd");
    assert_eq!(st["text"][0].as_str().unwrap().trim_end(), "Hello");
    assert_eq!(st["text"][1].as_str().unwrap().trim_end(), "ESP32 sim");
    assert_eq!(st["backlight"], json!(true));
    assert_eq!(st["on"], json!(true));
    assert_eq!(st["cursor"]["row"], json!(1));
    // clear display
    let mut i2c = I2c::new(&mut b);
    lcd_byte(&mut i2c, 0x27, 0x01, false);
    assert_eq!(state(&b, "lcd")["text"][0].as_str().unwrap().trim(), "");
}

#[test]
fn lcd2004_rows_three_and_four() {
    let mut b = i2c_board("lcd", "{id: lcd, type: lcd2004-i2c, address: 0x3F}");
    let mut i2c = I2c::new(&mut b);
    lcd_init(&mut i2c, 0x3F);
    lcd_byte(&mut i2c, 0x3F, 0x80 | 0x14, false);
    lcd_print(&mut i2c, 0x3F, "row3");
    lcd_byte(&mut i2c, 0x3F, 0x80 | 0x54, false);
    lcd_print(&mut i2c, 0x3F, "row4");
    let st = state(&b, "lcd");
    assert_eq!(st["rows"], json!(4));
    assert_eq!(st["text"][2].as_str().unwrap().trim_end(), "row3");
    assert_eq!(st["text"][3].as_str().unwrap().trim_end(), "row4");
}

#[test]
fn lcd_custom_characters_in_cgram() {
    let mut b = i2c_board("lcd", "{id: lcd, type: lcd1602-i2c}");
    let mut i2c = I2c::new(&mut b);
    lcd_init(&mut i2c, 0x27);
    lcd_byte(&mut i2c, 0x27, 0x40, false); // CGRAM address 0
    for row in [0x0A, 0x1F, 0x1F, 0x0E, 0x04, 0, 0, 0] {
        lcd_byte(&mut i2c, 0x27, row, true);
    }
    lcd_byte(&mut i2c, 0x27, 0x80, false);
    lcd_byte(&mut i2c, 0x27, 0x00, true); // custom char 0
    let st = state(&b, "lcd");
    assert!(st["cgram"].as_str().unwrap().starts_with("0a1f1f0e04"), "{}", st["cgram"]);
    assert!(st["codes"][0].as_str().unwrap().starts_with("00"));
}

// ------------------------------------------------------- I/O expanders

#[test]
fn pcf8574t_inputs_and_outputs() {
    let mut b = board_of(
        &["{id: io, type: pcf8574, address: 0x21}", "{id: btn, type: pushbutton}"],
        &["esp32.D21 io.SDA", "esp32.D22 io.SCL", "io.P3 btn.1", "btn.2 GND"],
    );
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.scan(), vec![0x21]);
    assert!(i2c.write(0x21, &[0xFF]));
    assert_eq!(i2c.read(0x21, 1).unwrap()[0], 0xFF, "quasi-bidirectional pins read high");
    b.input("btn", &json!({"action": "press"})).unwrap();
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.read(0x21, 1).unwrap()[0], 0xF7, "P3 pulled low by the button");
    assert!(i2c.write(0x21, &[0x0F]));
    assert_eq!(state(&b, "io")["output_latch"], json!(0x0F));
}

#[test]
fn pcf8574a_default_address_range() {
    let mut b = i2c_board("io", "{id: io, type: pcf8574a}");
    assert_eq!(I2c::new(&mut b).scan(), vec![0x38]);
}

#[test]
fn pcf8575_sixteen_bits() {
    let mut b = board_of(
        &["{id: io, type: pcf8575}", "{id: led, type: led}", "{id: r, type: resistor, value: 330}"],
        &["esp32.D21 io.SDA", "esp32.D22 io.SCL", "io.P10 r.1", "r.2 led.C", "led.A 3V3"],
    );
    let mut i2c = I2c::new(&mut b);
    // P0x all high, P10 low -> LED (to 3V3) turns on
    assert!(i2c.write(0x20, &[0xFF, 0xFE]));
    i2c.wait(MS);
    assert_eq!(i2c.read(0x20, 2).unwrap(), vec![0xFF, 0xFE]);
    assert!(state(&b, "led")["brightness"].as_f64().unwrap() > 0.2);
    assert_eq!(state(&b, "io")["bits"], json!(16));
}

#[test]
fn mcp23017_registers() {
    let mut b = board_of(
        &["{id: io, type: mcp23017}", "{id: led, type: led}", "{id: r, type: resistor, value: 330}", "{id: btn, type: pushbutton}"],
        &["esp32.D21 io.SDA", "esp32.D22 io.SCL", "io.GPA0 r.1", "r.2 led.A", "led.C GND", "io.GPB2 btn.1", "btn.2 GND"],
    );
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.reg8(0x20, 0x00), 0xFF, "IODIRA resets to inputs");
    assert!(i2c.write_reg(0x20, 0x00, &[0x00])); // GPA outputs
    assert!(i2c.write_reg(0x20, 0x12, &[0x01])); // GPA0 high
    assert!(i2c.write_reg(0x20, 0x0D, &[0xFF])); // GPPUB pull-ups
    i2c.wait(MS);
    assert_eq!(i2c.reg8(0x20, 0x14), 0x01, "OLATA");
    assert!(state(&b, "led")["brightness"].as_f64().unwrap() > 0.2);
    b.input("btn", &json!({"action": "press"})).unwrap();
    let mut i2c = I2c::new(&mut b);
    let gpio = i2c.read_reg(0x20, 0x12, 2); // sequential: GPIOA, GPIOB
    assert_eq!(gpio[1] & 0x04, 0, "GPB2 low while pressed: {:02x?}", gpio);
    assert_eq!(gpio[1] | 0x04, 0xFF);
}

#[test]
fn i2c_relay_board_channels() {
    let mut b = i2c_board("rel", "{id: rel, type: i2c-relay, channels: 4}");
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x20, &[0xFA])); // active low: bits 0 and 2 -> relays 1 and 3 on
    let on = state(&b, "rel")["on"].clone();
    assert_eq!(on, json!([true, false, true, false]));
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x20, &[0xFF]));
    assert_eq!(state(&b, "rel")["on"], json!([false, false, false, false]));
}

// ----------------------------------------------------------------- ADCs

fn divider(node: &str, rtop: &str, rbot: &str) -> Vec<String> {
    vec![format!("3V3 rt.1"), format!("rt.2 rb.1 {}", node), format!("rb.2 GND"), format!("# {} {}", rtop, rbot)]
}

#[test]
fn ads1115_single_shot_conversion() {
    let mut b = board_of(
        &["{id: adc, type: ads1115}", "{id: pot, type: potentiometer, value: 10k, position: 0.25}"],
        &["esp32.D21 adc.SDA", "esp32.D22 adc.SCL", "pot.1 3V3", "pot.2 GND", "pot.W adc.A0", "adc.ADDR GND"],
    );
    let _ = divider;
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.read_reg(0x48, 0x01, 2), vec![0x85, 0x83], "config reset value");
    // OS=1, AIN0 vs GND, ±4.096 V, single shot, 128 SPS
    assert!(i2c.write(0x48, &[0x01, 0xC3, 0x83]));
    i2c.wait(10 * MS);
    let cfg = i2c.read_reg(0x48, 0x01, 2);
    assert_ne!(cfg[0] & 0x80, 0, "conversion finished (OS=1)");
    let raw = be16(&i2c.read_reg(0x48, 0x00, 2)) as f64;
    let want = 2.475 / 4.096 * 32768.0;
    assert!((raw - want).abs() < 60.0, "raw {} want {}", raw, want);
    // ±2.048 V range clips at full scale
    assert!(i2c.write(0x48, &[0x01, 0xC5, 0x83]));
    i2c.wait(10 * MS);
    assert_eq!(be16(&i2c.read_reg(0x48, 0x00, 2)), 0x7FFF);
}

#[test]
fn ads1015_is_twelve_bit_left_aligned() {
    let mut b = board_of(
        &["{id: adc, type: ads1015}", "{id: pot, type: potentiometer, value: 10k, position: 0.5}"],
        &["esp32.D21 adc.SDA", "esp32.D22 adc.SCL", "pot.1 3V3", "pot.2 GND", "pot.W adc.A1"],
    );
    let mut i2c = I2c::new(&mut b);
    // AIN1 vs GND (MUX=101), ±4.096 V, single shot
    assert!(i2c.write(0x48, &[0x01, 0xD3, 0x83]));
    i2c.wait(5 * MS);
    let raw = be16(&i2c.read_reg(0x48, 0x00, 2));
    assert_eq!(raw & 0xF, 0, "low nibble is zero");
    let v = (raw >> 4) as f64 * 4.096 / 2048.0;
    assert!((v - 1.65).abs() < 0.01, "{} V", v);
}

#[test]
fn mcp3421_eighteen_bit_conversion() {
    let mut b = board_of(
        &["{id: adc, type: mcp3421}", "{id: rt, type: resistor, value: 23k}", "{id: rb, type: resistor, value: 10k}"],
        &["esp32.D21 adc.SDA", "esp32.D22 adc.SCL", "3V3 rt.1", "rt.2 rb.1 adc.VIN+", "rb.2 GND adc.VIN-"],
    );
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x68, &[0x1C])); // continuous, 18 bit, gain 1
    i2c.wait(300 * MS);
    let d = i2c.read(0x68, 4).unwrap();
    let raw = (((d[0] as i32) << 24 | (d[1] as i32) << 16 | (d[2] as i32) << 8) >> 8) as f64;
    let v = raw * 15.625e-6;
    assert!((v - 1.0).abs() < 0.002, "{} V (raw {:02x?})", v, d);
    assert_eq!(d[3] & 0x7F, 0x1C, "config echoed");
    // 16-bit mode: 62.5 µV per LSB, two data bytes
    assert!(i2c.write(0x68, &[0x18]));
    i2c.wait(100 * MS);
    let d = i2c.read(0x68, 3).unwrap();
    let v = be16(&d) as f64 * 62.5e-6;
    assert!((v - 1.0).abs() < 0.002, "{} V", v);
}

#[test]
fn ina219_measures_shunt_current_and_bus_voltage() {
    let mut b = board_of(
        &["{id: ina, type: ina219}", "{id: load, type: resistor, value: 100}"],
        &["esp32.D21 ina.SDA", "esp32.D22 ina.SCL", "5V ina.VIN+", "ina.VIN- load.1", "load.2 GND"],
    );
    b.advance(MS);
    let mut i2c = I2c::new(&mut b);
    assert_eq!(be16(&i2c.read_reg(0x40, 0x00, 2)) as u16, 0x399F, "config reset value");
    assert!(i2c.write(0x40, &[0x05, 0x10, 0x00])); // calibration 4096 (0.1 mA/bit with 0.1 Ω)
    i2c.wait(2 * MS);
    let shunt_uv = be16(&i2c.read_reg(0x40, 0x01, 2)) as f64 * 10.0;
    let bus = i2c.read_reg(0x40, 0x02, 2);
    let bus_v = ((u16::from_be_bytes([bus[0], bus[1]]) >> 3) as f64) * 0.004;
    let current_ma = be16(&i2c.read_reg(0x40, 0x04, 2)) as f64 * 0.1;
    let expect_ma = 5000.0 / 100.1;
    assert!((shunt_uv / 1000.0 - expect_ma * 0.1).abs() < 0.1, "shunt {} µV", shunt_uv);
    assert!((bus_v - 4.995).abs() < 0.02, "bus {} V", bus_v);
    assert!((current_ma - expect_ma).abs() < 0.5, "current {} mA", current_ma);
}

// --------------------------------------------------------------- EEPROM

#[test]
fn at24c32_write_then_random_and_sequential_read() {
    let mut b = i2c_board("ee", "{id: ee, type: at24c}");
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x50, &[0x01, 0x10, 1, 2, 3, 4]));
    i2c.wait(6 * MS);
    assert!(i2c.write(0x50, &[0x01, 0x10]));
    assert_eq!(i2c.read(0x50, 4).unwrap(), vec![1, 2, 3, 4]);
    // current-address read continues after the last byte
    assert!(i2c.write(0x50, &[0x01, 0x12]));
    assert_eq!(i2c.read(0x50, 3).unwrap(), vec![3, 4, 0xFF]);
}

#[test]
fn at24c_page_write_wraps_within_the_page() {
    let mut b = i2c_board("ee", "{id: ee, type: at24c}");
    let mut i2c = I2c::new(&mut b);
    // 32-byte pages: writing 4 bytes at 0x1E wraps to 0x00 / 0x01
    assert!(i2c.write(0x50, &[0x00, 0x1E, 0xA1, 0xA2, 0xA3, 0xA4]));
    i2c.wait(6 * MS);
    assert!(i2c.write(0x50, &[0x00, 0x1E]));
    assert_eq!(i2c.read(0x50, 2).unwrap(), vec![0xA1, 0xA2]);
    assert!(i2c.write(0x50, &[0x00, 0x00]));
    assert_eq!(i2c.read(0x50, 2).unwrap(), vec![0xA3, 0xA4]);
    assert!(i2c.write(0x50, &[0x00, 0x20]));
    assert_eq!(i2c.read(0x50, 1).unwrap(), vec![0xFF], "next page untouched");
}

// -------------------------------------------------------- motion sensors

#[test]
fn mpu6050_accel_gyro_temperature() {
    let mut b = i2c_board("imu", "{id: imu, type: mpu6050, temperature: 24}");
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.reg8(0x68, 0x75), 0x68, "WHO_AM_I");
    assert_eq!(i2c.reg8(0x68, 0x6B), 0x40, "sleeps after reset");
    assert!(i2c.write_reg(0x68, 0x6B, &[0x00]));
    let d = i2c.read_reg(0x68, 0x3B, 14);
    let (ax, ay, az) = (be16(&d[0..]), be16(&d[2..]), be16(&d[4..]));
    assert!(ax.abs() < 50 && ay.abs() < 50, "{} {}", ax, ay);
    assert!((az - 16384).abs() < 50, "az {}", az);
    let t = be16(&d[6..]) as f64 / 340.0 + 36.53;
    assert!((t - 24.0).abs() < 0.1, "temp {}", t);
    assert_eq!((be16(&d[8..]), be16(&d[10..]), be16(&d[12..])), (0, 0, 0));
    // rotate at 10 °/s around X and tilt 90° in roll
    b.input("imu", &json!({"gyro": {"x": 10, "y": 0, "z": -20}, "roll": 90})).unwrap();
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write_reg(0x68, 0x1C, &[0x08])); // ±4 g
    assert!(i2c.write_reg(0x68, 0x1B, &[0x00])); // ±250 °/s
    let d = i2c.read_reg(0x68, 0x3B, 14);
    assert!((be16(&d[2..]) - 8192).abs() < 50, "ay at ±4g: {}", be16(&d[2..]));
    assert!(be16(&d[4..]).abs() < 50);
    assert_eq!(be16(&d[8..]), 1310);
    assert_eq!(be16(&d[12..]), -2620);
}

#[test]
fn mpu9250_with_ak8963_magnetometer() {
    let mut b = i2c_board("imu", "{id: imu, type: mpu9250}");
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.reg8(0x68, 0x75), 0x71);
    assert!(i2c.write_reg(0x68, 0x6B, &[0x00]));
    assert!(i2c.write_reg(0x68, 0x37, &[0x02])); // bypass to the aux bus
    assert_eq!(i2c.reg8(0x0C, 0x00), 0x48, "AK8963 WIA");
    assert!(i2c.write_reg(0x0C, 0x0A, &[0x16])); // 16-bit continuous mode 2
    i2c.wait(20 * MS);
    let st1 = i2c.reg8(0x0C, 0x02);
    assert_ne!(st1 & 1, 0, "data ready");
    let m = i2c.read_reg(0x0C, 0x03, 7); // HXL..HZH + ST2
    let hx = le16(&m[0..]) as f64 * 0.15; // µT per LSB in 16-bit mode
    assert!((hx - 40.0).abs() < 1.0, "hx {} µT", hx);
}

#[test]
fn adxl345_full_resolution_axes() {
    let mut b = i2c_board("acc", "{id: acc, type: adxl345}");
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.reg8(0x53, 0x00), 0xE5, "DEVID");
    assert!(i2c.write_reg(0x53, 0x31, &[0x08])); // FULL_RES, ±2 g
    assert!(i2c.write_reg(0x53, 0x2D, &[0x08])); // measure
    let d = i2c.read_reg(0x53, 0x32, 6);
    let z = le16(&d[4..]) as f64 / 256.0;
    assert!((z - 1.0).abs() < 0.02, "z {} g", z);
    b.input("acc", &json!({"accel": {"x": -0.5, "y": 0.25, "z": 0.8}})).unwrap();
    let mut i2c = I2c::new(&mut b);
    let d = i2c.read_reg(0x53, 0x32, 6);
    assert!((le16(&d[0..]) as f64 / 256.0 + 0.5).abs() < 0.02);
    assert!((le16(&d[2..]) as f64 / 256.0 - 0.25).abs() < 0.02);
}

#[test]
fn hmc5883l_identification_and_field() {
    let mut b = i2c_board("mag", "{id: mag, type: hmc5883l}");
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.read_reg(0x1E, 0x0A, 3), b"H43".to_vec());
    assert!(i2c.write_reg(0x1E, 0x01, &[0x20])); // gain 1090 LSB/G
    assert!(i2c.write_reg(0x1E, 0x02, &[0x00])); // continuous
    i2c.wait(10 * MS);
    let d = i2c.read_reg(0x1E, 0x03, 6); // X, Z, Y (big endian)
    let (x, z, y) = (be16(&d[0..]) as f64, be16(&d[2..]) as f64, be16(&d[4..]) as f64);
    assert!((x / 1090.0 * 100.0 - 40.0).abs() < 0.5, "x {}", x);
    assert!(y.abs() < 10.0);
    assert!((z / 1090.0 * 100.0 + 30.0).abs() < 0.5, "z {}", z);
}

#[test]
fn qmc5883l_identification_and_field() {
    let mut b = i2c_board("mag", "{id: mag, type: qmc5883l}");
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.reg8(0x0D, 0x0D), 0xFF, "chip id");
    assert!(i2c.write_reg(0x0D, 0x0B, &[0x01])); // SET/RESET period
    assert!(i2c.write_reg(0x0D, 0x09, &[0x1D])); // continuous, 200 Hz, 8 G, OSR 512
    i2c.wait(10 * MS);
    let d = i2c.read_reg(0x0D, 0x00, 6); // X, Y, Z little endian
    let x = le16(&d[0..]) as f64 / 3000.0 * 100.0;
    assert!((x - 40.0).abs() < 0.5, "x {} µT", x);
}

// ---------------------------------------------------------- barometers

fn ms5611_crc4(prom: &[u16; 8]) -> u16 {
    let mut p = *prom;
    let mut rem: u32 = 0;
    p[7] &= 0xFF00;
    for cnt in 0..16 {
        rem ^= if cnt % 2 == 1 { (p[cnt >> 1] & 0xFF) as u32 } else { (p[cnt >> 1] >> 8) as u32 };
        for _ in 0..8 {
            rem = if rem & 0x8000 != 0 { (rem << 1) ^ 0x3000 } else { rem << 1 };
        }
    }
    ((rem >> 12) & 0xF) as u16
}

fn ms5611_read(i2c: &mut I2c, addr: u8) -> (f64, f64) {
    assert!(i2c.write(addr, &[0x1E]));
    i2c.wait(3 * MS);
    let mut c = [0u16; 8];
    for (k, v) in c.iter_mut().enumerate() {
        let d = i2c.read_reg(addr, 0xA0 + 2 * k as u8, 2);
        *v = u16::from_be_bytes([d[0], d[1]]);
    }
    assert_eq!(ms5611_crc4(&c), c[7] & 0xF, "PROM CRC4 {:04x?}", c);
    let adc = |i2c: &mut I2c, cmd: u8| -> i64 {
        assert!(i2c.write(addr, &[cmd]));
        i2c.wait(10 * MS);
        let d = i2c.read_reg(addr, 0x00, 3);
        (d[0] as i64) << 16 | (d[1] as i64) << 8 | d[2] as i64
    };
    let d1 = adc(i2c, 0x48);
    let d2 = adc(i2c, 0x58);
    let dt = d2 - ((c[5] as i64) << 8);
    let temp = 2000 + ((dt * c[6] as i64) >> 23);
    let off = ((c[2] as i64) << 16) + ((c[4] as i64 * dt) >> 7);
    let sens = ((c[1] as i64) << 15) + ((c[3] as i64 * dt) >> 8);
    let p = (((d1 * sens) >> 21) - off) >> 15;
    (temp as f64 / 100.0, p as f64 / 100.0)
}

#[test]
fn ms5611_prom_crc_and_compensated_values() {
    let mut b = i2c_board("baro", "{id: baro, type: ms5611, temperature: 24, pressure: 1000}");
    let mut i2c = I2c::new(&mut b);
    let (t, p) = ms5611_read(&mut i2c, 0x77);
    assert!((t - 24.0).abs() < 0.1, "T {}", t);
    assert!((p - 1000.0).abs() < 0.2, "P {} mbar", p);
}

struct Bme {
    t: [f64; 3],
    p: [f64; 9],
    h: Option<[f64; 6]>,
}

fn bmp280_calib(i2c: &mut I2c, addr: u8, humidity: bool) -> Bme {
    let c = i2c.read_reg(addr, 0x88, 24);
    let u = |i: usize| u16::from_le_bytes([c[i], c[i + 1]]) as f64;
    let s = |i: usize| i16::from_le_bytes([c[i], c[i + 1]]) as f64;
    let h = if humidity {
        let h1 = i2c.reg8(addr, 0xA1) as f64;
        let e = i2c.read_reg(addr, 0xE1, 7);
        let h2 = i16::from_le_bytes([e[0], e[1]]) as f64;
        let h3 = e[2] as f64;
        let h4 = (((e[3] as i8 as i16) << 4) | (e[4] & 0x0F) as i16) as f64;
        let h5 = (((e[5] as i8 as i16) << 4) | (e[4] >> 4) as i16) as f64;
        let h6 = e[6] as i8 as f64;
        Some([h1, h2, h3, h4, h5, h6])
    } else {
        None
    };
    Bme { t: [u(0), s(2), s(4)], p: [u(6), s(8), s(10), s(12), s(14), s(16), s(18), s(20), s(22)], h }
}

/// Bosch BMP280 / BME280 floating point compensation (datasheet 8.1).
fn bme_compensate(c: &Bme, adc_t: f64, adc_p: f64, adc_h: f64) -> (f64, f64, f64) {
    let v1 = (adc_t / 16384.0 - c.t[0] / 1024.0) * c.t[1];
    let v2 = (adc_t / 131072.0 - c.t[0] / 8192.0).powi(2) * c.t[2];
    let t_fine = v1 + v2;
    let t = t_fine / 5120.0;
    let mut v1 = t_fine / 2.0 - 64000.0;
    let mut v2 = v1 * v1 * c.p[5] / 32768.0;
    v2 += v1 * c.p[4] * 2.0;
    v2 = v2 / 4.0 + c.p[3] * 65536.0;
    v1 = (c.p[2] * v1 * v1 / 524288.0 + c.p[1] * v1) / 524288.0;
    v1 = (1.0 + v1 / 32768.0) * c.p[0];
    let mut p = 1048576.0 - adc_p;
    p = (p - v2 / 4096.0) * 6250.0 / v1;
    let v1 = c.p[8] * p * p / 2147483648.0;
    let v2 = p * c.p[7] / 32768.0;
    p += (v1 + v2 + c.p[6]) / 16.0;
    let mut h = f64::NAN;
    if let Some(hc) = c.h {
        let mut vh = t_fine - 76800.0;
        vh = (adc_h - (hc[3] * 64.0 + hc[4] / 16384.0 * vh)) * (hc[1] / 65536.0 * (1.0 + hc[5] / 67108864.0 * vh * (1.0 + hc[2] / 67108864.0 * vh)));
        vh *= 1.0 - hc[0] * vh / 524288.0;
        h = vh.clamp(0.0, 100.0);
    }
    (t, p / 100.0, h)
}

#[test]
fn bmp280_calibrated_temperature_and_pressure() {
    let mut b = i2c_board("baro", "{id: baro, type: bmp280, temperature: 21.5, pressure: 1005.3}");
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.reg8(0x76, 0xD0), 0x58, "chip id");
    let cal = bmp280_calib(&mut i2c, 0x76, false);
    assert!(i2c.write_reg(0x76, 0xF4, &[0x27])); // normal mode, x1 oversampling
    i2c.wait(50 * MS);
    let d = i2c.read_reg(0x76, 0xF7, 6);
    let adc_p = ((d[0] as u32) << 12 | (d[1] as u32) << 4 | (d[2] as u32) >> 4) as f64;
    let adc_t = ((d[3] as u32) << 12 | (d[4] as u32) << 4 | (d[5] as u32) >> 4) as f64;
    let (t, p, _) = bme_compensate(&cal, adc_t, adc_p, 0.0);
    assert!((t - 21.5).abs() < 0.05, "T {}", t);
    assert!((p - 1005.3).abs() < 0.1, "P {} hPa", p);
}

#[test]
fn bme280_adds_humidity() {
    let mut b = i2c_board("env", "{id: env, type: bme280, temperature: 30, humidity: 62.5}");
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.reg8(0x76, 0xD0), 0x60);
    let cal = bmp280_calib(&mut i2c, 0x76, true);
    assert!(i2c.write_reg(0x76, 0xF2, &[0x01]));
    assert!(i2c.write_reg(0x76, 0xF4, &[0x27]));
    i2c.wait(50 * MS);
    let d = i2c.read_reg(0x76, 0xF7, 8);
    let adc_p = ((d[0] as u32) << 12 | (d[1] as u32) << 4 | (d[2] as u32) >> 4) as f64;
    let adc_t = ((d[3] as u32) << 12 | (d[4] as u32) << 4 | (d[5] as u32) >> 4) as f64;
    let adc_h = ((d[6] as u32) << 8 | d[7] as u32) as f64;
    let (t, p, h) = bme_compensate(&cal, adc_t, adc_p, adc_h);
    assert!((t - 30.0).abs() < 0.05, "T {}", t);
    assert!((p - 1013.25).abs() < 0.1, "P {}", p);
    assert!((h - 62.5).abs() < 0.5, "H {}", h);
    // altitude input changes the pressure accordingly
    b.input("env", &json!({"altitude": 1000})).unwrap();
    let mut i2c = I2c::new(&mut b);
    i2c.wait(50 * MS);
    let d = i2c.read_reg(0x76, 0xF7, 6);
    let adc_p = ((d[0] as u32) << 12 | (d[1] as u32) << 4 | (d[2] as u32) >> 4) as f64;
    let adc_t = ((d[3] as u32) << 12 | (d[4] as u32) << 4 | (d[5] as u32) >> 4) as f64;
    let (_, p, _) = bme_compensate(&cal, adc_t, adc_p, 0.0);
    assert!((p - 898.7).abs() < 0.5, "P at 1000 m = {}", p);
}

#[test]
fn bmp180_datasheet_algorithm() {
    let mut b = i2c_board("baro", "{id: baro, type: bmp180, temperature: 24, pressure: 1013.25}");
    let mut i2c = I2c::new(&mut b);
    assert_eq!(i2c.reg8(0x77, 0xD0), 0x55);
    let c = i2c.read_reg(0x77, 0xAA, 22);
    let w = |i: usize| i16::from_be_bytes([c[2 * i], c[2 * i + 1]]) as i64;
    let uw = |i: usize| u16::from_be_bytes([c[2 * i], c[2 * i + 1]]) as i64;
    let (ac1, ac2, ac3, ac4, ac5, ac6, b1, b2, _mb, mc, md) = (w(0), w(1), w(2), uw(3), uw(4), uw(5), w(6), w(7), w(8), w(9), w(10));
    assert!(i2c.write_reg(0x77, 0xF4, &[0x2E]));
    i2c.wait(5 * MS);
    let d = i2c.read_reg(0x77, 0xF6, 2);
    let ut = (d[0] as i64) << 8 | d[1] as i64;
    let oss = 3i64;
    assert!(i2c.write_reg(0x77, 0xF4, &[0x34 + ((oss as u8) << 6)]));
    i2c.wait(26 * MS);
    let d = i2c.read_reg(0x77, 0xF6, 3);
    let up = ((d[0] as i64) << 16 | (d[1] as i64) << 8 | d[2] as i64) >> (8 - oss);
    let x1 = (ut - ac6) * ac5 >> 15;
    let x2 = (mc << 11) / (x1 + md);
    let b5 = x1 + x2;
    let t = (b5 + 8) >> 4;
    let b6 = b5 - 4000;
    let x1 = (b2 * (b6 * b6 >> 12)) >> 11;
    let x2 = ac2 * b6 >> 11;
    let x3 = x1 + x2;
    let b3 = (((ac1 * 4 + x3) << oss) + 2) / 4;
    let x1 = ac3 * b6 >> 13;
    let x2 = (b1 * (b6 * b6 >> 12)) >> 16;
    let x3 = ((x1 + x2) + 2) >> 2;
    let b4 = (ac4 as u64 * (x3 + 32768) as u64) >> 15;
    let b7 = ((up - b3) as u64) * (50000 >> oss);
    let mut p = if b7 < 0x8000_0000 { (b7 * 2) / b4 } else { (b7 / b4) * 2 } as i64;
    let x1 = (p >> 8) * (p >> 8);
    let x1 = (x1 * 3038) >> 16;
    let x2 = (-7357 * p) >> 16;
    p += (x1 + x2 + 3791) >> 4;
    assert!((t as f64 / 10.0 - 24.0).abs() <= 0.1, "T {}", t);
    assert!((p as f64 - 101325.0).abs() < 15.0, "P {} Pa", p);
}

// ------------------------------------------------------------- clocks

fn bcd(v: u8) -> u8 {
    (v >> 4) * 10 + (v & 0xF)
}

#[test]
fn ds3231_set_time_and_let_it_run() {
    let mut b = i2c_board("rtc", "{id: rtc, type: ds3231, temperature: 24}");
    let mut i2c = I2c::new(&mut b);
    // 2024-08-15 12:34:56, Thursday
    assert!(i2c.write_reg(0x68, 0x00, &[0x56, 0x34, 0x12, 0x05, 0x15, 0x08, 0x24]));
    i2c.wait(2_000 * MS);
    let r = i2c.read_reg(0x68, 0x00, 7);
    assert_eq!((bcd(r[2] & 0x3F), bcd(r[1]), bcd(r[0] & 0x7F)), (12, 34, 58), "{:02x?}", r);
    assert_eq!((bcd(r[4]), bcd(r[5] & 0x1F), bcd(r[6])), (15, 8, 24));
    let t = i2c.read_reg(0x68, 0x11, 2);
    assert_eq!(t[0] as i8, 24, "temperature MSB");
    assert!(summary(&b, "rtc").starts_with("2024-08-15 12:34:5"));
}

#[test]
fn ds1307_clock_and_battery_ram() {
    let mut b = i2c_board("rtc", "{id: rtc, type: ds1307}");
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write_reg(0x68, 0x00, &[0x00, 0x00, 0x08, 0x01, 0x01, 0x01, 0x25]));
    assert!(i2c.write_reg(0x68, 0x08, b"hello"));
    i2c.wait(61_000 * MS);
    let r = i2c.read_reg(0x68, 0x00, 3);
    assert_eq!((bcd(r[2]), bcd(r[1]), bcd(r[0] & 0x7F)), (8, 1, 1));
    assert_eq!(i2c.read_reg(0x68, 0x08, 5), b"hello".to_vec());
}

// ------------------------------------------------------ light / humidity

#[test]
fn bh1750_continuous_high_resolution() {
    let mut b = i2c_board("lux", "{id: lux, type: bh1750, lux: 250}");
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x23, &[0x01])); // power on
    assert!(i2c.write(0x23, &[0x10])); // continuous H-resolution
    i2c.wait(180 * MS);
    let d = i2c.read(0x23, 2).unwrap();
    let lux = u16::from_be_bytes([d[0], d[1]]) as f64 / 1.2;
    assert!((lux - 250.0).abs() < 1.0, "{} lux", lux);
    b.input("lux", &json!({"lux": 1000})).unwrap();
    let mut i2c = I2c::new(&mut b);
    i2c.wait(180 * MS);
    let d = i2c.read(0x23, 2).unwrap();
    assert!((u16::from_be_bytes([d[0], d[1]]) as f64 / 1.2 - 1000.0).abs() < 1.0);
}

fn crc8_31(data: &[u8]) -> u8 {
    let mut crc = 0xFFu8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x31 } else { crc << 1 };
        }
    }
    crc
}

#[test]
fn sht31_single_shot_with_crc() {
    let mut b = i2c_board("sht", "{id: sht, type: sht31, temperature: 19.5, humidity: 71}");
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x44, &[0x24, 0x00]));
    i2c.wait(20 * MS);
    let d = i2c.read(0x44, 6).unwrap();
    assert_eq!(crc8_31(&d[0..2]), d[2], "temperature CRC");
    assert_eq!(crc8_31(&d[3..5]), d[5], "humidity CRC");
    let t = -45.0 + 175.0 * u16::from_be_bytes([d[0], d[1]]) as f64 / 65535.0;
    let h = 100.0 * u16::from_be_bytes([d[3], d[4]]) as f64 / 65535.0;
    assert!((t - 19.5).abs() < 0.05 && (h - 71.0).abs() < 0.05, "{} °C {} %", t, h);
}

#[test]
fn aht20_measurement_frame() {
    let mut b = i2c_board("aht", "{id: aht, type: aht20, temperature: 26, humidity: 40}");
    let mut i2c = I2c::new(&mut b);
    assert!(i2c.write(0x38, &[0xBE, 0x08, 0x00]));
    i2c.wait(10 * MS);
    assert!(i2c.write(0x38, &[0xAC, 0x33, 0x00]));
    i2c.wait(80 * MS);
    let d = i2c.read(0x38, 7).unwrap();
    assert_eq!(d[0] & 0x80, 0, "not busy");
    assert_eq!(crc8_31(&d[..6]), d[6], "CRC");
    let hraw = ((d[1] as u32) << 12) | ((d[2] as u32) << 4) | (d[3] as u32 >> 4);
    let traw = (((d[3] & 0xF) as u32) << 16) | ((d[4] as u32) << 8) | d[5] as u32;
    let h = hraw as f64 / 1048576.0 * 100.0;
    let t = traw as f64 / 1048576.0 * 200.0 - 50.0;
    assert!((t - 26.0).abs() < 0.05 && (h - 40.0).abs() < 0.05, "{} °C {} %", t, h);
}

// --------------------------------------------------- multi-chip boards

fn enable_mpu_bypass(i2c: &mut I2c) {
    assert!(i2c.write_reg(0x68, 0x6B, &[0x00]));
    assert!(i2c.write_reg(0x68, 0x6A, &[0x00]));
    assert!(i2c.write_reg(0x68, 0x37, &[0x02]));
}

#[test]
fn gy86_flight_controller_board() {
    let mut b = i2c_board("fc", "{id: fc, type: gy-86}");
    let mut i2c = I2c::new(&mut b);
    enable_mpu_bypass(&mut i2c);
    let found = i2c.scan();
    for a in [0x1E, 0x68, 0x77] {
        assert!(found.contains(&a), "0x{:02x} missing from {:02x?}", a, found);
    }
    assert_eq!(i2c.reg8(0x68, 0x75), 0x68);
    assert_eq!(i2c.read_reg(0x1E, 0x0A, 3), b"H43".to_vec());
    let (t, p) = ms5611_read(&mut i2c, 0x77);
    assert!((t - 24.0).abs() < 0.2 && (p - 1013.25).abs() < 0.3, "{} {}", t, p);
    // attitude input shows up on the accelerometer
    b.input("fc", &json!({"pitch": 30})).unwrap();
    let mut i2c = I2c::new(&mut b);
    let d = i2c.read_reg(0x68, 0x3B, 6);
    let ax = be16(&d[0..]) as f64 / 16384.0;
    assert!((ax + 0.5).abs() < 0.01, "ax {} g", ax);
}

#[test]
fn gy87_and_gy91_boards() {
    let mut b = i2c_board("fc", "{id: fc, type: gy-87}");
    let mut i2c = I2c::new(&mut b);
    enable_mpu_bypass(&mut i2c);
    let found = i2c.scan();
    assert!(found.contains(&0x68) && found.contains(&0x1E) && found.contains(&0x77), "{:02x?}", found);
    assert_eq!(i2c.reg8(0x77, 0xD0), 0x55, "BMP180");

    let mut b = i2c_board("fc", "{id: fc, type: gy-91}");
    let mut i2c = I2c::new(&mut b);
    enable_mpu_bypass(&mut i2c);
    let found = i2c.scan();
    assert!(found.contains(&0x68) && found.contains(&0x76) && found.contains(&0x0C), "{:02x?}", found);
    assert_eq!(i2c.reg8(0x68, 0x75), 0x71, "MPU9250");
    assert_eq!(i2c.reg8(0x76, 0xD0), 0x58, "BMP280");
}

#[test]
fn shared_bus_scan_finds_every_device() {
    let mut b = board_of(
        &["{id: oled, type: ssd1306}", "{id: lcd, type: lcd1602-i2c}", "{id: rtc, type: ds3231}", "{id: ee, type: at24c, address: 0x57}", "{id: adc, type: ads1115}"],
        &["esp32.D21 oled.SDA lcd.SDA rtc.SDA ee.SDA adc.SDA", "esp32.D22 oled.SCL lcd.SCL rtc.SCL ee.SCL adc.SCL"],
    );
    assert_eq!(I2c::new(&mut b).scan(), vec![0x27, 0x3C, 0x48, 0x57, 0x68]);
    // a second bus on other pins is separate
    let mut b2 = board_of(
        &["{id: a, type: bh1750}", "{id: c, type: sht31}"],
        &["esp32.D21 a.SDA", "esp32.D22 a.SCL", "esp32.D16 c.SDA", "esp32.D17 c.SCL"],
    );
    let mut i2c = I2c::new(&mut b2);
    assert_eq!(i2c.scan(), vec![0x23]);
    i2c.scl = Some(17);
    i2c.sda = Some(16);
    assert_eq!(i2c.scan(), vec![0x44]);
}

//! SPI, UART and frame-based modules exercised with the same command flows
//! the popular Arduino libraries use (MFRC522, SdFat, Adafruit ILI9341 /
//! PN532, TinyGPS, HC-05 AT mode).
mod common;
use common::*;
use devices::circuit::Board;
use esp32_soc::io::IoBridge;
use serde_json::json;

// --------------------------------------------------------------- MFRC522

struct Rc522<'a> {
    b: &'a mut Board,
    t: u64,
}

impl<'a> Rc522<'a> {
    fn xfer(&mut self, d: &[u8]) -> Vec<u8> {
        self.t += 50 * US;
        spi(self.b, self.t, 18, 23, Some(19), 5, d)
    }
    fn w(&mut self, reg: u8, v: u8) {
        self.xfer(&[reg << 1, v]);
    }
    fn r(&mut self, reg: u8) -> u8 {
        self.xfer(&[0x80 | (reg << 1), 0])[1]
    }
    /// MFRC522::PCD_CommunicateWithPICC(Transceive)
    fn transceive(&mut self, data: &[u8], bits: u8) -> Option<Vec<u8>> {
        self.w(0x01, 0x00); // idle
        self.w(0x04, 0x7F); // clear IRQs
        self.w(0x0A, 0x80); // flush FIFO
        for &x in data {
            self.w(0x09, x);
        }
        self.w(0x0D, bits);
        self.w(0x01, 0x0C); // transceive
        self.w(0x0D, bits | 0x80); // start send
        for _ in 0..200 {
            let irq = self.r(0x04);
            if irq & 0x30 != 0 {
                if self.r(0x06) & 0x13 != 0 {
                    return None;
                }
                let n = self.r(0x0A);
                return Some((0..n).map(|_| self.r(0x09)).collect());
            }
            if irq & 0x01 != 0 {
                return None; // timer: no card answered
            }
            self.t += MS;
        }
        None
    }
}

/// ISO/IEC 14443-A CRC (init 0x6363, reflected poly 0x8408).
fn crc_a(d: &[u8]) -> [u8; 2] {
    let mut crc: u16 = 0x6363;
    for &b in d {
        let mut x = b ^ (crc as u8);
        x ^= x << 4;
        crc = (crc >> 8) ^ ((x as u16) << 8) ^ ((x as u16) << 3) ^ ((x as u16) >> 4);
    }
    crc.to_le_bytes()
}

fn rc522_board() -> Board {
    board_of(&["{id: rfid, type: mfrc522}"], &["esp32.D18 rfid.SCK", "esp32.D23 rfid.MOSI", "esp32.D19 rfid.MISO", "esp32.D5 rfid.SDA"])
}

#[test]
fn mfrc522_version_and_antenna() {
    let mut b = rc522_board();
    let mut rc = Rc522 { b: &mut b, t: MS };
    let v = rc.r(0x37);
    assert!(v == 0x91 || v == 0x92, "VersionReg {:02x}", v);
    let tx = rc.r(0x14);
    rc.w(0x14, tx | 0x03);
    assert_eq!(state(&b, "rfid")["antenna"], json!(true));
}

#[test]
fn mfrc522_detects_selects_and_halts_a_card() {
    let mut b = rc522_board();
    {
        let mut rc = Rc522 { b: &mut b, t: MS };
        rc.w(0x14, 0x83);
        assert_eq!(rc.transceive(&[0x26], 0x07), None, "no card in the field");
    }
    b.input("rfid", &json!({"uid": "11 22 33 44"})).unwrap();
    let mut rc = Rc522 { b: &mut b, t: 10 * MS };
    rc.w(0x14, 0x83);
    assert_eq!(rc.transceive(&[0x26], 0x07), Some(vec![0x04, 0x00]), "ATQA of MIFARE Classic 1K");
    let uid = rc.transceive(&[0x93, 0x20], 0x00).expect("anticollision");
    assert_eq!(&uid[..4], &[0x11, 0x22, 0x33, 0x44]);
    assert_eq!(uid[4], 0x11 ^ 0x22 ^ 0x33 ^ 0x44, "BCC");
    let mut sel = vec![0x93, 0x70];
    sel.extend_from_slice(&uid);
    let crc = crc_a(&sel);
    sel.extend_from_slice(&crc);
    let sak = rc.transceive(&sel, 0x00).expect("select");
    assert_eq!(sak[0], 0x08, "SAK of MIFARE Classic 1K");
    assert_eq!(&sak[1..3], &crc_a(&sak[..1]), "SAK CRC");
    assert_eq!(state(&b, "rfid")["selected"], json!(true));
    b.input("rfid", &json!({"action": "remove"})).unwrap();
    assert_eq!(state(&b, "rfid")["card"], json!(null));
}

#[test]
fn mfrc522_crc_coprocessor() {
    let mut b = rc522_board();
    let mut rc = Rc522 { b: &mut b, t: MS };
    assert_eq!(rc.r(0x11), 0x3F, "ModeReg reset value (CRC preset 0xFFFF)");
    rc.w(0x11, 0x3D); // PCD_Init: CRC preset 0x6363 (ISO 14443-A)
    rc.w(0x01, 0x00);
    rc.w(0x05, 0x04); // clear CRCIRq
    rc.w(0x0A, 0x80);
    for x in [0x30, 0x04] {
        rc.w(0x09, x); // MIFARE READ block 4
    }
    rc.w(0x01, 0x03); // CalcCRC
    let mut done = false;
    for _ in 0..100 {
        if rc.r(0x05) & 0x04 != 0 {
            done = true;
            break;
        }
    }
    assert!(done);
    let crc = [rc.r(0x22), rc.r(0x21)];
    assert_eq!(crc, crc_a(&[0x30, 0x04]));
}

// --------------------------------------------------------------- SD card

struct Sd<'a> {
    b: &'a mut Board,
    t: u64,
}

impl<'a> Sd<'a> {
    fn x(&mut self, d: &[u8]) -> Vec<u8> {
        self.t += 20 * US;
        spi(self.b, self.t, 18, 23, Some(19), 5, d)
    }
    /// Send a command and return the first non-0xFF response byte (R1).
    fn cmd(&mut self, idx: u8, arg: u32, crc: u8) -> u8 {
        let a = arg.to_be_bytes();
        self.x(&[0xFF]);
        self.x(&[0x40 | idx, a[0], a[1], a[2], a[3], crc]);
        for _ in 0..10 {
            let r = self.x(&[0xFF])[0];
            if r != 0xFF {
                return r;
            }
        }
        0xFF
    }
    fn wait_token(&mut self) -> u8 {
        for _ in 0..1000 {
            let r = self.x(&[0xFF])[0];
            if r != 0xFF {
                return r;
            }
        }
        0xFF
    }
}

fn sd_board() -> Board {
    board_of(&["{id: sd, type: sd-card, size_mb: 16, __ephemeral: true}"], &["esp32.D18 sd.SCK", "esp32.D23 sd.MOSI", "esp32.D19 sd.MISO", "esp32.D5 sd.CS"])
}

fn sd_init(sd: &mut Sd) {
    sd.x(&[0xFF; 10]); // >= 74 clocks
    assert_eq!(sd.cmd(0, 0, 0x95), 0x01, "CMD0 -> idle");
    assert_eq!(sd.cmd(8, 0x1AA, 0x87), 0x01, "CMD8 accepted (v2 card)");
    assert_eq!(sd.x(&[0xFF; 4]), vec![0x00, 0x00, 0x01, 0xAA], "R7 echo");
    let mut ready = false;
    for _ in 0..100 {
        sd.cmd(55, 0, 0x65);
        if sd.cmd(41, 0x4000_0000, 0x77) == 0x00 {
            ready = true;
            break;
        }
        sd.t += MS;
    }
    assert!(ready, "ACMD41 leaves idle state");
    assert_eq!(sd.cmd(58, 0, 0xFD), 0x00);
    let ocr = sd.x(&[0xFF; 4]);
    assert_ne!(ocr[0] & 0x40, 0, "CCS: high capacity (block addressing)");
}

#[test]
fn sd_card_initialisation_and_boot_sector() {
    let mut b = sd_board();
    let mut sd = Sd { b: &mut b, t: MS };
    sd_init(&mut sd);
    assert_eq!(sd.cmd(17, 0, 0xFF), 0x00, "CMD17 read block 0");
    assert_eq!(sd.wait_token(), 0xFE, "data token");
    let blk = sd.x(&[0xFF; 512]);
    sd.x(&[0xFF; 2]); // CRC
    assert_eq!(&blk[510..], &[0x55, 0xAA], "boot signature of the FAT image");
    assert_eq!(sd.cmd(9, 0, 0xFF), 0x00, "CMD9 CSD");
    assert_eq!(sd.wait_token(), 0xFE);
    let csd = sd.x(&[0xFF; 16]);
    assert_eq!(csd[0] >> 6, 1, "CSD version 2.0");
}

#[test]
fn sd_card_block_write_and_read_back() {
    let mut b = sd_board();
    let mut sd = Sd { b: &mut b, t: MS };
    sd_init(&mut sd);
    let data: Vec<u8> = (0..512).map(|i| (i * 7 % 256) as u8).collect();
    assert_eq!(sd.cmd(24, 1000, 0xFF), 0x00, "CMD24 write block 1000");
    sd.x(&[0xFF, 0xFE]);
    sd.x(&data);
    sd.x(&[0xFF, 0xFF]);
    let resp = sd.wait_token();
    assert_eq!(resp & 0x1F, 0x05, "data accepted");
    for _ in 0..1000 {
        if sd.x(&[0xFF])[0] == 0xFF {
            break; // not busy
        }
    }
    assert_eq!(sd.cmd(17, 1000, 0xFF), 0x00);
    assert_eq!(sd.wait_token(), 0xFE);
    assert_eq!(sd.x(&[0xFF; 512]), data);
    assert!(summary(&b, "sd").contains("1 written"), "{}", summary(&b, "sd"));
}

// ----------------------------------------------------------- TFT / LEDs

struct Tft<'a> {
    b: &'a mut Board,
    t: u64,
    dc: u8,
    cs: u8,
}

impl<'a> Tft<'a> {
    fn send(&mut self, dc: bool, d: &[u8]) {
        self.t += 20 * US;
        self.b.gpio_update(self.t, &drive(&[(self.dc, dc), (4, true)]));
        self.t += 20 * US;
        spi(self.b, self.t, 18, 23, None, self.cs, d);
    }
    fn cmd(&mut self, c: u8, data: &[u8]) {
        self.send(false, &[c]);
        if !data.is_empty() {
            self.send(true, data);
        }
    }
}

fn tft_pixel(b: &Board, id: &str, x: usize, y: usize) -> (u8, u8, u8) {
    let i = b.find(id).unwrap();
    let (w, _h, rgb) = b.comps[i].dev.framebuffer().expect("framebuffer");
    let o = (y * w + x) * 3;
    (rgb[o], rgb[o + 1], rgb[o + 2])
}

#[test]
fn ili9341_address_window_and_rgb565_pixels() {
    let mut b = board_of(
        &["{id: tft, type: ili9341}"],
        &["esp32.D18 tft.SCK", "esp32.D23 tft.MOSI", "esp32.D15 tft.CS", "esp32.D2 tft.DC", "esp32.D4 tft.RESET", "3V3 tft.LED"],
    );
    let mut t = Tft { b: &mut b, t: MS, dc: 2, cs: 15 };
    t.cmd(0x01, &[]);
    t.t += 150 * MS;
    t.cmd(0x11, &[]);
    t.cmd(0x3A, &[0x55]);
    t.cmd(0x36, &[0x48]); // MADCTL as Adafruit rotation 0 (MX | BGR)
    t.cmd(0x29, &[]);
    // 2x1 window at (10,20): red then green
    t.cmd(0x2A, &[0, 10, 0, 11]);
    t.cmd(0x2B, &[0, 20, 0, 20]);
    t.cmd(0x2C, &[0xF8, 0x00, 0x07, 0xE0]);
    let st = state(&b, "tft");
    assert_eq!((st["width"].as_u64(), st["height"].as_u64()), (Some(240), Some(320)));
    assert_eq!(st["on"], json!(true));
    assert_eq!(st["lit"], json!(2));
    // Adafruit/TFT_eSPI rotation 0 (MX|BGR) is upright with true colours
    assert_eq!(tft_pixel(&b, "tft", 10, 20), (255, 0, 0));
    assert_eq!(tft_pixel(&b, "tft", 11, 20), (0, 255, 0));
    assert_eq!(tft_pixel(&b, "tft", 12, 20), (0, 0, 0));
    // rotation 1 (MV|BGR, landscape): logical (0,0) is the panel's top-right corner
    let mut t = Tft { b: &mut b, t: 500 * MS, dc: 2, cs: 15 };
    t.cmd(0x36, &[0x28]);
    t.cmd(0x2A, &[0, 0, 0, 0]);
    t.cmd(0x2B, &[0, 0, 0, 0]);
    t.cmd(0x2C, &[0x00, 0x1F]);
    assert_eq!(tft_pixel(&b, "tft", 239, 0), (0, 0, 255));
}

#[test]
fn st7735_fills_a_rectangle() {
    let mut b = board_of(&["{id: tft, type: st7735}"], &["esp32.D18 tft.SCK", "esp32.D23 tft.MOSI", "esp32.D15 tft.CS", "esp32.D2 tft.DC", "esp32.D4 tft.RESET"]);
    let mut t = Tft { b: &mut b, t: MS, dc: 2, cs: 15 };
    t.cmd(0x11, &[]);
    t.cmd(0x3A, &[0x05]);
    t.cmd(0x36, &[0xC0]); // Adafruit rotation 0: MX | MY
    t.cmd(0x29, &[]);
    t.cmd(0x2A, &[0, 0, 0, 3]);
    t.cmd(0x2B, &[0, 0, 0, 3]);
    t.cmd(0x2C, &[0xFF; 32]); // 16 white pixels
    assert_eq!(state(&b, "tft")["lit"], json!(16));
    assert_eq!(tft_pixel(&b, "tft", 3, 3), (255, 255, 255));
    assert_eq!(tft_pixel(&b, "tft", 4, 4), (0, 0, 0));
    // without MADCTL the panel shows the controller's raw scan: rotated 180°
    let mut t = Tft { b: &mut b, t: 100 * MS, dc: 2, cs: 15 };
    t.cmd(0x36, &[0x00]);
    t.cmd(0x2A, &[0, 0, 0, 0]);
    t.cmd(0x2B, &[0, 0, 0, 0]);
    t.cmd(0x2C, &[0xF8, 0x00]);
    assert_eq!(tft_pixel(&b, "tft", 127, 159), (255, 0, 0));
}

#[test]
fn ssd1306_over_spi_with_dc_pin() {
    let mut b = board_of(&["{id: oled, type: ssd1306-spi}"], &["esp32.D18 oled.SCK", "esp32.D23 oled.MOSI", "esp32.D15 oled.CS", "esp32.D2 oled.DC", "esp32.D4 oled.RES"]);
    let mut t = Tft { b: &mut b, t: MS, dc: 2, cs: 15 };
    t.send(false, &[0xAE, 0x20, 0x00, 0xA1, 0xC8, 0x21, 0, 127, 0x22, 0, 7, 0xAF]);
    t.send(true, &[0x0F, 0xF0]);
    assert_eq!(state(&b, "oled")["on"], json!(true));
    assert!(summary(&b, "oled").contains("8 pixels lit"), "{}", summary(&b, "oled"));
}

#[test]
fn max7219_rows_intensity_and_shutdown() {
    let mut b = board_of(&["{id: m, type: max7219}"], &["esp32.D18 m.CLK", "esp32.D23 m.DIN", "esp32.D5 m.CS"]);
    let mut t = MS;
    for frame in [[0x0C, 0x01], [0x0F, 0x00], [0x0B, 0x07], [0x09, 0x00], [0x0A, 0x08], [0x01, 0x81], [0x08, 0xFF]] {
        t += 50 * US;
        spi(&mut b, t, 18, 23, None, 5, &frame);
    }
    let st = state(&b, "m");
    let c = &st["chips"][0];
    assert_eq!(c["on"], json!(true));
    assert_eq!(c["intensity"], json!(8));
    assert_eq!(c["scan_limit"], json!(7));
    let rows = c["rows"].as_str().unwrap();
    assert!(rows.starts_with("81") && rows.ends_with("ff"), "{}", rows);
    spi(&mut b, t + MS, 18, 23, None, 5, &[0x0C, 0x00]);
    assert_eq!(state(&b, "m")["chips"][0]["on"], json!(false));
}

// ------------------------------------------------------------- PN532

fn pn532_frame(cmd: &[u8]) -> Vec<u8> {
    let len = cmd.len() as u8 + 1;
    let mut f = vec![0x00, 0x00, 0xFF, len, len.wrapping_neg(), 0xD4];
    f.extend_from_slice(cmd);
    let sum = cmd.iter().fold(0xD4u8, |a, &b| a.wrapping_add(b));
    f.push(sum.wrapping_neg());
    f.push(0x00);
    f
}

/// Read a PN532 response frame over I2C (status byte first), checking checksums.
fn pn532_response(i2c: &mut I2c) -> Vec<u8> {
    for _ in 0..50 {
        let r = i2c.read(0x24, 1).unwrap();
        if r[0] & 1 != 0 {
            break;
        }
        i2c.wait(MS);
    }
    let r = i2c.read(0x24, 64).unwrap();
    assert_eq!(r[0] & 1, 1, "ready");
    assert_eq!(&r[1..4], &[0x00, 0x00, 0xFF], "preamble {:02x?}", r);
    let len = r[4] as usize;
    assert_eq!(r[4].wrapping_add(r[5]), 0, "LCS");
    let body = &r[6..6 + len];
    let dcs = r[6 + len];
    assert_eq!(body.iter().fold(dcs, |a, &b| a.wrapping_add(b)), 0, "DCS");
    assert_eq!(body[0], 0xD5, "response TFI");
    body[1..].to_vec()
}

fn pn532_cmd(i2c: &mut I2c, cmd: &[u8]) {
    assert!(i2c.write(0x24, &pn532_frame(cmd)));
    i2c.wait(MS);
    let ack = i2c.read(0x24, 7).unwrap();
    assert_eq!(&ack[1..], &[0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00], "ACK frame");
    i2c.wait(MS);
}

#[test]
fn pn532_firmware_version_and_card_listing() {
    let mut b = i2c_board("nfc", "{id: nfc, type: pn532}");
    let mut i2c = I2c::new(&mut b);
    pn532_cmd(&mut i2c, &[0x02]);
    assert_eq!(pn532_response(&mut i2c), vec![0x03, 0x32, 0x01, 0x06, 0x07], "GetFirmwareVersion: PN532 v1.6");
    pn532_cmd(&mut i2c, &[0x14, 0x01, 0x14, 0x01]);
    assert_eq!(pn532_response(&mut i2c), vec![0x15], "SAMConfiguration");
    b.input("nfc", &json!({"uid": "deadbeef"})).unwrap();
    let mut i2c = I2c::new(&mut b);
    pn532_cmd(&mut i2c, &[0x4A, 0x01, 0x00]);
    let r = pn532_response(&mut i2c);
    assert_eq!(&r[..2], &[0x4B, 0x01], "one target");
    // Tg, SENS_RES(2), SEL_RES, NFCID length, NFCID
    assert_eq!(&r[3..], &[0x00, 0x04, 0x08, 0x04, 0xDE, 0xAD, 0xBE, 0xEF]);
}

#[test]
fn pn532_waits_for_a_card_tap() {
    let mut b = i2c_board("nfc", "{id: nfc, type: pn532}");
    let mut i2c = I2c::new(&mut b);
    pn532_cmd(&mut i2c, &[0x4A, 0x01, 0x00]);
    assert_eq!(i2c.read(0x24, 1).unwrap()[0] & 1, 0, "not ready while no card");
    assert_eq!(state(&b, "nfc")["waiting_for_card"], json!(true));
    b.input("nfc", &json!({"card": "ntag213"})).unwrap();
    let mut i2c = I2c::new(&mut b);
    let r = pn532_response(&mut i2c);
    assert_eq!(r[0], 0x4B);
    assert_eq!(r[6], 7, "7-byte NTAG UID");
}

// ------------------------------------------------------------ UART parts

fn nmea_ok(line: &str) -> bool {
    let Some(body) = line.strip_prefix('$') else { return false };
    let Some((data, ck)) = body.split_once('*') else { return false };
    let sum = data.bytes().fold(0u8, |a, b| a ^ b);
    u8::from_str_radix(ck.trim(), 16).map(|c| c == sum).unwrap_or(false)
}

fn nmea_deg(v: &str, hemi: &str) -> f64 {
    let x: f64 = v.parse().unwrap();
    let d = (x / 100.0).floor();
    let r = d + (x - d * 100.0) / 60.0;
    if hemi == "S" || hemi == "W" {
        -r
    } else {
        r
    }
}

#[test]
fn gps_streams_valid_nmea_sentences() {
    let mut b = board_of(&["{id: gps, type: gps-neo6m, lat: -33.8688, lon: 151.2093, alt: 58}"], &["esp32.D16 gps.TX", "esp32.D17 gps.RX"]);
    uart_route(&mut b, 2, 17, 16, &[]);
    let out = String::from_utf8(uart_collect(&mut b, MS, 2_500 * MS, 2)).unwrap();
    let lines: Vec<&str> = out.split("\r\n").filter(|l| l.starts_with('$')).collect();
    assert!(lines.len() >= 10, "{} sentences", lines.len());
    for l in &lines {
        assert!(nmea_ok(l), "bad checksum: {}", l);
    }
    let rmc: Vec<&str> = lines.iter().find(|l| l.starts_with("$GPRMC")).unwrap().split(',').collect();
    assert_eq!(rmc[2], "A", "valid fix");
    assert!((nmea_deg(rmc[3], rmc[4]) + 33.8688).abs() < 1e-4, "{:?}", rmc);
    assert!((nmea_deg(rmc[5], rmc[6]) - 151.2093).abs() < 1e-4);
    let gga: Vec<&str> = lines.iter().find(|l| l.starts_with("$GPGGA")).unwrap().split(',').collect();
    assert_eq!(gga[6], "1", "GPS fix quality");
    assert_eq!(gga[9], "58.0");
    // losing the fix
    b.input("gps", &json!({"fix": false})).unwrap();
    let out = String::from_utf8(uart_collect(&mut b, 2_600 * MS, 4_000 * MS, 2)).unwrap();
    assert!(out.contains("$GPRMC") && out.lines().filter(|l| l.starts_with("$GPRMC")).all(|l| l.split(',').nth(2) == Some("V")));
}

#[test]
fn hc05_at_commands_then_phone_link() {
    let mut b = board_of(&["{id: bt, type: hc-05}"], &["esp32.D17 bt.RXD", "esp32.D16 bt.TXD", "esp32.D4 bt.STATE"]);
    uart_route(&mut b, 2, 17, 16, &[]);
    uart_send(&mut b, MS, 2, b"AT\r\n");
    assert_eq!(uart_collect(&mut b, 10 * MS, 20 * MS, 2), b"OK\r\n");
    uart_send(&mut b, 30 * MS, 2, b"AT+NAME=Robot\r\nAT+NAME?\r\n");
    assert_eq!(String::from_utf8(uart_collect(&mut b, 40 * MS, 60 * MS, 2)).unwrap(), "OK\r\n+NAME:Robot\r\nOK\r\n");
    assert_eq!(b.gpio_read(60 * MS, 4), Some(false), "STATE low while not connected");
    b.input("bt", &json!({"action": "connect"})).unwrap();
    b.advance(61 * MS);
    assert_eq!(b.gpio_read(62 * MS, 4), Some(true), "STATE high when a phone is connected");
    uart_send(&mut b, 70 * MS, 2, b"temp=21\n");
    assert_eq!(state(&b, "bt")["phone_rx"], json!("temp=21\n"));
    b.input("bt", &json!({"send": "led on"})).unwrap();
    let got = String::from_utf8(uart_collect(&mut b, 80 * MS, 90 * MS, 2)).unwrap();
    assert!(got.starts_with("led on"), "{:?}", got);
}

#[test]
fn serial_console_on_uart0() {
    let mut b = board_of(&[], &[]);
    uart_send(&mut b, MS, 0, b"hello");
    assert_eq!(b.console_out.iter().copied().collect::<Vec<u8>>(), b"hello");
    b.console_in.extend(b"cmd\n");
    assert_eq!(uart_collect(&mut b, 2 * MS, 3 * MS, 0), b"cmd\n");
}

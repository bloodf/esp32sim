//! eFuse controller (read-only view of a factory-programmed ESP32-D0WD-V3).

use super::RegFile;

/// CRC-8 used by the ESP32 ROM for the factory MAC (poly 0x8C reflected, init 0).
pub fn esp_crc8(data: &[u8]) -> u8 {
    let mut r: u8 = 0;
    for &b in data {
        r ^= b;
        for _ in 0..8 {
            let lsb = r & 1;
            r >>= 1;
            if lsb != 0 {
                r ^= 0x8C;
            }
        }
    }
    r
}

pub struct Efuse {
    pub regs: RegFile,
    pub mac: [u8; 6],
}

impl Efuse {
    pub fn new(mac: [u8; 6]) -> Efuse {
        let mut regs = RegFile::default();
        let crc = esp_crc8(&mac);
        // MAC_FACTORY: mac[5] at bits 32..39 ... mac[0] at bits 72..79, CRC at 80..87
        let w1 = (mac[5] as u32) | ((mac[4] as u32) << 8) | ((mac[3] as u32) << 16) | ((mac[2] as u32) << 24);
        let w2 = (mac[1] as u32) | ((mac[0] as u32) << 8) | ((crc as u32) << 16);
        regs.write(0x04, w1);
        regs.write(0x08, w2);
        // word3: CHIP_VER_REV1 (bit15), package D0WD (1 << 9), CPU freq rated (bit 13)
        regs.write(0x0C, (1 << 15) | (1 << 9) | (1 << 13));
        // word5: CHIP_VER_REV2 (bit 20)
        regs.write(0x14, 1 << 20);
        // word4: ADC vref calibration (signed 5 bit, units of 7mV) = 0
        regs.write(0x1FC, 0x16042000);
        Efuse { regs, mac }
    }

    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0x104 => 0, // commands complete immediately
            _ => self.regs.read(off),
        }
    }
    pub fn write(&mut self, off: u32, val: u32) {
        match off {
            0x00..=0x18 | 0x38..=0x97 => {} // read data registers are read-only
            _ => self.regs.write(off, val),
        }
    }
}

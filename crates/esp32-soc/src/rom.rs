//! High-level emulation of the ESP32 mask ROM.
//!
//! The ROM image itself is not shipped; instead every ROM function used by
//! applications is implemented here in Rust and dispatched by address using
//! Espressif's ROM linker scripts.

use crate::sys::Sys;
use std::collections::HashMap;
use xtensa_core::Cpu;

const ROM_LD: &[&str] = &[
    include_str!("../rom/esp32.rom.ld"),
    include_str!("../rom/esp32.rom.api.ld"),
    include_str!("../rom/esp32.rom.libgcc.ld"),
    include_str!("../rom/esp32.rom.redefined.ld"),
    include_str!("../rom/esp32.rom.syscalls.ld"),
    include_str!("../rom/esp32.rom.spiflash_legacy.ld"),
    include_str!("../rom/esp32.rom.eco3.ld"),
    include_str!("../rom/additional.ld"),
    include_str!("../rom/emulator-extra.ld"),
];

pub struct RomTable {
    pub by_addr: HashMap<u32, String>,
    pub by_name: HashMap<String, u32>,
}

impl RomTable {
    pub fn load() -> RomTable {
        let mut by_addr: HashMap<u32, String> = HashMap::new();
        let mut by_name = HashMap::new();
        for src in ROM_LD {
            for line in src.lines() {
                let l = line.trim();
                // forms: "PROVIDE ( name = 0x4000.... );"  or "name = 0x....;"
                let l = l.trim_start_matches("PROVIDE").trim().trim_start_matches('(').trim();
                let mut parts = l.splitn(2, '=');
                let name = match parts.next() {
                    Some(n) => n.trim(),
                    None => continue,
                };
                let rest = match parts.next() {
                    Some(r) => r.trim(),
                    None => continue,
                };
                if !rest.starts_with("0x") || name.is_empty() || name.contains(' ') {
                    continue;
                }
                let hex: String = rest[2..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
                if let Ok(a) = u32::from_str_radix(&hex, 16) {
                    by_name.insert(name.to_string(), a);
                    // prefer a name that we implement
                    let e = by_addr.entry(a).or_insert_with(|| name.to_string());
                    if implemented(name) && !implemented(e) {
                        *e = name.to_string();
                    }
                }
            }
        }
        RomTable { by_addr, by_name }
    }
    pub fn name(&self, addr: u32) -> Option<&str> {
        self.by_addr.get(&addr).map(|s| s.as_str())
    }
}

fn implemented(name: &str) -> bool {
    let mut dummy_done = false;
    let _ = &mut dummy_done;
    IMPLEMENTED.contains(&name)
}

const IMPLEMENTED: &[&str] = &[
    "memcpy", "memmove", "memset", "memcmp", "memchr", "bzero", "strlen", "strcpy", "strncpy", "strcat", "strncat", "strcmp",
    "strncmp", "strchr", "strlcpy", "strlcat", "strrchr", "strstr", "strcasecmp", "strncasecmp", "strnlen", "toupper", "tolower", "isalnum",
    "isalpha", "isdigit", "isspace", "isxdigit", "isupper", "islower", "atoi", "atol", "abs", "labs", "__adddf3", "__subdf3",
    "__muldf3", "__divdf3", "__negdf2", "__eqdf2", "__nedf2", "__gedf2", "__gtdf2", "__ledf2", "__ltdf2", "__unorddf2",
    "__extendsfdf2", "__truncdfsf2", "__fixdfsi", "__fixunsdfsi", "__fixdfdi", "__fixunsdfdi", "__floatsidf",
    "__floatunsidf", "__floatdidf", "__floatundidf", "__addsf3", "__subsf3", "__mulsf3", "__divsf3", "__negsf2",
    "__eqsf2", "__nesf2", "__gesf2", "__gtsf2", "__lesf2", "__ltsf2", "__unordsf2", "__fixsfsi", "__fixunssfsi",
    "__fixsfdi", "__fixunssfdi", "__floatsisf", "__floatunsisf", "__floatdisf", "__floatundisf", "__divdi3", "__udivdi3",
    "__moddi3", "__umoddi3", "__muldi3", "__ashldi3", "__ashrdi3", "__lshrdi3", "__negdi2", "__cmpdi2", "__ucmpdi2",
    "__bswapsi2", "__bswapdi2", "__popcountsi2", "__popcountdi2", "__clzsi2", "__ctzsi2", "__ffssi2", "__ffsdi2",
    "__clzdi2", "__ctzdi2", "__divsi3", "__udivsi3", "__modsi3", "__umodsi3", "__mulsi3", "__paritysi2", "__absvsi2",
    "__absvdi2", "ets_printf", "ets_install_putc1", "ets_install_putc2", "ets_install_uart_printf", "ets_delay_us",
    "ets_get_cpu_frequency", "ets_update_cpu_frequency", "ets_update_cpu_frequency_rom", "ets_get_detected_xtal_freq",
    "ets_set_appcpu_boot_addr", "rtc_get_reset_reason", "rtc_get_wakeup_cause", "uart_tx_one_char", "uart_tx_one_char2",
    "uart_tx_flush", "uart_tx_switch", "uart_rx_one_char", "uart_div_modify", "uart_tx_wait_idle", "ets_efuse_get_spiconfig",
    "ets_efuse_get_8M_clock", "esp_rom_spiflash_config_clk", "esp_rom_spiflash_config_param", "esp_rom_spiflash_read",
    "esp_rom_spiflash_write", "esp_rom_spiflash_erase_sector", "esp_rom_spiflash_erase_block", "esp_rom_spiflash_unlock",
    "esp_rom_spiflash_attach", "esp_rom_spiflash_read_status", "esp_rom_spiflash_wait_idle", "esp_rom_spiflash_select_qio_pins",
    "esp_rom_spiflash_write_encrypted_enable", "esp_rom_spiflash_write_encrypted_disable", "esp_rom_spiflash_prepare_encrypted_data",
    "esp_rom_spiflash_erase_chip", "esp_rom_spiflash_read_user_cmd", "esp_rom_spiflash_config_readmode", "SPIRead", "SPIWrite",
    "SPIEraseSector", "SPIEraseBlock", "SPIUnlock", "SPI_read_status_high", "Wait_SPI_Idle", "spi_flash_attach",
    "SPIParamCfg", "SPI_Write_Encrypt_Enable", "SPI_Write_Encrypt_Disable", "Cache_Read_Enable_rom", "Cache_Read_Disable_rom",
    "Cache_Flush_rom", "Cache_Read_Enable", "Cache_Read_Disable", "Cache_Flush", "cache_flash_mmu_set_rom", "cache_flash_mmu_set",
    "cache_sram_mmu_set_rom", "cache_sram_mmu_set", "mmu_init", "Cache_Read_Init_rom", "gpio_matrix_in", "gpio_matrix_out",
    "gpio_pad_select_gpio", "gpio_pad_pullup", "gpio_pad_pulldown", "gpio_pad_unhold", "gpio_pad_hold", "gpio_pad_set_drv",
    "gpio_output_set", "gpio_output_set_high", "gpio_input_get", "gpio_input_get_high", "intr_matrix_set", "ets_isr_mask",
    "ets_isr_unmask", "ets_intr_lock", "ets_intr_unlock", "_xtos_set_intlevel", "rom_i2c_writeReg", "rom_i2c_readReg",
    "rom_i2c_writeReg_Mask", "rom_i2c_readReg_Mask", "i2c_writeReg", "i2c_readReg", "i2c_writeReg_Mask", "i2c_readReg_Mask",
    "crc32_le", "crc32_be", "crc16_le", "crc16_be", "crc8_le", "crc8_be", "esp_crc8", "MD5Init", "MD5Update", "MD5Final",
    "esp_rom_md5_init", "esp_rom_md5_update", "esp_rom_md5_final", "roundup2", "set_rtc_memory_crc", "software_reset",
    "software_reset_cpu", "ets_get_xtal_scale", "abort", "ets_timer_init", "ets_timer_arm", "ets_timer_disarm",
    "ets_timer_setfn", "ets_timer_arm_us", "ets_timer_done", "esp_rom_efuse_get_flash_gpio_info", "phy_get_romfuncs",
    "rtc_boot_control", "ets_get_apb_freq", "strspn", "strcspn", "memccpy", "memrchr", "strlwr", "strupr", "strcoll", "strsep",
    "strtok_r", "__strtok_r", "strtol", "strtoul", "_strtol_r", "_strtoul_r", "div", "ldiv", "isascii", "toascii", "srand",
    "rand", "rand_r", "_atoi_r", "_atol_r", "__itoa", "__utoa", "strdup", "_strdup_r", "strndup", "_strndup_r", "qsort", "setjmp", "longjmp", "__longjmp_tail", "__sfp", "__sread", "__swrite", "__sseek", "__sclose", "__sinit", "_fwalk", "_fwalk_reent", "_fflush_r", "ets_sha_enable", "ets_sha_disable", "esp_rom_get_reset_reason",
    "ets_unpack_flash_code", "ets_run", "ets_write_char_uart", "xthal_get_ccount", "__sinit", "qsort", "itoa", "utoa",
    "ets_efuse_read_op", "ets_efuse_program_op", "ets_get_cpu_frequency_rom", "ets_intr_count", "rtc_select_apb_bridge",
    "esp_rom_regi2c_read", "esp_rom_regi2c_write", "ets_isr_attach", "xthal_set_intclear", "xthal_get_interrupt",
    "esp_rom_software_reset_system", "esp_rom_software_reset_cpu", "Cache_Flush_rom_app", "uartAttach", "Uart_Init",
    "uart_buff_switch", "ets_install_uart_printf_ex", "ets_wdt_disable", "ets_wdt_restore", "ets_wdt_enable",
    "ets_set_user_start", "ets_set_startup_callback", "ets_set_idle_cb", "Cache_Read_Init", "_cleanup_r", "_cleanup", "__sfp_lock_acquire", "__sfp_lock_release",
];

/// Where the fake ROM PHY function table lives (inside the ROM's reserved DRAM area).
pub const PHY_TABLE_ADDR: u32 = 0x3FFA_E0C4;
pub const PHY_TABLE_ENTRIES: u32 = 100;
/// Stub addresses (inside the ROM address range, unused by real functions).
pub const PHY_STUB_BASE: u32 = 0x4006_F000;

#[derive(Debug)]
pub enum HleOutcome {
    /// Function emulated, CPU returned to caller.
    Done,
    /// Function emulated and the core should be busy for this many ns.
    Delay(u64),
    /// Unknown / unsupported function
    Unsupported(String),
    /// Software reset requested
    Reset,
    /// Run this ROM function as a coroutine (it calls back into firmware)
    Coroutine(String),
}

// ------------------------------------------------------------------ helpers
fn arg(cpu: &mut Cpu, sys: &mut Sys, n: u32) -> u32 {
    cpu.hle_arg(sys, n)
}
fn d_arg(cpu: &mut Cpu, sys: &mut Sys, n: u32) -> f64 {
    let lo = arg(cpu, sys, n) as u64;
    let hi = arg(cpu, sys, n + 1) as u64;
    f64::from_bits((hi << 32) | lo)
}
fn l_arg(cpu: &mut Cpu, sys: &mut Sys, n: u32) -> u64 {
    let lo = arg(cpu, sys, n) as u64;
    let hi = arg(cpu, sys, n + 1) as u64;
    (hi << 32) | lo
}
fn f_arg(cpu: &mut Cpu, sys: &mut Sys, n: u32) -> f32 {
    f32::from_bits(arg(cpu, sys, n))
}
fn ret(cpu: &mut Cpu, v: u32) {
    cpu.hle_return(Some(v), None);
}
fn ret64(cpu: &mut Cpu, v: u64) {
    cpu.hle_return(Some(v as u32), Some((v >> 32) as u32));
}
fn retd(cpu: &mut Cpu, v: f64) {
    ret64(cpu, v.to_bits());
}
fn retf(cpu: &mut Cpu, v: f32) {
    ret(cpu, v.to_bits());
}
fn void(cpu: &mut Cpu) {
    cpu.hle_return(None, None);
}

fn f64_to_i32(x: f64) -> i32 {
    if x.is_nan() {
        0
    } else {
        x as i32
    }
}

/// Minimal printf implementation for ets_printf (integer/strings only).
pub fn format_printf(sys: &mut Sys, fmt: &[u8], mut next_arg: impl FnMut(&mut Sys) -> u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        if c != b'%' {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        if i >= fmt.len() {
            break;
        }
        let mut left = false;
        let mut zero = false;
        let mut plus = false;
        while i < fmt.len() && matches!(fmt[i], b'-' | b'0' | b'+' | b' ' | b'#') {
            match fmt[i] {
                b'-' => left = true,
                b'0' => zero = true,
                b'+' => plus = true,
                _ => {}
            }
            i += 1;
        }
        let mut width = 0usize;
        if i < fmt.len() && fmt[i] == b'*' {
            width = next_arg(sys) as usize;
            i += 1;
        }
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            width = width * 10 + (fmt[i] - b'0') as usize;
            i += 1;
        }
        let mut prec: Option<usize> = None;
        if i < fmt.len() && fmt[i] == b'.' {
            i += 1;
            let mut p = 0;
            if i < fmt.len() && fmt[i] == b'*' {
                p = next_arg(sys) as usize;
                i += 1;
            }
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                p = p * 10 + (fmt[i] - b'0') as usize;
                i += 1;
            }
            prec = Some(p);
        }
        let mut longs = 0;
        while i < fmt.len() && matches!(fmt[i], b'l' | b'h' | b'z' | b'j' | b't') {
            if fmt[i] == b'l' {
                longs += 1;
            }
            i += 1;
        }
        if i >= fmt.len() {
            break;
        }
        let conv = fmt[i];
        i += 1;
        let s: Vec<u8> = match conv {
            b'd' | b'i' => {
                let v: i64 = if longs >= 2 {
                    let lo = next_arg(sys) as u64;
                    let hi = next_arg(sys) as u64;
                    ((hi << 32) | lo) as i64
                } else {
                    next_arg(sys) as i32 as i64
                };
                let mut s = v.abs().to_string();
                if let Some(p) = prec {
                    while s.len() < p {
                        s.insert(0, '0');
                    }
                }
                if v < 0 {
                    s.insert(0, '-');
                } else if plus {
                    s.insert(0, '+');
                }
                s.into_bytes()
            }
            b'u' | b'x' | b'X' | b'o' => {
                let v: u64 = if longs >= 2 {
                    let lo = next_arg(sys) as u64;
                    let hi = next_arg(sys) as u64;
                    (hi << 32) | lo
                } else {
                    next_arg(sys) as u64
                };
                let mut s = match conv {
                    b'u' => v.to_string(),
                    b'x' => format!("{:x}", v),
                    b'X' => format!("{:X}", v),
                    _ => format!("{:o}", v),
                };
                if let Some(p) = prec {
                    while s.len() < p {
                        s.insert(0, '0');
                    }
                }
                s.into_bytes()
            }
            b'p' => format!("0x{:08x}", next_arg(sys)).into_bytes(),
            b'c' => vec![next_arg(sys) as u8],
            b's' => {
                let p = next_arg(sys);
                let mut s = if p == 0 { b"(null)".to_vec() } else { sys.read_cstr(p, 4096) };
                if let Some(pr) = prec {
                    s.truncate(pr);
                }
                s
            }
            b'f' | b'e' | b'g' => {
                // doubles are passed as two words
                let lo = next_arg(sys) as u64;
                let hi = next_arg(sys) as u64;
                let v = f64::from_bits((hi << 32) | lo);
                format!("{:.*}", prec.unwrap_or(6), v).into_bytes()
            }
            b'%' => b"%".to_vec(),
            _ => vec![b'%', conv],
        };
        if s.len() < width {
            let pad = width - s.len();
            if left {
                out.extend_from_slice(&s);
                out.extend(std::iter::repeat(b' ').take(pad));
            } else if zero && !matches!(conv, b's' | b'c') {
                let (sign, rest) = if !s.is_empty() && (s[0] == b'-' || s[0] == b'+') { (Some(s[0]), &s[1..]) } else { (None, &s[..]) };
                if let Some(sg) = sign {
                    out.push(sg);
                }
                out.extend(std::iter::repeat(b'0').take(pad));
                out.extend_from_slice(rest);
            } else {
                out.extend(std::iter::repeat(b' ').take(pad));
                out.extend_from_slice(&s);
            }
        } else {
            out.extend_from_slice(&s);
        }
    }
    out
}

/// strtol/strtoul-style parsing. Returns (value, bytes consumed) with C saturation semantics.
fn parse_int(b: &[u8], base: i32, unsigned: bool) -> (u32, usize) {
    let mut i = 0;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut base = base as u32;
    if (base == 0 || base == 16) && i + 1 < b.len() && b[i] == b'0' && (b[i + 1] | 0x20) == b'x' && i + 2 < b.len() && (b[i + 2] as char).is_ascii_hexdigit() {
        i += 2;
        base = 16;
    } else if base == 0 {
        base = if i < b.len() && b[i] == b'0' { 8 } else { 10 };
    }
    let start = i;
    let mut v: u64 = 0;
    let mut overflow = false;
    while i < b.len() {
        let d = match (b[i] as char).to_digit(base) {
            Some(d) => d,
            None => break,
        };
        v = v * base as u64 + d as u64;
        if v > u32::MAX as u64 * 2 {
            overflow = true;
        }
        i += 1;
    }
    if i == start {
        return (0, 0);
    }
    let r = if unsigned {
        if overflow || v > u32::MAX as u64 {
            u32::MAX
        } else if neg {
            (v as u32).wrapping_neg()
        } else {
            v as u32
        }
    } else {
        let lim = if neg { 0x8000_0000u64 } else { 0x7FFF_FFFF };
        if overflow || v > lim {
            if neg { 0x8000_0000 } else { 0x7FFF_FFFF }
        } else if neg {
            (v as i64).wrapping_neg() as u32
        } else {
            v as u32
        }
    };
    (r, i)
}

fn crc_generic(crc: u32, data: &[u8], poly: u32, width: u32, reflected: bool) -> u32 {
    let mask = if width == 32 { 0xFFFF_FFFF } else { (1u32 << width) - 1 };
    let mut c = !crc & mask;
    for &b in data {
        if reflected {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ poly } else { c >> 1 };
            }
        } else {
            c ^= (b as u32) << (width - 8);
            for _ in 0..8 {
                let top = c & (1 << (width - 1)) != 0;
                c = (c << 1) & mask;
                if top {
                    c ^= poly;
                }
            }
        }
    }
    !c & mask
}

// ------------------------------------------------------------------ MD5 (ROM context layout)
fn md5_transform(state: &mut [u32; 4], block: &[u8]) {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let m: Vec<u32> = (0..16).map(|i| u32::from_le_bytes(block[i * 4..i * 4 + 4].try_into().unwrap())).collect();
    let (mut a, mut b, mut c, mut d) = (state[0], state[1], state[2], state[3]);
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let f2 = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(f2.rotate_left(S[i]));
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

struct Md5Ctx {
    buf: [u32; 4],
    bits: u64,
    inb: [u8; 64],
}
fn md5_load(sys: &mut Sys, p: u32) -> Md5Ctx {
    let mut buf = [0u32; 4];
    for (i, x) in buf.iter_mut().enumerate() {
        *x = sys.peek32(p + 4 * i as u32);
    }
    let bits = sys.peek32(p + 16) as u64 | ((sys.peek32(p + 20) as u64) << 32);
    let mut inb = [0u8; 64];
    for (i, x) in inb.iter_mut().enumerate() {
        *x = sys.peek8(p + 24 + i as u32);
    }
    Md5Ctx { buf, bits, inb }
}
fn md5_store(sys: &mut Sys, p: u32, c: &Md5Ctx) {
    for i in 0..4 {
        sys.poke32(p + 4 * i as u32, c.buf[i]);
    }
    sys.poke32(p + 16, c.bits as u32);
    sys.poke32(p + 20, (c.bits >> 32) as u32);
    sys.write_bytes(p + 24, &c.inb);
}
fn md5_update(c: &mut Md5Ctx, data: &[u8]) {
    for &b in data {
        let idx = ((c.bits >> 3) & 63) as usize;
        c.inb[idx] = b;
        c.bits += 8;
        if idx == 63 {
            let blk = c.inb;
            md5_transform(&mut c.buf, &blk);
        }
    }
}

/// Emulate the ROM function at `pc`.
pub fn call(table: &RomTable, cpu: &mut Cpu, sys: &mut Sys) -> HleOutcome {
    let pc = cpu.pc;
    if (PHY_STUB_BASE..PHY_STUB_BASE + 4 * PHY_TABLE_ENTRIES).contains(&pc) {
        // radio calibration helpers: no radio hardware in the emulator
        ret(cpu, 0);
        return HleOutcome::Done;
    }
    let name = match table.name(pc) {
        Some(n) => n,
        None => return HleOutcome::Unsupported(format!("jump into ROM at 0x{:08x} (no known function)", pc)),
    };
    macro_rules! a {
        ($n:expr) => {
            arg(cpu, sys, $n)
        };
    }
    let core = cpu.core_id as usize;
    match name {
        // hottest first: string patterns are compared one by one
        "_xtos_set_intlevel" => {
            let lvl = a!(0);
            let ps = cpu.sr[xtensa_core::cpu::PS];
            cpu.sr[xtensa_core::cpu::PS] = (ps & !0xF) | (lvl & 0xF);
            ret(cpu, ps);
        }
        // ---------------------------------------------------------------- memory / strings
        "memcpy" | "memmove" => {
            let (d, s, n) = (a!(0), a!(1), a!(2));
            let data = sys.read_bytes(s, n as usize);
            sys.write_bytes(d, &data);
            cpu.advance_cycles(n / 4 + 10);
            ret(cpu, d);
        }
        "memset" => {
            let (d, c, n) = (a!(0), a!(1), a!(2));
            for i in 0..n {
                sys.poke8(d + i, c as u8);
            }
            cpu.advance_cycles(n / 4 + 10);
            ret(cpu, d);
        }
        "bzero" => {
            let (d, n) = (a!(0), a!(1));
            for i in 0..n {
                sys.poke8(d + i, 0);
            }
            void(cpu);
        }
        "memcmp" => {
            let (p, q, n) = (a!(0), a!(1), a!(2));
            let mut r = 0i32;
            for i in 0..n {
                let (x, y) = (sys.peek8(p + i), sys.peek8(q + i));
                if x != y {
                    r = x as i32 - y as i32;
                    break;
                }
            }
            ret(cpu, r as u32);
        }
        "memchr" => {
            let (p, c, n) = (a!(0), a!(1), a!(2));
            let mut r = 0;
            for i in 0..n {
                if sys.peek8(p + i) == c as u8 {
                    r = p + i;
                    break;
                }
            }
            ret(cpu, r);
        }
        "strlen" => {
            let p = a!(0);
            let n = sys.read_cstr(p, 1 << 20).len() as u32;
            ret(cpu, n);
        }
        "strlcpy" | "strlcat" => {
            let (d, src, n) = (a!(0), a!(1), a!(2));
            let sv = sys.read_cstr(src, 1 << 20);
            let dl = if name == "strlcat" { sys.read_cstr(d, n as usize).len() } else { 0 };
            if (n as usize) > dl {
                let room = n as usize - dl - 1;
                let mut v: Vec<u8> = sv.iter().take(room).cloned().collect();
                v.push(0);
                sys.write_bytes(d + dl as u32, &v);
            }
            ret(cpu, (dl + sv.len()) as u32);
        }
        "strspn" | "strcspn" => {
            let (p, set) = (a!(0), a!(1));
            let sv = sys.read_cstr(p, 1 << 20);
            let cs = sys.read_cstr(set, 4096);
            let n = if name == "strspn" {
                sv.iter().take_while(|c| cs.contains(c)).count()
            } else {
                sv.iter().take_while(|c| !cs.contains(c)).count()
            };
            ret(cpu, n as u32);
        }
        "memccpy" => {
            let (d, s2, c, n) = (a!(0), a!(1), a!(2) as u8, a!(3));
            let mut r = 0;
            for i in 0..n {
                let b = sys.peek8(s2 + i);
                sys.poke8(d + i, b);
                if b == c {
                    r = d + i + 1;
                    break;
                }
            }
            ret(cpu, r);
        }
        "memrchr" => {
            let (p, c, n) = (a!(0), a!(1) as u8, a!(2));
            let mut r = 0;
            for i in (0..n).rev() {
                if sys.peek8(p + i) == c {
                    r = p + i;
                    break;
                }
            }
            ret(cpu, r);
        }
        "strlwr" | "strupr" => {
            let p = a!(0);
            let sv = sys.read_cstr(p, 1 << 20);
            for (i, c) in sv.iter().enumerate() {
                let v = if name == "strlwr" { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() };
                sys.poke8(p + i as u32, v);
            }
            ret(cpu, p);
        }
        "strcoll" => {
            let (p, q) = (a!(0), a!(1));
            let (x, y) = (sys.read_cstr(p, 1 << 20), sys.read_cstr(q, 1 << 20));
            ret(cpu, match x.cmp(&y) { std::cmp::Ordering::Less => -1i32 as u32, std::cmp::Ordering::Equal => 0, _ => 1 });
        }
        "strsep" => {
            // char *strsep(char **stringp, const char *delim)
            let (pp, delim) = (a!(0), a!(1));
            let start = sys.peek32(pp);
            if start == 0 {
                ret(cpu, 0);
            } else {
                let ds = sys.read_cstr(delim, 256);
                let sv = sys.read_cstr(start, 1 << 20);
                match sv.iter().position(|c| ds.contains(c)) {
                    Some(i) => {
                        sys.poke8(start + i as u32, 0);
                        sys.poke32(pp, start + i as u32 + 1);
                    }
                    None => sys.poke32(pp, 0),
                }
                ret(cpu, start);
            }
        }
        "strtok_r" | "__strtok_r" => {
            // char *strtok_r(char *s, const char *delim, char **lasts)
            let (s0, delim, lasts) = (a!(0), a!(1), a!(2));
            let ds = sys.read_cstr(delim, 256);
            let mut p = if s0 != 0 { s0 } else { sys.peek32(lasts) };
            if p == 0 {
                ret(cpu, 0);
            } else {
                while sys.peek8(p) != 0 && ds.contains(&sys.peek8(p)) {
                    p += 1;
                }
                if sys.peek8(p) == 0 {
                    sys.poke32(lasts, 0);
                    ret(cpu, 0);
                } else {
                    let tok = p;
                    while sys.peek8(p) != 0 && !ds.contains(&sys.peek8(p)) {
                        p += 1;
                    }
                    if sys.peek8(p) != 0 {
                        sys.poke8(p, 0);
                        sys.poke32(lasts, p + 1);
                    } else {
                        sys.poke32(lasts, 0);
                    }
                    ret(cpu, tok);
                }
            }
        }
        "strtol" | "strtoul" | "_strtol_r" | "_strtoul_r" => {
            let sh = if name.starts_with('_') { 1 } else { 0 };
            let (np, endp, base) = (a!(sh), a!(sh + 1), a!(sh + 2) as i32);
            let bytes = sys.read_cstr(np, 256);
            let (v, used) = parse_int(&bytes, base, name.contains("ul"));
            if endp != 0 {
                sys.poke32(endp, np + used as u32);
            }
            ret(cpu, v);
        }
        "div" | "ldiv" => {
            // struct return: a2 = pointer to result (windowed ABI passes hidden pointer first)
            let (res, n, d) = (a!(0), a!(1) as i32, a!(2) as i32);
            let (q, r) = if d == 0 { (0, 0) } else { (n.wrapping_div(d), n.wrapping_rem(d)) };
            sys.poke32(res, q as u32);
            sys.poke32(res + 4, r as u32);
            ret(cpu, res);
        }
        "isascii" => {
            let c = a!(0);
            ret(cpu, (c < 128) as u32)
        }
        "toascii" => {
            let c = a!(0);
            ret(cpu, c & 0x7F)
        }
        "srand" => {
            let seed = a!(0);
            sys.rom_rand = seed as u64;
            void(cpu);
        }
        "rand" => {
            sys.rom_rand = sys.rom_rand.wrapping_mul(6364136223846793005).wrapping_add(1);
            let v = ((sys.rom_rand >> 32) & 0x7FFF_FFFF) as u32;
            ret(cpu, v);
        }
        "rand_r" => {
            let p = a!(0);
            let mut st = sys.peek32(p);
            st = st.wrapping_mul(1103515245).wrapping_add(12345);
            sys.poke32(p, st);
            ret(cpu, (st >> 1) & 0x7FFF_FFFF);
        }
        "_atoi_r" | "_atol_r" => {
            let p = a!(1);
            let bytes = sys.read_cstr(p, 64);
            let (v, _) = parse_int(&bytes, 10, false);
            ret(cpu, v);
        }
        "strnlen" => {
            let (p, m) = (a!(0), a!(1));
            let n = sys.read_cstr(p, m as usize).len() as u32;
            ret(cpu, n);
        }
        "strcpy" => {
            let (d, s) = (a!(0), a!(1));
            let mut v = sys.read_cstr(s, 1 << 20);
            v.push(0);
            sys.write_bytes(d, &v);
            ret(cpu, d);
        }
        "strncpy" => {
            let (d, s, n) = (a!(0), a!(1), a!(2));
            let mut v = sys.read_cstr(s, n as usize);
            v.resize(n as usize, 0);
            sys.write_bytes(d, &v);
            ret(cpu, d);
        }
        "strcat" | "strncat" => {
            let (d, s) = (a!(0), a!(1));
            let lim = if name == "strncat" { a!(2) as usize } else { 1 << 20 };
            let dl = sys.read_cstr(d, 1 << 20).len() as u32;
            let mut v = sys.read_cstr(s, lim);
            v.push(0);
            sys.write_bytes(d + dl, &v);
            ret(cpu, d);
        }
        "strcmp" | "strncmp" | "strcasecmp" | "strncasecmp" => {
            let (p, q) = (a!(0), a!(1));
            let lim = if name.starts_with("strn") { a!(2) as usize } else { usize::MAX };
            let ci = name.contains("case");
            let mut r = 0i32;
            let mut i = 0u32;
            while (i as usize) < lim {
                let (mut x, mut y) = (sys.peek8(p + i), sys.peek8(q + i));
                if ci {
                    x = x.to_ascii_lowercase();
                    y = y.to_ascii_lowercase();
                }
                if x != y {
                    r = x as i32 - y as i32;
                    break;
                }
                if x == 0 {
                    break;
                }
                i += 1;
            }
            ret(cpu, r as u32);
        }
        "strchr" | "strrchr" => {
            let (p, c) = (a!(0), a!(1) as u8);
            let s = sys.read_cstr(p, 1 << 20);
            let r = if c == 0 {
                p + s.len() as u32
            } else {
                let pos = if name == "strchr" { s.iter().position(|&x| x == c) } else { s.iter().rposition(|&x| x == c) };
                pos.map(|i| p + i as u32).unwrap_or(0)
            };
            ret(cpu, r);
        }
        "strstr" => {
            let (h, n) = (a!(0), a!(1));
            let hs = sys.read_cstr(h, 1 << 20);
            let ns = sys.read_cstr(n, 1 << 20);
            let r = if ns.is_empty() { h } else { hs.windows(ns.len()).position(|w| w == &ns[..]).map(|i| h + i as u32).unwrap_or(0) };
            ret(cpu, r);
        }
        "toupper" => {
            let c = a!(0);
            ret(cpu, (c as u8).to_ascii_uppercase() as u32)
        }
        "tolower" => {
            let c = a!(0);
            ret(cpu, (c as u8).to_ascii_lowercase() as u32)
        }
        "isalnum" | "isalpha" | "isdigit" | "isspace" | "isxdigit" | "isupper" | "islower" => {
            let c = a!(0) as u8;
            let r = match name {
                "isalnum" => c.is_ascii_alphanumeric(),
                "isalpha" => c.is_ascii_alphabetic(),
                "isdigit" => c.is_ascii_digit(),
                "isspace" => c.is_ascii_whitespace(),
                "isxdigit" => c.is_ascii_hexdigit(),
                "isupper" => c.is_ascii_uppercase(),
                _ => c.is_ascii_lowercase(),
            };
            ret(cpu, r as u32);
        }
        "atoi" | "atol" => {
            let p = a!(0);
            let bytes = sys.read_cstr(p, 64);
            let (v, _) = parse_int(&bytes, 10, false);
            ret(cpu, v);
        }
        "abs" | "labs" => {
            let v = a!(0) as i32;
            ret(cpu, v.wrapping_abs() as u32)
        }
        "itoa" | "utoa" | "__itoa" | "__utoa" => {
            let (v, p, radix) = (a!(0), a!(1), a!(2));
            let s = if name == "itoa" && (v as i32) < 0 && radix == 10 {
                format!("-{}", (v as i32).unsigned_abs())
            } else {
                let mut n = v as u64;
                let r = radix.max(2) as u64;
                let mut s = Vec::new();
                loop {
                    s.push(std::char::from_digit((n % r) as u32, r as u32).unwrap());
                    n /= r;
                    if n == 0 {
                        break;
                    }
                }
                s.iter().rev().collect()
            };
            let mut b = s.into_bytes();
            b.push(0);
            sys.write_bytes(p, &b);
            ret(cpu, p);
        }
        // ---------------------------------------------------------------- soft float (double)
        "__adddf3" | "__subdf3" | "__muldf3" | "__divdf3" => {
            let x = d_arg(cpu, sys, 0);
            let y = d_arg(cpu, sys, 2);
            let r = match name {
                "__adddf3" => x + y,
                "__subdf3" => x - y,
                "__muldf3" => x * y,
                _ => x / y,
            };
            cpu.advance_cycles(40);
            retd(cpu, r);
        }
        "__negdf2" => {
            let x = d_arg(cpu, sys, 0);
            retd(cpu, -x);
        }
        "__eqdf2" | "__nedf2" => {
            let (x, y) = (d_arg(cpu, sys, 0), d_arg(cpu, sys, 2));
            ret(cpu, if x == y { 0 } else { 1 });
        }
        "__gedf2" | "__gtdf2" => {
            let (x, y) = (d_arg(cpu, sys, 0), d_arg(cpu, sys, 2));
            let r: i32 = if x.is_nan() || y.is_nan() {
                -1
            } else if x > y {
                1
            } else if x == y {
                0
            } else {
                -1
            };
            ret(cpu, r as u32);
        }
        "__ledf2" | "__ltdf2" => {
            let (x, y) = (d_arg(cpu, sys, 0), d_arg(cpu, sys, 2));
            let r: i32 = if x.is_nan() || y.is_nan() {
                1
            } else if x > y {
                1
            } else if x == y {
                0
            } else {
                -1
            };
            ret(cpu, r as u32);
        }
        "__unorddf2" => {
            let (x, y) = (d_arg(cpu, sys, 0), d_arg(cpu, sys, 2));
            ret(cpu, (x.is_nan() || y.is_nan()) as u32);
        }
        "__extendsfdf2" => {
            let x = f_arg(cpu, sys, 0);
            retd(cpu, x as f64);
        }
        "__truncdfsf2" => {
            let x = d_arg(cpu, sys, 0);
            retf(cpu, x as f32);
        }
        "__fixdfsi" => {
            let x = d_arg(cpu, sys, 0);
            ret(cpu, f64_to_i32(x) as u32);
        }
        "__fixunsdfsi" => {
            let x = d_arg(cpu, sys, 0);
            ret(cpu, if x.is_nan() || x < 0.0 { 0 } else { x as u32 });
        }
        "__fixdfdi" => {
            let x = d_arg(cpu, sys, 0);
            ret64(cpu, if x.is_nan() { 0 } else { x as i64 as u64 });
        }
        "__fixunsdfdi" => {
            let x = d_arg(cpu, sys, 0);
            ret64(cpu, if x.is_nan() || x < 0.0 { 0 } else { x as u64 });
        }
        "__floatsidf" => {
            let v = a!(0) as i32;
            retd(cpu, v as f64);
        }
        "__floatunsidf" => {
            let v = a!(0);
            retd(cpu, v as f64);
        }
        "__floatdidf" => {
            let v = l_arg(cpu, sys, 0) as i64;
            retd(cpu, v as f64);
        }
        "__floatundidf" => {
            let v = l_arg(cpu, sys, 0);
            retd(cpu, v as f64);
        }
        // ---------------------------------------------------------------- soft float (single)
        "__addsf3" | "__subsf3" | "__mulsf3" | "__divsf3" => {
            let x = f_arg(cpu, sys, 0);
            let y = f_arg(cpu, sys, 1);
            let r = match name {
                "__addsf3" => x + y,
                "__subsf3" => x - y,
                "__mulsf3" => x * y,
                _ => x / y,
            };
            cpu.advance_cycles(30);
            retf(cpu, r);
        }
        "__negsf2" => {
            let x = a!(0);
            ret(cpu, x ^ 0x8000_0000);
        }
        "__eqsf2" | "__nesf2" => {
            let (x, y) = (f_arg(cpu, sys, 0), f_arg(cpu, sys, 1));
            ret(cpu, if x == y { 0 } else { 1 });
        }
        "__gesf2" | "__gtsf2" => {
            let (x, y) = (f_arg(cpu, sys, 0), f_arg(cpu, sys, 1));
            let r: i32 = if x.is_nan() || y.is_nan() {
                -1
            } else if x > y {
                1
            } else if x == y {
                0
            } else {
                -1
            };
            ret(cpu, r as u32);
        }
        "__lesf2" | "__ltsf2" => {
            let (x, y) = (f_arg(cpu, sys, 0), f_arg(cpu, sys, 1));
            let r: i32 = if x.is_nan() || y.is_nan() {
                1
            } else if x > y {
                1
            } else if x == y {
                0
            } else {
                -1
            };
            ret(cpu, r as u32);
        }
        "__unordsf2" => {
            let (x, y) = (f_arg(cpu, sys, 0), f_arg(cpu, sys, 1));
            ret(cpu, (x.is_nan() || y.is_nan()) as u32);
        }
        "__fixsfsi" => {
            let x = f_arg(cpu, sys, 0);
            ret(cpu, if x.is_nan() { 0 } else { x as i32 as u32 });
        }
        "__fixunssfsi" => {
            let x = f_arg(cpu, sys, 0);
            ret(cpu, if x.is_nan() || x < 0.0 { 0 } else { x as u32 });
        }
        "__fixsfdi" => {
            let x = f_arg(cpu, sys, 0);
            ret64(cpu, if x.is_nan() { 0 } else { x as i64 as u64 });
        }
        "__fixunssfdi" => {
            let x = f_arg(cpu, sys, 0);
            ret64(cpu, if x.is_nan() || x < 0.0 { 0 } else { x as u64 });
        }
        "__floatsisf" => {
            let v = a!(0) as i32;
            retf(cpu, v as f32);
        }
        "__floatunsisf" => {
            let v = a!(0);
            retf(cpu, v as f32);
        }
        "__floatdisf" => {
            let v = l_arg(cpu, sys, 0) as i64;
            retf(cpu, v as f32);
        }
        "__floatundisf" => {
            let v = l_arg(cpu, sys, 0);
            retf(cpu, v as f32);
        }
        // ---------------------------------------------------------------- integer helpers
        "__divdi3" | "__moddi3" => {
            let x = l_arg(cpu, sys, 0) as i64;
            let y = l_arg(cpu, sys, 2) as i64;
            let r = if y == 0 {
                0
            } else if name == "__divdi3" {
                x.wrapping_div(y)
            } else {
                x.wrapping_rem(y)
            };
            cpu.advance_cycles(60);
            ret64(cpu, r as u64);
        }
        "__udivdi3" | "__umoddi3" => {
            let x = l_arg(cpu, sys, 0);
            let y = l_arg(cpu, sys, 2);
            let r = if y == 0 {
                0
            } else if name == "__udivdi3" {
                x / y
            } else {
                x % y
            };
            cpu.advance_cycles(60);
            ret64(cpu, r);
        }
        "__muldi3" => {
            let x = l_arg(cpu, sys, 0);
            let y = l_arg(cpu, sys, 2);
            ret64(cpu, x.wrapping_mul(y));
        }
        "__ashldi3" | "__ashrdi3" | "__lshrdi3" => {
            let x = l_arg(cpu, sys, 0);
            let s = a!(2) & 63;
            let r = match name {
                "__ashldi3" => x << s,
                "__ashrdi3" => ((x as i64) >> s) as u64,
                _ => x >> s,
            };
            ret64(cpu, r);
        }
        "__negdi2" => {
            let x = l_arg(cpu, sys, 0) as i64;
            ret64(cpu, x.wrapping_neg() as u64);
        }
        "__cmpdi2" => {
            let x = l_arg(cpu, sys, 0) as i64;
            let y = l_arg(cpu, sys, 2) as i64;
            ret(cpu, if x < y { 0 } else if x == y { 1 } else { 2 });
        }
        "__ucmpdi2" => {
            let x = l_arg(cpu, sys, 0);
            let y = l_arg(cpu, sys, 2);
            ret(cpu, if x < y { 0 } else if x == y { 1 } else { 2 });
        }
        "__bswapsi2" => {
            let x = a!(0);
            ret(cpu, x.swap_bytes());
        }
        "__bswapdi2" => {
            let x = l_arg(cpu, sys, 0);
            ret64(cpu, x.swap_bytes());
        }
        "__popcountsi2" => {
            let x = a!(0);
            ret(cpu, x.count_ones());
        }
        "__popcountdi2" => {
            let x = l_arg(cpu, sys, 0);
            ret(cpu, x.count_ones());
        }
        "__paritysi2" => {
            let x = a!(0);
            ret(cpu, x.count_ones() & 1);
        }
        "__clzsi2" => {
            let x = a!(0);
            ret(cpu, x.leading_zeros());
        }
        "__ctzsi2" => {
            let x = a!(0);
            ret(cpu, x.trailing_zeros());
        }
        "__clzdi2" => {
            let x = l_arg(cpu, sys, 0);
            ret(cpu, x.leading_zeros());
        }
        "__ctzdi2" => {
            let x = l_arg(cpu, sys, 0);
            ret(cpu, x.trailing_zeros());
        }
        "__ffssi2" => {
            let x = a!(0);
            ret(cpu, if x == 0 { 0 } else { x.trailing_zeros() + 1 });
        }
        "__ffsdi2" => {
            let x = l_arg(cpu, sys, 0);
            ret(cpu, if x == 0 { 0 } else { x.trailing_zeros() + 1 });
        }
        "__divsi3" | "__modsi3" => {
            let (x, y) = (a!(0) as i32, a!(1) as i32);
            let r = if y == 0 {
                0
            } else if name == "__divsi3" {
                x.wrapping_div(y)
            } else {
                x.wrapping_rem(y)
            };
            ret(cpu, r as u32);
        }
        "__udivsi3" | "__umodsi3" => {
            let (x, y) = (a!(0), a!(1));
            let r = if y == 0 {
                0
            } else if name == "__udivsi3" {
                x / y
            } else {
                x % y
            };
            ret(cpu, r);
        }
        "__mulsi3" => {
            let (x, y) = (a!(0), a!(1));
            ret(cpu, x.wrapping_mul(y));
        }
        "__absvsi2" => {
            let x = a!(0) as i32;
            ret(cpu, x.wrapping_abs() as u32);
        }
        "__absvdi2" => {
            let x = l_arg(cpu, sys, 0) as i64;
            ret64(cpu, x.wrapping_abs() as u64);
        }
        // ---------------------------------------------------------------- console
        "ets_printf" => {
            let fmt_p = a!(0);
            let fmt = sys.read_cstr(fmt_p, 4096);
            let mut idx = 1u32;
            // collect up to 16 args eagerly (windowed ABI: a3..a7, then stack)
            let mut args = Vec::new();
            for k in 1..20 {
                args.push(arg(cpu, sys, k));
            }
            let out = format_printf(sys, &fmt, |_s| {
                let v = *args.get((idx - 1) as usize).unwrap_or(&0);
                idx += 1;
                v
            });
            let now = sys.now;
            for b in &out {
                sys.io.uart_tx(now, 0, *b);
            }
            ret(cpu, out.len() as u32);
        }
        "uart_tx_one_char" | "uart_tx_one_char2" | "ets_write_char_uart" => {
            let c = a!(0);
            let now = sys.now;
            sys.io.uart_tx(now, 0, c as u8);
            ret(cpu, 0);
        }
        "uart_rx_one_char" => {
            let p = a!(0);
            let now = sys.now;
            match sys.uart[0].rx.pop_front() {
                Some(b) => {
                    sys.poke8(p, b);
                    ret(cpu, 0)
                }
                None => match sys.io.uart_rx(now, 0) {
                    Some(b) => {
                        sys.poke8(p, b);
                        ret(cpu, 0)
                    }
                    None => ret(cpu, 1),
                },
            }
        }
        "uart_tx_flush" | "uart_tx_switch" | "uart_div_modify" | "uart_tx_wait_idle" | "ets_install_putc1"
        | "ets_install_putc2" | "ets_install_uart_printf" | "ets_isr_mask" | "ets_isr_unmask" | "ets_intr_lock"
        | "ets_intr_unlock" | "gpio_pad_hold" | "gpio_pad_unhold" | "gpio_pad_set_drv" | "set_rtc_memory_crc"
        | "ets_timer_init" | "ets_timer_arm" | "ets_timer_disarm" | "ets_timer_setfn" | "ets_timer_arm_us"
        | "ets_timer_done" | "ets_sha_enable" | "ets_sha_disable" | "rtc_select_apb_bridge" | "__sinit"
        | "xthal_set_intclear" | "ets_isr_attach" | "uartAttach" | "Uart_Init" | "uart_buff_switch"
        | "ets_install_uart_printf_ex" | "uart_rx_intr_handler" | "ets_wdt_disable" | "ets_wdt_restore" | "ets_wdt_enable"
        | "ets_set_user_start" | "ets_set_startup_callback" | "ets_set_idle_cb" | "Cache_Read_Init" | "xthal_window_spill"
        | "ets_intr_unlock_rom" | "_cleanup_r" | "_cleanup" | "__sfp_lock_acquire" | "__sfp_lock_release" | "rom_phy_disable_agc" | "rom_phy_enable_agc" | "rom_phy_disable_cca" | "rom_phy_enable_cca" => void(cpu),
        // ---------------------------------------------------------------- timing / clocks
        "ets_delay_us" => {
            let us = a!(0) as u64;
            void(cpu);
            return HleOutcome::Delay(us * 1000);
        }
        "ets_get_cpu_frequency" | "ets_get_cpu_frequency_rom" => {
            // g_ticks_per_us_pro / g_ticks_per_us_app (ESP-IDF writes these directly)
            let v = sys.peek32(if core == 0 { 0x3FFE_01E0 } else { 0x3FFE_40F0 });
            ret(cpu, if v == 0 { sys.ticks_per_us[core] } else { v })
        }
        "ets_update_cpu_frequency" | "ets_update_cpu_frequency_rom" => {
            let mhz = a!(0);
            sys.ticks_per_us[core] = mhz;
            // g_ticks_per_us_pro / _app
            sys.poke32(if core == 0 { 0x3FFE_01E0 } else { 0x3FFE_40F0 }, mhz);
            void(cpu);
        }
        "ets_get_detected_xtal_freq" => ret(cpu, 40_000_000),
        "ets_get_xtal_scale" => ret(cpu, 0),
        "ets_get_apb_freq" => {
            let v = sys.apb_hz() as u32;
            ret(cpu, v)
        }
        "xthal_get_ccount" => {
            let v = cpu.ccount();
            ret(cpu, v)
        }
        "ets_efuse_get_spiconfig" => ret(cpu, 0),
        "ets_efuse_get_8M_clock" => ret(cpu, 0x80),
        "esp_rom_efuse_get_flash_gpio_info" => ret(cpu, 0),
        "rtc_get_reset_reason" | "esp_rom_get_reset_reason" => {
            let r = sys.rtc.reset_reason;
            ret(cpu, r)
        }
        "rtc_get_wakeup_cause" => ret(cpu, 0),
        "ets_set_appcpu_boot_addr" => {
            let addr = a!(0);
            sys.dport.appcpu_boot_addr = addr;
            void(cpu);
        }
        "ets_intr_count" => ret(cpu, 0),
        "xthal_get_interrupt" => {
            let v = cpu.interrupt();
            ret(cpu, v)
        }
        "software_reset" | "software_reset_cpu" | "esp_rom_software_reset_system" | "esp_rom_software_reset_cpu" => {
            sys.rtc.reset_reason = 0x0C;
            return HleOutcome::Reset;
        }
        "longjmp" | "__longjmp_tail" => {
            // Restore the frame that called setjmp from its spilled save areas
            // (layout of the Overflow handler used when spilling) and resume it with `val`.
            // `__longjmp_tail` is entered with JX from ESP-IDF's longjmp wrapper,
            // with the arguments in a2/a3 of the current window.
            let (jb, val) = if name == "__longjmp_tail" { (cpu.ar(2), cpu.ar(3)) } else { (a!(0), a!(1)) };
            if sys.peek32(jb) != 0x534A_4D50 {
                return HleOutcome::Unsupported("longjmp with a jmp_buf not created by setjmp".into());
            }
            let sp_c = sys.peek32(jb + 4);
            let n = sys.peek32(jb + 8);
            let ra = sys.peek32(jb + 12);
            if std::env::var("DEBUG_SJLJ").is_ok() {
                eprintln!("longjmp jb={:08x} sp_c={:08x} n={} ra={:08x} val={}", jb, sp_c, n, ra, val);
            }
            let nregs = (n.clamp(1, 3) * 4) as usize;
            let mut regs = [0u32; 12];
            for (i, r) in regs.iter_mut().enumerate().take(nregs) {
                *r = sys.peek32(jb + 16 + 4 * i as u32);
            }
            regs[1] = sp_c;
            let wb = cpu.sr[xtensa_core::cpu::WINDOWBASE];
            cpu.sr[xtensa_core::cpu::WINDOWSTART] = 1 << wb;
            for (i, r) in regs.iter().enumerate().take(nregs) {
                cpu.set_ar(i as u8, *r);
            }
            cpu.set_ar((4 * n + 2) as u8, if val == 0 { 1 } else { val });
            cpu.pc = (cpu.pc & 0xC000_0000) | (ra & 0x3FFF_FFFF);
            return HleOutcome::Done;
        }
        "abort" => {
            return HleOutcome::Unsupported("ROM abort() called".into());
        }
        // ---------------------------------------------------------------- analog I2C (PLL etc.)
        "rom_i2c_writeReg" | "i2c_writeReg" | "esp_rom_regi2c_write" => {
            let (block, host, reg, data) = (a!(0), a!(1), a!(2), a!(3));
            sys.misc.write(0xA000_0000 | (block << 16) | (host << 8) | reg, data & 0xFF);
            void(cpu);
        }
        "rom_i2c_readReg" | "i2c_readReg" | "esp_rom_regi2c_read" => {
            let (block, host, reg) = (a!(0), a!(1), a!(2));
            let v = sys.misc.read(0xA000_0000 | (block << 16) | (host << 8) | reg);
            ret(cpu, v);
        }
        "rom_i2c_writeReg_Mask" | "i2c_writeReg_Mask" => {
            let (block, host, reg, msb, lsb, data) = (a!(0), a!(1), a!(2), a!(3), a!(4), a!(5));
            let key = 0xA000_0000 | (block << 16) | (host << 8) | reg;
            let old = sys.misc.read(key);
            let width = msb.saturating_sub(lsb) + 1;
            let mask = ((1u32 << width) - 1) << lsb;
            sys.misc.write(key, (old & !mask) | ((data << lsb) & mask));
            void(cpu);
        }
        "rom_i2c_readReg_Mask" | "i2c_readReg_Mask" => {
            let (block, host, reg, msb, lsb) = (a!(0), a!(1), a!(2), a!(3), a!(4));
            let key = 0xA000_0000 | (block << 16) | (host << 8) | reg;
            let width = msb.saturating_sub(lsb) + 1;
            let mut v = (sys.misc.read(key) >> lsb) & ((1u32 << width) - 1);
            // APLL calibration done flag
            if block == 0x6D && reg == 3 {
                v = 1;
            }
            ret(cpu, v);
        }
        // ---------------------------------------------------------------- GPIO / interrupt matrix
        "gpio_matrix_in" => {
            let (gpio, sig, inv) = (a!(0), a!(1), a!(2));
            if (sig as usize) < sys.gpio.in_sel.len() {
                sys.gpio.in_sel[sig as usize] = (gpio & 0x3F) | ((inv & 1) << 6) | (1 << 7);
            }
            void(cpu);
        }
        "gpio_matrix_out" => {
            let (gpio, sig, out_inv, oen_inv) = (a!(0), a!(1), a!(2), a!(3));
            if (gpio as usize) < sys.gpio.out_sel.len() {
                sys.gpio.out_sel[gpio as usize] = (sig & 0x1FF) | ((out_inv & 1) << 9) | ((oen_inv & 1) << 11);
                sys.gpio.drive_dirty = true;
                let now = sys.now;
                sys.refresh_pwm(now);
            }
            void(cpu);
        }
        "gpio_pad_select_gpio" => {
            let g = a!(0) as usize;
            if g < 40 {
                sys.gpio.iomux[g] = (sys.gpio.iomux[g] & !(7 << 12)) | (2 << 12);
                sys.gpio.drive_dirty = true;
            }
            void(cpu);
        }
        "gpio_pad_pullup" | "gpio_pad_pulldown" => {
            let g = a!(0) as usize;
            if g < 40 {
                if name == "gpio_pad_pullup" {
                    sys.gpio.iomux[g] = (sys.gpio.iomux[g] | (1 << 8)) & !(1 << 7);
                } else {
                    sys.gpio.iomux[g] = (sys.gpio.iomux[g] | (1 << 7)) & !(1 << 8);
                }
                sys.gpio.drive_dirty = true;
            }
            void(cpu);
        }
        "gpio_output_set" => {
            let (set, clr, en, dis) = (a!(0), a!(1), a!(2), a!(3));
            sys.gpio.out |= set as u64;
            sys.gpio.out &= !(clr as u64);
            sys.gpio.enable |= en as u64;
            sys.gpio.enable &= !(dis as u64);
            sys.gpio.drive_dirty = true;
            void(cpu);
        }
        "gpio_output_set_high" => {
            let (set, clr, en, dis) = (a!(0), a!(1), a!(2), a!(3));
            sys.gpio.out |= ((set & 0xFF) as u64) << 32;
            sys.gpio.out &= !(((clr & 0xFF) as u64) << 32);
            sys.gpio.enable |= ((en & 0xFF) as u64) << 32;
            sys.gpio.enable &= !(((dis & 0xFF) as u64) << 32);
            sys.gpio.drive_dirty = true;
            void(cpu);
        }
        "gpio_input_get" | "gpio_input_get_high" => {
            let now = sys.now;
            let apb_hz = sys.apb_hz();
            let cpu_hz = sys.cpu_hz;
            let v = {
                let ctx = &mut crate::periph::Ctx { now, apb_hz, cpu_hz, io: &mut *sys.io, irq: &mut sys.irq };
                sys.gpio.sample_inputs(ctx)
            };
            ret(cpu, if name == "gpio_input_get" { v as u32 } else { (v >> 32) as u32 });
        }
        "intr_matrix_set" => {
            let (cpu_no, source, inum) = (a!(0), a!(1), a!(2));
            if (source as usize) < crate::periph::src::NUM {
                sys.irq.map[(cpu_no & 1) as usize][source as usize] = (inum & 0x1F) as u8;
                sys.irq.dirty = true;
            }
            void(cpu);
        }
        // ---------------------------------------------------------------- CRC / hashing
        "crc32_le" => {
            let (crc, p, n) = (a!(0), a!(1), a!(2));
            let d = sys.read_bytes(p, n as usize);
            ret(cpu, crc_generic(crc, &d, 0xEDB8_8320, 32, true));
        }
        "crc32_be" => {
            let (crc, p, n) = (a!(0), a!(1), a!(2));
            let d = sys.read_bytes(p, n as usize);
            ret(cpu, crc_generic(crc, &d, 0x04C1_1DB7, 32, false));
        }
        "crc16_le" => {
            let (crc, p, n) = (a!(0), a!(1), a!(2));
            let d = sys.read_bytes(p, n as usize);
            ret(cpu, crc_generic(crc & 0xFFFF, &d, 0x8408, 16, true));
        }
        "crc16_be" => {
            let (crc, p, n) = (a!(0), a!(1), a!(2));
            let d = sys.read_bytes(p, n as usize);
            ret(cpu, crc_generic(crc & 0xFFFF, &d, 0x1021, 16, false));
        }
        "crc8_le" => {
            let (crc, p, n) = (a!(0), a!(1), a!(2));
            let d = sys.read_bytes(p, n as usize);
            ret(cpu, crc_generic(crc & 0xFF, &d, 0xE0, 8, true));
        }
        "crc8_be" => {
            let (crc, p, n) = (a!(0), a!(1), a!(2));
            let d = sys.read_bytes(p, n as usize);
            ret(cpu, crc_generic(crc & 0xFF, &d, 0x07, 8, false));
        }
        "esp_crc8" => {
            let (p, n) = (a!(0), a!(1));
            let d = sys.read_bytes(p, n as usize);
            ret(cpu, crate::periph::efuse::esp_crc8(&d) as u32);
        }
        "MD5Init" | "esp_rom_md5_init" => {
            let p = a!(0);
            let c = Md5Ctx { buf: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], bits: 0, inb: [0; 64] };
            md5_store(sys, p, &c);
            void(cpu);
        }
        "MD5Update" | "esp_rom_md5_update" => {
            let (p, data, n) = (a!(0), a!(1), a!(2));
            let mut c = md5_load(sys, p);
            let d = sys.read_bytes(data, n as usize);
            md5_update(&mut c, &d);
            md5_store(sys, p, &c);
            cpu.advance_cycles(n * 2);
            void(cpu);
        }
        "MD5Final" | "esp_rom_md5_final" => {
            // ROM signature: MD5Final(unsigned char digest[16], MD5Context *ctx)
            let (digest, p) = (a!(0), a!(1));
            let mut c = md5_load(sys, p);
            let bits = c.bits;
            let mut pad = vec![0x80u8];
            while ((c.bits >> 3) as usize + pad.len()) % 64 != 56 {
                pad.push(0);
            }
            md5_update(&mut c, &pad);
            md5_update(&mut c, &bits.to_le_bytes());
            let mut out = Vec::new();
            for w in c.buf {
                out.extend_from_slice(&w.to_le_bytes());
            }
            sys.write_bytes(digest, &out);
            void(cpu);
        }
        "roundup2" => {
            let (x, a) = (a!(0), a!(1));
            ret(cpu, (x + a - 1) & !(a - 1))
        }
        // ---------------------------------------------------------------- cache / MMU
        "Cache_Read_Enable_rom" | "Cache_Read_Enable" => {
            let c = a!(0);
            let off = if c == 0 { 0x40 } else { 0x58 };
            let v = sys.dport.regs.read(off) | (1 << 3);
            sys.dport.regs.write(off, v);
            void(cpu);
        }
        "Cache_Read_Disable_rom" | "Cache_Read_Disable" => {
            let c = a!(0);
            let off = if c == 0 { 0x40 } else { 0x58 };
            let v = sys.dport.regs.read(off) & !(1 << 3);
            sys.dport.regs.write(off, v);
            void(cpu);
        }
        "Cache_Flush_rom" | "Cache_Flush" | "Cache_Read_Init_rom" | "Cache_Flush_rom_app" => void(cpu),
        "mmu_init" => {
            let c = (a!(0) & 1) as usize;
            for e in sys.dport.mmu[c].iter_mut() {
                *e = 0x100;
            }
            sys.dport.mmu_dirty = true;
            void(cpu);
        }
        "cache_flash_mmu_set_rom" | "cache_flash_mmu_set" => {
            let (cpu_no, _pid, vaddr, paddr, psize, num) = (a!(0), a!(1), a!(2), a!(3), a!(4), a!(5));
            let page = psize.max(1) * 1024;
            let r = if psize != 64 || vaddr % page != 0 || paddr % page != 0 {
                1
            } else {
                for i in 0..num {
                    let v = vaddr + i * 0x10000;
                    let idx = if (0x3F40_0000..0x3F80_0000).contains(&v) {
                        (v - 0x3F40_0000) >> 16
                    } else if (0x4000_0000..0x40C0_0000).contains(&v) {
                        64 + ((v - 0x4000_0000) >> 16)
                    } else {
                        continue;
                    };
                    sys.dport.mmu[(cpu_no & 1) as usize][idx as usize] = (paddr >> 16) + i;
                }
                sys.dport.mmu_dirty = true;
                0
            };
            ret(cpu, r);
        }
        "cache_sram_mmu_set_rom" | "cache_sram_mmu_set" => ret(cpu, 0),
        // ---------------------------------------------------------------- SPI flash (ROM driver)
        "esp_rom_spiflash_read" | "SPIRead" => {
            let (src, dst, len) = (a!(0), a!(1), a!(2));
            let mut buf = vec![0u8; len as usize];
            sys.flash.read(src, &mut buf);
            sys.write_bytes(dst, &buf);
            cpu.advance_cycles(len);
            ret(cpu, 0);
        }
        "esp_rom_spiflash_write" | "SPIWrite" => {
            let (dst, src, len) = (a!(0), a!(1), a!(2));
            let d = sys.read_bytes(src, len as usize);
            sys.flash.program(dst, &d);
            ret(cpu, 0);
        }
        "esp_rom_spiflash_erase_sector" | "SPIEraseSector" => {
            let s = a!(0);
            sys.flash.erase(s * 0x1000, 0x1000);
            ret(cpu, 0);
        }
        "esp_rom_spiflash_erase_block" | "SPIEraseBlock" => {
            let b = a!(0);
            sys.flash.erase(b * 0x10000, 0x10000);
            ret(cpu, 0);
        }
        "esp_rom_spiflash_erase_chip" => {
            let n = sys.flash.size();
            sys.flash.erase(0, n);
            ret(cpu, 0);
        }
        "esp_rom_spiflash_read_status" => {
            let (_chip, p) = (a!(0), a!(1));
            let st = sys.chip.status;
            sys.poke32(p, st);
            ret(cpu, 0);
        }
        "SPI_read_status_high" => {
            let p = a!(0);
            let st = sys.chip.status >> 8;
            sys.poke32(p, st);
            ret(cpu, 0);
        }
        "esp_rom_spiflash_config_param" | "SPIParamCfg" => {
            // (deviceId, chip_size, block_size, sector_size, page_size, status_mask)
            for i in 0..6u32 {
                let v = a!(i);
                sys.poke32(0x3FFA_E270 + 4 * i, v);
            }
            ret(cpu, 0);
        }
        "esp_rom_spiflash_unlock" | "SPIUnlock" | "esp_rom_spiflash_attach" | "spi_flash_attach" | "esp_rom_spiflash_config_clk"
        | "esp_rom_spiflash_wait_idle" | "Wait_SPI_Idle"
        | "esp_rom_spiflash_select_qio_pins" | "esp_rom_spiflash_write_encrypted_enable" | "esp_rom_spiflash_write_encrypted_disable"
        | "SPI_Write_Encrypt_Enable" | "SPI_Write_Encrypt_Disable" | "esp_rom_spiflash_prepare_encrypted_data"
        | "esp_rom_spiflash_config_readmode" | "ets_efuse_read_op" | "ets_efuse_program_op" | "rtc_boot_control" => ret(cpu, 0),
        "esp_rom_spiflash_read_user_cmd" => ret(cpu, 0),
        "phy_get_romfuncs" => {
            // table of ROM PHY/RF helper functions; entries point to stub functions that return 0
            let table = PHY_TABLE_ADDR;
            if sys.peek32(table) == 0 {
                for i in 0..PHY_TABLE_ENTRIES {
                    sys.poke32(table + 4 * i, PHY_STUB_BASE + 4 * i);
                }
            }
            ret(cpu, table)
        }
        n if crate::hle::is_coroutine_fn(n) => {
            return HleOutcome::Coroutine(n.to_string());
        }
        _ => {
            return HleOutcome::Unsupported(format!("ROM function {} (0x{:08x}) is not emulated", name, pc));
        }
    }
    HleOutcome::Done
}

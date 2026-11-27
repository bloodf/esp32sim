//! Test support: assemble mock firmware, boot it on a full `Machine` and
//! record everything the chip does toward the outside world.
#![allow(dead_code)]
use esp32_soc::io::{IoBridge, PinDrive, PwmState, RmtItem, SpiPins};
use esp32_soc::{Machine, MachineConfig};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// Everything the firmware did, plus the stimuli the mock circuit provides.
#[derive(Default)]
pub struct IoLog {
    pub uart_out: [Vec<u8>; 3],
    pub uart_in: [VecDeque<u8>; 3],
    pub inputs: HashMap<u8, bool>,
    pub adc: HashMap<u8, f32>,
    pub touched: HashSet<u8>,
    pub drives: Vec<PinDrive>,
    pub i2c: Vec<String>,
    /// addresses that ACK
    pub i2c_devices: HashSet<u8>,
    pub i2c_read_data: VecDeque<u8>,
    pub spi: Vec<(u8, SpiPins, Vec<u8>)>,
    pub spi_reply: VecDeque<u8>,
    pub rmt: Vec<(u8, Vec<RmtItem>)>,
    pub pwm: Vec<(u8, PwmState)>,
}

impl IoLog {
    pub fn uart0(&self) -> String {
        String::from_utf8_lossy(&self.uart_out[0]).to_string()
    }
    pub fn last_drive(&self) -> PinDrive {
        self.drives.last().cloned().unwrap_or_default()
    }
}

pub struct MockIo(pub Arc<Mutex<IoLog>>);

impl IoBridge for MockIo {
    fn gpio_update(&mut self, _now: u64, d: &PinDrive) {
        self.0.lock().unwrap().drives.push(d.clone());
    }
    fn gpio_read(&mut self, _now: u64, pin: u8) -> Option<bool> {
        self.0.lock().unwrap().inputs.get(&pin).copied()
    }
    fn adc_read(&mut self, _now: u64, pin: u8) -> f32 {
        *self.0.lock().unwrap().adc.get(&pin).unwrap_or(&0.0)
    }
    fn uart_tx(&mut self, _now: u64, port: u8, b: u8) {
        self.0.lock().unwrap().uart_out[port as usize].push(b);
    }
    fn uart_rx(&mut self, _now: u64, port: u8) -> Option<u8> {
        self.0.lock().unwrap().uart_in[port as usize].pop_front()
    }
    fn i2c_start(&mut self, _now: u64, _scl: Option<u8>, _sda: Option<u8>, addr: u8, read: bool) -> bool {
        let mut l = self.0.lock().unwrap();
        l.i2c.push(format!("S {:02x}{}", addr, if read { "r" } else { "w" }));
        l.i2c_devices.contains(&addr)
    }
    fn i2c_write(&mut self, _now: u64, _scl: Option<u8>, _sda: Option<u8>, b: u8) -> bool {
        self.0.lock().unwrap().i2c.push(format!("W {:02x}", b));
        true
    }
    fn i2c_read(&mut self, _now: u64, _scl: Option<u8>, _sda: Option<u8>, ack: bool) -> u8 {
        let mut l = self.0.lock().unwrap();
        let b = l.i2c_read_data.pop_front().unwrap_or(0xFF);
        l.i2c.push(format!("R {:02x}{}", b, if ack { "" } else { " nack" }));
        b
    }
    fn i2c_stop(&mut self, _now: u64, _scl: Option<u8>, _sda: Option<u8>) {
        self.0.lock().unwrap().i2c.push("P".into());
    }
    fn spi_transfer(&mut self, _now: u64, host: u8, pins: SpiPins, data: &mut [u8], _bits: u32) {
        let mut l = self.0.lock().unwrap();
        l.spi.push((host, pins, data.to_vec()));
        for b in data.iter_mut() {
            *b = l.spi_reply.pop_front().unwrap_or(0xFF);
        }
    }
    fn rmt_tx(&mut self, _now: u64, pin: u8, items: &[RmtItem]) {
        self.0.lock().unwrap().rmt.push((pin, items.to_vec()));
    }
    fn pwm_update(&mut self, _now: u64, pin: u8, pwm: PwmState) {
        self.0.lock().unwrap().pwm.push((pin, pwm));
    }
    fn touch_read(&mut self, _now: u64, pin: u8) -> bool {
        self.0.lock().unwrap().touched.contains(&pin)
    }
}

pub const CODE: u32 = 0x4008_0400;
/// Scratch area the test firmwares use to report results.
pub const RESULTS: u32 = 0x3FFB_0000;

/// Assemble `body` as the firmware entry function (called like the ROM calls
/// `call_start_cpu0`: windowed, with a stack) and return an app image.
pub fn image(body: &str) -> (Vec<u8>, xtensa_core::Program) {
    let rom = esp32_soc::rom::RomTable::load();
    let mut consts = String::new();
    for name in ["ets_set_appcpu_boot_addr", "ets_printf", "ets_delay_us", "rtc_get_reset_reason"] {
        if let Some(a) = rom.by_name.get(name) {
            consts.push_str(&format!(".equ {}, 0x{:08x}\n", name, a));
        }
    }
    let src = format!(
        "{consts}
        .equ RESULTS, 0x{RESULTS:08x}
        .equ UART0, 0x3ff40000
        .equ GPIO, 0x3ff44000
        _start: entry a1, 64
        {body}
        _halt:  j _halt
        "
    );
    let prog = xtensa_core::assemble(&src, CODE).unwrap_or_else(|e| panic!("{}", e));
    let img = esp32_soc::elf::build_app_image(prog.symbol("_start"), &[(CODE, prog.bytes.clone())]);
    (img, prog)
}

pub fn boot_image(img: &[u8]) -> (Machine, Arc<Mutex<IoLog>>) {
    let flash = esp32_soc::flash::build_flash(img, 4 << 20, None, None).expect("flash");
    let mut m = Machine::new(flash, MachineConfig::default());
    let log = Arc::new(Mutex::new(IoLog::default()));
    m.set_io(Box::new(MockIo(log.clone())));
    m.boot().expect("boot");
    (m, log)
}

/// Boot `body`; `prepare` may set up the mock circuit before the firmware runs.
pub fn boot_with(body: &str, prepare: impl FnOnce(&mut IoLog)) -> (Machine, Arc<Mutex<IoLog>>) {
    let (img, _) = image(body);
    let (m, log) = boot_image(&img);
    prepare(&mut log.lock().unwrap());
    (m, log)
}

pub fn boot(body: &str) -> (Machine, Arc<Mutex<IoLog>>) {
    boot_with(body, |_| {})
}

pub fn ms(n: u64) -> u64 {
    n * 1_000_000
}

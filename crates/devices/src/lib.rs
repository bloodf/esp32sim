//! Simulated circuit: wiring, electrical solver and device models that can be
//! attached to the emulated ESP32.

pub mod circuit;
pub mod config;
pub mod i2cbus;
pub mod parts;
pub mod registry;

pub use circuit::Board;
pub use config::ProjectConfig;
pub use esp32_soc::io::{PwmState, RmtItem};
use serde_json::Value;

/// Electrical element contributed by a device, expressed with the device's own pin indices.
#[derive(Clone, Debug)]
pub enum Stamp {
    /// Resistor between two pins.
    Resistor { a: usize, b: usize, ohms: f64 },
    /// Voltage source (relative to GND) driving pin `a` through an internal resistance.
    Source { a: usize, volts: f64, ohms: f64 },
    /// Diode / LED from anode `a` to cathode `k` (forward voltage, on-resistance).
    Diode { a: usize, k: usize, vf: f64, ron: f64, tag: u32 },
    /// Resistor from pin `a` to the supply net named `rail` ("3V3", "5V", "GND").
    ToRail { a: usize, rail: Rail, ohms: f64 },
    /// Capacitor between two pins (transient: charges through the circuit).
    Capacitor { a: usize, b: usize, farads: f64, tag: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rail {
    Gnd,
    V3,
    V5,
}

/// Pin electrical role (used by the editor and for sanity checks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PinKind {
    Power,
    Ground,
    Input,
    Output,
    Io,
    Passive,
    Analog,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PinInfo {
    pub name: &'static str,
    pub kind: PinKind,
}

pub const fn pin(name: &'static str, kind: PinKind) -> PinInfo {
    PinInfo { name, kind }
}

/// Result of solving the circuit, seen from one device.
pub struct PinView<'a> {
    pub volts: &'a [Option<f64>],
    /// Current through diodes, keyed by tag (amps).
    pub diode_current: &'a dyn Fn(u32) -> f64,
    /// PWM duty (0..1) driving each pin's net, if the net is driven by a PWM output.
    pub pwm: &'a [Option<PwmState>],
}

impl<'a> PinView<'a> {
    pub fn level(&self, pin: usize) -> Option<bool> {
        self.volts.get(pin).copied().flatten().map(|v| v > 1.65)
    }
    pub fn v(&self, pin: usize) -> Option<f64> {
        self.volts.get(pin).copied().flatten()
    }
}

/// A simulated part.
#[allow(unused_variables)]
pub trait Device: Send {
    /// Type name (as used in project files).
    fn kind(&self) -> &'static str;
    /// Pin names, in index order.
    fn pins(&self) -> Vec<PinInfo>;
    /// Electrical contribution in the current state.
    fn stamps(&self, out: &mut Vec<Stamp>) {}
    /// Called after every circuit solve that touched this device.
    fn on_solved(&mut self, now: u64, view: &PinView) {}
    /// Timed behaviour; returns the next time the device wants to be polled.
    fn poll(&mut self, now: u64) -> Option<u64> {
        None
    }
    /// True if the device's electrical stamps changed and the circuit must be re-solved.
    fn take_dirty(&mut self) -> bool {
        false
    }

    // ---- I2C (transaction level)
    /// (SCL pin index, SDA pin index) if the device is an I2C target.
    fn i2c_pins(&self) -> Option<(usize, usize)> {
        None
    }
    /// Address probe + start of a transaction. Return true to ACK.
    fn i2c_start(&mut self, addr: u8, read: bool) -> bool {
        false
    }
    fn i2c_write(&mut self, byte: u8) -> bool {
        true
    }
    fn i2c_read(&mut self, ack: bool) -> u8 {
        0xFF
    }
    fn i2c_stop(&mut self) {}

    // ---- SPI
    /// (SCK, MOSI, MISO, CS) pin indices if the device is an SPI target.
    fn spi_pins(&self) -> Option<(usize, usize, Option<usize>, usize)> {
        None
    }
    /// Full duplex transfer while selected; `data` in: MOSI, out: MISO.
    fn spi_transfer(&mut self, now: u64, data: &mut [u8], pin_levels: &dyn Fn(usize) -> bool) {
        for b in data.iter_mut() {
            *b = 0xFF;
        }
    }
    fn spi_select(&mut self, selected: bool) {}

    // ---- UART
    /// (RX pin index = data into the device, TX pin index = data out of the device)
    fn uart_pins(&self) -> Option<(usize, usize)> {
        None
    }
    fn uart_receive(&mut self, now: u64, byte: u8) {}
    fn uart_transmit(&mut self, now: u64) -> Option<u8> {
        None
    }

    // ---- other signals
    /// RMT-generated waveform arriving on a device pin (e.g. WS2812 DIN).
    fn waveform(&mut self, now: u64, pin: usize, items: &[RmtItem]) {}
    /// PWM signal on a device pin changed.
    fn pwm(&mut self, now: u64, pin: usize, pwm: PwmState) {}
    /// Capacitive touch: is a finger on the electrode connected to `pin`?
    fn touched(&self, pin: usize) -> bool {
        false
    }

    // ---- user interface
    /// Current visual state (for the GUI / console).
    fn state(&self) -> Value {
        Value::Null
    }
    /// User interaction (button press, sensor value change, ...).
    fn input(&mut self, now: u64, action: &Value) -> Result<(), String> {
        Err(format!("{} does not accept input", self.kind()))
    }
    /// Short human readable status line.
    fn summary(&self) -> String {
        String::new()
    }
    /// Colour framebuffer (width, height, RGB888) for graphical displays.
    fn framebuffer(&self) -> Option<(usize, usize, Vec<u8>)> {
        None
    }
}

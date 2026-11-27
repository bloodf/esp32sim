//! Interface between the SoC peripherals and the outside world (the simulated circuit).

pub const NUM_GPIO: usize = 40;

/// How the ESP32 is driving its pads.
#[derive(Clone, Debug, PartialEq)]
pub struct PinDrive {
    /// bit n set: pad n output enabled
    pub out_en: u64,
    /// output level for enabled pads
    pub out_level: u64,
    /// open-drain pads (only drive low)
    pub open_drain: u64,
    pub pull_up: u64,
    pub pull_down: u64,
    /// input enabled
    pub in_en: u64,
    /// Peripheral signal currently routed to each pad's output (256 = plain GPIO)
    pub out_signal: [u16; NUM_GPIO],
    /// Pads driven by a DAC (GPIO25/26) -> volts
    pub dac: [Option<f32>; 2],
    /// Pad feeding each UART's RX signal (from the GPIO matrix), if routed.
    pub uart_rx_pads: [Option<u8>; 3],
}

impl Default for PinDrive {
    fn default() -> Self {
        PinDrive {
            out_en: 0,
            out_level: 0,
            open_drain: 0,
            pull_up: 0,
            pull_down: 0,
            in_en: 0,
            out_signal: [0x100; NUM_GPIO],
            dac: [None; 2],
            uart_rx_pads: [None; 3],
        }
    }
}

impl PinDrive {
    pub fn is_output(&self, pin: u8) -> bool {
        self.out_en & (1 << pin) != 0
    }
    pub fn level(&self, pin: u8) -> bool {
        self.out_level & (1 << pin) != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RmtItem {
    pub level: bool,
    pub duration_ns: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpiPins {
    pub sclk: Option<u8>,
    pub mosi: Option<u8>,
    pub miso: Option<u8>,
    /// chip select pads that are asserted (active low) during the transfer
    pub cs: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PwmState {
    pub freq_hz: f64,
    /// 0.0 ..= 1.0
    pub duty: f64,
    pub running: bool,
}

/// Events and callbacks toward the external circuit.  All methods have
/// default implementations so a bare board (nothing connected) works.
#[allow(unused_variables)]
pub trait IoBridge: Send {
    /// Pad drive state changed.
    fn gpio_update(&mut self, now_ns: u64, drive: &PinDrive) {}
    /// Logic level observed at a pad input.  `None` means nothing external
    /// drives the pin (the SoC then uses its own output / pull resistors).
    fn gpio_read(&mut self, now_ns: u64, pin: u8) -> Option<bool> {
        None
    }
    /// Analog voltage on a pad (ADC), in volts.
    fn adc_read(&mut self, now_ns: u64, pin: u8) -> f32 {
        0.0
    }
    /// Byte transmitted by a UART (port 0..2).
    fn uart_tx(&mut self, now_ns: u64, port: u8, byte: u8) {}
    /// Fetch a pending byte for a UART receiver.
    fn uart_rx(&mut self, now_ns: u64, port: u8) -> Option<u8> {
        None
    }
    /// I2C transaction primitives for the hardware I2C controllers.
    /// `scl`/`sda` are the pads the controller is routed to.
    fn i2c_start(&mut self, now_ns: u64, scl: Option<u8>, sda: Option<u8>, addr7: u8, read: bool) -> bool {
        false
    }
    fn i2c_write(&mut self, now_ns: u64, scl: Option<u8>, sda: Option<u8>, byte: u8) -> bool {
        false
    }
    fn i2c_read(&mut self, now_ns: u64, scl: Option<u8>, sda: Option<u8>, ack: bool) -> u8 {
        0xFF
    }
    fn i2c_stop(&mut self, now_ns: u64, scl: Option<u8>, sda: Option<u8>) {}
    /// Full-duplex SPI transfer.  `data` holds MOSI bytes on entry and MISO bytes on return.
    fn spi_transfer(&mut self, now_ns: u64, host: u8, pins: SpiPins, data: &mut [u8], bits: u32) {
        for b in data.iter_mut() {
            *b = 0xFF;
        }
    }
    /// RMT waveform transmitted on a pad (e.g. WS2812 data).
    fn rmt_tx(&mut self, now_ns: u64, pin: u8, items: &[RmtItem]) {}
    /// PWM (LEDC) output on a pad changed.
    fn pwm_update(&mut self, now_ns: u64, pin: u8, pwm: PwmState) {}
    /// Called regularly; returns the time of the next scheduled external event, if any.
    fn poll(&mut self, now_ns: u64) -> Option<u64> {
        None
    }
    /// Touch sensor: returns true if the pad is touched.
    fn touch_read(&mut self, now_ns: u64, pin: u8) -> bool {
        false
    }
    /// Downcasting support (lets the simulator reach its concrete board).
    fn as_any(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
}

/// A bridge that only captures UART0 output and feeds UART0 input (no circuit).
#[derive(Default)]
pub struct NullIo {
    pub uart_out: Vec<u8>,
    pub uart_in: std::collections::VecDeque<u8>,
}

impl IoBridge for NullIo {
    fn uart_tx(&mut self, _now: u64, port: u8, byte: u8) {
        if port == 0 {
            self.uart_out.push(byte);
        }
    }
    fn uart_rx(&mut self, _now: u64, port: u8) -> Option<u8> {
        if port == 0 {
            self.uart_in.pop_front()
        } else {
            None
        }
    }
}

//! Pin-level I2C target decoder: lets bit-banged (software) I2C masters talk to
//! the same transaction-level device models used by the hardware controller.

use crate::circuit::Board;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum St {
    #[default]
    Idle,
    Addr,
    Write,
    Read,
    /// addressed nobody / NACKed: wait for STOP or repeated START
    Ignore,
}

#[derive(Clone, Debug, Default)]
pub struct SoftI2c {
    pub scl: usize,
    pub sda: usize,
    last_scl: bool,
    last_sda: bool,
    st: St,
    /// rising edges seen in the current byte (0..=9)
    n: u8,
    shift: u8,
    target: Option<usize>,
    /// byte being shifted out in Read state
    out: u8,
    master_acked: bool,
    rd: bool,
    /// true while we pull SDA low
    pub drive_low: bool,
}

impl SoftI2c {
    pub fn new(scl: usize, sda: usize) -> SoftI2c {
        SoftI2c { scl, sda, last_scl: true, last_sda: true, ..Default::default() }
    }

    /// Feed new line levels. Returns true if our SDA drive changed.
    pub fn update(&mut self, board: &mut Board, scl: bool, sda: bool) -> bool {
        let before = self.drive_low;
        if scl && self.last_scl && sda != self.last_sda {
            if !sda {
                // (repeated) START
                if let Some(t) = self.target.take() {
                    // repeated start keeps the transaction; devices treat a new start() as restart
                    let _ = t;
                }
                self.st = St::Addr;
                self.n = 0;
                self.shift = 0;
                self.drive_low = false;
            } else {
                // STOP
                if let Some(t) = self.target.take() {
                    board.comp_mut(t).i2c_stop();
                }
                self.st = St::Idle;
                self.drive_low = false;
            }
        } else if scl && !self.last_scl {
            // rising edge: sample
            match self.st {
                St::Addr | St::Write => {
                    if self.n < 8 {
                        self.shift = (self.shift << 1) | sda as u8;
                    }
                    self.n += 1;
                }
                St::Read => {
                    if self.n == 8 {
                        self.master_acked = !sda;
                    }
                    self.n += 1;
                }
                _ => {}
            }
        } else if !scl && self.last_scl {
            // falling edge: drive phase
            match self.st {
                St::Addr => {
                    if self.n == 8 {
                        let addr = self.shift >> 1;
                        let rd = self.shift & 1 != 0;
                        self.rd = rd;
                        let (scl_net, sda_net) = (self.scl, self.sda);
                        match board.i2c_bus_start(scl_net, sda_net, addr, rd) {
                            Some(t) => {
                                self.target = Some(t);
                                self.drive_low = true; // ACK
                            }
                            None => {
                                self.st = St::Ignore;
                                self.drive_low = false;
                            }
                        }
                    } else if self.n == 9 {
                        self.drive_low = false;
                        self.n = 0;
                        self.shift = 0;
                        if let Some(t) = self.target {
                            // address byte read/write bit decides the direction
                            let rd = self.read_dir(board, t);
                            if rd {
                                self.st = St::Read;
                                self.out = board.comp_mut(t).i2c_read(true);
                                self.drive_low = self.out & 0x80 == 0;
                            } else {
                                self.st = St::Write;
                            }
                        }
                    }
                }
                St::Write => {
                    if self.n == 8 {
                        let ack = match self.target {
                            Some(t) => board.comp_mut(t).i2c_write(self.shift),
                            None => false,
                        };
                        self.drive_low = ack;
                    } else if self.n == 9 {
                        self.drive_low = false;
                        self.n = 0;
                        self.shift = 0;
                    }
                }
                St::Read => {
                    if self.n < 8 {
                        // next data bit
                        let bit = (self.out >> (7 - self.n)) & 1;
                        self.drive_low = bit == 0;
                    } else if self.n == 8 {
                        // release for master ACK
                        self.drive_low = false;
                    } else {
                        // after master ack clock
                        self.n = 0;
                        if self.master_acked {
                            if let Some(t) = self.target {
                                self.out = board.comp_mut(t).i2c_read(true);
                                self.drive_low = self.out & 0x80 == 0;
                            }
                        } else {
                            self.drive_low = false;
                            self.st = St::Ignore;
                        }
                    }
                }
                _ => {}
            }
        }
        self.last_scl = scl;
        self.last_sda = sda;
        before != self.drive_low
    }

    fn read_dir(&self, _board: &mut Board, _t: usize) -> bool {
        self.rd
    }
}

# MFRC522 driver for MicroPython, adapted from MFRC522-Python (Mario Gomez, LGPL)
# -- same register sequences, with machine.SPI and a GPIO chip select.
from machine import Pin, SPI

class MFRC522:
    OK, NOTAGERR, ERR = 0, 1, 2
    REQIDL, AUTHENT1A = 0x26, 0x60

    def __init__(self, spi, cs, rst=None):
        self.spi, self.cs = spi, cs
        self.cs.init(Pin.OUT, value=1)
        if rst is not None:
            rst.init(Pin.OUT, value=0); rst.value(1)
        self.init()

    def _wreg(self, reg, val):
        self.cs.value(0)
        self.spi.write(bytes(((reg << 1) & 0x7E, val)))
        self.cs.value(1)

    def _rreg(self, reg):
        self.cs.value(0)
        self.spi.write(bytes((((reg << 1) & 0x7E) | 0x80,)))
        val = self.spi.read(1)
        self.cs.value(1)
        return val[0]

    def _sflags(self, reg, mask):
        self._wreg(reg, self._rreg(reg) | mask)

    def _cflags(self, reg, mask):
        self._wreg(reg, self._rreg(reg) & (~mask))

    def _tocard(self, cmd, send):
        recv, bits, stat = [], 0, self.ERR
        irq_en, wait_irq = (0x12, 0x10) if cmd == 0x0E else (0x77, 0x30) if cmd == 0x0C else (0, 0)
        self._wreg(0x02, irq_en | 0x80)
        self._cflags(0x04, 0x80)
        self._sflags(0x0A, 0x80)
        self._wreg(0x01, 0x00)
        for c in send:
            self._wreg(0x09, c)
        self._wreg(0x01, cmd)
        if cmd == 0x0C:
            self._sflags(0x0D, 0x80)
        i = 2000
        while True:
            n = self._rreg(0x04)
            i -= 1
            if not ((i != 0) and not (n & 0x01) and not (n & wait_irq)):
                break
        self._cflags(0x0D, 0x80)
        if i:
            if (self._rreg(0x06) & 0x1B) == 0x00:
                stat = self.OK
                if n & irq_en & 0x01:
                    stat = self.NOTAGERR
                if cmd == 0x0C:
                    n = self._rreg(0x0A)
                    lbits = self._rreg(0x0C) & 0x07
                    bits = (n - 1) * 8 + lbits if lbits != 0 else n * 8
                    n = 1 if n == 0 else min(n, 16)
                    for _ in range(n):
                        recv.append(self._rreg(0x09))
            else:
                stat = self.ERR
        return stat, recv, bits

    def _crc(self, data):
        self._cflags(0x05, 0x04)
        self._sflags(0x0A, 0x80)
        for c in data:
            self._wreg(0x09, c)
        self._wreg(0x01, 0x03)
        i = 0xFF
        while True:
            n = self._rreg(0x05)
            i -= 1
            if not ((i != 0) and not (n & 0x04)):
                break
        return [self._rreg(0x22), self._rreg(0x21)]

    def init(self):
        self._wreg(0x01, 0x0F)
        self._wreg(0x2A, 0x8D)
        self._wreg(0x2B, 0x3E)
        self._wreg(0x2D, 30)
        self._wreg(0x2C, 0)
        self._wreg(0x15, 0x40)
        self._wreg(0x11, 0x3D)
        self.antenna_on()

    def antenna_on(self, on=True):
        if on and ~(self._rreg(0x14) & 0x03):
            self._sflags(0x14, 0x03)
        else:
            self._cflags(0x14, 0x03)

    def version(self):
        return self._rreg(0x37)

    def request(self, mode):
        self._wreg(0x0D, 0x07)
        stat, recv, bits = self._tocard(0x0C, [mode])
        if (stat != self.OK) | (bits != 0x10):
            stat = self.ERR
        return stat, bits

    def anticoll(self):
        ser_chk = 0
        ser = [0x93, 0x20]
        self._wreg(0x0D, 0x00)
        stat, recv, bits = self._tocard(0x0C, ser)
        if stat == self.OK:
            if len(recv) == 5:
                for i in range(4):
                    ser_chk = ser_chk ^ recv[i]
                if ser_chk != recv[4]:
                    stat = self.ERR
            else:
                stat = self.ERR
        return stat, recv

    def select_tag(self, ser):
        buf = [0x93, 0x70] + ser[:5]
        buf += self._crc(buf)
        stat, recv, bits = self._tocard(0x0C, buf)
        return self.OK if (stat == self.OK) and (bits == 0x18) else self.ERR

    def auth(self, mode, addr, sect, ser):
        return self._tocard(0x0E, [mode, addr] + sect + ser[:4])[0]

    def stop_crypto1(self):
        self._cflags(0x08, 0x08)

    def read(self, addr):
        data = [0x30, addr]
        data += self._crc(data)
        stat, recv, _ = self._tocard(0x0C, data)
        return recv if stat == self.OK and len(recv) == 16 else None

    def write(self, addr, data):
        buf = [0xA0, addr]
        buf += self._crc(buf)
        stat, recv, bits = self._tocard(0x0C, buf)
        if not (stat == self.OK) or not (bits == 4) or not ((recv[0] & 0x0F) == 0x0A):
            return self.ERR
        buf = list(data[:16])
        buf += self._crc(buf)
        stat, recv, bits = self._tocard(0x0C, buf)
        if not (stat == self.OK) or not (bits == 4) or not ((recv[0] & 0x0F) == 0x0A):
            return self.ERR
        return self.OK

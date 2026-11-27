#pragma once
#include "Arduino.h"

class TwoWire : public Stream {
public:
  TwoWire(uint8_t bus_num);
  bool begin(int sda = -1, int scl = -1, uint32_t frequency = 0);
  bool begin(uint8_t slaveAddr, int sda = -1, int scl = -1, uint32_t frequency = 0);
  bool end();
  bool setPins(int sda, int scl);
  bool setClock(uint32_t frequency);
  uint32_t getClock();
  void setTimeOut(uint16_t timeOutMillis);
  void beginTransmission(uint8_t address);
  uint8_t endTransmission(bool sendStop = true);
  uint8_t requestFrom(uint8_t address, uint8_t size, bool sendStop = true);
  size_t write(uint8_t) override;
  size_t write(const uint8_t *, size_t) override;
  using Print::write;
  int available(void) override;
  int read(void) override;
  int peek(void) override;
  void flush(void) override;
  void onReceive(void (*)(int));
  void onRequest(void (*)(void));
};
extern TwoWire Wire;
extern TwoWire Wire1;

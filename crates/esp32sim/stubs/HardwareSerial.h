#pragma once
#include "Stream.h"
#define SERIAL_8N1 0x800001c
#define SERIAL_8E1 0x800001e
#define SERIAL_8O1 0x800001f

class HardwareSerial : public Stream {
public:
  HardwareSerial(uint8_t uart_nr);
  void begin(unsigned long baud, uint32_t config = SERIAL_8N1, int8_t rxPin = -1, int8_t txPin = -1, bool invert = false, unsigned long timeout_ms = 20000UL);
  void end();
  void updateBaudRate(unsigned long baud);
  int available(void) override;
  int availableForWrite(void) override;
  int peek(void) override;
  int read(void) override;
  size_t read(uint8_t *buffer, size_t size);
  void flush(void) override;
  size_t write(uint8_t) override;
  size_t write(const uint8_t *buffer, size_t size) override;
  using Print::write;
  uint32_t baudRate();
  void setRxBufferSize(size_t);
  void setTxBufferSize(size_t);
  void setPins(int8_t rxPin, int8_t txPin, int8_t ctsPin = -1, int8_t rtsPin = -1);
  void onReceive(void (*function)(void), bool onlyOnTimeout = false);
  operator bool() const;
};

extern HardwareSerial Serial;
extern HardwareSerial Serial1;
extern HardwareSerial Serial2;

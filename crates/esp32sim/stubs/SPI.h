#pragma once
#include "Arduino.h"
#define SPI_MODE0 0
#define SPI_MODE1 1
#define SPI_MODE2 2
#define SPI_MODE3 3
#define HSPI 2
#define VSPI 3
#define FSPI 1

class SPISettings {
public:
  SPISettings();
  SPISettings(uint32_t clock, uint8_t bitOrder, uint8_t dataMode);
};

class SPIClass {
public:
  SPIClass(uint8_t spi_bus = HSPI);
  void begin(int8_t sck = -1, int8_t miso = -1, int8_t mosi = -1, int8_t ss = -1);
  void end();
  void setHwCs(bool use);
  void setBitOrder(uint8_t bitOrder);
  void setDataMode(uint8_t dataMode);
  void setFrequency(uint32_t freq);
  void beginTransaction(SPISettings settings);
  void endTransaction(void);
  uint8_t transfer(uint8_t data);
  uint16_t transfer16(uint16_t data);
  uint32_t transfer32(uint32_t data);
  void transfer(void *data, uint32_t size);
  void transferBytes(const uint8_t *data, uint8_t *out, uint32_t size);
  void writeBytes(const uint8_t *data, uint32_t size);
  void write(uint8_t data);
  void write16(uint16_t data);
  void write32(uint32_t data);
};
extern SPIClass SPI;

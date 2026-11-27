#pragma once
#include <stdint.h>

class EspClass {
public:
  uint32_t getHeapSize();
  uint32_t getFreeHeap();
  uint32_t getMinFreeHeap();
  uint32_t getMaxAllocHeap();
  uint32_t getPsramSize();
  uint32_t getFreePsram();
  uint16_t getChipRevision();
  const char *getChipModel();
  uint8_t getChipCores();
  uint32_t getCpuFreqMHz();
  uint32_t getCycleCount();
  const char *getSdkVersion();
  uint32_t getFlashChipSize();
  uint32_t getFlashChipSpeed();
  uint32_t getSketchSize();
  uint32_t getFreeSketchSpace();
  uint64_t getEfuseMac();
  void restart();
  void deepSleep(uint32_t time_us);
};
extern EspClass ESP;

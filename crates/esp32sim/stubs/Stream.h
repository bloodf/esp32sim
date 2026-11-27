#pragma once
#include "Print.h"

class Stream : public Print {
public:
  virtual int available() = 0;
  virtual int read() = 0;
  virtual int peek() = 0;
  void setTimeout(unsigned long timeout);
  unsigned long getTimeout(void);
  bool find(const char *target);
  bool findUntil(const char *target, const char *terminator);
  long parseInt();
  float parseFloat();
  virtual size_t readBytes(char *buffer, size_t length);
  size_t readBytes(uint8_t *buffer, size_t length);
  size_t readBytesUntil(char terminator, char *buffer, size_t length);
  String readString();
  String readStringUntil(char terminator);
};

#pragma once
#include <stdint.h>
#include <stddef.h>
#include "WString.h"
#define DEC 10
#define HEX 16
#define OCT 8
#define BIN 2

class Printable;

class Print {
public:
  virtual ~Print() {}
  virtual size_t write(uint8_t) = 0;
  virtual size_t write(const uint8_t *buffer, size_t size);
  size_t write(const char *str);
  size_t write(const char *buffer, size_t size);
  int getWriteError();
  void clearWriteError();
  virtual int availableForWrite();
  size_t printf(const char *format, ...) __attribute__((format(printf, 2, 3)));
  size_t print(const String &);
  size_t print(const char[]);
  size_t print(char);
  size_t print(unsigned char, int = DEC);
  size_t print(int, int = DEC);
  size_t print(unsigned int, int = DEC);
  size_t print(long, int = DEC);
  size_t print(unsigned long, int = DEC);
  size_t print(long long, int = DEC);
  size_t print(unsigned long long, int = DEC);
  size_t print(double, int = 2);
  size_t print(const Printable &);
  size_t println(const String &s);
  size_t println(const char[]);
  size_t println(char);
  size_t println(unsigned char, int = DEC);
  size_t println(int, int = DEC);
  size_t println(unsigned int, int = DEC);
  size_t println(long, int = DEC);
  size_t println(unsigned long, int = DEC);
  size_t println(long long, int = DEC);
  size_t println(unsigned long long, int = DEC);
  size_t println(double, int = 2);
  size_t println(const Printable &);
  size_t println(void);
  virtual void flush();
};

class Printable {
public:
  virtual ~Printable() {}
  virtual size_t printTo(Print &p) const = 0;
};

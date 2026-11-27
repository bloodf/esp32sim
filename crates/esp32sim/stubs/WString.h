#pragma once
#include <stdint.h>
#include <stddef.h>

class __FlashStringHelper;

class String {
public:
  String(const char *cstr = "");
  String(const String &str);
  String(String &&rval);
  explicit String(char c);
  explicit String(unsigned char, unsigned char base = 10);
  explicit String(int, unsigned char base = 10);
  explicit String(unsigned int, unsigned char base = 10);
  explicit String(long, unsigned char base = 10);
  explicit String(unsigned long, unsigned char base = 10);
  explicit String(float, unsigned int decimalPlaces = 2);
  explicit String(double, unsigned int decimalPlaces = 2);
  ~String();
  bool reserve(unsigned int size);
  unsigned int length(void) const;
  bool isEmpty(void) const;
  String &operator=(const String &rhs);
  String &operator=(const char *cstr);
  bool concat(const String &str);
  bool concat(const char *cstr);
  bool concat(char c);
  bool concat(int num);
  bool concat(unsigned long num);
  bool concat(float num);
  bool concat(double num);
  template <typename T> String &operator+=(T rhs) { concat(rhs); return *this; }
  friend String operator+(const String &lhs, const String &rhs);
  friend String operator+(const String &lhs, const char *rhs);
  friend String operator+(const char *lhs, const String &rhs);
  int compareTo(const String &s) const;
  bool equals(const String &s) const;
  bool equals(const char *cstr) const;
  bool equalsIgnoreCase(const String &s) const;
  bool operator==(const String &rhs) const;
  bool operator==(const char *cstr) const;
  bool operator!=(const String &rhs) const;
  bool operator!=(const char *cstr) const;
  bool operator<(const String &rhs) const;
  bool startsWith(const String &prefix) const;
  bool endsWith(const String &suffix) const;
  char charAt(unsigned int index) const;
  void setCharAt(unsigned int index, char c);
  char operator[](unsigned int index) const;
  char &operator[](unsigned int index);
  void getBytes(unsigned char *buf, unsigned int bufsize, unsigned int index = 0) const;
  void toCharArray(char *buf, unsigned int bufsize, unsigned int index = 0) const;
  const char *c_str() const;
  int indexOf(char ch, unsigned int fromIndex = 0) const;
  int indexOf(const String &str, unsigned int fromIndex = 0) const;
  int lastIndexOf(char ch) const;
  int lastIndexOf(const String &str) const;
  String substring(unsigned int beginIndex) const;
  String substring(unsigned int beginIndex, unsigned int endIndex) const;
  void replace(char find, char replace);
  void replace(const String &find, const String &replace);
  void remove(unsigned int index);
  void remove(unsigned int index, unsigned int count);
  void toLowerCase(void);
  void toUpperCase(void);
  void trim(void);
  long toInt(void) const;
  float toFloat(void) const;
  double toDouble(void) const;
};

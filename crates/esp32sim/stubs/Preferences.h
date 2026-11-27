#pragma once
#include "Arduino.h"

class Preferences {
public:
  bool begin(const char *name, bool readOnly = false, const char *partition_label = nullptr);
  void end();
  bool clear();
  bool remove(const char *key);
  size_t putInt(const char *key, int32_t value);
  size_t putUInt(const char *key, uint32_t value);
  size_t putFloat(const char *key, float value);
  size_t putBool(const char *key, bool value);
  size_t putString(const char *key, const char *value);
  size_t putString(const char *key, String value);
  size_t putBytes(const char *key, const void *value, size_t len);
  int32_t getInt(const char *key, int32_t defaultValue = 0);
  uint32_t getUInt(const char *key, uint32_t defaultValue = 0);
  float getFloat(const char *key, float defaultValue = NAN);
  bool getBool(const char *key, bool defaultValue = false);
  String getString(const char *key, String defaultValue = String());
  size_t getBytes(const char *key, void *buf, size_t maxLen);
  bool isKey(const char *key);
};

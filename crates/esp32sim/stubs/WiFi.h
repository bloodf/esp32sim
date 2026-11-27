#pragma once
#include "Arduino.h"
// Note: esp32sim does not emulate the radio; WiFi calls run but never connect.
typedef enum { WL_IDLE_STATUS = 0, WL_NO_SSID_AVAIL, WL_SCAN_COMPLETED, WL_CONNECTED, WL_CONNECT_FAILED, WL_CONNECTION_LOST, WL_DISCONNECTED } wl_status_t;
typedef enum { WIFI_OFF = 0, WIFI_STA, WIFI_AP, WIFI_AP_STA } wifi_mode_t;

class IPAddress : public Printable {
public:
  IPAddress();
  IPAddress(uint8_t a, uint8_t b, uint8_t c, uint8_t d);
  String toString() const;
  size_t printTo(Print &p) const override;
};

class WiFiClass {
public:
  wl_status_t begin(const char *ssid, const char *passphrase = nullptr);
  bool mode(wifi_mode_t m);
  bool disconnect(bool wifioff = false);
  wl_status_t status();
  bool isConnected();
  IPAddress localIP();
  String macAddress();
  int32_t RSSI();
  String SSID();
  bool softAP(const char *ssid, const char *passphrase = nullptr);
  IPAddress softAPIP();
  int16_t scanNetworks();
  String SSID(uint8_t i);
  int32_t RSSI(uint8_t i);
  bool setHostname(const char *name);
  void setSleep(bool enable);
};
extern WiFiClass WiFi;

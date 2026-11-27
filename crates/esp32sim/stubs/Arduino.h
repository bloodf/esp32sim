// esp32sim IntelliSense stub of the Arduino-ESP32 core API.
// Used only for code completion before the first PlatformIO build
// (after a build, the real framework headers from compile_commands.json are used).
#pragma once
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <string.h>
#include <stdlib.h>
#include <stdio.h>
#include <math.h>
#include "WString.h"
#include "Print.h"
#include "Stream.h"
#include "HardwareSerial.h"
#include "Esp.h"
#include "freertos/FreeRTOS.h"

typedef uint8_t byte;
typedef bool boolean;
typedef uint16_t word;

#define HIGH 0x1
#define LOW 0x0
#define INPUT 0x01
#define OUTPUT 0x03
#define PULLUP 0x04
#define INPUT_PULLUP 0x05
#define PULLDOWN 0x08
#define INPUT_PULLDOWN 0x09
#define OPEN_DRAIN 0x10
#define OUTPUT_OPEN_DRAIN 0x13
#define ANALOG 0xC0
#define RISING 0x01
#define FALLING 0x02
#define CHANGE 0x03
#define ONLOW 0x04
#define ONHIGH 0x05
#define LED_BUILTIN 2
#define LSBFIRST 0
#define MSBFIRST 1
#define PI 3.1415926535897932384626433832795
#define HALF_PI 1.5707963267948966192313216916398
#define TWO_PI 6.283185307179586476925286766559
#define DEG_TO_RAD 0.017453292519943295769236907684886
#define RAD_TO_DEG 57.295779513082320876798154814105
#define A0 36
#define A3 39
#define A4 32
#define A5 33
#define A6 34
#define A7 35
#define T0 4
#define SDA 21
#define SCL 22
#define SS 5
#define MOSI 23
#define MISO 19
#define SCK 18
#define TX 1
#define RX 3
#define DAC1 25
#define DAC2 26
#define IRAM_ATTR
#define DRAM_ATTR
#define RTC_DATA_ATTR
#define F(s) (s)
#define PROGMEM
#define pgm_read_byte(addr) (*(const unsigned char *)(addr))
#define pgm_read_word(addr) (*(const unsigned short *)(addr))
#define bit(b) (1UL << (b))
#define bitRead(value, bit) (((value) >> (bit)) & 0x01)
#define bitSet(value, bit) ((value) |= (1UL << (bit)))
#define bitClear(value, bit) ((value) &= ~(1UL << (bit)))
#define bitWrite(value, bit, bitvalue) ((bitvalue) ? bitSet(value, bit) : bitClear(value, bit))
#define lowByte(w) ((uint8_t)((w)&0xff))
#define highByte(w) ((uint8_t)((w) >> 8))
#define digitalPinToInterrupt(p) (p)
#define NOT_A_PIN -1

template <class T, class L> auto min(const T &a, const L &b) -> decltype((b < a) ? b : a) { return (b < a) ? b : a; }
template <class T, class L> auto max(const T &a, const L &b) -> decltype((b < a) ? b : a) { return (a < b) ? b : a; }
#define constrain(amt, low, high) ((amt) < (low) ? (low) : ((amt) > (high) ? (high) : (amt)))
#define sq(x) ((x) * (x))

typedef void (*voidFuncPtr)(void);
typedef void (*voidFuncPtrArg)(void *);

void setup(void);
void loop(void);

void pinMode(uint8_t pin, uint8_t mode);
void digitalWrite(uint8_t pin, uint8_t val);
int digitalRead(uint8_t pin);
uint16_t analogRead(uint8_t pin);
uint32_t analogReadMilliVolts(uint8_t pin);
void analogReadResolution(uint8_t bits);
void analogSetAttenuation(int attenuation);
void analogWrite(uint8_t pin, int value);
void analogWriteResolution(uint8_t bits);
void analogWriteFrequency(uint32_t freq);
void dacWrite(uint8_t pin, uint8_t value);
uint16_t touchRead(uint8_t pin);

unsigned long millis(void);
unsigned long micros(void);
void delay(uint32_t ms);
void delayMicroseconds(uint32_t us);
void yield(void);

void attachInterrupt(uint8_t pin, voidFuncPtr handler, int mode);
void attachInterruptArg(uint8_t pin, voidFuncPtrArg handler, void *arg, int mode);
void detachInterrupt(uint8_t pin);
void interrupts(void);
void noInterrupts(void);

unsigned long pulseIn(uint8_t pin, uint8_t state, unsigned long timeout = 1000000L);
unsigned long pulseInLong(uint8_t pin, uint8_t state, unsigned long timeout = 1000000L);
uint8_t shiftIn(uint8_t dataPin, uint8_t clockPin, uint8_t bitOrder);
void shiftOut(uint8_t dataPin, uint8_t clockPin, uint8_t bitOrder, uint8_t val);
void tone(uint8_t pin, unsigned int frequency, unsigned long duration = 0);
void noTone(uint8_t pin);

long random(long max);
long random(long min, long max);
void randomSeed(unsigned long seed);
long map(long x, long in_min, long in_max, long out_min, long out_max);

// LEDC (PWM): Arduino-ESP32 2.x and 3.x APIs
double ledcSetup(uint8_t channel, double freq, uint8_t resolution_bits);
void ledcAttachPin(uint8_t pin, uint8_t channel);
void ledcDetachPin(uint8_t pin);
bool ledcAttach(uint8_t pin, uint32_t freq, uint8_t resolution);
bool ledcAttachChannel(uint8_t pin, uint32_t freq, uint8_t resolution, uint8_t channel);
bool ledcDetach(uint8_t pin);
void ledcWrite(uint8_t pin_or_channel, uint32_t duty);
uint32_t ledcRead(uint8_t pin_or_channel);
double ledcWriteTone(uint8_t pin_or_channel, double freq);
double ledcChangeFrequency(uint8_t pin_or_channel, double freq, uint8_t resolution);

// RGB LED / NeoPixel helper of the core
void neopixelWrite(uint8_t pin, uint8_t red, uint8_t green, uint8_t blue);
void rgbLedWrite(uint8_t pin, uint8_t red, uint8_t green, uint8_t blue);

uint32_t getCpuFrequencyMhz(void);
bool setCpuFrequencyMhz(uint32_t mhz);
float temperatureRead(void);

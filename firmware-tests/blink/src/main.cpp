#include <Arduino.h>
void setup() {
  Serial.begin(115200);
  pinMode(2, OUTPUT);
  Serial.println("Hello from ESP32 sim!");
}
void loop() {
  digitalWrite(2, HIGH);
  Serial.println("LED on");
  delay(500);
  digitalWrite(2, LOW);
  Serial.println("LED off");
  delay(500);
}

/* Reference C source for the hand-assembled debug test program (blink.s). */
volatile int counter;

int step(int x) {
  return x + 3;
}

void app_main(void) {
  for (;;) {
    counter = step(counter);
    *(volatile unsigned *)0x3FF40000 = '.';
  }
}

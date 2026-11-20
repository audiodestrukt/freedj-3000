/* FX2LP output test: D7 (PB7) only, toggling slowly; D0-D6 stay inputs. */
__sfr __at (0x90) IOB;
__sfr __at (0xB3) OEB;
__xdata __at (0xE600) volatile unsigned char CPUCS;
__xdata __at (0xE601) volatile unsigned char IFCONFIG;

static void wait(void) {
    volatile unsigned int i, j;
    for (i = 0; i < 300; i++) for (j = 0; j < 1000; j++) ;
}

void main(void) {
    CPUCS = 0x10;      /* 48 MHz CPU clock */
    IFCONFIG = 0x80;   /* ports mode: PB is plain GPIO */
    OEB = 0x80;        /* PB7 output, PB0-PB6 inputs */
    for (;;) { IOB = 0x80; wait(); IOB = 0x00; wait(); }
}

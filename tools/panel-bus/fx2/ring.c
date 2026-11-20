/* FX2LP as the XDJ-1000MK2 panel-link master, standalone: sweep the jog ring.
 * D0 = SCK (out), D1 = RXD to the panel (out), D2 = TXD from the panel (in),
 * D3 = BUSY (in, low = panel ready).  SPI mode 3, LSB first, 32-byte frames,
 * byte 30 = ones'-complement sum of 0..29, byte 31 = 0x8F.  The frame is the
 * stock main board's, with byte 9 (the ring playhead, 1..135) stepping. */
__sfr __at (0x90) IOB;
__sfr __at (0xB3) OEB;
__sbit __at (0x90) SCK;
__sbit __at (0x91) MOSI;
__sbit __at (0x92) MISO;
__sbit __at (0x93) BUSY;
__xdata __at (0xE600) volatile unsigned char CPUCS;
__xdata __at (0xE601) volatile unsigned char IFCONFIG;

static __data unsigned char tx[32] = {
    0x30, 0xdf, 0, 0, 0, 0, 0, 0, 0x58, 0x01, 0, 0x10, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0x02, 0x02, 0x02, 0, 0, 0, 0, 0, 0, 0x8f };
static __data unsigned char rx[32];

static void frame(void) {
    unsigned char i, n, b, r;
    unsigned int s = 0;
    for (i = 0; i < 30; i++) { s += tx[i]; s = (s & 0xff) + (s >> 8); }
    tx[30] = (unsigned char)s;
    for (i = 0; i < 32; i++) {
        b = tx[i]; r = 0;
        for (n = 0; n < 8; n++) {
            SCK = 0;
            MOSI = b & 1;
            b >>= 1;
            r >>= 1;
            SCK = 1;                 /* the panel samples here */
            __asm nop
 nop
 nop
 nop
 nop
 nop __endasm;
            if (MISO) r |= 0x80;
        }
        rx[i] = r;
    }
}

void main(void) {
    unsigned char step = 0;
    CPUCS = 0x10;                    /* 48 MHz */
    IFCONFIG = 0x80;                 /* ports mode */
    SCK = 1; MOSI = 1;
    OEB = 0x03;                      /* PB0, PB1 out; the rest in */
    for (;;) {
        while (BUSY) ;               /* wait for the panel's ready */
        frame();
        { unsigned int t = 0; while (!BUSY && ++t) ; }   /* high just after the last clock; give up after ~65k polls */
        if (++step == 4) {           /* 135 positions in ~1.8 s at ~3 ms a frame */
            step = 0;
            tx[9] = tx[9] >= 135 ? 1 : tx[9] + 1;
        }
    }
}

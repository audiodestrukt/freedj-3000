/* bootlog: the analyser as panel-link master in the power-on window only,
 * logging to RAM for readlog.py (which halts the CPU, then reads with 0xA0).
 *
 * Loaded with the deck off.  Pins stay inputs until the panel's TXD goes high
 * (it is powered).  Timer 0 (4 MHz, an overflow = 16.384 ms) then bounds
 * everything: the line changes on TXD/BUSY are logged until the panel is
 * ready (BUSY high, then low; at most 2 s), sending runs from then to at most
 * 4.0 s after power-up, and D0/D1 are released then,
 * whatever the panel does: the stock main board starts clocking ~7 s after
 * power-on and must find the bus free.  Frames are sent only when BUSY is low.
 *
 * Log at 0x2000:
 *   0x2000  "XDJL", stage (1 wait power, 2 watch, 3 send, 4 done), events,
 *           frames logged, frames sent (u16), frames where BUSY rose (u16);
 *           an event with bit 7 set = a frame after which BUSY did not rise
 *   0x2010  events: 64 x (tick u16, TH0, state: bit0 TXD, bit1 BUSY)
 *   0x2110  frames: 48 x (BUSY-low wait in ticks, BUSY-rose flag, tick u16, 32 reply bytes)
 * D0 = SCK and D1 = RXD, driven push-pull at the stock ~842 kHz (an
 * open-drain SCK on the deck's pull-up does not rise fast enough).  hdr[12..13] = polls for SCK to rise after a
 * test release (0xFFFF = it never did: no pull-up, nothing is sent).  D2 = TXD in, D3 = BUSY in; D5 high while sending.
 * SPI mode 3, LSB first, byte 30 = ones'-complement sum, byte 31 = 0x8F. */
__sfr __at (0x90) IOB;
__sfr __at (0xB3) OEB;
__sfr __at (0x89) TMOD;
__sfr __at (0x8C) TH0;
__sfr __at (0x8A) TL0;
__sbit __at (0x8C) TR0;
__sbit __at (0x8D) TF0;
__sbit __at (0x90) SCK;
__sbit __at (0x91) MOSI;
__sbit __at (0x92) MISO;
__sbit __at (0x93) BUSY;
__xdata __at (0xE600) volatile unsigned char CPUCS;
__xdata __at (0xE601) volatile unsigned char IFCONFIG;

#define NEV 64
#define NFR 48
__xdata __at (0x2000) volatile unsigned char hdr[16];
__xdata __at (0x2010) volatile unsigned char ev[NEV][4];
__xdata __at (0x2110) volatile unsigned char fl[NFR][36];

#define T_WATCH_END 122     /* give up waiting for ready after 2.0 s */
#define T_SEND_END  244     /* 4.0 s: released well before the main board */

static const __code unsigned char LAMPS_ON[30] = {
    0x73, 0x18, 0, 0, 0, 0, 0, 0x33, 0x40, 0x3f, 0x28, 0x10, 0x01, 0, 0, 0,
    0, 0, 0x20, 0, 0, 0, 0x02, 0x02, 0x02, 0, 0, 0, 0, 0 };
static __data unsigned char tx[32];
static __data unsigned char rx[32];
static __data unsigned int ticks;

static void tick(void) { if (TF0) { TF0 = 0; ticks++; } }
#define SCK_LOW()  (OEB |= 0x01)
#define SCK_REL()  (OEB &= ~0x01)

/* One frame in about 300 us, as the stock main board clocks it: ~857 kHz,
 * no gap inside a byte.  A bit is 14 cycles at 48 MHz (83 ns each): SCK low
 * for 7, high for 7, both driven (an open-drain SCK on the deck's pull-up
 * does not rise in 7 cycles).  RXD is driven too.  One rotate through the
 * carry sends a bit and keeps the one just received, so a byte is 8 of those
 * and one more rotate.  tx[] and rx[] are in __data. */
static void shift_frame(void) __naked {
    __asm
    mov r1,#_tx
    mov r0,#_rx
    mov r2,#32
00001$:
    mov a,@r1
    clr _SCK            ; SCK low                    2 cycles
    rrc a               ; C = next bit out; a7 = last bit in   1
    mov _MOSI,c         ; RXD                        2
    nop                 ;                            1
    nop                 ;                            1  -> 7 low
    setb _SCK           ; SCK high: the panel samples   2
    nop                 ;                            1
    nop                 ;                            1
    nop                 ;                            1
    mov c,_MISO         ; the panel bit              2  -> 7 high
    clr _SCK            ; SCK low                    2 cycles
    rrc a               ; C = next bit out; a7 = last bit in   1
    mov _MOSI,c         ; RXD                        2
    nop                 ;                            1
    nop                 ;                            1  -> 7 low
    setb _SCK           ; SCK high: the panel samples   2
    nop                 ;                            1
    nop                 ;                            1
    nop                 ;                            1
    mov c,_MISO         ; the panel bit              2  -> 7 high
    clr _SCK            ; SCK low                    2 cycles
    rrc a               ; C = next bit out; a7 = last bit in   1
    mov _MOSI,c         ; RXD                        2
    nop                 ;                            1
    nop                 ;                            1  -> 7 low
    setb _SCK           ; SCK high: the panel samples   2
    nop                 ;                            1
    nop                 ;                            1
    nop                 ;                            1
    mov c,_MISO         ; the panel bit              2  -> 7 high
    clr _SCK            ; SCK low                    2 cycles
    rrc a               ; C = next bit out; a7 = last bit in   1
    mov _MOSI,c         ; RXD                        2
    nop                 ;                            1
    nop                 ;                            1  -> 7 low
    setb _SCK           ; SCK high: the panel samples   2
    nop                 ;                            1
    nop                 ;                            1
    nop                 ;                            1
    mov c,_MISO         ; the panel bit              2  -> 7 high
    clr _SCK            ; SCK low                    2 cycles
    rrc a               ; C = next bit out; a7 = last bit in   1
    mov _MOSI,c         ; RXD                        2
    nop                 ;                            1
    nop                 ;                            1  -> 7 low
    setb _SCK           ; SCK high: the panel samples   2
    nop                 ;                            1
    nop                 ;                            1
    nop                 ;                            1
    mov c,_MISO         ; the panel bit              2  -> 7 high
    clr _SCK            ; SCK low                    2 cycles
    rrc a               ; C = next bit out; a7 = last bit in   1
    mov _MOSI,c         ; RXD                        2
    nop                 ;                            1
    nop                 ;                            1  -> 7 low
    setb _SCK           ; SCK high: the panel samples   2
    nop                 ;                            1
    nop                 ;                            1
    nop                 ;                            1
    mov c,_MISO         ; the panel bit              2  -> 7 high
    clr _SCK            ; SCK low                    2 cycles
    rrc a               ; C = next bit out; a7 = last bit in   1
    mov _MOSI,c         ; RXD                        2
    nop                 ;                            1
    nop                 ;                            1  -> 7 low
    setb _SCK           ; SCK high: the panel samples   2
    nop                 ;                            1
    nop                 ;                            1
    nop                 ;                            1
    mov c,_MISO         ; the panel bit              2  -> 7 high
    clr _SCK            ; SCK low                    2 cycles
    rrc a               ; C = next bit out; a7 = last bit in   1
    mov _MOSI,c         ; RXD                        2
    nop                 ;                            1
    nop                 ;                            1  -> 7 low
    setb _SCK           ; SCK high: the panel samples   2
    nop                 ;                            1
    nop                 ;                            1
    nop                 ;                            1
    mov c,_MISO         ; the panel bit              2  -> 7 high
    rrc a               ; the last bit in
    mov @r0,a
    inc r0
    inc r1
    djnz r2,00003$      ; the loop is too long for a relative jump back
    ret
00003$:
    ljmp 00001$
    __endasm;
}

static void frame(void) {
    unsigned char i;
    unsigned int s = 0;
    for (i = 0; i < 30; i++) { s += tx[i]; s = (s & 0xff) + (s >> 8); }
    tx[30] = (unsigned char)s;
    tx[31] = 0x8f;
    shift_frame();
}

/* ~28 us: the stock main board's pause between BUSY falling and the clock. */
static void pause28(void) __naked {
    __asm
    mov r7,#112         ; 112 x 3 cycles = 336 cycles = 28 us
00002$:
    djnz r7,00002$
    ret
    __endasm;
}

void main(void) {
    unsigned char i, ring = 1, on, nev = 0, nfr = 0, st, last;
    unsigned int sent = 0, rose = 0, t0;
    CPUCS = 0x10;                            /* 48 MHz; timer 0 at CLKOUT/12 = 4 MHz */
    IFCONFIG = 0x80;
    IOB = 0x00;
    OEB = 0x00;
    for (i = 0; i < 16; i++) hdr[i] = 0;
    hdr[0] = 'X'; hdr[1] = 'D'; hdr[2] = 'J'; hdr[3] = 'L'; hdr[4] = 1;

    while (!MISO) ;                          /* the panel is powered */
    TMOD = 0x01; TH0 = 0; TL0 = 0; TF0 = 0; TR0 = 1; ticks = 0;
    hdr[4] = 2;
    last = 0xff;
    /* Watch TXD and BUSY until BUSY has been high and then low (ready). */
    { unsigned char seen_high = 0;
      while (ticks < T_WATCH_END) {
        tick();
        st = (MISO ? 1 : 0) | (BUSY ? 2 : 0);
        if (st != last && nev < NEV - 8) {
            ev[nev][0] = ticks & 0xff; ev[nev][1] = ticks >> 8; ev[nev][2] = TH0; ev[nev][3] = st;
            hdr[5] = ++nev;
            last = st;
        }
        if (BUSY) seen_high = 1;
        else if (seen_high) break;
      }
    }

    hdr[4] = 3;
    IOB = 0x22;                              /* SCK released for the pull-up test, RXD high */
    OEB = 0x22;                              /* D5 = sending, RXD driven */
    { unsigned int w = 0;                    /* is there a pull-up? time one release */
      SCK_LOW(); __asm nop
 nop
 nop
 nop __endasm; SCK_REL();
      while (!SCK && ++w < 0xffff) ;
      hdr[12] = w & 0xff; hdr[13] = w >> 8;
      if (w == 0xffff) goto done; }
    IOB = 0x23;                              /* SCK idles high, now driven */
    OEB = 0x23;
    while (ticks < T_SEND_END) {
        on = (sent / 33) & 1 ? 0 : 1;            /* ~0.1 s on, ~0.1 s off */
        for (i = 0; i < 30; i++) tx[i] = on ? LAMPS_ON[i] : 0;
        tx[22] = tx[23] = tx[24] = 0x02;
        if ((sent & 3) == 0) ring = ring >= 135 ? 1 : ring + 1;
        tx[9] = ring;
        t0 = ticks;
        while (BUSY && ticks < T_SEND_END) tick();
        if (ticks >= T_SEND_END) break;
        pause28();
        frame();
        sent++;
        t0 = ticks - t0;
        st = 0;
        { unsigned int g = 0; while (!BUSY && ++g < 2000) tick(); st = BUSY; }   /* ~1 ms */
        if (st) rose++;
        else if (nev < NEV) {                /* BUSY did not rise: note when */
            ev[nev][0] = ticks & 0xff; ev[nev][1] = ticks >> 8; ev[nev][2] = TH0; ev[nev][3] = 0x80 | (MISO ? 1 : 0) | (BUSY ? 2 : 0);
            hdr[5] = ++nev;
        }
        if (nfr < NFR) {
            fl[nfr][0] = t0 > 255 ? 255 : t0; fl[nfr][1] = st; fl[nfr][2] = ticks & 0xff; fl[nfr][3] = ticks >> 8;
            for (i = 0; i < 32; i++) fl[nfr][4 + i] = rx[i];
            hdr[7] = ++nfr;
        }
        hdr[8] = sent & 0xff; hdr[9] = sent >> 8; hdr[10] = rose & 0xff; hdr[11] = rose >> 8;
    }

done:
    OEB = 0x00;                              /* release the bus */
    hdr[4] = 4;
    for (;;) ;
}

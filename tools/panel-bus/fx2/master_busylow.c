/* master: the analyser as the panel-link master after a normal boot, with the
 * main board cut off from SCK and RXD (ribbon contacts 17-20 masked at
 * CN8002, see docs/reference/xdj-1000mk2-panel-bus.md).
 *
 * Interlock first: SCK is only watched for ~200 ms; any clock edge means the
 * main board can still reach it, and nothing is driven (stage 9).  Then it
 * clocks a frame each time the panel pulls BUSY low, 28 us later, at the
 * stock ~857 kHz: lamps on and off every ~0.5 s and the jog ring stepping.
 * If BUSY does not rise after a frame, it waits 20 ms with no clock before
 * the next one.  Runs until halted (readlog.py halts it and reads the log).
 * D4 = PNL_RST (CN8002 leg 13): pulsed high ~33 ms to restart the panel
 * (as the stock main board does before talking to it), then held low: the main
 * board drives it through 242 ohm (R18 + R1176), so holding it low is a
 * ~14 mA fight at worst and keeps the panel out of reset.  It is sampled
 * first: hdr[14] = share of 200 ms it read high (0-255), hdr[15] = its edges.
 * After it is held, the panel's start-up (TXD/BUSY changes) is logged until
 * it is ready (BUSY high then low, at most 2 s).
 * Log as bootlog.c: hdr "XDJL", stage (1 watch, 3 sending, 9 refused),
 * events, frames logged, sent (u16), BUSY rose (u16), SCK edges seen (u16);
 * frames: (0, BUSY-rose flag, tick u16, 32 reply bytes). */
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
__sbit __at (0x94) RST;
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
    unsigned char i, ring = 1, nfr = 0, st, on, last;
    unsigned int sent = 0, rose = 0, edges = 0, t;
    CPUCS = 0x10;
    IFCONFIG = 0x80;
    IOB = 0x03;
    OEB = 0x00;                              /* nothing driven until the interlock passes */
    for (i = 0; i < 16; i++) hdr[i] = 0;
    hdr[0] = 'X'; hdr[1] = 'D'; hdr[2] = 'J'; hdr[3] = 'L'; hdr[4] = 1;
    TMOD = 0x01; TH0 = 0; TL0 = 0; TF0 = 0; TR0 = 1; ticks = 0;

    last = SCK;                              /* interlock: is anyone else clocking? */
    { unsigned int hi = 0, n = 0; unsigned char rlast = RST, redges = 0;
      while (ticks < 12) {                   /* ~200 ms; RST watched too */
        tick();
        if (SCK != last) { last = SCK; edges++; }
        n++; if (RST) hi++;
        if (RST != rlast) { rlast = RST; if (redges != 255) redges++; }
      }
      hdr[14] = (unsigned char)(((unsigned long)hi * 255) / n); hdr[15] = redges; }
    hdr[12] = edges & 0xff; hdr[13] = edges >> 8;
    if (edges) { hdr[4] = 9; for (;;) ; }    /* the main board still reaches SCK */

    /* SCK and RXD held high from here on, as the main board holds them: a
     * floating clock input during the panel's start-up can pick up edges.
     * Then restart the panel as the stock main board does before its first
     * frame: PNL_RST high ~33 ms (Q8026 pulls the M16C's RESET low), then low.
     * BUSY is held low through the restart and ~0.5 s after: a test of whether
     * the panel waits to see it low (a main board present) before it is ready. */
    IOB = 0x13;                              /* SCK, RXD high; RST high; BUSY latch 0 */
    OEB = 0x1b;                              /* ... and BUSY held low, as a present main board might */
    { unsigned char k = ticks + 2; while ((unsigned char)ticks != k) tick(); }
    IOB = 0x03;                              /* RST low: the panel starts */
    { unsigned char k = ticks + 31; while ((unsigned char)ticks != k) tick(); }   /* ~0.5 s */
    OEB = 0x13;                              /* let go of BUSY */
    hdr[4] = 2;
    { unsigned char nev = 0, st0, lst = 0xff, seen_high = 0; unsigned int t0 = ticks;
      while ((unsigned int)(ticks - t0) < 122) {     /* up to 2 s for it to be ready */
        tick();
        st0 = (MISO ? 1 : 0) | (BUSY ? 2 : 0);
        if (st0 != lst && nev < NEV) {
            ev[nev][0] = ticks & 0xff; ev[nev][1] = ticks >> 8; ev[nev][2] = TH0; ev[nev][3] = st0;
            hdr[5] = ++nev; lst = st0;
        }
        if (BUSY) seen_high = 1;
        else if (seen_high) break;
      } }
    { unsigned char k;                       /* BUSY driven, or floating? */
      __asm
        anl 0x90,#0xf7      ; BUSY latch 0
        orl 0xb3,#0x08      ; drive BUSY low ~1 us
        nop
        nop
        nop
        nop
        anl 0xb3,#0xf7      ; let go
      __endasm;
      for (k = 0; k < 20; k++) ;
      hdr[6] = BUSY ? 0xd1 : 0xf1; }        /* d1 = driven high again, f1 = stayed low: floating */
    hdr[4] = 3;
    OEB = 0x13;                              /* SCK and RXD are ours too, idling high */
    for (;;) {
        on = (sent / 166) & 1 ? 0 : 1;       /* ~0.5 s on, ~0.5 s off */
        for (i = 0; i < 30; i++) tx[i] = on ? LAMPS_ON[i] : 0;
        tx[22] = tx[23] = tx[24] = 0x02;
        if ((sent & 3) == 0) ring = ring >= 135 ? 1 : ring + 1;
        tx[9] = ring;
        { unsigned int s = 0;
          for (i = 0; i < 30; i++) { s += tx[i]; s = (s & 0xff) + (s >> 8); }
          tx[30] = (unsigned char)s; tx[31] = 0x8f; }
        t = 0; while (BUSY && ++t < 60000) tick();    /* ready (or give up, ~50 ms) */
        pause28();
        shift_frame();
        sent++;
        t = 0; while (!BUSY && ++t < 2000) ;           /* BUSY should rise ~3 us after */
        st = BUSY;
        if (st) rose++;
        else { unsigned char k = ticks + 2; while ((unsigned char)ticks != k) tick(); }   /* 20-30 ms quiet */
        if (nfr < NFR) {
            fl[nfr][0] = 0; fl[nfr][1] = st; fl[nfr][2] = ticks & 0xff; fl[nfr][3] = ticks >> 8;
            for (i = 0; i < 32; i++) fl[nfr][4 + i] = rx[i];
            hdr[7] = ++nfr;
        }
        hdr[8] = sent & 0xff; hdr[9] = sent >> 8; hdr[10] = rose & 0xff; hdr[11] = rose >> 8;
    }
}

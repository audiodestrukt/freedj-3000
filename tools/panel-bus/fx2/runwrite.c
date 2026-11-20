/* runwrite: send frames into a *running* deck, alongside its main board.
 *
 * Loaded after the deck has booted.  The stock main board clocks a frame
 * ~27 us after the panel pulls BUSY low; this starts the instant BUSY falls,
 * at the main board's speed, on every 4th frame, for 2 s, and releases D0/D1
 * (back to inputs) right after each frame.  The two masters overlap on SCK
 * and RXD for most of each of those frames: the user chose this knowing the
 * output-against-output risk to the main board's pins; total overlap is kept
 * to ~50 ms.  Same log layout as bootlog.c (read with readlog.py):
 *   hdr: "XDJL", stage (1 wait traffic, 3 sending, 4 done), 0, frames
 *   logged, frames sent (u16), frames after which BUSY rose (u16).
 *   frames: (0, BUSY-rose flag, tick u16, 32 reply bytes).
 */
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

#define T_RUN 122     /* 2 s of timer-0 overflows */

void main(void) {
    unsigned char i, ring = 1, nfr = 0, n, st;
    unsigned int sent = 0, rose = 0, cyc = 0;
    CPUCS = 0x10;
    IFCONFIG = 0x80;
    IOB = 0x03;                              /* SCK, RXD latches idle high */
    OEB = 0x00;                              /* inputs until a frame */
    for (i = 0; i < 16; i++) hdr[i] = 0;
    hdr[0] = 'X'; hdr[1] = 'D'; hdr[2] = 'J'; hdr[3] = 'L'; hdr[4] = 1;

    for (n = 0; n < 20; n++) {               /* the main board is polling: BUSY cycles */
        while (BUSY) ;
        while (!BUSY) ;
    }
    TMOD = 0x01; TH0 = 0; TL0 = 0; TF0 = 0; TR0 = 1; ticks = 0;
    hdr[4] = 3;
    while (ticks < T_RUN) {
        tick();
        while (!BUSY) tick();                /* wait for a frame to end */
        if ((++cyc & 3) != 0) { while (BUSY) ; continue; }   /* every 4th */
        for (i = 0; i < 30; i++) tx[i] = LAMPS_ON[i];         /* frame ready first */
        tx[22] = tx[23] = tx[24] = 0x02;
        ring = ring >= 135 ? 1 : ring + 1;
        tx[9] = ring;
        { unsigned int s = 0;
          for (i = 0; i < 30; i++) { s += tx[i]; s = (s & 0xff) + (s >> 8); }
          tx[30] = (unsigned char)s; tx[31] = 0x8f; }
        while (BUSY) ;                       /* the panel is ready: the race starts */
        OEB = 0x03;                          /* drive SCK, RXD */
        shift_frame();
        OEB = 0x00;                          /* let go at once */
        sent++;
        { unsigned int g = 0; while (!BUSY && ++g < 2000) ; st = BUSY; }
        if (st) rose++;
        if (nfr < NFR) {
            fl[nfr][0] = 0; fl[nfr][1] = st; fl[nfr][2] = ticks & 0xff; fl[nfr][3] = ticks >> 8;
            for (i = 0; i < 32; i++) fl[nfr][4 + i] = rx[i];
            hdr[7] = ++nfr;
        }
        hdr[8] = sent & 0xff; hdr[9] = sent >> 8; hdr[10] = rose & 0xff; hdr[11] = rose >> 8;
    }
    OEB = 0x00;
    hdr[4] = 4;
    for (;;) ;
}

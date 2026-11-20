# XDJ-1000MK2: driving the real panel from OpenDeck

Notes from the unit's service schematic (Pioneer order no. RRV4661, Aug 2016;
Dan's own copy, **not** in this repo — it is Pioneer's document, these notes
are ours). Goal: use the XDJ's own jog, buttons, LEDs, tempo fader, browse
encoder and touch screen with a Pi running OpenDeck. Written 2026-10-25,
before anything was probed; every voltage and polarity below is "per the
schematic", to be confirmed with a meter before connecting a Pi.

## How the unit is built

| Board | Pioneer no. | What is on it |
|---|---|---|
| MAIN | DWX3920 | main CPU **Renesas SH7724** (R8A77240D500BG2, SH-4A, 500 MHz), a TI DSP coprocessor booted over HPI, DRAM, audio, Ethernet (RMII), USB host |
| LCDB | DWX3921 | the 7" TFT (module CWX4352) and its resistive touch panel (DSX1128), DC/DC for LCD and LEDs; **passes the panel link through** to PNLB on two 20-pin FFCs |
| PNLB | DWX3923 | **PANEL CPU** IC8003, **Renesas M16C/64A** (R5F364AENFA, 4 MHz crystal). Owns every control: buttons (analog key ladders), LEDs, browse encoder, jog, jog sheet switch, tempo fader, and the LCD's resistive touch panel |
| KSWB | DWX3924 | PLAY / CUE switches + LEDs, TRACK SEARCH / SEARCH switches on a resistor ladder (KEY0) |
| SLDB | DWX3925 | tempo fader (pot, ADIN) with its centre-detent sense (ADCT) |
| JFLB | DWX3927 | jog centre **VFD** (DEL1074-A, "JOG FL") with its 26 V/12 V supplies; buffers the jog encoder; carries the jog sheet switch |
| JOGB | DWX3928 | the jog **photo-interrupter** (KE-2K18-15, two channels) = quadrature A/B |
| USBA / PSWB / UTMB / ACIN | DWX3922 / 3929 / 3930 / 3926 | front USB, power switch, utility (Ethernet, digital out), mains in |

The architecture is the same shape as the CDJ-3000's (see
`cdj-3000-target.md`): a sub-CPU owns the whole front panel and talks to the
main CPU over one serial link. Everything a DJ touches, including the
touchscreen, arrives through that link.

## The panel link

Six signals between the SH7724 and the M16C, routed MAIN CN1103 → LCDB CN2001
(pins 38–45) → LCDB CN2602 → PNLB **CN8002**:

| Signal | M16C pin | SH7724 side | Direction (per schematic) | Notes |
|---|---|---|---|---|
| PNL_SCK | 33, P6_5/CLK1 | SCIF2_SCK | main → panel | serial clock; the M16C's UART1 in **clock-synchronous** mode with external clock, i.e. SPI-like, main is clock master |
| PNL_RXD | 32, P6_6/RXD1 | SCIF2_TXD | main → panel | data to the panel (MOSI) |
| PNL_TXD | 31, P6_7/TXD1 | SCIF2_RXD | panel → main | data from the panel (MISO), 220 Ω series |
| PNL_BUSY | 34, P6_4 | GPIO PTH0 | panel → main | handshake: panel says when it can be clocked |
| PNL_RST | 13, RESET | GPIO (open-collector driver) | main → panel | panel reset, active low |
| PNL_CNVSS | 14, CNVSS | GPIO | main → panel | M16C flash-programming mode select. High = boot mode (the main CPU can reflash the panel firmware). **Keep low.** |

Logic level: the M16C's VCC is V+3R3E (3.3 V regulated on PNLB from V+3R3),
and the SH7724 I/O is 3.3 V, so the link is **3.3 V**: directly compatible
with a Pi's SPI0 (SCLK, MOSI, MISO) plus two GPIOs (BUSY in, RST out). Verify
on the unit with a meter before wiring.

### CN8002 (PNLB ↔ LCDB CN2602), 20-pin FFC

| Pin | Signal | Pin | Signal |
|---|---|---|---|
| 1 | V+5R2 | 11 | GND |
| 2 | GND_LED | 12 | PWR_SW |
| 3 | V+5R2 | 13 | PNL_RST |
| 4 | GND_LED | 14 | PNL_CNVSS |
| 5 | GND_LED | 15 | PNL_TXD |
| 6 | QTZ_ILM (QUANTIZE lamp) | 16 | EUP_CON |
| 7 | EUPL (lamp) | 17 | PNL_RXD |
| 8 | USB_ILM_G | 18 | PNL_BUSY |
| 9 | USB_ILM_R | 19 | PNL_SCK |
| 10 | USB_ILM_B | 20 | GND |

**The two ends are numbered opposite ways.** The DDD1787 is reverse face, so
CN8002 pin *n* is LCDB CN2602 pin 21−*n* (RRV4661 §10.1): PNL_SCK is CN8002
19 but CN2602 **2**, GND is CN8002 20 but CN2602 1. A breakout or header
numbered from the LCDB side puts "pin 20" on V+5R2, so count from the PNLB
socket and confirm with a meter (GND beeps to chassis, V+5R2 reads 5 V) before
clipping anything. On the LCDB side the signals are CN2602 2 SCK, 3 BUSY, 4
RXD, 6 TXD, 7 CNVSS, 8 RST, 9 PWR_SW; GND on 1, 10, 16, 17, 19.

**Tapping without a passthrough.** The cable cannot just be unplugged and
probed: PWR_SW (pin 12) rides on it, so the deck will not power on, and with
the two CPUs apart nothing talks anyway. On PNLB (§10.14) each link line has a
series resistor with a test pad between it and the M16C, so the analyser can
clip there with the stock cable in: BUSY pin 34 → **R8070** 100 Ω (out),
SCK pin 33 → **R8071** 22 Ω (in), RXD pin 32 → **R8102** 22 Ω (in), TXD pin 31
→ **R8072** 220 Ω (out).

### CN8001 (PNLB ↔ LCDB CN2601), 20-pin FFC

| Pin | Signal | Pin | Signal |
|---|---|---|---|
| 1 | V+3R3 | 11 | V_DET |
| 2 | GND_FL | 12 | GND |
| 3 | V+12FL | 13 | RTY_LED |
| 4 | V+12FL | 14 | TCP_XV |
| 5 | GND_FL | 15 | TCP_YV |
| 6 | GND_FL | 16 | GND |
| 7 | ENC_2 | 17 | TCP_YIN |
| 8 | KEY1 | 18 | TCP_XIN |
| 9 | ENC_1 | 19 | TCP_XIN |
| 10 | ENC_SW | 20 | V+3R3E |

ENC_* is the browse encoder (on LCDB), KEY1 the key ladder for the buttons
around the display, TCP_* the 4-wire resistive touch panel, which the panel
MCU drives (XV/YV) and reads (XIN/YIN on its ADC). So the touch screen is a
panel-CPU input, exactly as on the CDJ-3000.

Power the PNLB needs from an interposer: **V+3R3**, **V+5R2** (LEDs), and for
the jog VFD **V+12FL** and V+5R2FL (through CN8004 to JFLB).

## Connectors and cables (from the overall wiring diagram + parts list)

| Link | Cable | Board-side connectors |
|---|---|---|
| LCDB ↔ PNLB, both links | **2× DDD1787**: 20-way FFC, **1.0 mm pitch**, 40 mm long, *reverse face* (contacts on opposite sides at the two ends) | LCDB CN2601/CN2602 = DKN1663-A, **vertical**; PNLB **CN8001/CN8002 = VKN1280-A, horizontal** |
| PNLB ↔ JFLB (jog + VFD) | **DDD1681**: 16-way FFC, 1.0 mm pitch, 80 mm, *same face* | PNLB CN8004 = VKN1276-A horizontal; JFLB CN9202 = VKN1247-A vertical |
| MAIN ↔ LCDB | DDD1675, **60-way FFC, 0.5 mm pitch**, 140 mm, reverse face. **This is the cable you unplug to split the top half from the bottom half**: it carries the LCD's parallel RGB bus (R/G/B data, DCLK, backlight) *and* the panel link (pins 36–45, below). Not a tap point: 0.5 mm 60-way breakouts are rare and extending the pixel bus is asking for display trouble | MAIN CN1103 / LCDB CN2001 = CKS6543 |
| PNLB ↔ KSWB (PLAY/CUE) | 7-way pre-made jumper harness D20PDY0720E, 200 mm | Molex 52147-0710 (CN8501) / 51048-0700 holder (JH8004); confirm pitch with calipers, Molex 52147 family |
| PNLB ↔ SLDB (fader) | 4-way jumper D20PDD0410E, 100 mm | Molex 51048-0400 both ends |
| JFLB ↔ JOGB (jog sensor) | 4-way jumper D20PYY0405E, 50 mm | Molex 52151-0410 (CN9301) / 51048-0400 (JH9201) |

Panel-link pins on the 60-way (MAIN CN1103 = LCDB CN2001), for reference
only: 36 PWR_SW, 37 EUP_CONT, 38 PNL_RST, 39 GND, 40 PNL_RXD, 41 PNL_TXD,
42 PNL_BUSY, 43 PNL_CNVSS, 44 GND, 45 PNL_SCK. The signals then pass through
the LCDB unchanged to CN2602 and the 20-way DDD1787 into PNLB CN8002.

**Where the 20-way cables are.** Both DDD1787s live *inside the top
assembly*: 40 mm jumpers between the LCD board (behind the LCD) and the
panel board (behind the faceplate). Splitting the halves only exposes the
60-way; reach the 20-ways from the underside of the top half, where the LCDB
sits over the PNLB (there may be a shield plate over them).

So the panel link is **generic 1.0 mm-pitch 20-way FFC**, nothing proprietary:
the VKN/DKN numbers are Pioneer's own numbers for ordinary 1.0 mm ZIF sockets.

**Interposer / sniffer.** Replace one DDD1787 with a longer 20-way 1.0 mm FFC
(100–150 mm) through a 1.0 mm 20-pin FFC breakout (two ZIF sockets in series
with a 0.1" header: an off-the-shelf part), and the logic analyser clips on
the header. With the stock main board unplugged, the same breakout is where
the Pi connects, so one board serves both steps. For our own board: two
20-way 1.0 mm **dual-contact** ZIF sockets (contacts top and bottom, so cable
face does not matter) for CN8001/CN8002, plus a 16-way 1.0 mm socket if the
jog VFD link is taken over. Buy both *same-face* and *reverse-face* 20-way
cables: which one mates CN8001's horizontal socket depends on which side its
contacts are on, and the schematic does not say.

## Frame format (captured 2026-11-19)

Tapped at CN8002's solder legs with the stock cable in, deck running, 16 MHz
fx2 analyser (`tools/panel-bus/capture.sh`).

- **Clock** ~842 kHz, idles high. Sample on the **rising** edge, **LSB first**
  (the M16C UART default).
- **One 32-byte frame each way per transfer**, full duplex, 256 clocks with no
  gaps, every ~3.0 ms (~333 Hz; 2.5–2.8 ms between frames).
- **BUSY is the panel's "ready", active low, and the panel sets the pace.**
  BUSY idles high; the panel pulls it low when its reply is loaded, the main
  board starts clocking ~27 µs later, clocks all 256 bits in 318 µs, and BUSY
  goes high again 2.4 µs after the last clock. The next low comes ~2.65 ms
  later. A master just waits for BUSY low, then clocks one frame.
- **Byte 30 = checksum**: 8-bit ones'-complement sum (add, fold the carry back
  in) of bytes 0–29. Held on 1666/1666 frames each way. **Byte 31 = 0x8F**.
  An all-0xFF frame (the panel not driving TXD, the line idling high) also
  passes this sum, so check the header too: panel byte 0 is 0x81.

Main → panel at idle is one fixed frame (bytes 0–29):
`30 df 00 00 00 00 00 00 58 00 00 10 00 00 00 00 00 00 00 00 00 00 02 02 02 00 00 00 00 00`.
A one-frame blip of byte 1 = 0x1E recurs every few seconds, not tied to
input; byte 0 blipped to 0xB0 once. Byte 1 bit 2 cleared (0xDF → 0xDB) and
stayed clear after the button at panel byte 20 bits 1–2 was pressed: the first
lamp/mode output seen. The rest of the LED fields are still unknown.

### Power-on (captured with the deck switched on mid-capture)

No handshake. RST/CNVSS were not on the analyser, so whether the main board
pulses the panel's reset is not known. The capture is kept in
`reference/panel-bus/xdj-1000mk2-boot-2026-11-19.sr` (raw, sigrok) and `.log`
(every burst decoded, from power-on to 40 steady frames after the byte-wise
stretch).

1. Power on: the panel CPU comes up and holds **BUSY low** (ready).
2. ~7 s of silence while the main board boots.
3. First two frames from the main board, likely a lamp test (bytes 7, 9, 10,
   12, 18 set, never seen in steady state):
   `73 18 00 00 00 00 00 33 40 3f 28 10 01 00 00 00 00 00 20 00 00 00 02 02 02 00 00 00 00 00`.
4. **The panel's first reply is one bit late**: the normal reply shifted right
   one bit (header 0x40, not 0x81) with a bad trailer. Its receiver is not in
   step until a frame has gone by. Drop it.
5. One frame of all-zero lamps (`00 18 00 … 00 10 …`), then steady polling;
   every reply from the second on is valid.
6. ~1.8 s later the main board sends the same frames **one byte at a time**,
   80 µs or more between bytes and some frames in pieces (its own driver
   changing hands). The panel does not mind: a frame is delimited by BUSY, not
   by clock gaps.

To start the panel ourselves: CNVSS low, release RST, wait for BUSY low, clock
frames, drop the first reply (optionally send the lamp test first).

### Buttons (panel → main, 1 = pressed)

Each named by pressing it twice in a known order (`tools/panel-bus/bits.py`).

| Bit | Button |
|---|---|
| 16.0 | PLAY |
| 16.1 | CUE |
| 18.0 | TRACK SEARCH ◀ |
| 18.1 | TRACK SEARCH ▶ |
| 18.2 | SEARCH ◀◀ |
| 18.3 | SEARCH ▶▶ |
| 16.4 | LOOP IN (order not yet confirmed by the presser) |
| 16.3 | LOOP OUT (ditto) |
| 16.2 | RELOOP/EXIT (ditto) |
| 16.7 | REV |
| 20.0 | VINYL (jog mode) |
| 20.1 | TEMPO RANGE |
| 20.2 | MASTER TEMPO (by elimination) |
| 19.0 | QUANTIZE |
| 19.1 | TIME MODE |
| 19.2 | BACK |
| 19.3 | TAG TRACK |
| 19.4 | SHORTCUT |
| 19.5 | seen in the first sweep, not named: USB STOP by elimination |
| 17.0 | browse rotary push |

There is no BEAT SYNC or MASTER key: the schematic's only key labels are PLAY,
CUE, LOOP IN/OUT, RELOOP, REV, VINYL, M.TEMPO, T.RANGE, the jog touch, and on
LCDB BACK, TAG TRACK, TIME MODE, SHORTCUT, QUANTIZE, USB STOP and the rotary
(turn + push). Sync and master are on the touch screen.

**Ladder keys glitch.** 20.1 and 20.2 share an analog key ladder, and a press
passes through the neighbouring code for 30–90 ms (TEMPO RANGE shows brief
20.2s, MASTER TEMPO a brief 20.1 first). Accept a ladder key only after it has
read the same for ~100 ms. Panel byte 15 bit 1 dropped to 0 exactly while REV
was held; meaning unknown.

### Lamps (main → panel), provisional

Seen to follow the deck's own state; names are inferred. The full map wants a
bit walk once we drive the panel ourselves (one bit set per frame, note what
lights).

| Bit | Followed |
|---|---|
| 0.0, 0.1 | PLAY: alternate every 0.5 s while paused (two bits: maybe two levels) |
| 0.4, 0.5 | CUE: blink, 300 ms on / 300 ms off |
| 0.2 | REV on |
| 0.6 | set by LOOP OUT (loop active?) |
| 1.2 | toggled with MASTER TEMPO, and also during TEMPO RANGE presses |
| 1.3 | QUANTIZE on |
| 1.1, 1.7, 8.3, 8.4 | VINYL on |
| 7.1–7.6 | LOOP IN / LOOP OUT / RELOOP states |
| 8.5 | CUE (0x58 → 0x78) |
| 1.0, 1.6, 1.7 | single-frame blips every ~50 ms or few seconds; not a lamp |
| 22–24 bits 1, 3, 5 | alternate every 0.5 s, always; unknown |

With a track playing (not analysed in rekordbox, so no waveform):

| Main → panel byte | Meaning |
|---|---|
| 9 | **jog ring playhead**, 0x01–0x87 = 135 positions; +1 every ~13.3 ms, wraps every 1.80 s (one turn at 33⅓ rpm); holds while paused. The panel CPU draws it on the jog VFD; the main board only sends the position |
| 22, 24 | toggle together 0x02 ↔ 0x28 every 0.5 s while playing (a blinking lamp or the beat; the track was ~120 BPM, so not yet told apart) |
| 0, 8 | changed with the CUE/PLAY presses (byte 8 0x58 → 0x78 → 0x58): lamps, unnamed |

The jog VFD itself (JFLB §10.16, V9201 DEL1074-A) is a chip-in-glass FL with a
shift-register input (SI, CLK, LAT, BK, VDD, VH), fed from the panel CPU over
CN8004 4 J_SCLK, 5 J_DSO (M16C pin 30 via R8134), 6 J_LAT, 7 J_BK_N. Driving
it directly instead of through byte 9 would give arbitrary patterns, at the
cost of splitting CN8004 (it also carries the jog sensor to PNLB).

Panel → main, decoded so far:

| Byte | Meaning |
|---|---|
| 0–1 | `81 00`, constant: byte 0 = 0x81 is the header to check |
| 2 | slow ±1 drift between a few values per session (0x00, 0x7A, 0xAC, 0xB2/0xB3); not a control; unknown |
| 3, 12, 13, 21, 26–29 | always 0x00 in every capture |
| 4–5 | **tempo fader**, 16-bit big-endian, 0x00xx at the top (slow), 0xFFxx at the bottom, 0x80xx at centre; low byte is noise |
| 6–7 | a second analog channel, 0x80xx at rest; not the touch panel; unknown |
| 14 | **browse rotary**, 8-bit wrapping counter, +1 per detent clockwise (0xFE → 0xFF → 0x00 → 0x01) |
| 16, 18, 19, 20 | **button bits**, 1 = pressed; see *Buttons* below |
| 8–9 | **jog position**, signed 16-bit big-endian, free-running (~600 counts for a part turn) |
| 10–11 | **jog speed**: period between pulses, 0xFFFF stopped, smaller = faster |
| 15 | bit 7 **jog moving**, bit 6 **jog direction** (1 = counting up, held after stop), bits 4–5 probably **jog touch** (1 = released) |
| 16 | bit 0 **PLAY** (1 = pressed) |
| 22–23 | **LCD touch X**, 16-bit big-endian, 10-bit range, counted from the **right** edge: ~55 at the right, ~950 at the left |
| 24–25 | **LCD touch Y**, 16-bit big-endian, from the top: ~85 at the top, ~900 at the bottom |

No touch reads X and Y as 0–3 (the low bytes flicker; treat both under ~16
as released). Corners measured: top-left (0x3B5, 0x057), top-right (0x037,
0x053), bottom-right (0x03B, 0x380), bottom-left (0x3B6, 0x38A).

## In OpenDeck

`crates/app/src/surface/xdjpanel.rs` decodes panel frames into the same
events as the CDJ-3000's `subucom` (keys by their cdj3k-emu names, jog,
fader, rotary, LCD touch). `OPENDECK_XDJPANEL=sniff` listens to a stock unit
through the fx2 analyser (`sigrok-cli` streaming 4 MHz to the app), so the
deck's own controls drive OpenDeck while its main board keeps the panel and
the lamps. Checked on the unit 2026-11-19: keys, rotary, fader (direction
fixed), jog (CDJ bend / VINYL scrub). Jog counts per turn and the jog touch
polarity are still guesses. REV has no action: OpenDeck has no reverse play.

## Driving the panel (master side), first attempts

The fx2 analyser can drive its pins (no input buffer on this one), so
`tools/panel-bus/fx2/` holds 8051 firmware that bit-bangs the master side.
With the main board still connected the only free time is the power-on
window (the main board first clocks ~7 s after power-on), so `bootlog.c`
sends only between the panel's ready (~0.4 s) and 4 s, and logs to RAM.

- The panel answers our clock: the first reply is a valid frame (header,
  fields, checksum). So mode 3 / LSB first / BUSY are right for a master too.
- Then it falls out of step: the end of the reply is one bit early (the panel
  counted one clock more than was sent), BUSY never rises again, TXD idles.
  Most likely ringing: D0 drives a sharp edge into the whole SCK net, which
  runs back through the ribbon, LCDB and the 60-way to the main board, and
  one edge reads as two. Next try: 100-330 Ω in series with D0 (and D1) at
  the analyser.
- No lamp change has been seen yet. The real master (Pi or RP2350 on a
  passthrough with 17 and 19 left out) does not share the net with the main
  board and should not see this.

## Route A: speak to the panel CPU (recommended)

Unplug the two FFCs between LCDB and PNLB and plug in an interposer board
that supplies power, ties CNVSS low, drives RST, and brings SCK/RXD/TXD/BUSY to
the Pi's SPI0 + 2 GPIO. The PNLB keeps its own firmware and keeps scanning
everything; OpenDeck gets buttons, encoder, jog, fader, touch and drives every
LED and the jog VFD through one connector. The CDJ-3000 adapter planned in
`cdj-3000-target.md` ("opendeck-subucom") is the same shape of code.

**What is not known: the frame format.** It has to be captured on the live
unit with a logic analyser on CN8002 pins 15/17/18/19/20 (TXD, RXD, BUSY, SCK,
GND) while the stock main board runs: idle traffic, then one button at a
time, jog turns, fader moves, a touch, and the LEDs changing. A 24 MHz
8-channel analyser is enough (the clock will be a few MHz at most; the
M16C's UART in sync mode tops out around f/2 of its bus clock). Decode as SPI
mode 0–3, find the one that gives stable bytes, then the frame length, the
BUSY timing, and the field map. Expect something like the 3000's: a fixed
frame, inputs as bitmasks, encoder/fader as 16-bit fields, LED bits on the
other direction, a checksum. The main board's firmware is never read or
touched: this is bus observation on our own unit, the same stance as the
Link captures.

Then: `crates/app/src/surface/` gets a third backend next to `midi` and
`i2c` (`panel = "xdj1000mk2"` in controllers.toml), owning `/dev/spidev0.0`
and the two GPIOs, polling at whatever rate the capture shows the stock
main board uses.

## Route B: tap the raw signals (no protocol work)

Everything the panel CPU reads is on plain connectors, so our RP2350 hub
board (`hardware/control-board`) can take them instead:

| Connector | Pins | Signals | Into the hub |
|---|---|---|---|
| **CN8004** (PNLB ↔ JFLB CN9202), 16-pin | 1 JOG1, 2 JOG2, 3 JOG_SW, 4 J_SCLK, 5 J_DSO, 6 J_LAT, 7 J_BK_N, 8 V+3R3, 9/11/13/15 GND, 10/12 V+12FL, 14/16 V+5R2FL | jog A/B (3.3 V, buffered on JFLB), jog sheet switch, and the **jog VFD's 3-wire serial** (clock, data, latch, blank) | J_JOG A/B/TOUCH directly; the VFD from a spare SPI or PIO |
| **CN9301** (JOGB), 4-pin | 1 JOG2, 2 JOG1, 3 V+3R3, 4 GNDD | the raw photo-interrupter, if JFLB is not kept | J_JOG |
| **CN8501 / JH8004** (KSWB), 7-pin | 1 PLAY, 2 CUE, 3 PLAYL, 4 CUEL, 5 KEY0, 6 GND, 7 V+5 | PLAY/CUE switches to GND; PLAYL/CUEL drive the LEDs; KEY0 is a **resistor ladder** (T.SEARCH ±, SEARCH ± at 3.3k/5.1k steps) | 2 GPIO + 2 LED outputs + **ADC** |
| **JH8003 / JH8701** (SLDB), 4-pin | 1 V+3R3E, 2 ADIN, 3 ADCT, 4 GNDS | tempo pot wiper, detent sense | ADC + GPIO |
| CN8001 | 7/9/10 ENC_2/ENC_1/ENC_SW, 8 KEY1 | browse encoder, display-side key ladder | 3 GPIO + ADC |

The RP2350A has exactly four ADC inputs: KEY0, KEY1, KEY2 (the third ladder
stays on PNLB) and ADIN fit, but only just, and the LEDs on PNLB itself
(loop, reloop, sync, master, vinyl, …) and the touch panel stay behind the
panel CPU. Route B is a good way to get a real jog, PLAY/CUE and fader into
the hub in an afternoon; it is not the whole face.

## Which unit

This is the only XDJ-1000MK2 we have and it is the Pro DJ Link reference for
everything in this repo. Route A is non-destructive by design: the interposer
goes between two existing FFC connectors and the stock LCDB ribbons go back
in minutes. Do not cut or solder on the original boards; a second-hand or
faulty MK2 is the donor if the face is to live permanently on a Pi.

## Corrections to earlier notes

The XDJ-1000MK2's main CPU is a **Renesas SH7724**, not a TI DaVinci: the TI
part previously identified from its marking is the DSP coprocessor (HPI
boot) next to it. The conclusion in `cdj-3000-target.md` stands: the unit is
not a software target for OpenDeck, single SH-4A core, no GPU; but its front
panel is a very good peripheral.

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

| Signal | M16C pin | SH7724 side (MAIN §10.2/§10.8) | Direction | Notes |
|---|---|---|---|---|
| PNL_SCK | 33, P6_5/CLK1 | SCIF2_SCK via R1171 220 Ω | main → panel | serial clock; the M16C's UART1 in **clock-synchronous** mode with external clock, i.e. SPI-like, main is clock master |
| PNL_RXD | 32, P6_6/RXD1 | SCIF2_TXD via R1175 220 Ω | main → panel | data to the panel (MOSI) |
| PNL_TXD | 31, P6_7/TXD1 | SCIF2_RXD via R1174 100 Ω | panel → main | data from the panel (MISO), R8072 220 Ω on PNLB |
| PNL_BUSY | 34, P6_4 | GPIO via R1173 100 Ω | panel → main | handshake. **The panel drives it high while busy and lets go when ready; "low" comes from the main board's side** (see *Driving the panel*) |
| PNL_RST | 13 → Q8026 → RESET | GPIO (CPU_PNL_RST) via R18 22 Ω + R1176 220 Ω, R27 4.7 k pull-down | main → panel | **active high at the connector**: high turns Q8026 on and pulls the M16C's RESET low. Push-pull, not open-collector. The panel has its own reset IC (IC8001 PST8430N), so a floating or low PNL_RST lets it run. Pulsing it resets the panel, which glitches PWR_SW and **reboots the deck** |
| PNL_CNVSS | 14, CNVSS (pin 10) | GPIO via R1172 220 Ω | main → panel | M16C flash-programming mode select. High = boot mode (the main CPU can reflash the panel firmware). **Keep low.** R8029 4.7 k on PNLB looks like a pull-down (not yet metered) |
| EUP_CON | 16 | XEUP_CONT via R1177 100 Ω | **panel → main** (an input on MAIN) | power/standby signalling with PWR_SW (12); the panel wakes the deck from standby when a control moves |

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
  BUSY idles high; it goes low when the panel's reply is loaded, the main
  board starts clocking ~27 µs later, clocks all 256 bits in 318 µs, and BUSY
  goes high again 2.4 µs after the last clock. The next low comes ~2.65 ms
  later. A master just waits for BUSY low, then clocks one frame. Electrically
  the panel only *drives* the high: for "ready" it lets go and the main
  board's side pulls the line low (*Driving the panel* below), so a master
  without that pull-down sees BUSY high for ever.
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

To start the panel ourselves: see *Driving the panel* below (CNVSS and RST
low, SCK/RXD high from power-up, a pull-down on BUSY, frames every ~3 ms).

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
| 19.5 | TRACK FILTER (confirmed on the unit with the map page): the sixth key on the KEY1 ladder (LCDB §10.11: BACK S2601, TAG TRACK S2604, FILTER S2602, SHORTCUT S2605, TIME MODE S2606, QUANTIZE S2608), the only one left unmatched |
| 17.0 | browse rotary push |

**USB STOP is not on the panel link.** Its switch (S2607 on LCDB) is a plain
contact to ground on its own net, USB_STOP, to LCDB's connector to the main
board: the main CPU reads it directly, and pressing it changes nothing in the
panel's frame.

There is no BEAT SYNC or MASTER key: the schematic's only key labels are PLAY,
CUE, LOOP IN/OUT, RELOOP, REV, VINYL, M.TEMPO, T.RANGE, the jog touch, and on
LCDB BACK, TAG TRACK, FILTER, TIME MODE, SHORTCUT, QUANTIZE (the KEY1 ladder),
USB STOP (to the main board) and the rotary (turn + push). Sync and master are on the touch screen.

**Ladder keys glitch.** 20.1 and 20.2 share an analog key ladder, and a press
passes through the neighbouring code for 30–90 ms (TEMPO RANGE shows brief
20.2s, MASTER TEMPO a brief 20.1 first). Accept a ladder key only after it has
read the same for ~100 ms. Panel byte 15 bit 1 dropped to 0 exactly while REV
was held; meaning unknown.

### Lamps and the jog display (main → panel)

First from the stock captures (what changed with which key), then checked by
driving the panel (2026-11-21, `OPENDECK_XDJPANEL_POKE`, see *In OpenDeck*).
"Driven" = confirmed by setting it ourselves and looking.

| Field | Meaning | How sure |
|---|---|---|
| 0.0 | PLAY lamp (on while playing; the stock deck blinks it paused) | driven |
| 0.1 | alternates with 0.0 while paused on the stock deck; a second PLAY level? | captures |
| 0.2 | REV lamp | captures |
| 0.4, 0.5 | CUE lamp (blinks 300 ms on / off on the stock deck) | captures |
| 0.6 | set by LOOP OUT on the stock deck | captures |
| 1.2 | MASTER TEMPO lamp | driven |
| 1.3 | QUANTIZE lamp | driven |
| 1.1, 1.7, 8.3, 8.4 | VINYL: the key's lamp and the **"VINYL" mark in the jog display's centre** (all four switch together) | driven |
| **7** | **red cue mark on the jog ring: 1–80 round the ring, 0 = none**, wrapping past 80 (81 = 1). Each step lights a pair of red segments (never one alone: the ring is ~160 segments in 80 pairs). Same origin as the playhead ring (byte 7 = 1 and byte 9 = 1 line up), so the marks meet when the playhead reaches the cue. The stock "LOOP IN" changes in this byte were the cue point moving | driven |
| 8.5 | the jog display's **inner white ring** (set by CUE on the stock deck) | driven |
| **9** | **jog ring playhead**, 1–135, one turn per 1.8 s (below) | driven |
| 10 | no visible change at 40, 100 or 255 | driven |
| **11** | **standby**: 0xFF (the stock deck sends 0x10) puts the panel into a standby state: lamps and display off, the controls still reported but scanned slowly (long latency). Restoring 0x10 does not bring it back; a power cycle does. Which bit: not narrowed | driven |
| 12.0 | lights a single white ring inside the display | driven |
| 2–5 | at 40 together (with 8.5): both inner white rings and the outer ring lit except at the playhead; not narrowed to a byte or bit | driven |
| 13–21, 25–29, 2–6 at 0xFF | no visible change | driven |
| 22–24 | bits 1, 3, 5 alternate every 0.5 s on the stock deck; 0xFF shows nothing new | captures / driven |
| 1.0, 1.6, 1.7 | single-frame blips every ~50 ms or few seconds on the stock deck; not a lamp | captures |

A **position** field shows nothing when set to 0xFF (out of range): the cue
mark was only found by setting the untried bytes to a valid position (40) and
halving. Bytes still open: 0.1/0.4–0.6, the rest of 8, 2–5, 12, and the bits of
11 (which must not be set while hunting, or the panel needs a power cycle).

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
| 2 | **VINYL SPEED ADJUST knob**, 8-bit (range end to end not yet measured; it held 0x00, 0x7A, 0xAC, 0xB2 in different sessions because the knob sat there). The CDJ-1000MK3 has its two vinyl-speed knobs (TOUCH/BRAKE, RELEASE/START) in bytes 2 and 3 |
| 3, 12, 13, 21, 26–29 | always 0x00 in every capture |
| 4–5 | **tempo fader**, 16-bit big-endian, 0x00xx at the top (slow), 0xFFxx at the bottom, 0x80xx at centre; low byte is noise |
| 6–7 | a second analog channel, 0x80xx at rest; by the CDJ-1000MK3's layout (below) the tempo fader's centre-detent pot, ADCT |
| 14 | **browse rotary**, 8-bit wrapping counter, +1 per detent clockwise (0xFE → 0xFF → 0x00 → 0x01) |
| 16, 18, 19, 20 | **button bits**, 1 = pressed; see *Buttons* below |
| 8–9 | **jog position**, signed 16-bit big-endian, free-running: **~3272 counts per turn** (one turn by hand; the CDJ-3000's is 3240), **counting up clockwise** (checked on the unit) |
| 10–11 | **jog speed**: period between pulses, 0xFFFF stopped, smaller = faster |
| 15 | bit 7 **jog moving**, bit 6 **jog direction** (1 = counting up, held after stop), bits 4–5 **jog touch**: 11 = hand on the top plate, 00 = free (checked with the map page) |
| 16 | bit 0 **PLAY** (1 = pressed) |
| 22–23 | **LCD touch X**, 16-bit big-endian, 10-bit range, counted from the **right** edge: ~55 at the right, ~950 at the left |
| 24–25 | **LCD touch Y**, 16-bit big-endian, from the top: ~85 at the top, ~900 at the bottom |

No touch reads X and Y as 0–3 (the low bytes flicker; treat both under ~16
as released). Corners measured: top-left (0x3B5, 0x057), top-right (0x037,
0x053), bottom-right (0x03B, 0x380), bottom-left (0x3B6, 0x38A).

## Kin: the CDJ-1000MK3 panel link

Anatsko Andrei's decode of the CDJ-1000MK3 panel link ("Reverse Engineering
Pioneer CDJ-1000 serial protocol.pdf", 2017, in
github.com/djgreeb/CDJ-1000mk3_new_life_project, whose STM32 drives the stock
panel) has nearly the same panel → main layout, so the XDJ-1000MK2 panel
firmware descends from it:

| Field | CDJ-1000MK3 | XDJ-1000MK2 |
|---|---|---|
| vinyl speed knobs | byte 2 TOUCH/BRAKE, byte 3 RELEASE/START, 8-bit ADC | byte 2 VINYL SPEED ADJUST (one knob); byte 3 always 0 |
| tempo fader | bytes 4–5, MSB first, 14-bit | bytes 4–5, MSB first |
| pitch centre-detent pot (ADCT) | bytes 6–7 | bytes 6–7 (so that is what they are) |
| jog pulse counter | bytes 8–9, **3600 per turn** | bytes 8–9, ~3272 per turn |
| jog speed | bytes 10–11, 65535 = stopped | bytes 10–11, 0xFFFF = stopped |
| jog flags | bit 7 rotating, bit 6 direction, bits 4–5 touch | byte 15, same bits |
| keys | bitmasks, bytes 14–18 | bitmasks, bytes 16–20 |
| check | byte sum mod 256 | ones'-complement sum |

Main → panel differs: the MK3 rotates 8 "packages" of 27 bytes (package
number in byte 0) because it also feeds the LCD (waveform, 7-segment digits,
bars); its jog ring position is byte 25, **1–135**, with 136 = fill the
circle, 137 = load animation, 138 = eject animation, and a cue cursor 1–85 in
package 7 byte 21. Worth trying on the XDJ's byte 9 once the panel can be
driven.

The original CDJ-1000 (service manual RRV2468, 2001) already has the same
shape. Its "Display Control CPU" IC1101 (PE5243A, on MFLB) is clocked by the
main "System Control CPU" IC201 (PD3431A9): IC1101 pin 17 P27/SCK0 = SCLK1
"clock input from host CPU", 18 SDO1 data out, 19 SDI1 data in, **20 P24/BUSY
= ENABLE "enable output to host CPU"** (a hardware busy pin of its serial
port), read by the main CPU's pin 55 KEYI "key processing demand from front
CPU"; the main CPU's XSRST resets peripheral ICs and the manual shows the
display CPU's /RST (pin 10). The display CPU reads the keys on five analog
ladders (KEYAD1-5) and drives the jog FL itself (SCLK2, SDO2, BLK to the
µPD16306B on JFLB), as the XDJ's panel CPU drives its jog VFD. Frame format
not documented; a capture would tell how close it is to the MK3's.

## In OpenDeck

`crates/app/src/surface/xdjpanel.rs` decodes panel frames into the same
events as the CDJ-3000's `subucom` (keys by their cdj3k-emu names, jog,
fader, rotary, LCD touch), with two transports:

- **`OPENDECK_XDJPANEL=sniff`**: listens to a stock unit through the fx2
  analyser (`sigrok-cli` streaming 4 MHz to the app), so the deck's own
  controls drive OpenDeck while its main board keeps the panel and the lamps.
- **`OPENDECK_XDJPANEL=fx2`**: OpenDeck is the panel's master through the
  analyser running `bridge.c` (embedded in the app, `crates/app/firmware/`,
  loaded with the 0xA0 request at start). Set up as in *Driving the panel*;
  start OpenDeck with the deck off, then switch the deck on.
  `OPENDECK_XDJPANEL_LOAD=0` reconnects to a bridge already running (restart
  OpenDeck without power-cycling the deck). The deck's controls drive OpenDeck
  and OpenDeck lights the face from its own state ([`mosi_frame`]): PLAY,
  QUANTIZE, MASTER TEMPO, VINYL (key and centre mark), the playhead ring
  (byte 9) and the red cue mark (byte 7), on top of the stock idle frame; no
  other bit is touched. Checked on the unit 2026-11-21.
- **`OPENDECK_XDJPANEL_POKE=<file>`** (mapping aid): lines `7 = 40`,
  `8 |= 0x20`, `1 &= 0xfd` override bits of the frame sent, re-read whenever
  the file changes; how the cue mark and the standby byte were found.

The transport is a `Link` (send 30 bytes, get the panel's reply); a Pi's
spidev + GPIO is the next one (*Route A*). Calibrated on the unit: keys,
rotary, fader (0x0000 at the top), jog (~3272 counts per turn, clockwise
counts up), jog touch (11 = touched). REV has no action: OpenDeck has no
reverse play.

## Driving the panel (master side): it works

2026-11-21: the fx2 analyser, running our own firmware, is the panel's only
master on the stock deck. It sets every lamp and the jog ring and reads every
control, from a browser page (`tools/panel-bus/fx2/bridge.py`). What it takes:

### Setup

| Line | What we do | Why |
|---|---|---|
| SCK, RXD, BUSY (and a GND) | **masked at the LCDB end of the CN8002 ribbon: CN2602 contacts 1–4** (1 GND, 2 SCK, 3 BUSY, 4 RXD), Kapton tape on the ribbon's contacts | keeps the main board off the bus. Count by CN2602's own labels: CN2602 17–20 is the panel's 4–1, the 5 V lamp supply (taping those darkens the lamps and leaves SCK connected) |
| SCK (CN8002 leg 19) | analyser D0, push-pull, **driven high from the moment the panel powers up** | the main board holds it high; a floating clock input while the panel starts leaves it never answering |
| RXD (leg 17) | D1, same | as SCK |
| TXD (leg 15) | D2, input | the replies |
| BUSY (leg 18) | D3, **pull-down faked in firmware** | see below |
| PNL_RST (leg 13) | D4, **held low, never pulsed** | the main board holds the panel in reset when it cannot talk to it (it read high 100 % once SCK/BUSY were cut); holding it low through R18+R1176 (242 Ω) is a ~14 mA fight at worst. A pulse resets the panel, which glitches PWR_SW and the deck reboots |
| GND (leg 20) | analyser GND | |
| everything else | connected as stock | power, PWR_SW, EUP_CON, CNVSS, lamp lines |

The tap wires are tacked onto CN8002's solder legs on PNLB, which is the one
departure from "nothing soldered on the original boards".

### Bring-up, step by step (as `bridge.c` does it)

Every step below is there because leaving it out failed on the unit.

1. **Deck switched off.** The analyser freshly plugged in (its default USB
   descriptors are needed for the bridge; after a sigrok run, replug).
2. **Load the firmware** with the FX2's own 0xA0 request: CPUCS = 1 (CPU held
   in reset), write the image, CPUCS = 0 (`load.py`, or `bridge.py`).
3. **Firmware start-up, before the deck has power:** CPU to 48 MHz, port B as
   plain GPIO, **every pin an input** (nothing driven into an unpowered
   board). Port latches preset: SCK = 1, RXD = 1, RST = 0, BUSY = 0.
4. **If TXD (D2) already reads high** the deck is on: watch SCK for ~200 ms;
   any edge means a main board can still clock the panel, and the firmware
   stops without driving anything (the interlock). Otherwise:
5. **Wait for TXD to go high** = the panel CPU has power. Switch the deck on.
6. **At once:** drive **SCK high, RXD high, PNL_RST low** (OEB = 0x13). RST
   stays held low for the whole session, never pulsed. Do not wait or watch
   first: SCK/RXD floating during the panel's start-up leaves it deaf.
7. **Wait ~0.6 s.** The panel is ready by ~0.4 s (stock boot: BUSY falls at
   +395 ms). The main board, booting meanwhile, cannot reach SCK/RXD/BUSY,
   and its PNL_RST (which it would drive high, the panel being silent to it)
   loses to ours.
8. **Then, every frame:**
   1. take the latest 30 bytes from the Mac (EP1 OUT), if any; byte 30 = the
      ones'-complement sum of 0–29, byte 31 = 0x8F;
   2. **ready check, faked pull-down:** hold BUSY low, let go, read it ~0.2 µs
      later: high = the panel drives it (busy), let go and look again ~50 µs
      on; low = released (ready), hold it low;
   3. **wait 28 µs** (the stock gap from BUSY falling to the first clock);
   4. **clock 256 bits, BUSY held low:** SPI mode 3 (SCK idles high, data set
      while SCK is low, sampled on the rising edge), **LSB first**, ~857 kHz
      (7 CPU cycles low, 7 high at 48 MHz; hand-timed 8051 assembly, one
      rotate through the carry sends a bit and keeps the one received);
   5. **let go of BUSY** (the panel drives it high ~2.4 µs after the last
      clock);
   6. **wait ~3 ms** (the stock period); the next frame must not start while
      the panel is busy;
   7. check the reply (byte 0 = 0x81, byte 31 = 0x8F, the sum) and hand it to
      the Mac (EP1 IN) if it is valid and the Mac has taken the last one.
9. **Afterwards:** a main-board reboot or standby does not disturb the panel
   (it stays ours). If the analyser is unplugged or reloaded mid-session the
   panel goes deaf again: switch the deck off and start from step 1.

Line states while running:

| Line | Level | Driven by |
|---|---|---|
| SCK | idles high, clocks during frames | analyser |
| RXD | idles high, data during frames | analyser |
| BUSY | panel drives high while busy; held low by the analyser when released (ready) and through each frame | panel / analyser |
| PNL_RST | low | analyser (the main board's drive loses through 242 Ω) |
| PNL_CNVSS | low | main board (untouched) |
| PWR_SW, EUP_CON | as stock | panel → main board (untouched) |

### How we got there

In order, each with what it taught:

1. **Listen first** (stock deck, analyser on CN8002's legs): the frame
   format, timing and every input field. Nothing driven.
2. **Boot window, main board connected** (`bootlog.c`; drive only between the
   panel's ready and 4 s, before the main board first clocks at ~7 s): the
   panel's first reply was valid, then it stopped. Same at every clock speed
   and drive (push-pull/open-drain, ~300 kHz and ~857 kHz). A firmware bug in
   the first version counted frames instead of time and kept driving past the
   main board's start for ~20 s: harmless, then fixed with a hardware timer.
3. **Write into a running deck** (`runwrite.c`): two masters scramble each
   other. Learned that the main board drives SCK/RXD through 220 Ω, so
   overlap is ~15 mA, not a short.
4. **Mask the main board off the bus.** First masked CN2602 17–20 by the LCDB
   connector's labels: that is the panel's 1–4, the 5 V lamp supply; the
   lamps went dark and SCK stayed connected, which the firmware's interlock
   caught (2400+ SCK edges in 200 ms, nothing driven). Masking CN2602 1–4
   (GND, SCK, BUSY, RXD) cut the main board off.
5. **Panel silent, BUSY high.** RST read high: the main board holds the panel
   in reset once it cannot talk to it. Added D4 on PNL_RST, held low.
6. **Pulsing RST to restart the panel** (as the main board appears to before
   its first frame): the panel restarted, but the whole deck rebooted with it
   (the USB LED's colour cycle is the deck's boot). Never pulse it.
7. **Still silent from a cold start**, until a capture with nothing driven
   showed BUSY flat high from before power-on: the pull-down is on the main
   board's side. Timing the rise after a forced low showed a weak pull-up, not
   a driver.
8. **Faked pull-down + SCK/RXD held high from power-up** (`master_pd2.c`): the
   first valid replies under our control, the lamps and the jog ring followed
   the frames.
9. **The bridge** (`bridge.c` = `master_pd2.c` + EP1 to the Mac): first
   versions answered `ff` because an interlock wait left SCK/RXD floating at
   power-up, and because frames went out faster than the panel's ~3 ms cycle.
   Both fixed; replies valid.
10. **"All on"** (bytes 2–29 = 0xFF) put the deck into standby (see below).

### BUSY needs a pull-down

With nothing driving the line and the main board's side cut off, BUSY reads
high from before power-on and never moves (capture `tapeboot`, 30 s from
power-on: TXD active, BUSY flat). On the stock deck it reads low with the deck
off and drops at ~0.4 s. Holding it low, letting go and timing the rise shows
the panel's side: still low after 1 µs, high by 10 µs, a weak pull-up, not a
driver. So the panel drives BUSY **high** while busy and **releases** it when
ready, and the low is the main board's pull-down (on its side of R1173). A
master without the main board must supply it: **10 kΩ from leg 18 to GND**,
or, as `bridge.c` does, fake it: hold BUSY low, and to read it let go for
~0.2 µs (a panel driving high snaps the line back at once; a released line is
still low). Two further timing rules came out of it:

- **Start from power-on.** The panel answers a master that has held SCK and
  RXD high and served it since it powered up. Taking over a panel that has
  sat unclocked (after a deck boot with the bus cut, or after replugging the
  analyser) gets only `ff`; switch the deck off and on with the firmware
  already waiting (it watches TXD go high).
- **Pace frames at ~3 ms**, the stock period. The faked-pull-down busy check
  does not reliably hold back the next frame, and frames sent while the panel
  is still busy come back `ff`, make the lamps flicker and move the ring.

With those, every reply is a valid frame (`81 … 8f`, sum right) and the
panel follows what we send: the lamps, the jog ring position (byte 9) and the
jog display's centre.

### What we have seen it do

- The main board's start-up frame (`73 18 00 00 00 00 00 33 40 3f 28 10 01 …
  20 …`) lights LOOP IN, LOOP OUT, the rotary ring and most panel lamps, but
  not VINYL, and only some of the jog display's red centre segments: other
  bits light the rest. USB may be wired from the main board (CN8002 8–10).
- With the main board cut off and nobody clocking, the panel lights its lamps
  solid on its own, and shows a short TXD pulse every ~0.75 ms.
- **Byte 11 = 0xFF puts the panel into standby** ("All on" in the page): the
  lamps and the jog display go dark and the controls are scanned slowly (long
  latency); setting byte 11 back to the stock 0x10 does not wake it, a power
  cycle does. The bit within byte 11 is not narrowed yet.
- The main board rebooting does not disturb the panel or the bridge: with the
  bus cut, the panel stays ours.

### What did not work, and why (for the record)

- **The power-on window with the main board connected** (`bootlog.c`): the
  panel answered one frame, then stopped (BUSY never rose, TXD idle). Not
  ringing on SCK, as first thought: the same happened at every clock speed and
  drive. The likely cause is the BUSY pull-down above being shared with the
  main board's pin and the panel's state in the window; superseded.
- **Writing into a running deck** (`runwrite.c`: every 4th frame for 2 s,
  starting the instant BUSY falls, ~27 µs ahead of the main board): two
  masters on one bus. Each reply starts right (`81 00 00 00`) and is scrambled
  after; nothing visible changes (the main board's next frame rewrites every
  lamp 3 ms later). The deck was unharmed by the ~50 ms of overlap.
- **Open-drain SCK on the deck's pull-up**: fine at ~300 kHz, but at the stock
  ~842 kHz the pull-up cannot raise SCK in the 0.6 µs high half.
- **Pulsing PNL_RST** to restart the panel: reboots the whole deck.

### FX2 notes

- This analyser ("Lonely Binary" 24 MHz 8CH, CY7C68013A) has no input buffer:
  D0–D7 are PB0–PB7 and can drive (checked with `blink.c`).
- A bit write to a port reads the *pins* back (read-modify-write): an
  open-drain pin's latch must be re-cleared after writing a neighbour.
- The 0xA0 RAM upload returns junk while the CPU runs; halt it (CPUCS = 1)
  first (`readlog.py`).
- After a fresh plug-in the FX2 enumerates with its built-in descriptors;
  alternate setting 1 has a bulk EP1 OUT/IN pair, so firmware can talk to the
  Mac with no USB code (`bridge.c`). After sigrok has run, replug first.

### The map page

`tools/panel-bus/fx2/bridge.py` (pyusb + Homebrew libusb): with the deck off,
it loads `bridge.ihx`, which waits for the panel to power up; switch the deck
on and open http://127.0.0.1:8765. Left: every bit of the main → panel frame
as a toggle (click a byte number for all 8), the ring slider (0–138; 136–138
are the CDJ-1000MK3's fill/load/eject animations, untried here), presets.
Right: the panel's reply decoded live (keys, jog, fader, rotary, touch) and
raw. The firmware sends the page's frame every ~3 ms and hands back each
valid reply. `bridge.py --no-load` restarts the page while the firmware runs.

## Route A: speak to the panel CPU (recommended)

Unplug the two FFCs between LCDB and PNLB and plug in an interposer board
that supplies power, ties CNVSS low, drives RST, and brings SCK/RXD/TXD/BUSY to
the Pi's SPI0 + 2 GPIO. The PNLB keeps its own firmware and keeps scanning
everything; OpenDeck gets buttons, encoder, jog, fader, touch and drives every
LED and the jog VFD through one connector. The CDJ-3000 adapter planned in
`cdj-3000-target.md` ("opendeck-subucom") is the same shape of code.

The frame format and the master's rules are now known (above): a Pi or the
RP2350 needs SPI mode 3 with the bytes bit-reversed (LSB first), a **10 kΩ
pull-down on BUSY** (the main board's, which it no longer has), CNVSS low,
PNL_RST low, SCK/RXD driven from power-up, and frames every ~3 ms.

Then: `crates/app/src/surface/xdjpanel.rs` (today the listening backend)
gets a master mode owning `/dev/spidev0.0` and the GPIOs, sending OpenDeck's
lamps and ring every ~3 ms and decoding the replies it already understands.

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

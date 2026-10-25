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

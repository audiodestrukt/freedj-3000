# XDJ-1000MK2 panel link: the logic analyser setup

How the XDJ-1000MK2's panel link was tapped, listened to and then driven with
an 8-channel fx2 logic analyser, as done on our unit in November 2026. The
protocol itself (frame format, field map, BUSY rules, the main board's
behaviour) is in [docs/reference/xdj-1000mk2-panel-bus.md](../reference/xdj-1000mk2-panel-bus.md);
this page is the bench setup: hardware, wiring, software, the procedures and
what goes wrong.

The same analyser does three jobs:

| Mode | What it does | Deck state |
|---|---|---|
| **Listen** | records the stock main board ↔ panel traffic (`capture.sh`, `bits.py`), or feeds it live to OpenDeck (`OPENDECK_XDJPANEL=sniff`) | stock, nothing masked |
| **Drive** | runs our firmware and is the panel's only master: the map page (`bridge.py`) or OpenDeck itself (`OPENDECK_XDJPANEL=fx2`) | main board masked off SCK, RXD, BUSY |
| **Diagnose** | test firmware that logs to the analyser's RAM (`bootlog.c`, `readlog.py`) | either |

## Hardware

| Item | Notes |
|---|---|
| Logic analyser | "Lonely Binary" USB logic analyser, 24 MHz, 8 channels, in a plastic case: a Cypress **CY7C68013A** (FX2LP) Saleae-Logic clone. USB ID **0925:3881** (`ioreg` prints it in decimal as 2341:14465). **No input buffer**: D0–D7 are the FX2's port B pins, so they can also drive outputs, which is what makes Drive mode possible. Firmware is loaded into its RAM on every plug-in (sigrok's `fx2lafw`, or ours) |
| Wire | thin (about 30 AWG) wire tacked onto the panel board's connector legs, 10–15 cm to the analyser's clips, one good ground |
| Multimeter | continuity and voltage; every wiring step below has a meter check |
| Kapton tape | thin strips (~0.8 mm) to mask ribbon contacts for Drive mode |
| Service manual | Pioneer RRV4661 (not in the repo): PNLB sheet §10.14, LCDB §10.10–10.11, MAIN §10.2/§10.8, wiring §10.1, PCB layout p. 29 |

Not needed: a breakout or passthrough board. The first plan used one and it
cost a dead-deck scare (below, *Ribbon numbering*); the taps went onto the
connector's own legs instead.

## Where to tap

The link runs over the 20-way 1.0 mm FFC between the LCD board (LCDB, CN2602)
and the panel board (PNLB, **CN8002**). The analyser wires are tacked onto
**CN8002's solder legs on PNLB**, with the stock ribbon left plugged in, so
the deck works normally while being probed.

### Ribbon numbering: read this first

The stock ribbon (DDD1787) is **reverse face**: CN8002 pin *n* is CN2602
pin **21 − *n***. PNL_SCK is CN8002 19 but CN2602 **2**; GND is CN8002 20 but
CN2602 1; the 5 V lamp supply V+5R2 is CN8002 1/3 but CN2602 20/18.

- Always count by **the connector you are at**: PNLB's silkscreen prints "1"
  and "20" at the two ends of CN8002, LCDB's prints its own.
- Confirm with the meter before anything is clipped: the GND contact beeps to
  chassis, V+5R2 reads 5 V with the deck on.
- What going wrong looked like: a breakout numbered from the other end put the
  analyser's GND on 5 V (the deck would not power on: PWR_SW, pin 12, rides on
  this ribbon too); masking "CN2602 17–20" by mistake for the panel's 17–20
  cut the lamp supply instead of the clock.

### CN8002 (on PNLB)

| Leg | Signal | Leg | Signal |
|---|---|---|---|
| 1 | V+5R2 | 11 | GND |
| 2 | GND_LED | 12 | PWR_SW |
| 3 | V+5R2 | 13 | **PNL_RST** |
| 4 | GND_LED | 14 | PNL_CNVSS (keep away) |
| 5 | GND_LED | 15 | **PNL_TXD** (panel → main) |
| 6 | QTZ_ILM | 16 | EUP_CON |
| 7 | EUPL | 17 | **PNL_RXD** (main → panel) |
| 8 | USB_ILM_G | 18 | **PNL_BUSY** |
| 9 | USB_ILM_R | 19 | **PNL_SCK** |
| 10 | USB_ILM_B | 20 | **GND** |

Neighbours that matter when tacking wires at 1.0 mm pitch: **14 (CNVSS) next
to 13 and 15** (a bridge that pulls CNVSS high starts the panel CPU in its
flash-programming mode), **19 next to 20** (would short the main board's
clock to ground). Check every pair of adjacent legs with the meter after
soldering.

Alternatives if the legs are awkward: the series resistors between CN8002 and
the panel CPU on PNLB, PCB layout p. 29 just below IC8003: **R8071** SCK
(22 Ω), **R8102** RXD (22 Ω), **R8072** TXD (220 Ω), **R8070** BUSY (100 Ω).

## Wiring

| Analyser | CN8002 leg | Signal | Listen | Drive |
|---|---|---|---|---|
| **D0** | 19 | PNL_SCK | input | output: clock, idles high |
| **D1** | 17 | PNL_RXD | input | output: data to the panel, idles high |
| **D2** | 15 | PNL_TXD | input | input: the panel's replies |
| **D3** | 18 | PNL_BUSY | input | input, with the main board's pull-down faked in firmware |
| **D4** | 13 | PNL_RST | input (optional) | output: **held low, never pulsed** |
| D5 | (14, CNVSS) | | optional, not needed | not wired |
| **GND** | 20 | GND | | |

`capture.sh` names the channels D0=SCK D1=RXD D2=TXD D3=BUSY D4=RST
D5=CNVSS; everything else here follows the same map.

Wire **GND first** when connecting and last when disconnecting, keep the
leads short (10–15 cm; clock edges are fast even though the clock is
842 kHz) and keep the ground lead close to the signal leads.

## Software (macOS; Linux in brackets)

```sh
brew install sigrok-cli sdcc            # [apt install sigrok-cli sigrok-firmware-fx2lafw sdcc]
python3 -m venv .venv && .venv/bin/pip install pyusb numpy
```

- `sigrok-cli --scan` must list `fx2lafw - Saleae Logic`. [Linux: the
  package's udev rule, then replug.]
- pyusb needs libusb: Homebrew's `/opt/homebrew/lib/libusb-1.0.dylib` (the
  fx2 scripts point at it).
- OpenDeck's `fx2` transport uses the `rusb` crate (libusb), built only for
  macOS and Linux.
- Firmware: `cd tools/panel-bus/fx2 && make` (sdcc, 8051 C).

## Listen mode (stock deck)

1. Deck as stock, nothing masked. Tap wires as in *Wiring*, GND first.
2. **Plug in the analyser.** The first capture after a plug-in uploads
   sigrok's firmware and then fails ("Device has been disconnected"); run it
   again. `capture.sh` retries are cheap.
3. Record:

   ```sh
   tools/panel-bus/capture.sh idle 5          # 5 s at 16 MHz
   tools/panel-bus/capture.sh buttons 30 4M   # longer runs at 4 MHz
   ```

   **Keep `.sr` captures to about 30 s at 4 MHz**: sigrok's file writer stalls
   around 35 s on this clone and holds the device (the script's watchdog then
   kills it). Streaming to a pipe does not stall (90 s checked).
4. Decode:
   - `tools/panel-bus/decode.py capture.sr` (survey + frames; `--lsb`, rising
     edge is the right setting);
   - `tools/panel-bus/bits.py capture.sr` (every bit that changed, as
     intervals in time order, checksums verified: press controls in a known
     order and read them off in that order).
5. Live into OpenDeck: `OPENDECK_XDJPANEL=sniff make chrome TRACK=...`
   (OpenDeck runs `sigrok-cli` itself and decodes the stream).

Tip from the sessions: instructions shown while a capture runs in the
foreground reach the person at the deck late. Start captures in the
background, let them act, and decode afterwards.

## Drive mode (the analyser as the panel's master)

### Preparing the deck

1. **Mask the main board off the bus**: at the **LCDB end** of the CN8002
   ribbon, Kapton over **CN2602 contacts 1–4** by CN2602's labels (1 GND,
   2 SCK, 3 BUSY, 4 RXD). Leave 5 (EUP_CON) and up alone. [Equivalent at the
   PNLB end: CN8002 17–20.]
2. Check, deck unplugged: from the ribbon's CN2602-end contacts 2 and 4 to
   CN8002 legs 19 and 17, **open**; leg 20 still beeps to chassis (through
   pin 11 and CN8001's grounds); CN2602 5 and up still connect.
3. D4 on CN8002 leg 13 (PNL_RST), checked against 12 and 14.

### Running

Order matters: the panel only answers a master that has held SCK and RXD high
and served it **from its power-up**.

1. **Deck off.**
2. **Analyser freshly plugged in** (our firmware needs its default USB
   descriptors; after sigrok has run, replug).
3. Start the master:
   - the map page: `cd tools/panel-bus/fx2 && make bridge.ihx && python3 bridge.py`,
     then open http://127.0.0.1:8765;
   - or OpenDeck: `OPENDECK_XDJPANEL=fx2 make chrome TRACK=...`.

   Either loads `bridge.ihx` into the analyser, which then waits for the
   panel's power (it watches TXD go high).
4. **Switch the deck on.** The firmware drives SCK/RXD high and PNL_RST low at
   once, waits ~0.6 s, then exchanges a frame every ~3 ms.
5. To restart the host side without losing the panel: `bridge.py --no-load`,
   or `OPENDECK_XDJPANEL_LOAD=0` for OpenDeck (the firmware keeps running).

`docs/reference/xdj-1000mk2-panel-bus.md`, *Driving the panel*, has the
bring-up step by step with the reasons and the line states.

The XDJ's own LCD touch panel is decoded but not used: OpenDeck draws on a
screen of its own, and the XDJ's touch readings would land on it as stray
presses. `OPENDECK_XDJPANEL_TOUCH=1` passes them on as the pointer.

### On a Raspberry Pi

The analyser plugs into the Pi's USB and OpenDeck runs as the Pi's boot
service (`deploy/pi/`, `opendeck.service` under cage), drawing on the Pi's
own screen; the deck's controls and lamps go through the analyser exactly as
on a desktop. Tested on a Pi 5 (Debian 13).

1. **Let OpenDeck open the analyser without root:**
   `sudo cp deploy/pi/xdjpanel/60-xdj-fx2-analyser.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger`,
   then replug it. The user OpenDeck runs as must be in `plugdev`.
2. **Build** (natively on the Pi: about 13 min the first time, about 5 min
   after a change to the app crate; needs `libusb-1.0-0-dev` besides the
   usual packages), or cross-build with `deploy/pi/cross/build.sh`, whose
   image has libusb:
   `cargo build --release -p opendeck-app --features renderer-sdl`.
3. **Install** the stripped binary where `OPENDECK_BIN` in
   `/etc/default/opendeck` points, and add `OPENDECK_XDJPANEL=fx2` to that
   file.
4. **Deck off, then `sudo systemctl restart opendeck`, then deck on.**
5. To restart OpenDeck without losing the panel:
   `sudo systemctl set-environment OPENDECK_XDJPANEL_LOAD=0 && sudo systemctl restart opendeck && sudo systemctl unset-environment OPENDECK_XDJPANEL_LOAD`.

Under cage, OpenDeck's log does not reach the journal; to see the
`xdjpanel:` lines, stop the service and run the binary by hand from a
console. The analyser draws little power, but a Pi 5 on a 3 A supply (most
USB-C chargers, however many watts they are rated) caps its USB ports at
600 mA in total.

### Mapping outputs

- The map page: every output bit as a toggle (click a byte number for all
  8), the ring slider, presets; inputs decoded live on the right.
- With OpenDeck running: `OPENDECK_XDJPANEL_POKE=<file>`, and lines such as
  `7 = 40` or `8 |= 0x20` in the file override bits of the frame OpenDeck
  sends, live.
- **Position fields show nothing at 0xFF** (out of range): to find one, set
  the untried bytes to a valid position (e.g. 40) and halve the set.
- **Never set byte 11 to 0xFF** (or "All on"): the panel goes into standby
  and only a power cycle brings it back.

## Diagnose mode

- `bootlog.c` / `master*.c` log to the analyser's RAM (`0x2000…`).
  `readlog.py` reads it: it **halts the CPU first**, because the FX2's 0xA0
  memory reads return junk while the CPU runs. Reload the firmware to run
  again.
- `tools/panel-bus/fx2/README.md` lists every firmware and what each taught.

## Switching modes

| From → to | Do |
|---|---|
| Listen → Drive | tape on (CN2602 1–4), D4 wired, deck off, **replug the analyser**, start the master, deck on |
| Drive → Listen | tape off, **replug the analyser** (sigrok reloads its own firmware), deck on as normal |
| map page ↔ OpenDeck | stop one, start the other with `--no-load` / `OPENDECK_XDJPANEL_LOAD=0`: no power cycle |
| desktop ↔ Pi | move the analyser's USB, deck off, start the master on the other host, deck on |

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `No devices found` right after `--scan` listed it | first capture after a plug-in, firmware upload re-enumerating | run it again |
| capture stops by itself at ~35 s, later captures say "Unable to open device" | sigrok's `.sr` writer stalled and still holds the analyser | keep captures ≤ 30 s at 4 MHz; kill the stuck `sigrok-cli` (the script's watchdog does) |
| every channel reads constant high | nothing connected: this clone's inputs float high | check the tap wires and GND |
| deck will not power on with the analyser/breakout in | ribbon numbering reversed: GND on V+5R2, PWR_SW broken | count by the connector you are at; meter check |
| lamps dark after masking, SCK still active | masked CN2602 17–20 (the panel's 1–4, lamp supply) instead of 1–4 | move the tape to CN2602 1–4 |
| master refuses to start ("SCK edges" in the log) | its interlock saw the main board still clocking SCK | the tape is not cutting SCK; fix before driving anything |
| every reply `ff`, BUSY constant high | the panel was not served from power-up, or SCK/RXD floated while it started, or the BUSY pull-down is missing | deck off, start the master, deck on; `bridge.c` handles the rest |
| PNL_RST reads high 100 % | a loose D4 (floats high), or the main board holding the panel in reset | check D4; drive it low |
| the whole deck reboots (USB LED colour cycle) | PNL_RST was pulsed: a panel reset glitches PWR_SW | never pulse it |
| lamps off, controls work with long lag | byte 11 set: panel standby | power cycle (deck off, restart the master, deck on) |
| "reply not valid" flickering on the map page | frames sent faster than the panel's ~3 ms cycle (old firmware) | current `bridge.c` paces at 3 ms and passes only valid replies |
| an occasional wrong button | a key ladder (bytes 18–20) passing through a neighbour's code | expected; OpenDeck's decoder waits for a stable reading |
| USB "Input/Output Error" when loading or reading | the analyser's USB state went bad (after a glitch) | replug the analyser |
| USB STOP never shows | it is wired to the main board, not the panel | nothing to fix |

## Safety

- Every tap and every tape strip gets a meter check before power.
- GND first on, last off.
- With the main board connected, the analyser only listens (Listen mode). The
  main board drives SCK/RXD through 220 Ω and PNL_RST through 242 Ω, so an
  accidental overlap is ~15 mA, not a short, but two masters on one bus never
  work anyway.
- Keep CNVSS (leg 14) low and untouched.
- Do not pulse PNL_RST; do not set byte 11.

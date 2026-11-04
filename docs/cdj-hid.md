# CDJ-3000 HID control mode, measured from the emulator

What a stock CDJ-3000 (firmware 3.22, EP122) puts on its rear USB-B port in
CONTROL MODE, observed through the cdj3k emulator's PC Link socket on
2026-11-03. Everything here is black-box observation of the USB traffic;
nothing was read from the firmware. Raw frame logs stay in the scratchpad,
not in this repo.

Public reference for the protocol: swiftb0y.github.io/CDJHidProtocol
(`control.html` for the input report, `handshake.html` for the handshake).

## Recipe

1. Emulator slot booted. Turn on **Developer Mode** (Linux: hamburger menu
   in the strip at the top of the window → Emulation → Developer Mode).
   The control API comes up on TCP 4400+slot at once, but the guest shell
   (`devd`) only loads at the next start of the emulation, so:
   `cdj3k-emu ctl --slot 1 restart`, then `ctl --slot 1 wait boot 300000`.
2. Start the guest bridge:
   `ctl --slot 1 sh 'systemctl start cdj3k-pc-link-bridge'`. Its journal
   should end with "forwarding started", "sent the USB-B connect event" and
   "forced PC mode on EP122".
3. `tools/cdj_hidtap.py --decode --log <scratch>/session.jsonl` connects to
   `/tmp/cdj3k-emu-<uid>/instance-1/usb-link.sock`, sends HELLO, prints
   IDENTITY and then every frame.
4. Drive the deck: `ctl --slot 1 press PLAY 300`, `jog 90 1000`,
   `jog 360 2000 side`, `tempo 0.75`, `rotary 3`. `ctl --slot 1 buttons`
   lists the names.

## Gadget identity

VID 0x2B73, PID 0x002F, bcdDevice 0x0322 (= firmware 3.22), manufacturer
"Pioneer DJ", product "CDJ-3000". The HID report descriptor is a vendor
page (0xFFA0) collection with one 64-byte input and one 64-byte output
report, no report IDs. A USB MIDI interface sits beside it.

## What the deck sends without a host handshake

With PC mode forced by the emulator's bridge and no host reply:

- **HID:** once a second, an all-zero 64-byte report (the documented reset
  packet) followed by handshake stage 1. Nothing else. Not one type-0x20
  input report, whatever is pressed or turned.
- **MIDI:** every control, immediately. The MIDI interface is live without
  the handshake.

Stage 1 decoded, as observed:

| bytes | meaning |
|---|---|
| `00` | deck 0 |
| `f0` | extended header |
| `01 00` | fragment index 1 (LE) |
| `01 00` | fragment count 1 (LE) |
| `11 06` | super-TLV: stage 0x11, length 6 incl. header |
| `01 04 03 16` | TLV 0x01: firmware version, major 3 minor 22 |
| rest | zero padding to 64 |

So the deck is waiting for the host's stage-2 challenge. Per the public
analysis the input reports start only after stage 5, and the host side of
stages 2 and 4 needs a vendor secret and a vendor device ID. Whether to
implement that host side is a project decision, not a technical one; see
"Decision" below.

## MIDI map observed (CDJ-3000, control mode)

Notes on channel 1, velocity 0x7f press / 0x00 release:

| control | note |
|---|---|
| PLAY | 0x00 |
| CUE | 0x01 |
| QUANTIZE | 0x06 |
| SLIP | 0x07 |
| TEMPO RANGE | 0x0a |
| MASTER TEMPO | 0x0b |
| MASTER | 0x15 |
| jog top-plate touch | 0x17 |
| LOOP IN | 0x23 |
| LOOP OUT | 0x24 |
| RELOOP | 0x25 |

Control changes on channel 1:

| control | CC | value |
|---|---|---|
| jog, top plate | 0x10 | 0x40 = still, 0x41.. forward, ..0x3f backward; speed ratio, 7-bit |
| jog, side (pitch bend) | 0x30 | same encoding |
| tempo slider | 0x1d | 0x00..0x7f, 0x40 = centre |

A steady turn repeats the CC about every 150 ms. A 360° side turn over 2 s
reads 0x45 (+5), a 90° top turn over 1 s climbed 0x44..0x48 as the turn
accelerated. BACK and the rotary selector produced no MIDI from the idle
screen (not chased). This matches the XDJ-1000MK2's documented control-mode
MIDI table, so one `[[surface]]` MIDI preset should serve both units.

## Decision

The HID input report (jog position to ~9728 steps per turn, 1000-step tempo
fader, needle search, every button) is reachable only behind the handshake.
The emulator cannot teach it to us: its bridge skips the handshake by forcing
the PC-mode flag, and the deck then sits at stage 1 forever. Two routes:

- **MIDI now.** 7-bit jog speed and tempo, no handshake, works on the
  emulator today and on the XDJ-1000MK2 over a USB cable. The map above is
  the preset.
- **HID behind the handshake.** Implement stages 2 and 4 as the public
  analysis describes, posing as one of the vendors it lists, with that
  vendor's published secret and CDJ-3000 device ID. That is pretending to be
  licensed software; the repo has kept out of that so far. If taken, the
  implementation is small (FNV-1a, two packets, a keepalive every 400 ms)
  and the emulator will then show the 0x20 stream and the real report rate.

Not yet known either way: the type-0x21 output report (LEDs, time, BPM) and
the actual input report rate; both need the handshake first.

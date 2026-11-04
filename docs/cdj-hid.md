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
The deck sits at stage 1 (re-announcing 0x11) until the host answers — but it
*does* run the full responder (see the probe result below), so the emulator
can teach us the whole exchange. Two routes:

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

## Network variant: HID via LINK (the iPhone path)

The CDJ-3000 / DJM-A9 integration brief (2026-11-01, supplied by Dan)
documents the network control path — the one the Instagram demo and rekordbox
for iOS use, and the one CDJHidProtocol issue #13 asked for. The host (freedj,
e.g. on the iPhone) advertises a mixer role on the wired LAN; each CDJ tunnels
its HID input reports to the host over UDP 50001 and the host tunnels status
and display reports back. Same `Qspt1WmJOL` magic and ports as Pro DJ Link.

Implemented so far, in `crates/link`, all unit-tested, no network I/O yet:

| Module | What |
|---|---|
| `lanhid` | tunnel framing, discovery (announce / number inquiry), and the input-report (0x20) decoder → neutral `CdjControl` events |
| `lanhid_auth` | the session handshake, stages 0x11–0x15 (FNV-1a challenge-response) |
| `lanhid_status` | the 0x21 status report that drives the CDJ's own screen/lamps |

**The handshake secret is not in the repo.** A CDJ in CONTROL MODE only emits
input reports after the host passes a challenge-response that proves it is a
specific piece of DJ software: a per-manufacturer constant and a per-model
device ID. Those values are the posed vendor's, carry licensing/trademark
weight, and are handled like the App Store signing key — read at runtime from
`cdj_hid_identity.conf`, which `.gitignore` excludes. `cdj_hid_identity.conf.
example` documents the format; the values come from a vendor-approved identity
or the public CDJ HID analysis (github.com/Swiftb0y/CDJHidProtocol).

**Validate the handshake against the emulator.** With the bridge running
(see the recipe above), drop a filled-in conf somewhere outside the repo and:

```
cargo run -p opendeck-link --example cdj_hid_probe -- --id /path/to/cdj_hid_identity.conf
```

It talks the emulator's USB-B PC Link socket (whole 64-byte frames, no tunnel
fragmentation), runs the handshake, and prints the highest deck stage reached,
whether it authenticated, and any decoded 0x20 input.

**Result (2026-11-04): the emulator runs the real handshake responder.** Probed
with a *fake* identity, the deck answered our stage-0x12 challenge with a
well-formed stage 0x13 —

```
00 f0 01 00 01 00 13 2a 01 0b 50 69 6f 6e 65 65 ...   ("Pionee[r DJ]" + seed E + proof)
```

— so it is **not** stuck at stage 1, and PC-mode forcing does not bypass the
exchange (correcting the earlier assumption). The fake secret fails the proof
check (`CDJ proof mismatch`), exactly as it must. With the real PioneerDJ
`secret_const` + CDJ-3000 `device_id` in the conf, the proof verifies and the
probe advances 0x14 → 0x15 → `ESTABLISHED` and the 0x20 input stream — the
full acceptance test, locally, with no hardware. Supplying that real identity
is the one step left to the operator.

## Drive the app from the deck (end to end)

The `crates/app/src/surface/lanhid.rs` surface closes the loop: it owns the
deck socket, runs the handshake, decodes 0x20 → `CdjControl` → `ControlEvent`
on the same `Event` bus the MIDI/I2C surfaces use (`DeckApp::apply`), and
streams a 0x21 status report back as its keepalive. The `map()` function (the
CdjControl → ControlEvent table: jog→JogDelta, tempo→TempoFader, pads→hot
cues/beat loops, PLAY/CUE/VINYL/etc.) is pure and unit-tested.

It is enabled out of band (no controllers.toml change):

```
OPENDECK_CDJ_HID=/tmp/cdj3k-emu-1000/instance-1/usb-link.sock \
OPENDECK_CDJ_HID_ID=/path/to/cdj_hid_identity.conf \
cargo run -p opendeck-app -- amplificate.mp3
```

With the real identity in the conf the handshake establishes and turning the
emulator (or a real CDJ's) jog scrubs the loaded waveform; buttons and pads
map as the table says. The identity conf is the operator's to supply (it
authenticates as the posed vendor); it is never in the repo.

## Still to build

- Display commands (0x22–0x30): titles, RGB waveforms, beat grid, hot cues,
  and the fragmentation/reassembly they share with the tunnel (brief page 6).
  The status (0x21) feedback is sent already; these richer screen payloads are
  not yet.
- The UDP socket layer: discovery, per-player sessions, tunnel (de)fragment,
  keepalive cadence; and the iOS interface binding (IP_BOUND_IF, the wired
  link-local address) the brief's page 2 describes. The `lanhid` surface
  currently speaks the Unix/USB-B socket; `map()` is transport-agnostic and
  carries straight over to the UDP tunnel.

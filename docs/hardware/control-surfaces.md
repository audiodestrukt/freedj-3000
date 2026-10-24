# Control surfaces: MIDI controllers and I2C boards

OpenDeck takes its controls from whatever is plugged in, described in one
file, `controllers.toml`. A USB MIDI controller, a PCF8574 button board on a
Raspberry Pi's I2C bus, several of each: every one is a `[[surface]]` that
names its transport and lists its controls — buttons, encoders, faders,
LEDs — each tied to a deck action. The file is read at launch; a mistake in
it is reported with the surface and control it is in, and the rest still
works.

Where the file lives:

| Platform | Path |
|---|---|
| Linux / Raspberry Pi | `~/.local/share/opendeck/controllers.toml` (or `$XDG_DATA_HOME/opendeck/`) |
| any | `--controllers PATH` or `OPENDECK_CONTROLLERS=PATH` on the command line |

Built-in presets (today: the Numark DJ2Go, `crates/app/surfaces/dj2go.toml`)
are always loaded first; your file is merged over them, so a `[[surface]]`
with the same `name` as a preset replaces it. Worked examples to copy:

- [`surfaces/pcf8574-transport.toml`](surfaces/pcf8574-transport.toml): one
  expander as PLAY, CUE, SYNC, SHIFT, a browse knob and two lamps
- [`surfaces/pcf8574-pads.toml`](surfaces/pcf8574-pads.toml): a second
  expander as eight hot-cue pads
- [`surfaces/generic-midi.toml`](surfaces/generic-midi.toml): a template for
  any class-compliant MIDI controller

The design follows the input bus in [`../INPUT_PLAN.md`](../INPUT_PLAN.md):
a surface produces events and owns no deck state. Code:
`crates/app/src/surface/` (`config.rs` schema, `map.rs` bindings → events,
`midi.rs`, `i2c.rs`).

---

## Quick start: a PCF8574 board on a Raspberry Pi

### Wiring

The PCF8574 (and PCF8574A; also the PCF8574T/AT packages) is an 8-pin I2C
expander with no registers: reading it gives the eight pin levels, writing
it sets them. A pin written `1` is a weak pull-up (about 100 µA), so it reads
a button to ground with no external resistor; written `0` it sinks up to
25 mA, so it drives a LED directly. One board can carry buttons, an encoder
and LEDs on its eight pins at once; OpenDeck does both directions on every
poll.

| PCF8574 | Raspberry Pi header |
|---|---|
| VCC | 3V3 (pin 1) — **not 5 V**: the Pi's GPIO is 3.3 V only |
| GND | GND (pin 6 or 9) |
| SDA | GPIO 2 / SDA1 (pin 3) |
| SCL | GPIO 3 / SCL1 (pin 5) |
| A0 A1 A2 | address: all GND = 0x20 (PCF8574) / 0x38 (PCF8574A); each pin to 3V3 adds 1, 2, 4 |
| INT | leave unconnected (OpenDeck polls) |
| P0–P7 | your controls |

- **Button**: pin → switch → GND. That is all; `active_low = true` (the
  default) reads it.
- **Encoder** (a browse knob; EC11 or similar): common → GND, A and B →
  two pins. `steps_per_detent = 4` for a typical EC11; try 2 if one click
  steps twice.
- **LED**: 3V3 → resistor (1 kΩ for a quiet lamp, 330 Ω for bright) → LED
  anode; cathode → pin. `active_low = true` (the default) lights it. The
  chip can source only ~0.3 mA, so a LED from pin to ground will not light;
  use `active_low = false` only for a pin driving a transistor.
- The Pi already has 1.8 kΩ pull-ups on SDA/SCL. Several boards share the
  bus; give each its own address.
- Keep leads short (under ~30 cm) or bring the bus speed down; see *Bus
  budget* below.

### Enable I2C on the Pi

```sh
sudo raspi-config nonint do_i2c 0            # or: Interface Options → I2C in raspi-config
sudo apt install -y i2c-tools
sudo usermod -aG i2c $USER                   # then log out and in again
i2cdetect -y 1                               # the board shows up as "20" (or your address)
```

`i2cdetect` is the first thing to run when a board does not answer: if it is
not in that grid, no software will see it (check VCC, GND, SDA/SCL swapped,
the address pins).

### Describe the board

Copy [`surfaces/pcf8574-transport.toml`](surfaces/pcf8574-transport.toml) to
`~/.local/share/opendeck/controllers.toml` and change the pins to match your
wiring. Run OpenDeck; the log says

```
surface transport-board: /dev/i2c-1 @0x20 (Pcf8574), 4 button(s) 1 encoder(s) 2 LED(s), 1000 Hz
```

and from then on the buttons work and the lamps follow the deck. To see
every pin change while you test the wiring:

```sh
RUST_LOG=opendeck_app::surface=debug ./opendeck
# i2c rx [transport-board]: 11111110      ← pin 0 pulled low
```

If the board is not answering, the log says so once and OpenDeck keeps
trying every two seconds; plug it in late and it comes up by itself.

---

## The file, field by field

```toml
[[surface]]
name = "…"                 # any name; replaces a preset of the same name

[surface.midi]             # ── either this …
port = "DJ2Go"             # substring of the MIDI port name

[surface.i2c]              # ── … or this
bus = "/dev/i2c-1"         # default
address = 0x20             # 7-bit address, required
chip = "pcf8574"           # default; the only chip today
poll_hz = 500              # default; 1000 for an encoder, 200 is enough for buttons
debounce_ms = 5            # default; a button must hold its new level this long
active_low = true          # default; buttons pull the pin to ground

[[surface.button]]  …      # any number of each kind
[[surface.encoder]] …
[[surface.fader]]   …
[[surface.led]]     …
```

Every control names one **source** (how it arrives) and one **action** (what
it does), and may carry `deck = "A"` / `"B"` and, for buttons and encoders,
`shift = true`.

### Buttons

| Source | Meaning |
|---|---|
| `pin = N` | I2C pin 0–7 |
| `note = N` | MIDI: Note On (velocity > 0) is the press, Note Off or velocity 0 the release |
| `note_off = N` | MIDI: a control that only ever sends Note Off (the DJ2Go's LOAD/BACK/ENTER); the Note Off is the press, there is no release |
| `cc = N` | MIDI: a button sent as a Control Change; value ≥ 64 is down |
| `channel = N` | MIDI: match only this channel 0–15 (default: any) |

### Encoders (relative)

| Source | Meaning |
|---|---|
| `pins = [A, B]` | I2C quadrature pair; `steps_per_detent` (default 4) transitions make one step |
| `cc = N` | MIDI relative CC; `cc_mode` = `twos_complement` (default: 1–63 up, 127–65 down), `offset64` (64 = rest), or `sign_bit` (bit 6 = direction) |
| `reverse = true` | swap direction |
| `scale = 1.0` | multiply each step (jog: counts per detent) |

### Faders (absolute, MIDI only today)

| Field | Meaning |
|---|---|
| `cc = N` | 7-bit value 0–127 |
| `cc_lsb = N` | the low CC of a 14-bit pair, when the controller sends one |
| `invert = true` | 127 becomes the bottom of travel |
| `center_detent = true` | default: the two middle codes snap to exactly centre (a tempo fader's notch) |

A PCF8574 has no analog input, so an I2C tempo fader needs an ADC; see
*Not yet* below.

### LEDs

| Field | Meaning |
|---|---|
| `pin = N` | I2C pin; `active_low = true` (default) = the pin sinks the LED |
| `note = N` / `cc = N` | MIDI: sent as Note On / CC on `channel` (default 0) with velocity `on` (default 127) when lit, `off` (default 0) when not |
| `shows = "…"` | lit steadily while this indicator is true |
| `blink_on = "…"` | blinks at 2 Hz while this indicator is true; wins over `shows` |

Indicators: `playing`, `paused`, `cue` (parked on the cue point — the
XDJ's steady CUE lamp), `cue_settable` (paused off the cue — the XDJ blinks
here), `sync`, `master`, `loop`, `slip`, `key_lock`, `vinyl`, `linked`
(another player on the network), `hot_cue_0` … `hot_cue_7` (that slot is
set), `shift` (this surface's SHIFT is held), `on`.

The XDJ's own lamps, in this vocabulary: PLAY = `shows = "playing"`,
`blink_on = "paused"`; CUE = `shows = "cue"`, `blink_on = "cue_settable"`.

### Actions

Button actions, and the parameter they take if any:

| `action` | Does | Parameter |
|---|---|---|
| `play_pause`, `play`, `pause` | transport | |
| `cue` | CUE, held: back-cue while playing; set + preview while paused; release returns | |
| `sync`, `master` | SYNC toggle; ask for tempo master | |
| `slip`, `key_lock`, `jog_mode` | toggles (SLIP, MASTER TEMPO, VINYL/CDJ) | |
| `loop_in`, `loop_out`, `loop`, `reloop` | loop controls | |
| `beat_loop` | arm a loop of `beats` | `beats = 4` |
| `beat_jump` | jump by `beats` (negative = back) | `beats = -4` |
| `hot_cue` | pad, held: play from hot cue `slot`; release returns in SLIP | `slot = 0`–`7` (A–H) |
| `hot_cue_set`, `hot_cue_delete` | | `slot` |
| `memory_set`, `memory_prev`, `memory_next`, `memory_delete` | memory cues | |
| `tempo_nudge` | pitch ± as a fraction per press | `delta = 0.01` (1 %) |
| `jog_touch` | the platter touch sensor, held | |
| `load`, `back`, `eject` | browser | |
| `shift` | hold for this surface's shifted layer | |
| `time_mode`, `auto_cue`, `wave_color`, `phase_view` | display toggles | |
| `zoom` | zoom the enlarged waveform | `step = 1` / `-1` |
| `screen` | show a screen | `name = "browse"` / `"tag_list"` / `"info"` / `"menu"` / `"perform"` |
| `perform_mode` | pads as hot cues or beat jumps | `mode = "hot_cue"` / `"beat_jump"` |
| `perform_bank`, `perform_delete`, `tag_track` | PERFORM / TAG keys | |

Encoder actions: `jog` (the platter), `browse` (the list), `zoom` (`step`
per click), `tempo_nudge` (`delta` per click), `beat_jump` (`beats` per
click). Fader actions: `tempo`, `needle`.

A parameter an action does not take is an error (`slot = 2` on
`play_pause`), as is a missing one (`hot_cue` with no `slot`).

### Decks and SHIFT

- `deck = "A"` or `"B"` on a control ties it to the table the app runs as
  (`--deck A|B`, default A). A control without `deck` is always live. This
  is how a two-deck controller like the DJ2Go, which puts both decks on one
  MIDI channel, is described once and used by two OpenDeck instances.
- A button with `action = "shift"` gives the surface a shifted layer. A
  control marked `shift = true` fires only while SHIFT is held and shadows
  the plain binding of the same source; a source with only a plain binding
  keeps working under SHIFT. A button pressed under SHIFT releases to the
  binding it was pressed under, even if SHIFT let go first.

---

## MIDI controllers

Any class-compliant USB MIDI controller works; the DJ2Go preset is the
model. Finding a controller's numbers: `RUST_LOG=opendeck_app::surface=debug`
logs every message as `midi rx [name]: [90, 3B, 7F]` — status, number,
value in hex; `90`/`80` note on/off, `B0` control change, low nibble = channel.
When the port named in the file is absent, the log lists the ports it does
see. Controllers are scanned every two seconds, so plugging in after launch
(or re-plugging) just works, and LED feedback goes out on the output port of
the same name when the controller has one.

Other things with a MIDI output — a USB-HID adapter running MIDI firmware, a
microcontroller board enumerating as a USB MIDI device (Pico, Teensy, Arduino
Leonardo) — plug into the same path with no code: that is the quickest way
to attach controls OpenDeck cannot read directly, a jog encoder above all.

---

## Bus budget and what the PCF8574 is good for

The PCF8574 is a 100 kHz I2C part. One byte read is about 0.25 ms of bus,
so the bus can carry roughly 4000 reads a second in total, shared by every
board on it:

| Board | `poll_hz` | Share of the bus |
|---|---|---|
| buttons + LEDs only | 200 | 5 % |
| default | 500 | 12 % |
| with an encoder | 1000 | 25 % |

Four boards at the default fit comfortably; eight do not — give the
button-only boards 200. A LED write is another transaction but only happens
when a lamp changes.

An encoder at 1000 Hz can count about 500 transitions a second without
missing any: fine for a browse knob (a flick is well under that), not for a
jog wheel. A platter encoder puts out thousands of edges a second and needs
a counter that does not poll — a microcontroller speaking MIDI (above) or
the RP2350 surface in
[`../design/faceplate-and-hardware.md`](../design/faceplate-and-hardware.md).

Timing: a press reaches the deck within one poll plus the debounce (at
500 Hz and 5 ms, about 7 ms), on top of which the audio path adds what it
adds for every control; a MIDI button is the same minus the poll.

## Not yet

- **Analog on I2C** (a tempo fader, a crossfader): an `ads1115` chip type
  reading one channel as a `fader`. The schema is ready for it (`chip = …`);
  the driver is not written.
- **MCP23017** (16 pins, interrupts) and **PCA9555**: same shape as the
  PCF8574 with registers; straightforward additions when a board needs them.
- **Hot-plug on I2C** is a retry loop (every 2 s), not detection.
- **GPIO directly** (no expander): not supported; put the buttons on an
  expander or a MIDI microcontroller.
- **HID controllers** (Native Instruments and the like): not supported; the
  earlier S2 experiment is why the bus exists (`../INPUT_PLAN.md`).

## Adding a preset

A controller that works deserves to be built in: add its file under
`crates/app/surfaces/`, list it in `PRESETS` in `crates/app/src/surface/mod.rs`,
and the `presets_parse` test checks it on every build. Say in the file's
header what was confirmed against hardware.

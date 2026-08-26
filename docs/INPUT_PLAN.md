# Input plan: touch, controls, and a hardware simulator

Goal: the deck is driven by **one event stream**, whatever produces it — a
finger on the panel, a mouse standing in for the finger, keys, a MIDI
controller, a test script, or eventually the RP2350 surface over serial.
Anything that can be tested with a script can be tested without hardware,
and a recorded session from real hardware can be replayed as a script.

## What exists today

Three input paths, none shared:

| Source | Where | Reaches the deck by |
|---|---|---|
| Keyboard | `main.rs` `WindowEvent::KeyboardInput` | writing `AudioHandle` atomics directly |
| MIDI (DJ2Go) | `midi.rs` | writing the same atomics, plus its own `State` mutex for jog/nudge |
| Touch / mouse | — | nothing; the screen is paint-only |

And one unused piece that is the right shape:
`crates/protocol/src/lib.rs` defines `ControlEvent` — `Play`, `Cue`,
`JogDelta { delta, velocity_rpm }`, `JogTouch`, `HotCueTrigger`, `BeatLoop`,
`TempoFader`, `NeedleSearch`, `BrowseEncoderDelta`, … — plus the 10-byte
`McuPacket` wire format and `ButtonId` for the physical surface. The engine
crate's `Transport::handle_event` consumes it. Nothing in the binary produces
or consumes it.

## Architecture

```
 keyboard ─┐
 mouse/touch (egui hit-test on Layout rects) ─┤
 MIDI ─────────────────────────────────────────┤
 sim script (file / stdin / UDP) ──────────────┼──▶  ControlEvent bus  ──▶  Deck::apply(ev)  ──▶ atomics / transport
 MCU serial (later) ───────────────────────────┘           │
                                                            └──▶ event log (timestamped, replayable)
```

- **`ControlEvent` is the only way in.** Sources are adapters; they own no
  deck state. `midi.rs`'s jog/nudge `State` moves into the deck.
- **`Deck::apply(&mut self, ev, now)`** is the single place events change
  state. Today it writes the same atomics the sources write now; when
  `crates/engine` is wired (WORKSTREAMS C2) it forwards to
  `Transport::handle_event` unchanged. Sources never need to know.
- **The bus is a lock-free queue** (`rtrb`, already a dependency) drained
  once per frame in `render_frame` before the snapshot is built, so the
  screen always reflects every event up to this frame.
- **Every event is logged** with a monotonic timestamp. That log is the
  recording format and the replay format — the simulator's script syntax is
  just a human-writable spelling of it.

## Why the seam matters: the S2 lesson

The first controller was the Kontrol S2 MK2 over HID, and it stalled because
a bad jog delta and a bad nudge looked identical — reading the wheel and
acting on it were one function, and there was no way to tell which half was
wrong. The bus fixes this by construction:

- **The adapter's output is inspectable on its own.** `--record` (or
  `RUST_LOG=opendeck::input=debug`) shows the raw `JogDelta` stream with
  timestamps. Does it wrap cleanly at the 24-bit boundary, is the rate what
  the hardware should produce, does it go to zero at rest? Those are
  questions about the adapter, answered without the deck involved.
- **The deck's behaviour is testable with hardware out of the loop.** A
  simulator jog profile is a known-good delta stream; if the nudge misbehaves
  on it, the nudge is wrong. If it behaves on the profile and not on the
  device, the adapter is wrong.

Same for every source. This is the main reason the seam is worth half a day
before any feature.

## Touch

The XDJ screen is a touch panel. winit delivers `WindowEvent::Touch` and
egui-winit already folds touch and mouse into the same pointer model, so
**mouse *is* the touch simulator** — nothing extra to build for phase one.

`screen.rs` currently paints with the painter and never registers hit
regions. Each touch key becomes `ui.interact(rect, id, Sense::click())`
with the same paint; the `Layout` rects are already the hit areas. Touch
targets on the XDJ-1000MK2 screen, in order of usefulness:

| Region | Event | Notes |
|---|---|---|
| Overview (playing address) | `NeedleSearch { position }` | Touch/drag to jump; the reference unit calls it NEEDLE SEARCH |
| Enlarged waveform | touch-cue preview (later); zoom via drag | Zoom needs `cols_visible` to become state |
| ZOOM – GRID pill | toggle zoom / grid-adjust mode | |
| SYNC / MASTER | `SyncToggle` / `MasterRequest` | New variants; meaningful once Link send exists |
| CUE/LOOP DELETE · MEMORY, CALL ◀ ▶ | cue memory ops | Need `crates/engine` cues |
| BROWSE · TAG LIST · INFO · MENU · PERFORM | screen switch | Need those screens |
| LINK / USB, SLIP | source select / `SlipToggle` | |
| Phase meter | toggle phase meter ↔ waveform (per the manual) | |

Gestures: single tap, drag (needle search, zoom), long-press (grid adjust).
No multi-touch on the XDJ; ignore it.

## Physical controls: keyboard as the first surface

Map every `ButtonId` to a key so the whole surface is reachable without
hardware, with the existing keys kept:

| Control | Key | Event |
|---|---|---|
| Play/pause | `Space` | `Play` / `Pause` |
| Cue | `Q` → move to `Backspace`; keep `Q`=quit on `Esc` only | `Cue` |
| Hot cues 1–8 | `1`–`8` (`Shift` = set, `Ctrl` = delete) | `HotCueTrigger/Set/Delete` |
| Loop in / out / exit / reloop | `I` `O` `L` `R` | `LoopIn` … |
| Beat loop 1/2/4/8 | `[` `]` with Shift | `BeatLoop { beats }` |
| Beat jump ± | `,` `.` | `BeatJump` |
| Tempo fader | `↑`/`↓` fine, `PgUp`/`PgDn` coarse, `0` reset | `TempoFader { position }` |
| Key lock, slip, sync, master | `K` `S` `Y` `M` | toggles |
| Jog (nudge) | `←`/`→` held = jog delta at a fixed velocity | `JogDelta` |
| Jog (scratch) | `Shift` + `←`/`→` = `JogTouch(true)` then deltas | |
| Browse encoder | `Tab`/`Shift-Tab`, `Enter` = load | `BrowseEncoderDelta`, `Load` |
| Waveform colour, time mode | `C`, `T` | display settings (not `ControlEvent`) |

`←`/`→` as seek ±10 s today becomes jog; seek moves to needle search.

Mouse jog: drag left/right over the enlarged waveform with the right button,
or scroll wheel = `JogDelta`. Cheap, and useful with a trackpad.

## Simulator

A script is a list of timestamped `ControlEvent`s, one per line, in the log
format:

```
# t(ms)   event
0         play
0         tempo 0.52
1200      jog_touch 1
1200      jog +12 @33.3
1250      jog +12 @33.3
1300      jog +12 @33.3
1300      jog_touch 0
4000      hotcue 3
4000      sleep 500
4500      needle 0.25
```

Three ways to feed it, all going onto the same bus:

1. **File**: `opendeck track.mp3 --script test.evt` runs it and exits at
   the end (or on `quit`). Deterministic; the frame instrument and a
   screenshot at the end make it a regression test.
2. **stdin**: same syntax, interactive, for driving by hand or from a shell
   loop.
3. **UDP** (`--sim-port 7300`): same lines as datagrams, so a script on
   another machine — or a browser page with on-screen buttons and a jog
   wheel — can drive the deck. This is also the path a Python test runner
   uses.

Jog realism matters more than button realism. A real platter produces a
stream of encoder deltas at ~1 kHz with a velocity that ramps; a single
`jog +40` is not what hardware sends and will exercise different code. The
simulator gets **jog profiles**: `jog_spin <rpm> <ms>`, `jog_nudge <±deltas>
<ms>`, `scratch <pattern>` expanded into the delta stream the MCU protocol
would carry (`PacketKind::JogDelta` + `JogVelocity` at the documented
rates). Profiles are expanded on the host side before the bus, so the deck
never sees anything a real surface would not send.

**Recording**: `--record session.evt` writes every event from any source in
the same format. Drive it with the DJ2Go, replay it with `--script`. That
is the regression suite for MIDI mapping changes and, later, for RP2350
firmware.

## Two decks on one controller

Controllers split into two families, and the adapter handles both by a
`--deck A|B` flag (channel select):

- **True two-deck** (Kontrol S2/S4, DDJ-series): the right deck sends on MIDI
  channel 1, the left on channel 0, same note/CC numbers. `--deck A` listens
  to channel 0, `--deck B` to channel 1. Two freedj instances — one per
  channel, each also its own Link player — split the controller. `make
  link-pair` does exactly this (`--player 1 --deck A`, `--player 2 --deck B`).

- **Note-numbered two-deck** (Numark DJ2Go): both decks send on MIDI channel
  0, distinguished by note/CC *number* (left play 0x3B, right play 0x42; left
  jog CC 0x19, right jog CC 0x18; etc. — `docs/reference/dj2go-midi-map.md`).
  Here `--deck A|B` selects a note *table*, not a channel. Two freedj
  instances still split the controller — one per deck — each also a Link
  player.

Both mechanisms live behind the same `--deck A|B` flag; the adapter picks a
note table for the DJ2Go and (for a future S2 adapter) a channel filter for a
true two-channel controller.

## Hardware sources (all adapters, all later)

Each is a thread that turns device input into `ControlEvent`s on the bus and
takes LED/display feedback back out. None touches deck state.

| Source | Transport | Notes |
|---|---|---|
| **USB MIDI controllers** (DJ2Go today; DDJ-400/FLX4, Mixtrack, …) | `midir`, already used | Becomes a mapping file per controller instead of constants in `midi.rs` (WORKSTREAMS G1). Jog wheels arrive as relative CCs at the controller's own rate — profiles in the simulator should include a "MIDI-rate jog" so this path is tested too. |
| **USB HID controllers** (Native Instruments S2/S4, Denon, anything vendor-specific) | `hidapi`, already a dependency, unused | Report descriptors per device; higher-rate jog than MIDI, plus LED/screen feedback over the same interface. Same adapter shape as MIDI with a different decoder. |
| **Direct hardware on the Pi** — encoders, buttons, capacitive strip on GPIO / SPI / I²C | `gpiod` / `rppal` | For a first physical build without the RP2350: read the jog encoder from a GPIO interrupt, buttons from a matrix, LEDs over SPI. Runs on a dedicated thread with RT priority so the jog stream is not at the mercy of the UI. Simplest route from "app on a Pi" to "deck you can touch". |
| **RP2350 control surface** over serial / SPI | `McuPacket` (`crates/protocol`) | The eventual product path: the MCU does the timing-critical encoder work and the Pi gets a clean 1 kHz `JogDelta` stream. Below. |

Which of the last two to build first is a hardware-availability question, not
a software one: the bus and the simulator are identical either way, and the
Pi-GPIO adapter is a reasonable stepping stone to the RP2350 one (same events,
worse timing guarantees).

## MCU path (later, unchanged by this plan)

The RP2350 sends `McuPacket`s; a serial adapter decodes them into
`ControlEvent`s and puts them on the bus. Because the simulator already
speaks in `ControlEvent`s at MCU-realistic rates, the firmware can be
developed against the same tests. LED feedback goes the other way through
the same adapter (`LedPad`, `LedRing`, `LedIndicator`), and the simulator
gets a `--leds` printout so button-light logic is testable too.

## Order of work

1. **Bus + `Deck::apply`.** Add `ControlEvent` bus (rtrb) and one `apply`
   that does what the keyboard and MIDI handlers do now. Port keyboard to
   emit events. Port MIDI. Delete the duplicated atomics writes. Behaviour
   identical; two sources become adapters. *Half a day.*
2. **Event log + `--script`/`--record`.** Timestamped log, the line format,
   file playback with exit-at-end. First regression test: play 3 s, jog,
   screenshot, compare frame instrument. *Half a day.*
3. **Keyboard surface.** The full `ButtonId` map above. *Small.*
4. **Touch on the screen.** `ui.interact` on Layout rects; needle search
   first (it is the one that changes what you hear), then ZOOM/GRID, SLIP,
   and the phase-meter toggle. Mouse tests it; a touch panel needs no code.
   *One day.*
5. **Jog profiles + mouse jog.** Realistic delta streams in the simulator;
   right-drag / wheel on the waveform. *Half a day.*
6. **UDP sim port.** Same parser on a socket. *Small.* Optional browser
   control page later.

Steps 1–2 are prerequisites for everything else and for the transport work
in WORKSTREAMS §C: once events flow through `Deck::apply`, wiring
`crates/engine` is swapping the body of one function.

## Not in scope

- Multi-touch gestures (the XDJ panel is single-touch).
- Making `screen.rs` a general widget toolkit; it stays paint + hit-test.
- The MCU firmware itself.

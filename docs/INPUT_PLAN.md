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

# OpenDeck as a plugin (VST3 / CLAP)

Status: first cut, 2026-11-20.  Runs in REAPER 7.60 on Linux (CLAP and
VST3); untested in other hosts and on macOS / Windows.

## What it is

`crates/plugin` (nih-plug) builds one `.so` that is both a CLAP and a VST3
instrument with a stereo output.  The whole deck runs inside the host's
process:

- **Audio.**  The deck's engine is unchanged: the processor thread decodes
  and time-stretches into the ring buffer.  The host's audio callback takes
  the sound card's place: `process` calls `HostOutput::fill`, the same
  `OutputFill` the cpal callback uses (stop fade, seek flush, output gain).
  The deck runs at the host's sample rate (`AudioHandle::open_host`), so
  loads resample to it as they do to a sound card's.
- **Window.**  The deck's own window (the desktop UI, wgpu + winit), on a
  thread of its own (`with_any_thread`), not inside the host's plugin
  window.  BROWSE, LOAD, PLAY, CUE and the rest work as on the desktop.
- **Pro DJ Link** as on the desktop: receive-only unless
  `OPENDECK_LINK_SEND=1`.
- **Parameters.**  *Pitch* (−16 … +16 %, the default TEMPO RANGE) moves
  the tempo fader through the input bus, as a MIDI fader does, so host
  automation, LFOs and parameter modulation play the deck.

## Limits of this cut

- One deck per process: winit allows one event loop per process.  The
  output sits in a process-wide slot that whichever instance is
  processing pulls from, since a host can create a probe instance before
  the track's own.
- A closed deck window does not come back until the host restarts.
- The window is not embedded in the host's plugin editor.  That needs a
  child-window path (baseview, or winit's parent-window support) and
  rendering that doesn't own the event loop.
- Latency: the ring (~93 ms) plus the stretcher, not reported to the
  host, so the deck is late against the host's grid by that much.
- No host tempo or transport sync yet; Pitch is percent on a ±16 % fader
  whatever MENU → TEMPO RANGE says.
- Offline render runs at real time at best (the processor is paced by the
  ring).

## Try it

```sh
make plugin                                  # build, install in ~/.clap and ~/.vst3
make reaper-demo TRACK=~/Music/some.mp3      # REAPER: deck on a track, Pitch automated
```

The demo (`tools/reaper/opendeck_demo.lua`) adds OpenDeck to a new track,
draws a Pitch envelope (hold, ramp to +8 %, ramp to −8 %, a ±6 % LFO, back
to 0), loops 0–50 s and presses play.  `OPENDECK_TRACK` loads a track at
start and `OPENDECK_AUTOPLAY=1` starts it playing; without them the deck
boots empty, to BROWSE.

REAPER notes:
- An empty project stops at once on play; the demo adds an item for the
  length.
- A play pressed from a startup script is dropped, so the demo presses it
  after 2 s.

## Tests

- `audio::host_output_tests::a_plugin_host_hears_the_deck_it_hosts`: a
  host pulling 512-frame blocks hears a loaded track on PLAY and silence
  when paused.
- `opendeck-plugin` `pitch_moves_the_fader_like_a_cdj`: the Pitch to
  fader mapping.
- In REAPER, by hand: the envelope moved Pitch 0 → +8 % → −8 %, and the
  deck's playhead ran 8.6 % fast at the top of the ramp.

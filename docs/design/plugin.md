# OpenDeck as a plugin (VST3 / CLAP)

Status: first cut 2026-11-20; any number of instances since 2026-11-21.
Runs in REAPER 7.60 on Linux (CLAP and VST3); untested in other hosts and on
macOS / Windows.

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
- **Window.**  The deck's own window (the desktop UI, wgpu + winit), not
  inside the host's plugin window.  BROWSE, LOAD, PLAY, CUE and the rest
  work as on the desktop.
- **Any number of instances.**  winit allows one event loop per process, so
  the decks share one: `DeckHost` (crates/app) runs it on a thread of its
  own (`with_any_thread`), started by the first instance.  Each later
  instance adds a deck with its own window, its own output slot and its own
  Pitch.  `MultiDeck` routes each window's events to its deck, and wakes the
  loop for whichever deck needs it soonest.  Removing an instance closes
  its deck.  Decks number themselves 1, 2, … (the lowest free number): deck
  1 keeps its MENU PLAYER No., deck *n* takes PLAYER No. *n*, and decks
  after the first listen on controller channel B.
- **The track is saved.**  Each instance keeps its track's path in its state
  (the `track` field), so a project reopens with each deck's track loaded.
  A host or script that writes the field (REAPER: `clap_chunk`) loads that
  track.  The direction back, a track loaded in the deck's own browser
  updating the field, is not done yet.
- **Quit.**  The library pins itself in memory (dlopen RTLD_NODELETE), so a
  host that unloads it doesn't pull the code out from under the deck loop's
  thread.  An exit hook closes the decks before the process's destructors
  run.  Hosted decks ask wgpu for the primary backends only (Vulkan /
  Metal), unless `WGPU_BACKEND` says otherwise: an unused EGL instance
  panicked in its drop at REAPER's quit, because EGL was already gone.
  **Still open:** REAPER can still segfault at quit, after it has removed
  the first deck, with no Rust panic.  That points into the graphics driver
  or REAPER's own teardown.  The session itself is unaffected.
- **Pro DJ Link** as on the desktop: receive-only unless
  `OPENDECK_LINK_SEND=1`.
- **Parameters.**  All of these go through the input bus, as a controller
  does, so host automation, LFOs and parameter modulation play the deck:
  - *Pitch* (−16 … +16 %, the default TEMPO RANGE) moves the tempo fader.
  - *Play* is a level: on plays, off pauses.
  - *Cue* is the momentary CUE button.  Pressed while playing, it jumps
    back to the cue and stops: the back-cue a juggle needs.
  - *Platter touch* is a hand on the record (VINYL: it holds a playing deck).
  - *Platter* is the record's position in seconds (±8).  Moving it turns
    the jog by exactly that much (`JOG_SECS_PER_TICK`, with the remainder
    kept), so drawing it scratches, with the scrub audio.
  The parameters are read once a host block (11.6 ms at 512 frames), so
  button edges need about 12 ms between them.

## Limits of this cut

- A closed deck window does not come back until its plugin instance is
  re-created.
- Several decks in one process share the network ports: the second deck's
  media server finds 1051/12523 taken and doesn't start (the first deck's
  serves the library), and both announce on Pro DJ Link as their own player
  numbers.
- The window is not embedded in the host's plugin editor.  That needs a
  child-window path (baseview, or winit's parent-window support) and
  rendering that doesn't own the event loop.
- Latency: the ring (~93 ms) plus the stretcher, not reported to the
  host, so the deck is late against the host's grid by that much.
- No host tempo or transport sync yet; Pitch is percent on a ±16 % fader
  whatever MENU → TEMPO RANGE says.
- Offline render runs at real time at best (the processor is paced by the
  ring).

## Two decks and a crossfader

```sh
make reaper-mixer TRACK_A=~/Music/a.mp3 TRACK_B=~/Music/b.mp3
```

`tools/reaper/opendeck_mixer.lua` builds:
- **Deck A** and **Deck B**: OpenDeck on each, with each one's track written
  into the plugin's saved state;
- **Mixer**: `tools/reaper/opendeck_xfader.jsfx`, a crossfader with deck A
  on channels 1/2 and deck B on 3/4, constant-power / linear / cut curves
  and two channel faders.  It's test harness only: OpenDeck stays a deck,
  not a mixer.

The crossfader is automated: A for 20 s, over to B in 8 s, B for 20 s, back
in 8 s, looped.  `OPENDECK_MIXER_LOG=file` logs each track's peak once a
second, to check a run without listening.

Measured (2026-11-21, REAPER 7.60, RTX 4070 box): both decks play, and the
Mixer equals A at crossfader 0 and B at 1, with both through the fades.
There are underruns only in the first 1.5 s, while both tracks decode at
once, and none after.  REAPER quits cleanly.

REAPER can also run a plugin in its own process ("Run as → Dedicated
process", per plugin, from the FX browser).  That isn't needed for OpenDeck
any more, and a script can't set it: it isn't in the project chunk.

## A beat-juggle and scratch routine

```sh
make reaper-routine TRACK=~/Music/a.mp3 BPM=130
```

`tools/reaper/opendeck_routine.lua` puts the same track on both decks, at
its cue, with the crossfader on its Cut curve.  It's 16 bars, all envelopes:
- **Bars 1–4:** deck A plays the intro.
- **Bars 5–8:** a loop juggle on the first beats, A and B taking turns, each
  back-cued while the other plays: half-bars, then beats, then half-beats.
- **Bars 9–12:** A plays, and B scratches over it from its cue: baby
  scratches, a transformer (crossfader cuts every eighth of a beat), chirps.
- **Bars 13–16:** B is let go and plays; over to B; A stops.
- **The end:** both back to their cues, and the project loops.

Measured (2026-11-21, 130 BPM): the meters follow each section: A alone,
A and B alternating through the juggle, B sounding in bursts over A, then B
alone, then looping.  The timing of a deck's moves is only as fine as the
host block plus a 1 ms poll, a few milliseconds.

`make reaper-mixer-check TRACK_A=… TRACK_B=…` runs the two-deck mixer for
60 s and checks it with `tools/reaper/check_meters.py`.  It reads the
decks' log first, for the cause (did two decks start, load and play?), and
then the meters, for the result (does the Mixer follow the crossfader?).
REAPER's exit status is reported too.  It needs a display and an audio
device, and refuses to start while another REAPER holds the device.

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

- `host_load_tests::a_host_load_plays_only_once_the_track_is_in`: a host's
  load parks at the cue, and with `play` it starts once loaded, not before.
  A play sent ahead of the load hit the empty deck and stopped at its end,
  which is why deck A stayed silent in the first two-deck run.
- `host_load_tests::the_deck_loop_wakes_for_the_soonest_deck`.
- `opendeck-plugin` `decks_take_the_lowest_free_number`,
  `platter_moves_turn_the_jog_by_the_distance_drawn`,
  `button_parameters_press_on_their_edges`, and
  `the_saved_track_has_the_shape_the_reaper_script_writes` (pins the state
  shape the REAPER scripts write).
- `cue_latency_tests::a_touched_scratch_moves_the_record_by_the_ticks_and_plays_on`.
- `audio::host_output_tests::two_host_decks_each_fill_their_own_output`.

- `audio::host_output_tests::a_plugin_host_hears_the_deck_it_hosts`: a
  host pulling 512-frame blocks hears a loaded track on PLAY and silence
  when paused.
- `opendeck-plugin` `pitch_moves_the_fader_like_a_cdj`: the Pitch to
  fader mapping.
- In REAPER, by hand: the envelope moved Pitch 0 → +8 % → −8 %, and the
  deck's playhead ran 8.6 % fast at the top of the ramp.

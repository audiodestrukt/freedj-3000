# Track loading: one pipeline, off the UI thread

Status: **as built** (2026-09-17, commit `8bf7e67`, issue #19). This page
records how a track gets from "the DJ pressed LOAD" to "the deck is parked at
the cue", where each step runs, and the nuances that are easy to forget when
touching it next. Code: `crates/app/src/lib.rs`, search for `start_fetch`,
`Prep`, `Prepared`, `apply_prepared`.

## The shape

Every load, whatever the source, goes through the same three stages:

```
 UI thread                      track-load thread                     UI thread
 ─────────                      ─────────────────                     ─────────
 load_selected(Load)
   └─ load_track / load_track_link / load_track_db
        prep = self.prep()  ──▶  fetch bytes (disk / NFS / dbserver+NFS)
        start_fetch(name, ‖)     decode  →  resample to deck rate
                                 waveform build
                                 grid: ANLZ | dbserver beats | our detector
                                 auto cue (first_sound)
                                 Ok(Prepared) ──▶ mpsc ──▶ poll_fetch (each frame)
                                                            └─ apply_prepared
                                                               audio.load_samples
                                                               renderer.set_waveform
                                                               cue / transport reset
```

- **Stage 1, UI thread, microseconds.** Capture what the loader needs from
  the deck into a `Prep` (sample rate, channel count, AUTO CUE on/off and its
  level) and spawn the thread. The deck keeps playing.
- **Stage 2, loader thread, hundreds of ms to seconds.** Everything heavy.
  `Prep::finish` is the shared tail; `Prep::complete` wraps it for network
  loads that still have to decode from bytes. The result is a `Prepared`: the
  samples in an `Arc`, the waveform cache, the resolved grid, the cues, the
  cue point, the tags.
- **Stage 3, UI thread, about 5 ms.** `poll_fetch` runs once per frame after
  input events. When the channel has a result, `apply_prepared` hands the
  `Arc` to the audio thread, uploads the waveform to the GPU, applies a
  hand-adjusted grid from `grids.json` if there is one, pushes the grid to
  the Link sender, and parks the deck at the cue.

Measured headless (debug build, a full-length MP3): 1054 ms in stage 2 with frames
rendering throughout and no frame-spike line; 5 ms in stage 3. Before this
the same load froze the UI for the whole second, and several on a Pi.

## Sources

| `Load` variant | fetch | grid source |
|---|---|---|
| `Local { path, analyze }` | `decode_file` from disk | ANLZ file next to a rekordbox USB export, else our detector |
| `Link { ip, rel_path, analyze_rel }` | NFS read of audio and ANLZ from a player's USB | ANLZ bytes, else our detector |
| `Db { ip, id, title, rekordbox }` | dbserver `0x2102` for the path and `0x2204` for the grid, then NFS read | dbserver beat grid, else our detector |

The startup track (`opendeck <file>`) is the one exception: `AudioHandle::open`
decodes it synchronously before the window exists, so there is nothing to
block. It does not go through `Prep`; if you change how a grid or cue is
derived, check `DeckApp::new` still agrees.

## Nuances worth remembering

**Why `Prep` exists.** The loader thread cannot borrow `self`, and it must not
see deck state that changes while it runs. `prep()` copies the four facts it
needs at spawn time. If the tail ever needs another deck setting, add it to
`Prep`, do not reach for `self` from the thread.

**The channel check lives on the loader thread.** A track with the wrong
channel count fails in `Prep::finish` before any decode work is wasted, and
the error comes back through the channel like any other load failure. The
audio thread's `load_samples` re-checks rate and channels as a last line of
defence; both messages mention A1 because channel conversion is still not
implemented.

**Latest load wins.** `start_fetch` overwrites `fetch_rx`. An older thread
still running finishes its work and its `send` fails silently because the
receiver is gone. There is no cancellation, so a slow network load followed
by a fast local one costs the network bytes anyway. Fine today; revisit if
loads get expensive on the Pi.

**AUTO CUE is decided on the loader thread.** `first_sound` scans the decoded
samples, which is a full pass over the track. It runs in stage 2 with the
AUTO CUE state captured in `Prep`, so toggling AUTO CUE mid-load applies to
the next load, not this one. A rekordbox memory cue still wins over the auto
cue; that is the DJ's choice.

**The cue is captured from prepared samples, not heard position.** Load is
the one place where the cue is set from the sample buffer directly. Every
later cue capture (CUE, hot cues, loops) must use `smoothed_pos`, the heard
position, not `audio.position`. See the memory note "cue capture uses heard,
not raw".

**Grid precedence in `apply_prepared`.** A hand-adjusted grid in `grids.json`
(keyed by path) beats whatever the loader resolved. `grid_orig` keeps the
loader's grid so GRID ADJUST can reset. The Link sender's grid is swapped at
the same moment so SYNC divides by the new track's BPM immediately.

**`set_waveform` is the only GPU work in the load.** It rebuilds the waveform
texture from the cache and is the bulk of the 5 ms. If the swap-in ever shows
up in the frame-spike detector, that is where to look first.

**The frame-spike detector is the regression test.** Run with
`OPENDECK_AUTOLOAD=<file>` under xvfb and grep the log for `frame spike`. The
autoload is the async path (it sets `play_on_load` and playback starts when
the load lands), so it exercises exactly what LOAD does. Recipe:

```bash
env -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 RUST_LOG=info OPENDECK_SERVE=0 \
  OPENDECK_AUTOLOAD=/path/next.mp3 timeout 20 xvfb-run -a -s "-screen 0 1280x800x24" \
  target/debug/opendeck /path/first.mp3 > load.log 2>&1
grep -E "loading|prepared|loaded|frame spike" load.log
```

Expected: a `loading … in the background` line, a `prepared … in N ms` line
from the loader thread, a `loaded …` line a few ms later, and no
`frame spike` between them.

**`DeckApp::loading` is set but not drawn.** The name of the track in flight
is available to the screen for the XDJ's loading indicator. Nothing renders
it yet; that is the next visual piece.

**Memory.** The whole decoded track is in RAM (stereo 44.1 kHz is about
10 MB a minute). Two tracks exist briefly during a load: the playing one and
the prepared one, until the audio thread drops the old `Arc`. The streaming
loader (WORKSTREAMS A4) is the answer if that ever matters; this pipeline is
where it would plug in, since stage 2 is already the only producer of
`samples`.

## Related

- `docs/reference/prodj-link-media.md`: how the bytes arrive for the two
  network sources.
- `AUDIO_ENGINE.md`, "Track Loading Sequence": the original target design.
- WORKSTREAMS A1 (channel conversion, real-time SRC) and A4 (streaming loader).

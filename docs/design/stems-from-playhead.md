# Stems from the playhead

Status: **built** 2026-11-15 (parts 1–3 below); an iPad pass is still to
come. Code: `StemBank` pages (`crates/app/src/stems.rs`), `separate`,
`run`, `Geometry` (`crates/app/src/separate.rs`),
`DeckApp::release_idle_model`.

**Measured** (Linux desktop, ONNX Runtime on the CPU, debug build). Opened
STEMS 2:00 into a 3-minute track: model load 3.8 s, the first run started
at 2:08 (the playhead plus the 4 s untimed look-ahead), chunks 2.0–2.3 s
each (about 2.6× real time), so the stems were at the playhead about 8 s
after the panel opened. The old top-to-bottom split needed about 46 s of
chunks to get to 2:00, plus the load. With the model kept loaded, the next
track saves the load.

**As built, where it differs from the proposal below:**
- Pages are 4,096 frames (85 ms at 48 kHz), not a stride; a per-page count
  of written samples, and a page is ready when full.
- Runs always start writing on a page boundary and stop at the first page
  another run has written, so each sample is written once. A run that
  stops partway through a page (a jump) forgets that page
  (`forget_partial`) for the next run to fill.
- A run starts a quarter chunk plus 4,096 model frames before the page it
  writes from, so the stretch its first chunk has to itself and the output
  converter's start-up fall before it. With a pointwise model a run-split
  bank matches a top-to-bottom one within 2 LSB (tests).
- Model frames map exactly onto deck frames: input windows and the output
  converter start on multiples of 147 model / 160 deck frames (48 kHz).
- The untimed look-ahead is 4 s; once a chunk is timed it's 1.5 × the
  chunk time + 1 s. A run gives way only when the playhead jumps (further
  than playing at 4x could take it) or overtakes a run that began ahead of
  it; while the playhead stays put the next run aims where the last did.
  Before (2026-11-15 morning) a run gave way whenever the re-measured
  look-ahead moved, which with a paused deck restarted runs over and over:
  38 chunks for a 31-chunk track. Now 32, paused or playing.
- Model release: 10 s after STEMS closes with no split running, on a
  thread of its own; at once on an iOS memory warning
  (`UIApplicationDidReceiveMemoryWarningNotification`, counted in
  `ios/freedj/main.m`) when no split is running. A warning during a split
  is only logged to `opendeck-stems-test.txt` with the memory figures.
- Prepared banks (local stick or LINK) are read in 4 s steps in the same
  order: from the step the playhead is in (the cue, right after a load),
  then a jump's stretch next if it isn't read yet, the top last. Steps are
  independent (each converts with its own margin), so any order gives the
  same bank. The whole-role hash is still taken in page order; pages a
  later start skipped past are read again at the end to finish it (no
  extra reads when the start is the top).

## The problem

A split on the device runs HTDemucs over the track from the first second
to the last (`separate::separate`, `crates/app/src/separate.rs`). The STEMS
keys work only up to how far it has got (`StemBank::ready`, one counter
from the top). A DJ who opens STEMS two minutes into a track waits for two
minutes of track to be split before the stem they touch does anything,
plus the model load (seconds on the iPad).

Goal: the stems at the playhead are ready a few seconds after STEMS is
opened, wherever the playhead is, and stay ahead of it.

## What already fits

- `StemBank` is 16-bit interleaved, allocated for the whole track, written
  by one writer and read lock-free by the audio thread.
- Where a stem is not ready, the stem mix plays the track itself
  (`stems.rs`, `i < b.ready()`). So a gap in the bank already sounds like
  the full mix rather than silence.
- The split works in chunks: `seg` frames (HTDemucs: 343,980 at 44.1 kHz,
  7.8 s) every `stride = 0.75 * seg` (5.85 s), crossfaded with Demucs's
  triangle weights.

## Design

### 1. The bank knows which ranges are ready

Replace the single `ready` counter with one ready flag per **stride**
(an `AtomicBool`, or a bitset of `AtomicU64`s, per 5.85 s). A 6-minute track
has 62 flags.

- `StemBank::ready_at(i) -> bool`: the audio thread's test per block
  (block-aligned, not per sample: one load per callback).
- `StemBank::write(range, block)` replaces `append`; it sets the flags it
  completes with `Release`.
- `percent()` counts flags. The panel's progress band becomes "the next
  N seconds ready" rather than a fill from the top.
- **Seams:** where the stem mix switches between the stems and the
  full-mix fallback, crossfade over ~10 ms, so a stem key pressed at the
  edge of a ready range doesn't click.

### 2. The split runs chunks in the playhead's order

`separate()` becomes a loop over **runs** of consecutive chunks, not
a single pass from chunk 0:

1. Start at the chunk holding `playhead + look-ahead` (look-ahead = one
   chunk's compute time, measured, plus a margin).
2. Go forward to the end of the track, then wrap round to the top and stop
   at the first ready chunk.
3. Before each chunk, check the playhead (an `AtomicU64` the deck already
   publishes). If it has jumped (hot cue, needle search, a loop elsewhere,
   a track search) to a range that isn't ready and isn't coming within
   the look-ahead, end the run and start a new one there.

**Run edges.** Inside a run, overlap-add works as now. At the first
chunk of a run there is no earlier chunk to crossfade with; normalising
by `wsum` makes those 1.95 s come from that chunk alone. That's audibly
fine, but it doesn't match a top-to-bottom split bit for bit. When a later
run reaches a ready range from below, it stops one stride short and
crossfades into what's there. Tests compare a run-split bank with a
straight split within a tolerance, not exactly.

**Normalisation.** Demucs normalises by the whole track's mean and
standard deviation. The streaming loader has the whole track within about
a second of a load (0.6 s for 3 minutes on a desktop), long before
anyone opens STEMS, so take the statistics then. If STEMS is opened
before the decode finishes, use the decoded part's statistics and keep
them for the rest of the track. Changing them mid-track would put a level
step at a seam.

**Input.** Resample each chunk's input as it is needed (a `seg`-frame
window plus the resampler's delay), not the whole track up front as now
(`mix`, a second whole-track copy at 44.1 kHz).

### 3. The model stays loaded while STEMS is open

The model load is the biggest part of the first stems' wait on the iPad.
Keep it while the STEMS panel (or XDJ STEMS pad mode) is open and while a
split runs. Release it on closing the panel, on loading a track with
prepared stems, and on a memory warning (iOS `didReceiveMemoryWarning` via
`main.m`).

**Memory caveat.** On the iPad the model is the ~4.7 GB peak
(`SPLIT_NEEDS_MB`). Holding it while the panel is open is the decision
Dan's `opendeck-stems-test.txt` log must settle: if the peak is the model
itself, holding it costs nothing more than a split does now; if the peak
comes from the run, holding the model idle may be fine and loading it per
open may not.

## Relation to a paged track buffer

The decoded track is still whole in RAM as `f32` (about 10.6 MB per
minute). A paged track buffer (pages kept near the playhead, the cues, the
loop and the SLIP shadow) has the same core as part 1: a page table with
ready flags, one writer, the audio thread reading lock-free, a fallback
for a page that isn't there. Build part 1's ready map as that page table
(page = stride, generic over the sample type), and the track buffer reuses
it later. The track buffer itself isn't needed for stems from the playhead.

## Tests

Deck-harness tests, event in → deck state out:

- Load, play to 2:00, open STEMS: drums-only is heard at the playhead
  within look-ahead + one chunk (`FakeSeparator` with a fixed per-chunk
  delay), and the bank's ready ranges start at the playhead.
- Hot cue back to 0:10 during a split: the next run starts there.
- A loop at 3:00: the split covers the loop, then carries on.
- Seams: a stem gain change across a ready-range boundary has no
  sample step larger than the mix's own.
- The finished bank matches a top-to-bottom split within tolerance.
- The model is released on closing STEMS and on loading another track.

## Size

About two days for parts 1 and 2 with tests, half a day for part 3, and
then an iPad pass (time to first stems at 2:00 into a track, memory with
the panel open).

# Power strategy for the phone and iPad

*2026-09-19.  Status: 0.2.1 ships phase 1; phases 2–4 are the plan.*

A DJ set is an hour or more with the screen pinned on, in the hand.  The
device warming up is the complaint, and heat is the integral of power, so
the question is where the watts go over that hour.  This note fixes the
model, records what has been measured, and lays out the work in the order
of payoff per effort.  Every phase ends with a number from the device, not
a feeling.

## Where the power goes

Five consumers, roughly in the order we can do something about them:

| consumer | what drives it | what we control |
|---|---|---|
| **CPU wakeups** | threads that wake to do nothing (polls, 1 ms ticks). A core that wakes 1000×/s never reaches its sleep states, whatever its "% busy" says | everything |
| **CPU work** | decode + Rubber Band R3, egui, tessellation, Link packets | a little; R3 is ~10 % of a phone core and already the right engine |
| **GPU shading** | fragments × cost per fragment × frames per second. The LCD is shaded at native 3× on the phone: 2868×1320 = 3.8 Mpx per frame, the waveform pass over ~40 % of it, the overview pass with a loop of storage reads per pixel — all of it re-done every frame although only a scroll offset and a playhead moved | everything: resolution, what is recomputed, frame rate |
| **display** | an OLED at the brightness the user chose; the largest single draw on many phones | only the pixels: the UI is already black-ground |
| **radio** | Pro DJ Link: status to every peer at 5 Hz, beats, keepalives, and listening to broadcasts, which keeps Wi-Fi out of power-save | nothing while Link is on; it is the product |

The first two are the cheap ones, and they were the whole of the paused
case.  The third is the playing case, and the plan below is mostly about it.

## What has been measured

Desktop, release build, iPhone layout, Link send on, 30 s windows, percent
of one core (an A17/A18 core is roughly half this i9's single-thread speed):

| thread | playing | paused, 0.2.0 | paused, 0.2.1 |
|---|---|---|---|
| audio-proc (decode + R3) | 4.7 | 4.8, polling 500×/s | 0.0, parked |
| prodj-tx (Link sender) | 0.6 | 0.7, 1 kHz tick | 0.0, deadline sleep |
| frame loop | display rate | display rate | 10 fps |

R3's real-time factor: 3.4 % at 0 % tempo, 6.2 % at −50 % (`make perf`).
The CPU side of a frame is ~0.1 ms.  So on the CPU there is little left to
win while playing; what is left is the GPU, the display and the radio, and
of those only the GPU is ours.

Not yet measured: anything on the device.  0.2.1 adds the meter (below);
the numbers it produces decide the order of phases 2 and 3.

## Phase 1 — stop doing nothing, expensively  *(shipped in 0.2.1)*

- Frame loop paced: paused, no touch, nothing loading → 10 fps
  (`OPENDECK_IDLE_FPS`).  egui-winit's "repaint" answer to
  `RedrawRequested` is no longer turned into a request, which had made the
  loop self-driving.
- Audio thread parks while paused; `AudioHandle::set_playing` unparks it.
- Link sender sleeps to its next deadline when not playing.
- A self CPU meter: `cpu:` line every 10 s (process and per thread) to the
  log and, on iOS, `Documents/opendeck-perf.log`; INFO shows the figure.

**Verify:** paused with the screen on, `cpu:` reads a few percent and the
phone stays cool.  Playing, `cpu:` is the CPU's whole share of the heat.

## Phase 2 — measure the GPU and the thermal state  *(small; do first)*

Two additions to the meter so the rest of the plan is driven by numbers:

1. **GPU time per frame** via wgpu timestamp queries
   (`Features::TIMESTAMP_QUERY`, supported by the Metal backend on Apple
   GPUs): write a timestamp before and after the waveform pass and the egui
   pass, resolve once a second, report "gpu: wave 2.1 ms  ui 0.4 ms  of
   16.7".  That is the GPU's duty cycle, which is its power.
2. **`ProcessInfo.thermalState`** through the UIKit shim (`main.m`, like
   the idle-timer call): nominal / fair / serious / critical, in every
   `cpu:` line.  That is the number the user feels; every later phase is
   judged by how long a set runs before it leaves *nominal*.

## Phase 3 — shade less: resolution, then recomputation  *(the playing case)*

The order inside this phase depends on phase 2, but both halves are worth
doing.

**3a. Render the LCD below native resolution and upscale.**  The waveform
is line art at 8 px per beat; the XDJ's own panel is 800×480.  Nothing on
it needs 460 ppi.

- Render the waveform pass (and the overview) into an offscreen texture at
  1× or 1.5× points instead of 3×: 4× to 9× fewer fragments for the passes
  that cost anything.  Composite at native with a bilinear sample.
- Keep egui at native.  Its cost is a few hundred textured quads and it is
  where the text lives; upscaled text is the one thing a user would notice.
- MetalFX spatial upscaling would give a sharper result for the same input,
  but wgpu does not expose it; it needs a Metal interop step (wgpu-hal
  texture → `MTLFXSpatialScaler` → back).  Try plain bilinear first; if the
  waveform looks soft at 1.5×, that is the moment to pay for MetalFX, not
  before.  Note a Metal-only path means an iOS-only branch in the renderer.
- Measure: GPU ms per frame before and after, same track, same screen.

**3b. Re-sample instead of re-shading.**  From frame to frame the enlarged
waveform changes only by a scroll offset; the overview does not change at
all.  Both are currently recomputed per pixel per frame.

- **Overview:** bake once per track (or per size) into a texture; the
  per-frame pass becomes one texture sample plus the playhead and loop
  overlays.  Removes the loop of up to 18 storage-buffer reads per pixel.
- **Enlarged waveform:** bake the track's columns into a texture strip (one
  texel per column, height = the waveform's pixel height, at the 3a
  resolution) and draw the visible window with a translate.  Per pixel: one
  sample.  Beat marks, cue points, the playhead and the loop are drawn as
  overlays, which they already effectively are.  Zoom levels are separate
  strips or a mip; a track's strip is small (a 10-minute track is ~52 k
  columns × ~200 px).
- Then the whole LCD costs about what a photo viewer costs to scroll, and
  the frame rate is a free choice.

**What this is not:** partial re-rendering of the swapchain.  A Metal
drawable is a whole frame; "only update the waveform" means making the
waveform cheap to draw, not skipping the rest of the UI.  egui's share is
already small (CPU ~0.1 ms, GPU a few quads), so there is nothing to skip.

## Phase 4 — frame rate follows what is on screen

Once 3 is in, the rate is a policy question, not a cost one:

- Waveform visible and playing → the display rate on 60 Hz panels; **60,
  not 120, on ProMotion iPads** (needs a `CADisplayLink` at 60 in the UIKit
  shim that requests the redraw, since winit's iOS backend has no display
  link and Fifo present runs at the panel's maximum).
- BROWSE / INFO / MENU / TAG LIST, or paused: 10–20 fps.  Nothing scrolls.
- The playhead readout and phase meter are fine at 30 if the waveform ever
  needs to drop; the waveform itself should not go below the panel rate
  while it scrolls — smooth scrolling is the one thing this display does
  better than the XDJ (see PERFORMANCE.md), and it is worth its watts.

## Phase 5 — the wakeups that are left

Small, and each one is a test case, so after 2–4:

- **Link sender while playing:** the 1 ms tick exists to hit beat instants
  within a millisecond.  Compute the next beat time and sleep until ~2 ms
  before it, then spin; between beats only the 200 ms status and 1.5 s
  announce deadlines remain.  ~10 wakeups/s instead of 1000, playing.
- **Audio producer:** produce in whole blocks and sleep a block's worth;
  the proportional 0–8 ms sleeps are a leftover from tuning the ring.  Keep
  the cpal buffer at 512 frames — doubling it halves the callback wakeups
  but adds 12 ms of latency the CUE button would feel.
- **Media analysis:** it is work, not waste, but it runs at install time on
  every track in Documents at full tilt.  Run it only while the deck is
  paused (and, if the UIKit shim reports it, charging), one track at a
  time, so it never overlaps a set.
- **Listeners:** the three Link sockets use 500 ms read timeouts (2
  wakeups/s each).  Fine.  Leave them.

## What we will not do

- Swap R3 for R2 or bypass the stretcher at 0 %.  Worth 5 % of a core at
  most, audible, and it complicates the seek path.  Not until everything
  above is in and the meter still points at audio-proc.
- Touch the radio.  Link is the point of the app; the packets are what a
  CDJ sends.  When Link is off (no peers, or a future MENU switch) the
  sender can stop announcing, and that is the whole of the saving.
- Dim the screen.  The user's choice; the black UI is already the best
  case for an OLED.

## Order and expected result

| phase | effort | expected saving while playing | while paused |
|---|---|---|---|
| 1 (shipped) | done | small: ~1500 wakeups/s gone | most of it |
| 2 | half a day | none — it tells us what 3 and 4 will save | — |
| 3a | a day | GPU fragments ÷ 4 to ÷ 9 on the expensive passes | — |
| 3b | two days | the expensive passes become trivial | — |
| 4 | a day | half the frames on ProMotion; 3–6× fewer on list screens | — |
| 5 | a day | ~1000 wakeups/s gone while playing | — |

After 3 and 4 the app's own draw should be small enough that the display
and, when Link is on, the radio are the rest of the phone's warmth — and
those are what every other app on it pays too.

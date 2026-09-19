# Rendering performance


## We render the waveform more smoothly than the XDJ-1000MK2

Observed on the real hardware (2026-08-26): the **XDJ-1000MK2's own waveform
display flickers slightly** during playback. freedj's does not — the whole track
lives in a GPU storage buffer and scrolls in the shader by a single phase-locked
playhead uniform (no per-frame re-upload, no re-analysis in the render loop), and
frames are vsync-paced via the compositor frame callback (see the frame
instrument above). Net result: a steadier scroll than the unit we are cloning.
Worth keeping as the bar — "at least as smooth as the CDJ" — as features land.

## Power on a phone (0.2.1)

Measured on the desktop (release, iPhone layout, Link send on, 30 s windows,
% of one core), the audio pipeline is not what warms a phone:

| thread | playing | paused, 0.2.0 | paused, 0.2.1 |
|---|---|---|---|
| audio-proc (decode + Rubber Band R3) | 4.7 | 4.8 (polling 500×/s) | 0.0 (parked) |
| prodj-tx (Link sender) | 0.6 | 0.7 (1 kHz tick) | 0.0 (deadline sleep) |
| frame loop | display rate | display rate | 10 fps |

R3's own real-time factor is 3.4 % at 0 % tempo, 6.2 % at −50 % (`make
perf`). The CPU side of a frame (snapshot, egui, tessellation) is ~0.1 ms.
What was expensive was never idling: the frame loop requested the next frame
from inside every frame — twice, since egui-winit also answers "repaint" to
`RedrawRequested` — so a paused deck rendered at 60 fps (120 on a ProMotion
iPad) forever, and two threads woke 500–1000 times a second doing nothing.
Wakeups, not work, keep a phone's core out of its idle state.

Left for a later release: the GPU while playing. The big waveform shader
covers ~40 % of the LCD per pixel per frame, and the overview shader loops
over up to 18 storage-buffer reads per pixel to redraw a static image. The
app's own `cpu:` meter (see ios/README.md) tells CPU from GPU on the device:
low `cpu:` and a warm phone means the GPU and the screen.

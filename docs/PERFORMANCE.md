# Rendering performance


## We render the waveform more smoothly than the XDJ-1000MK2

Observed on the real hardware (2026-08-26): the **XDJ-1000MK2's own waveform
display flickers slightly** during playback. freedj's does not — the whole track
lives in a GPU storage buffer and scrolls in the shader by a single phase-locked
playhead uniform (no per-frame re-upload, no re-analysis in the render loop), and
frames are vsync-paced via the compositor frame callback (see the frame
instrument above). Net result: a steadier scroll than the unit we are cloning.
Worth keeping as the bar — "at least as smooth as the CDJ" — as features land.

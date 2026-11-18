# Pi + USB interface: a dropout every 200 ms

**Status:** fixed in `bc6a482` (2026-11-18), on the test Pi 5 the same night.
**Affected:** the Raspberry Pi 5 appliance with a Behringer UCA202 (ALSA,
`hw:CODEC` through `plug`).  Any ALSA device with ~25 ms periods could hit
it; PipeWire on a desktop asks for small chunks and never showed it.
**Reported by:** Dan, "I'm getting audio dropouts on the rpi with the uca202
interface".

---

## Symptom

Short gaps in the sound while playing, regular, like a stutter.  The deck's
log:

```
AUDIO UNDERRUN: 1215 events, 273508 samples of silence (3101.0 ms total) — ring starved
AUDIO UNDERRUN: 1216 events, 273740 samples of silence (3103.6 ms total) — ring starved
AUDIO UNDERRUN: 1217 events, 273972 samples of silence (3106.3 ms total) — ring starved
```

One event every 200 ms, 232 samples (116 frames, 2.6 ms) each, starting
when PLAY was pressed and never stopping.

## What it was not

- Not CPU: the audio thread used 20 % of a core (`cpu: 28.3% of one core —
  audio-proc 19.5`), no throttling (`get_throttled` showed only the boot
  undervoltage flag), 50 °C.
- Not stems: none split on the Pi (no model there).
- Not a hiccup: the rhythm was exact, which points at arithmetic, not load.

## The mechanism

The device: ALSA period 1 103 frames (25 ms), buffer 4 410 frames (from the
`hw_params` of `card2`).  200 ms is 8 periods.

The deck: a ring of 8 192 samples (4 096 frames, ~93 ms) between the
processing thread (time-stretch, mix) and the device callback.  The
processor only made another 512-frame block when **at least 4 096 samples
were free** (`BACK_PRESSURE_SLOTS`), the room one block can need at the
slowest speed (0.25x, four times the output).  So at normal speed the ring
sat about half full: 4 096 to 5 120 samples.

When the callback was woken late, ALSA asked for **two periods at once**:
2 x 1 103 frames = 4 412 samples, more than half a ring.  The difference,
~232 samples, was filled with silence.  On a desktop PipeWire asks for a few
hundred frames at a time, so half a ring was always enough.

## Proving it

1. **On the Pi:** the ring made larger (`OPENDECK_RING_MS=200`, 17 640
   samples): underruns fell from one every 200 ms to two in five minutes,
   both at a track load and a CUE return (where the deck empties the ring
   on purpose).
2. **In a test:** a stand-in device that takes 2 206 frames every 50 ms
   (`Output::TestPulls`, `a_device_taking_two_periods_at_once_never_starves`):
   **13 underruns in 2.5 s** with the old rule, the same 200 ms rhythm as
   the Pi, and **0** with the fix.

## The fix

The free room the processor waits for now depends on the speed
(`back_pressure_slots`): twice what one block puts out at the current speed
(the time-stretcher's output comes in uneven chunks), capped at the old
0.25x worst case.  At 1x that is 2 048 samples, so the ring runs about 3/4
full and covers a two-period request; slow speeds keep the worst-case room.
The ring stays 93 ms, so the tempo fader is as responsive as before.

## Side effects

- More audio is in flight at 1x (~70 ms instead of ~46 ms).  CUE latency is
  unaffected (a jump flushes the ring); `cue_latency_tests` measure 8.7 ms
  from CUE or PLAY to sound.
- The heard-position estimate ignores a loop's wrap, so right after a wrap
  it reads just before the loop's start for as long as the ring holds; with
  the fuller ring a loop test caught that under load
  (`slip_off_during_a_loop_exits_without_returning` now lets the ring play
  out first).  The dip itself is a TODO.

## Lessons

- A steady underrun rhythm is a sizing bug, not a performance one: look at
  the device's period and buffer (`/proc/asound/cardN/pcm0p/sub0/hw_params`)
  against the ring.
- Back-pressure sized for the worst case starves the common case.
- Test with a device that behaves like the real one (two periods at once),
  not one that pulls neat 10 ms chunks.

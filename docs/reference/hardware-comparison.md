# OpenDeck vs the XDJ-1000MK2: performance scorecard

Status: **partial** (2026-10-14). One row is a true head-to-head measurement,
one is an observation on the unit plus a measurement on ours, the rest are
measured on ours and still open on the XDJ. The open rows need the real unit
on the desk, a phone that films at 240 fps, and a two-channel recorder — the
procedures are below so they come out comparable. Add a row when something
new is measured; replace an open cell with the number and the date.

This page is the source for the blog post
`audiodestrukt.com/blog/posts/pi-vs-xdj-1000-performance.html`. The post
quotes this table; update here first.

Nothing here comes from opening the unit or reading its firmware. The XDJ is
measured from the outside: what the screen shows, what the network carries,
what comes out of the jack.

## Scorecard

| What | XDJ-1000MK2 | OpenDeck | How |
|---|---|---|---|
| Beat packet timing, sd of inter-beat interval at one receiver | 1.33 ms | 1.23 ms (desktop build) | Same receiver, same run, 2026-08-26. CHANGELOG "Beat timing at Pioneer's level". Both at 44.1 kHz, both playing at 0 %. Pi 5 number: **open** |
| Waveform scroll | Visible flicker during playback (observed 2026-08-26; DJ TechTools review, 2018, calls the display refresh low) | Pi 5: 60 fps, frame dt sd 0.12 ms, 0 stalled in the sample; Pi 4: 30 fps (display mode), 0 stalled | Frame instrument (`RUST_LOG=opendeck=debug`, `frame: dt` lines), BENCHMARKS.md. XDJ update rate in frames/s: **open** (procedure 1) |
| Track load, press to PLAY ready | **open** (procedure 2) | Pi 5: 0.7 s decode + waveform + BPM (5:13 MP3, 2026-08-26). Desktop 2026-10-14: 0.53 s cold (0.23 s decode, 0.30 s waveform + BPM), 0.37 s warm (grid from cache) | Log lines `decoded … in`, `waveform done … in`. Note the asterisk: the XDJ reads rekordbox's analysis off the stick; OpenDeck analyses from scratch unless cached |
| Power-on to ready | **open** (procedure 3) | Desktop: 0.87 s process start to first frame cold, 0.65 s warm (Xvfb, 2026-10-14). Pi 5 app start: **open**. OS boot not counted and not comparable yet | Timestamped log; first `frame: dt` line |
| CUE press to sound | **open** (procedure 4) | Press to seek applied: a few ms; release to paused seek: ≤ 4 ms; 3 ms fade on stop only, none on start. Press-to-sound at the jack: **open** (procedure 4) | cue_hold recipe in `cue-latency-confirmed-good` memory / transport-rules.md; null-sink recording |
| CPU while playing at 0 % | not observable from outside | Pi 5 ~0.6 of one core total; Pi 4 ~1.0 core (R3 timestretch ~72 % of a core) | `/proc` per-thread utime, BENCHMARKS.md |
| Memory | not observable | 249 MB RSS on the Pi 5 (whole track decoded to RAM) | `ps -o rss` |
| Price | €1,299 SRP at launch, 2016 (DJworx) | Pi 5 ~$80 plus a screen, PSU, cooler | — |

## Procedures for the open rows

Do each one on both machines, same day, same track, same USB stick where
possible. Ten repeats, report the median and the spread.

### 1. Screen update rate (waveform)

Film the LCD with a phone at 240 fps (iPhone: Settings → Camera → Record
Slo-mo → 1080p at 240 fps) while the track plays at 0 %, zoomed to the default
waveform view. Step through 240 frames (one second) in a frame-stepping app
and count how many distinct waveform positions appear. That count is the
update rate. Do the same at the Pi's HDMI screen and the iPad. Note the LCD's
own refresh may be higher than the waveform's update rate; the waveform is what
the DJ sees move.

### 2. Track load

Same phone, 240 fps, framed on the screen and the LOAD control. Start: the
frame where the finger lands on the track in the browser (XDJ: the touchscreen
track row and LOAD; OpenDeck: the BROWSE list and LOAD). End: the first frame
where the full-track overview and the zoomed waveform are both drawn and the
time readout shows 00:00. Count frames, divide by 240. Use a track rekordbox
has already analysed on the stick; for OpenDeck, run once with a fresh
`XDG_DATA_HOME` (cold) and once again (warm), and report both.

### 3. Power-on to ready

Stopwatch or the same phone. Start: power switch on. End: the browser is
usable (XDJ: USB source lit and the list responds; Pi: the window is up with
the waveform area drawn). For the Pi report two numbers: OS boot to desktop
and app launch to first frame. Until it boots straight into the deck, the OS
number is the honest one.

### 4. CUE press to sound

Two-channel recording into a DAW at 48 kHz: channel 1 is a small mic taped
beside the CUE button (the tact switch click is the time reference); channel 2
is the line out. Set a cue on a sharp transient (a kick on a downbeat). Press
CUE ten times. In the DAW, measure from the click transient on channel 1 to the
first sample of the kick on channel 2. On OpenDeck do it with a mouse click on
the desktop (the mouse's microswitch clicks), with the DJ2Go's CUE pad on the
Pi, and with a stylus tap on the iPad (a fingertip is too quiet). Report the
median and the spread. Expect the device buffer to dominate on both.

### 5. Beat timing on the Pi (to close the first row)

Same as the 2026-08-26 run (CHANGELOG, "Beat timing at Pioneer's level"): one
receiver captures beat packets on UDP 50001 from both players in the same run;
sd of the inter-beat interval at 0 %. The desktop number is 1.23 ms; the Pi
number replaces it in the table. Follow the no-master rule: the Pi sends beats
as a plain player, never `OPENDECK_MASTER=1` while the XDJ is on the network.

## Reproduction, OpenDeck side (desktop, 2026-10-14)

```bash
pactl load-module module-null-sink sink_name=opendeck_test
XDG=$(mktemp -d)       # cold cache; run twice for warm
env -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 WINIT_X11_SCALE_FACTOR=1 \
  OPENDECK_WINDOW=1024x600 XDG_DATA_HOME=$XDG OPENDECK_SERVE=0 OPENDECK_LINK_SEND=0 \
  PULSE_SINK=opendeck_test RUST_LOG=opendeck=debug,wgpu=off,naga=off,egui=off \
  timeout 10 xvfb-run -a -s "-screen 0 1100x700x24" target/release/opendeck techno.mp3 2>&1 \
  | python3 -u -c 'import sys,time; t0=time.monotonic()
for l in sys.stdin: print(f"{(time.monotonic()-t0)*1000:8.1f} {l.rstrip()}")' > run.log
grep -E "decoded|waveform done|GPU:|present mode" run.log
grep -m1 "frame: dt" run.log            # first frame, ms since launch
```

The Pi recipe is in BENCHMARKS.md. Frame pacing under Xvfb is not vsynced
(Fifo on a virtual display), so take frame-pacing numbers from the Pi or a real
display only.

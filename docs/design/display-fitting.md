# Fitting the deck to a display

How OpenDeck gets its screen onto a panel of whatever size and shape the
hardware has, why that was harder than "the UI is resizable" suggests, and
what to check when a new display turns up.  Written after bringing up the
official Raspberry Pi 7" touch display (800 × 480) on a Pi 5, 2026-11-16.

## The matrix

Every combination of these has to look right:

| Layouts (the design's own shape) | Size | Aspect |
|---|---|---|
| XDJ-1000MK2 screen | 1024 × 600 | 1.71 |
| CDJ-3000 screen + key columns ("side" layout, the default) | 1620 × 720 | 2.25 |
| CDJ-3000 bare screen (on the unit, `OPENDECK_SUBUCOM`) | 1280 × 720 | 1.78 (16:9) |
| CDJ-3000 whole deck (iPad, `--faceplate`) | 1255 × 1721 | 0.73 |
| Phone pages | 932 × 430 | 2.17 |

| Displays we have met | Pixels | Aspect |
|---|---|---|
| Raspberry Pi 7" touch display (DSI) | 800 × 480 | 1.67 |
| 7" HDMI panels, the XDJ's own LCD size | 1024 × 600 | 1.71 |
| CDJ-3000 LCD | 1280 × 720 | 1.78 |
| HDMI monitors | 1920 × 1080 and up | 1.78 |
| Desktop windows, iPad, phones | anything | anything |

| Renderers | Used on | Fullscreen |
|---|---|---|
| wgpu (Vulkan / Metal / GL) | desktop, Pi 4/5, iOS, Android | no option (a window, or the whole screen on mobile) |
| SDL (`OPENDECK_RENDERER=sdl`) | Pi Zero W, and any Pi that wants fullscreen | `OPENDECK_FULLSCREEN=1` |

Only one row of that matrix matches a layout exactly (the 1024 × 600 panel
with the XDJ screen).  Everything else is a fit.

## Why it was hard

### 1. "Resizable" means any size, not any shape

The window has always resized, and that hides the real rule: every
layout scales **uniformly**.  `fit_contain` finds the biggest copy of the
layout, at its own shape, that fits the window, and the rest is bars.
Drag a desktop window to a different shape and you get bars too; on the
800 × 480 panel they just never go away.

The CDJ-3000 screen code (`cdj3000.rs`) is written against the unit's
1280 × 720 pixels with one scale, `u = screen.height() / 720`, used for x,
y, text size and line width alike: about 260 places.  `layout()` itself
keeps separate x and y scales, but nothing drawn from it does.  Handing it
a box of another shape puts things off the right edge; it does not
stretch.

### 2. The source design has a fixed shape, and so does the panel

The real CDJ-3000 screen is 16:9.  At 480 pixels tall a 16:9 screen needs
853 pixels across, and the panel has 800.  Add the key columns (SOURCE,
BROWSE, … left; BACK, TAG TRACK, rotary right) and the side layout is
2.25:1 against a 1.67:1 panel.  There is no answer that keeps the shape,
keeps the keys and fills the panel.  Choices:

- letterbox (bars; the old behaviour, 62 px top and bottom here);
- drop the keys (16:9 screen, still 15 px bars);
- distort (what we did: see below);
- design a layout for the panel's own shape (the real fix for a product
  that ships on one panel).

Dan chose: keep the keys, fill the panel, stretch the screen.

### 3. Scaling a finished picture blurs it

The first fullscreen path rendered the design at its own size
(1620 × 720) and let the GPU shrink the finished frame to the panel
(× 0.49, linear).  Every output pixel is an average of about four, so
text and 1 px lines go to mush.  Rendering at the panel's own pixels
(`OPENDECK_WINDOW=800x480`) fixed the sharpness but kept the bars.

Rule: **shrink geometry, never a finished image.**  Scale vertex
positions and let lines and text be rasterised at the output's pixels.

### 4. Squeezing geometry is not free either

The fill (below) lays the deck out at 1080 × 480 (the side layout's
shape at the panel's height) and squeezes x by 0.741 as it draws.  Shapes
and lines come out sharp.  Two things do not:

- **Text.**  Glyphs come from egui's font atlas, rasterised at layout
  size, then drawn 74 % as wide with bilinear filtering and no mipmaps.  A
  1 px stem lands between texels and fades out: MENU reads MENJ, TEMPO
  reads TEMPC, REMOVE reads REMCVE.  Minifying a glyph loses strokes;
  magnifying one only softens it.  Fix (not done yet): rasterise text at
  the output's width (`pixels_per_point` = the squeeze) and stretch it
  1.35× vertically instead.
- **Round things.**  The rotary knob comes out an oval.  Fix (not done):
  draw it wider by 1/squeeze before the squeeze.

### 5. Motion shows what a still screenshot hides

The enlarged waveform on the SDL path scrolls in **whole output pixels**:
it is cached and shifted, so a frame only draws the columns that came in
(about a tenth of the CPU of redrawing it, which a Zero W needs).  On
this panel at the default zoom it moves about 29 px/s, so at 60 fps it
steps one pixel every second or third frame: a 2-2-3 cadence that reads
as a slight judder.

The beat ticks above and below it are drawn by egui, at the exact
playhead, which moves a fraction of a pixel every frame.  A 1 px line at
a fractional x is smeared over two pixels by anti-aliasing, so each tick
flickered bright/dim/bright, and drifted against the bars it marks.  The
squeeze made it worse: the tick was 0.74 output pixels wide.

Fix: anything that scrolls with the waveform has to use **the same pixel
grid as the waveform**.  `cdj3000::OutPx` quantises the playhead exactly
as the waveform cache does (one output pixel's worth of columns), puts
each tick on an output pixel centre, and makes it at least one output
pixel wide.  It needs to know the squeeze, so the renderer publishes it
(`renderer::squeeze_x()`).  Cue, memory and hot-cue markers use it too.

`OPENDECK_WAVE_SCROLL=smooth` redraws the waveform at the exact
sub-pixel playhead every frame instead (about 80 % of a Pi 5 core).  The
cost of smooth scrolling is different flicker: each output pixel spans
about 5 waveform columns, the waveform takes one of them, and which one
changes as it moves.  Taking the peak of the columns under each pixel
would fix that; not done.

### 6. Two renderers, one picture

The wgpu path (shader) and the SDL path (`waveform_cpu.rs`) must draw the
same waveform, checked by `tools/render-parity.sh`.  Fill exists only on
SDL, because only SDL has a fullscreen mode; the wgpu path on a Pi 5
would need a winit fullscreen option first, then the same squeeze in its
egui pass, the shader's viewports and its pointer mapping.

### 7. Vsync is the compositor's, not ours

Under labwc (the Pi desktop) SDL's vsync waits for the compositor's frame
callback, and the compositor flips on the panel's vblank: no tearing, 60
fps steady here.  On KMSDRM (no desktop, the Zero W's fast path) SDL owns
the flip.  The panel ran at 60.029 Hz; SDL reports whole hertz, so the
frame interval is assumed 16.667 ms.  Harmless, but frame-time logs are
only as exact as that.

## What fill does (SDL, `sdl_backend::Fill`)

When `OPENDECK_FULLSCREEN=1` and the panel is **no taller than the
design** (a small panel):

1. Lay the deck out at the panel's height in the design's own shape:
   800 × 480 → 1080 × 480 for the CDJ side layout.  egui sees that size,
   so every layout is unchanged and uniform.
2. Draw every egui vertex and clip rect with x × `xs` (0.741), y × 1.
3. Draw the two waveforms straight at their squeezed size: their rects
   are squeezed before rasterising, so the waveform is drawn at the
   panel's own pixels, not shrunk.
4. Map mouse x back by 1 / `xs`.  Touch comes as 0..1 of the window and
   maps by itself.
5. Publish `xs` for `OutPx`.

On a panel of the design's own shape `xs` is 1 and this is exactly 1:1.
On a display **bigger** than the design (an HDMI monitor) the old path
stays: render at the design size, GPU-scale, letterbox, so a 1080p
monitor costs a small board no more CPU than its 7" panel.

`OPENDECK_WINDOW=WxH` changes the design size, and so the shape fill
aims for: `OPENDECK_WINDOW=800x480` gives a 1:1 panel with the CDJ
layout letterboxed inside it.  Leave it unset for fill.

## Bringing up a new display: checklist

1. **Does the system see it?**  `/sys/class/drm/card*-*/status`,
   `wlr-randr`, backlight under `/sys/class/backlight`.  A dark panel that
   is "connected, enabled, brightness 255" is power: the Pi 7" display
   needs its 5 V jumper.  `grim` shows what is being sent to it.
2. **Its pixels and shape** against the layout's: the startup log says
   `fullscreen: WxH output, laid out at … squeezed x…`.  A squeeze far
   from 1 (below ~0.85) means text damage until the text fix lands.
3. **Sharpness**: no step should shrink a finished frame.  Look at 1 px
   lines and small text on the panel itself, not a screenshot scaled for
   your monitor.
4. **Motion**: play a track and watch the waveform, the beat ticks and the
   overview for at least a minute.  Flicker in anything that scrolls means
   it is off the waveform's pixel grid.
5. **Input**: tap the corner keys and the waveform; a squeeze or letterbox
   that the pointer mapping does not know about puts taps beside their
   targets.
6. **Frame rate and CPU**: the `frames:` log lines (fps, spikes), `top`.
7. **Both renderers**, if the board can run both.

## Not done yet

- Text drawn at the output's width (MENU / MENJ).
- A round rotary under squeeze.
- Fill on the wgpu renderer (it has no fullscreen option at all).
- Waveform peaks under smooth scroll (several columns per pixel).
- Touch on the 7" panel is detected (`edt_ft5x06`) but untested on the
  deck.
- A layout designed for 5:3 panels, if we ship one.

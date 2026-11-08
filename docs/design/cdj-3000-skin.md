# CDJ-3000 skin

Status: plan + phase 0 in progress (2026-11-08).

Goal: a second, switchable deck UI that reproduces the Pioneer CDJ-3000's
player screens. The goal is feature-complete with the unit. The XDJ-1000MK2 UI
stays in full; the skin is a setting, not a fork.

References:

- `docs/reference/cdj-3000-features.md` — the full inventory from the manual:
  every screen, callout, control, feature and setting, with page refs.
- `reference/pioneer/cdj3000-emu/*.png` — live captures from cdj3k-emu
  (control API `lcd main` / `lcd jog`, 1280×720 / 320×240). These are renders
  of AlphaTheta's UI, so they are **local only** (gitignored with the rest of
  `reference/pioneer/`). The emulator is the ground truth for layout and
  behaviour. Capture the **player mode** screens (SOURCE → USB), not Control
  Mode: in Control Mode the CDJ draws a host variant of the playback screen
  that differs in several ways (elapsed vs REMAIN time, no A.HOT CUE badge, no
  ZOOM/GRID pill, a QUANTIZE box, positive ruler times).
- Emulator USB stick: `media attach <img>` with an MBR-partitioned FAT32 image
  (an unpartitioned image does not mount). After attaching, run `sh blockdev
  --rereadpt /dev/sdb`; if the stick still does not appear, eject and attach
  again. Without a rekordbox database only FOLDER browsing exists. PLAYLIST,
  TRACK FILTER and the category lists need a rekordbox export on the stick.

## Ground rules

1. **The screen is the unit's screen.** The layout is measured from the
   1280×720 captures and drawn at 16:9, letterboxed in any window. Nothing that
   is a hardware control on the CDJ gets drawn on the screen (project rule:
   screen fidelity, button state on a separate controls surface).
2. **One deck, two views.** Both skins read the same `DeckSnapshot` and emit
   the same `Event`s; `DeckApp` state is shared. Switching skins mid-set loses
   nothing.
3. **Hardware buttons come from surfaces.** SOURCE, BROWSE, TAG LIST,
   PLAYLIST, SEARCH, MENU, hot-cue pads A–H, BEAT JUMP ◀▶, 4/8 BEAT LOOP,
   CALL/DELETE, SLIP, KEY SYNC… arrive as `ControlEvent`/`UiEvent`s from:
   - the keyboard;
   - a CDJ-3000 faceplate (a later phase; a touch controls page like the XDJ
     faceplate);
   - MIDI / I2C surfaces;
   - **a real CDJ-3000 over `surface::lanhid`** (its buttons drive the skin).
4. Features the XDJ skin already has (browse, tag list, info, menu, hot cues,
   loops, beat jump, slip, quantize, grid adjust, colour modes, needle
   search…) are re-presented, not re-implemented.

## Architecture

- `settings::Skin { Xdj1000Mk2, Cdj3000 }`, persisted in `settings.json`.
  Switch it with a MENU row, `OPENDECK_SKIN=cdj3000` (dev/screenshots) or a
  keyboard toggle.
- `crates/app/src/cdj3000/`:
  - `layout.rs` — regions in fractions of a 1280×720 screen, measured from the
    captures.
  - `draw.rs` — egui painting per view.
  - Later, one module per screen family.

  `screen.rs` stays the XDJ skin. Small shared helpers (text fitting, key
  faces) can move to a common module when a second user needs them.
- `render_frame` picks the screen rect and layout per skin. For the CDJ skin
  the rect is `fit_contain(16:9, window)` and the shader viewports come from
  the CDJ layout. Phase 0 is screen-only: no faceplate, portrait or phone
  variant. Phone keeps the XDJ layout until the CDJ skin has its own.
- View state: the CDJ's modal structure differs (touch overlays on the
  playback screen, full-screen lists with a status strip, an INFO overlay), so
  the skin gets its own view enum (`Cdj3kView`) next to `ScreenMode`. Both map
  onto the same deck actions.
- Waveform renderer: a style switch in `WaveformParams` (both the wgpu shader
  and `waveform_cpu`) for the CDJ look: grid ticks in rows above and below the
  waveform with red downbeats, bar numbers, the overview's played and cue bar,
  and hot-cue letters above the overview. The colour modes already exist; the
  CDJ default is BLUE.

## Gap list (CDJ-3000 vs freedj today)

Size: S < 1 day, M 1–3 days, L > 3 days.

| Area | Item | Status | Size |
|---|---|---|---|
| Playback screen | Layout (header, waveform field, info block, overview with ruler) | phase 0 | M |
| | Beat countdown (bars/beats to the next cue, two rows) | missing | S |
| | ZOOM / GRID pill + Grid Adjust touch buttons (CDJ set) | logic exists | S |
| | BEAT LOOP panel 1/4–32 (paged) | logic exists | S |
| | BEAT JUMP panel ◀▶ ×(1/2–4, paged) | logic exists | S |
| | KEY SHIFT panel | needs key shift | M |
| | INFO overlay (player tabs 1–6, rating, details) | partial (XDJ INFO) | M |
| | Touch Cue / needle search on overview | logic exists | S |
| | A.HOT CUE / AUTO CUE / GATE CUE captions | partial | S |
| Lists | BROWSE two-pane (list + preview/INFO pane), status strip | XDJ browse exists | M |
| | SOURCE screen (devices, LINK, Control Mode) | partial (source keys) | M |
| | TAG LIST (+ GLOBAL) | exists (XDJ) | S |
| | PLAYLIST + Playlist Bank | needs rekordbox playlists in UI | M |
| | Category lists (artist, album, key, date added, history, matching) | rekordbox reader exists | M |
| | SEARCH screen + keyboard | missing | M |
| | TRACK FILTER | missing | M |
| | LOAD PREVIOUS TRACK (history) | missing | S |
| | Artwork in header, lists, INFO, jog | missing (tags have it) | S |
| Settings | SHORTCUT screen (time mode, auto cue, colour, position…) | partial (MENU) | M |
| | UTILITY list (CDJ set) | partial | M |
| | Waveform Current Position CENTER/LEFT | missing | S |
| Deck features | Hot cue colours A–H (RGB), comments | positions only | M |
| | Gate Cue, Smart Cue | missing | S |
| | Key detection + KEY readout (green when compatible) | missing | L |
| | KEY SHIFT (semitones) + KEY SYNC | missing (stretcher can) | M |
| | Beat Jump beat value 1/2–64, Quantize beat value | partial | S |
| | Slip reverse lever, reverse | missing | M |
| | Active loop, emergency loop, loop slip | partial | M |
| | Vinyl speed adjust (touch/release) | planned (varispeed) | M |
| | 6 players on PRO DJ LINK | 4 | M |
| Jog | Jog-centre display (artwork, ring marker, cue marker, indicators) | XDJ draws a jog centre | M |
| Surfaces | CDJ-3000 faceplate / controls page; full CDJ HID button map → skin events | partial (lanhid map) | M |

3000X-only items (Wi-Fi, cloud, streaming, NFC) are out of scope.

## Phases

0. **Skin switch + playback screen.** `Skin` setting, MENU row and env var;
   16:9 letterboxed CDJ layout; header (source icon, artwork, title,
   length/BPM/key, info), waveform field (playhead, bar numbers, ZOOM pill,
   beat countdown), info block (PLAYER, TRACK, A.HOT CUE, REMAIN/time, SINGLE/
   CONTINUE, TEMPO + range, BPM + MASTER, BEAT JUMP, MT, KEY) and overview +
   ruler. Validate side by side against `playback-usb.png`.
1. **Playback touch layer.** BEAT LOOP / BEAT JUMP / KEY SHIFT panels, ZOOM–
   GRID + Grid Adjust, INFO overlay, Touch Cue. CDJ waveform style in the
   renderer.
2. **Lists.** SOURCE, BROWSE (two-pane + INFO pane), TAG LIST, LOAD PREVIOUS
   TRACK, a shared status strip. Keyboard bindings for the hardware buttons;
   the CDJ HID map for SOURCE/BROWSE/… on a real or emulated unit.
3. **Library depth.** PLAYLIST + bank, categories, SEARCH + keyboard, TRACK
   FILTER (rekordbox library via the existing reader; test against an export
   on the emulator stick).
4. **Deck features the CDJ adds.** Hot cue colours, Gate/Smart Cue, beat-jump
   and quantize values, key detection, KEY SHIFT/SYNC, slip reverse, active
   loop.
5. **Jog display + faceplate.** Jog-centre LCD (320×240 reference), a CDJ-3000
   controls page/faceplate.
6. **SHORTCUT + UTILITY** settings parity.

Each phase ends with emulator-vs-freedj captures of the same state side by side.

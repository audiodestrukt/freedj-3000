# CDJ-3000 skin

Status: phase 0 done (2e77df5); phase 1 in progress (2026-11-08).

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
| | ZOOM / GRID pill + Grid Adjust touch buttons (CDJ set) | done (phase 1) | S |
| | BEAT LOOP panel 1/4–32 (paged) | logic exists | S |
| | BEAT JUMP panel ◀▶ ×(1/2–4, paged) | logic exists | S |
| | KEY SHIFT panel | needs key shift | M |
| | INFO overlay (player tabs 1–6, rating, details) | partial (XDJ INFO) | M |
| | Touch Cue / needle search on overview | logic exists | S |
| | A.HOT CUE / AUTO CUE / GATE CUE captions | partial | S |
| Lists | BROWSE two-pane (list + preview/INFO pane), status strip | XDJ browse exists | M |
| | SOURCE screen (devices, LINK, Control Mode) | done (phase 2) | M |
| | TAG LIST (+ GLOBAL) | exists (XDJ) | S |
| | PLAYLIST + Playlist Bank | needs rekordbox playlists in UI | M |
| | Category lists (artist, album, key, date added, history, matching) | artist/album/track/key/playlist/folder done (local stick) | M |
| | SEARCH screen + keyboard | missing | M |
| | TRACK FILTER | missing | M |
| | LOAD PREVIOUS TRACK (history) | done (phase 2) | S |
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

## Progress notes

- **Phase 1 (part 1).** BEAT LOOP and BEAT JUMP panels, the loop marker,
  INFO and the CDJ waveform look are done.
  - Behaviour read off the unit:
    - A panel shrinks the waveform field (wave y 228–318) and adds a row of
      eight keys (y 383–466) plus a 1|2 page switch. Each panel remembers its
      page.
    - BEAT LOOP page 1 is 1/4…32 with orange edges; page 2 is 3/4, 1, 3/2, 3,
      5, 6, 7, 9 with white edges. The running length's key fills orange, and
      tapping it again ends the loop.
    - BEAT JUMP page 1 is 1/2–4, page 2 is 8–64, each a ◀◀/▶▶ pair.
    - An active loop draws amber shading with no in/out lines, an orange
      start triangle and "↻ n", and keeps them after the panel closes.
    - INFO is a full-width overlay over the field (player tabs 1–6, track
      details); the info block stays live under it.
  - The CDJ look in both renderers (`Viewports::cdj`, `WaveformParams::style`):
    no in-waveform ticks, amber loop shade, bars ×1.5 to fill the field.
  - Zoom factor 5.3: 18 px per beat at 128 BPM, measured on the unit.
  - KEY SHIFT stays unwired until the deck can shift key. EDIT RATING and the
    other players' tabs are drawn but inert.
  - Dev: `OPENDECK_CDJ_PANEL=loop|jump|info`.
- **Phase 2 (part 1).**
  - The CDJ skin renders the deck's own screen modes, so both skins share one
    navigation model and any surface (keyboard, MIDI, a real CDJ) drives either.
    INFO moved onto `ScreenMode::Info` (ⓘ and ✕ toggle it).
  - **BROWSE:**
    - The header has the source tile, a drawn ← BACK key and the folder title.
    - The category column (FOLDER), then the list pane: 60 px header, 48 px
      rows alternating grey/black, a blue highlight and a scrollbar.
    - The right pane previews the highlighted folder (`Browser::preview`,
      filesystem folders only, cached per folder) or shows INFO + LOAD for a
      track.
    - A tap selects a row; a second tap loads it.
  - **TAG LIST** and **UTILITY** (the deck's settings) use the same list
    renderer.
  - Every list screen keeps the status strip (PLAYER, mm:ss, mini overview
    drawn by the shader, MT, range, tempo, BPM).
  - New keys for the CDJ's hardware screen buttons: L = TAG LIST, I = INFO,
    U = MENU/UTILITY (B = BROWSE as before).
  - Dev: `OPENDECK_SCREEN=browse|taglist|menu|info`,
    `OPENDECK_BROWSE_SELECT=n`.
  - The INFO pane shows the highlighted track's tags (title, artist, album,
    BPM, key, date, comment) and cover.
- **Phase 2 (part 2).**
  - **SOURCE** (`ScreenMode::Source`, key O): one 90 px row per medium on a
    92 px pitch. The rows are the folder freedj started in, each mounted stick,
    then each LINK source (badge = player number). The loaded track's source
    is green with a bracketed "Loaded"; the highlighted row is blue with a ›.
    The right panel shows Songs / Playlists (from a read rekordbox export, 0
    without one, as the unit shows) and Total / Available from `statvfs`.
    Picking a row opens it in BROWSE (`Browser::open_source`). The XDJ skin has
    no SOURCE screen and shows its browser instead.
  - The CDJ's BROWSE key reaches freedj over lanhid as a level (0x05/0x80), so
    a real or emulated CDJ opens and closes BROWSE. Its SOURCE key sends no
    bit in Control Mode; it only clears that level.
  - Dev: `OPENDECK_SCREEN=source`.
  - **LOAD PREVIOUS TRACK** (`ScreenMode::History`, key H; MENU short press
    on the unit): a pop-up over the right of the waveform screen (568×464 at
    x 712, grey 128 frame, 58 px header). It has 50 px rows of cover + title,
    alternating 25/0 grey, with two empty value columns at x 1093/1193. Tracks
    are listed newest first and the loaded track is never listed. Nothing is
    highlighted until the selector turns. The deck keeps 100
    (`record_load`), in memory only, like the unit's power-cycle reset. XDJ
    skin: no such screen, H does nothing.
  - Dev: `OPENDECK_SCREEN=history`, `OPENDECK_HISTORY=a.mp3:b.mp3`.
  - Still to do: categories for rekordbox and LINK sources.
- **Phase 3 (part 1): rekordbox categories.** Measured from Dan's rekordbox
  stick on the emulator (`browse-rekordbox-artist.png`,
  `browse-rekordbox-track.png`).
  - **The rekordbox root** now lists the CDJ-3000's categories: ARTIST, ALBUM,
    TRACK, KEY, PLAYLIST (always, as the unit does), HISTORY and MATCHING
    (shown, not walkable yet) and FOLDER (the stick's files). The XDJ skin's
    list shows the same rows.
  - **ARTIST / ALBUM / KEY** open name lists (`Loc::RbNames`), and a name
    opens its tracks.
  - **Reader:** it now resolves album, key and date added
    (`RbTrack::album/key/date_added`).
  - **CDJ skin:**
    - The category column is 90×66 tiles on a 67 px pitch, the open one blue;
      a tap opens it (`UiEvent::BrowseCategory`).
    - Name lists carry their category's icon and preview the highlighted
      name on the right: an artist's albums, else its tracks.
    - Track lists are one full-width table: PREVIEW 92–331, TRACK ▲ [count],
      BPM 1080–1189, KEY 1190–1280, and LOAD in the highlighted row.
    - Opening a rekordbox stick from SOURCE lands on ARTIST, as on the unit.
  - The PREVIEW column draws each track's blue preview (PWAV from its
    `.DAT`, `RbAnalysis::preview`) bottom-anchored on black, and its cover
    (artwork table → `RbTrack::artwork_rel`). Both are read once per file and
    cached. Covers are unchecked: the demo export has none.
  - Not yet: sorting (the sort key is drawn, inert), HISTORY, MATCHING,
    LINK/dbserver sources in the column, the "H" (hot cues) mark by the ♪.
  - Dev: `OPENDECK_SCRIPT` browse actions (`screen=browse; open=rekordbox;
    category=2; select=0`).
- **Phase 1 (part 2): GRID ADJUST.** A long rotary press on the unit (key A,
  or a tap on the pill) swaps in the GRID key row: SNAP GRID (CUE) = beat 1
  on the cue (`GridAdjust::ShiftCue`), SHIFT GRID (dim: it commits a sync
  pitch-bend offset; see below), ◀◀ 1/2 and ▶▶ 1/2
  (`GridAdjust::HalfBeat`), and RESET, lit once the grid is edited
  (`DeckSnapshot::grid_edited`). The field shrinks as for a touch panel and
  the pill boxes GRID ("– ZOOM [GRID]"). The selector moves the grid 1/64 beat
  per detent, as before.
  - SNAP GRID (read off the unit with a rekordbox-analysed track): dim while
    the only cue is the one placed at load; lit once a cue is set by hand
    (pause off the cue + CUE). It then stays lit whether playing, paused, on
    or off the cue, and after a snap. freedj follows this with
    `DeckSnapshot::cue_set` (set by a new CUE or a called memory point, cleared
    on load).
  - SHIFT GRID lights while SYNC holds a phase offset left by a jog bend and
    folds it into the grid; freedj's SYNC now holds that offset instead of
    pulling back to 0. Full findings, data and the test rig:
    [cdj-grid-adjust.md](cdj-grid-adjust.md).
  - The manual also lists ×2, ×1/2, Enlarge and Reduce. The emulated firmware
    shows only the five keys, for analysed and unanalysed tracks alike, so
    freedj draws five.


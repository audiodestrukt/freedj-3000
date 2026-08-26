# XDJ-1000MK2 screens — what the unit shows

Observed from photographs of Dan's units, tracked in `reference/photos/`
(the manual page extracts in `reference/pioneer/` stay gitignored).
The normal playback screen is implemented in `crates/app/src/screen.rs`; the
other two are described here so they can be built to the same standard.

Common to all three: the left source column (logo, LINK, USB, PLAYER, SLIP),
the info row (TRACK, A.CUE, time with QUANTIZE caption, TEMPO with MT,
SYNC / MASTER), and the bottom overview with NEEDLE SEARCH, the ±range badge
and BPM. **Only the middle band changes between screens.** That is the
structural fact for the implementation: `screen.rs` keeps one layout and
swaps the middle.

## Normal playback

See `screen.rs`. Top touch-key row (BROWSE / TAG LIST / INFO / MENU /
PERFORM), title bar with key, phase meter + Bars readouts, enlarged waveform
with the CUE/LOOP · CALL · ZOOM column on its right.

## PERFORM

Reached by the PERFORM key, which stays lit blue top-right; the other four
top keys disappear. The middle band becomes pads:

```
┌──────────┬──────────────────────────────────────────────┬──────────────┐
│ HOT CUE  │   phase meter (2×4)          --.- Bars        │              │
│ BEAT JUMP│   compressed waveform (~10% H, ticks, red PH) │  PERFORM ⤢   │
├──────────┼──────────────────────────────────────────────┼──────────────┤
│ DELETE   │                                              │ CUE/LOOP     │
│ – CALL   │   [   A   ] [   B   ] [   C   ] [   D   ]    │ DELETE MEMORY│
│ BANK     │                                              │ CALL  ◀  ▶   │
│ BEAT LOOP│   [1/2] [ 1 ] [ 2 ] [ 4 ] [ 8 ] [ 16 ]       │              │
└──────────┴──────────────────────────────────────────────┴──────────────┘
```

- **Left column, top:** a two-way toggle HOT CUE / BEAT JUMP selecting what
  the four wide pads do. Below it DELETE (– CALL) and BANK keys, then the
  BEAT LOOP caption for the bottom pad row.
- **Pads:** hot cues A–D as four wide blue keys (one bank of the eight
  A–H); beat loops 1/2 · 1 · 2 · 4 · 8 · 16 as six keys. Faces are the
  slate-blue key colour; a set hot cue would show its colour.
- **Top of the band:** the phase meter and Bars readouts keep their place;
  the enlarged waveform is compressed to a short strip above the pads —
  same ticks and red playhead, about a third of its normal height.
- **Right column:** CUE/LOOP DELETE · MEMORY and CALL ◀ ▶ exactly as on the
  playback screen; the ZOOM – GRID pill is gone.

Data needed: hot cues and beat loops — i.e. `crates/engine` wired
(WORKSTREAMS C2). Until then the pads can be drawn and pressed (events on
the bus) with nothing behind them.

## BROWSE

Reached by BROWSE; the key lights (green underline). The middle band becomes
a list:

```
┌──────┬──────────────────────────────────────────────────────────────────┐
│      │ BROWSE  TAG LIST  INFO  MENU  PERFORM     (row stays, BROWSE lit) │
│ logo ├──────────────────────────────────────────────────────────────────┤
│      │ 【ARTIST】                       ← category header, full width   │
│ LINK ├──────────────────────────────────┬───────────────────────────────┤
│      │ 👤 Ali Storm                     │ ● Just Dip                    │
│ USB  │ 👤 ALOTT                         │                               │
│      │ 👤 ALRT                          │   (right pane: contents of    │
│      │ 👤 Aluna/SIDEPIECE               │    the highlighted row, or    │
│      │ 👤 Andrew Lux            ◀       │    the loaded track)          │
│      │ 👤 Anti Up                       │                               │
│      │ 👤 Arnold & Lane                 │                               │
└──────┴──────────────────────────────────┴───────────────────────────────┘
```

- **Title bar is replaced** by a category header `【ARTIST】` on the same
  navy bar.
- **Left pane (~55% of the band):** one row per item with a type icon
  (person for artist, note for track), ~7 rows visible; the highlighted row
  is inverted (white face, dark text) with a ◀ marker at its right edge.
  Row height ≈ 4% of screen height.
- **Right pane:** light grey field listing the highlighted item's contents
  (here the artist's one track, with a ● marker); this is where track info
  and the preview waveform appear when a track row is highlighted.
- **Info row and overview below are unchanged** — the playing track keeps
  running while you browse, which is the whole point.

Navigation on the unit is the rotary selector (turn = move highlight, press
= enter / load) plus touch on rows. On the bus that is
`BrowseEncoderDelta`, `Load`, `Back`, and taps on rows.

Data needed: a library (WORKSTREAMS F1/F2). A first version can browse the
filesystem — folder = category header, files = rows — which is exactly what
the unit does for a USB stick without a rekordbox export.

## Implementation note

`Layout` already has every rect the two new screens reuse. Add a
`ScreenMode { Playback, Perform, Browse }` in `DeckApp`, set by
`UiEvent::Screen(..)`, and give `screen::draw` a `match` over the middle
band only. The touch-key row, source column, info row and bottom band are
drawn once for all modes.

## Two players linked

`reference/photos/xdj-1000mk2-linked-player1-master.jpg` (player 1, master)
and `xdj-1000mk2-linked-player4-synced.jpg` (player 4, synced to it).

What changes with **link** state:
- The PLAYER box in the left column goes from dim text to a solid blue box
  with white "PLAYER" / number.
- The bar under the overview reads NEEDLE COUNTDOWN instead of NEEDLE
  SEARCH. Whether that is tied to link mode or a utility setting is not yet
  known — it changed between the standalone and linked shots.

What changes with **master** state (player 1):
- MASTER key lit gold/amber.
- BPM box turns gold and reads `130.0 MASTER` — the caption replaces "BPM".
- In the 4-box beat display, this deck's current-beat cell is orange
  instead of blue.

What changes with **sync** state (player 4): nothing visible in the shot
beyond REMAIN/SINGLE display settings; SYNC was not lit.

### The phase-meter slot has two views

Independent of link/master/sync — it is a display mode, toggled by
touching the widget or in SHORTCUT (manual: "Waveform/Phase Meter"). The
two units simply happened to be in different modes.

1. **Beat display** (player 1 shot): two rows of four outlined boxes, the
   current beat solid — blue, or orange when this deck is master.
2. **Alignment view** (player 4 shot): a `MASTER PLAYER [1]` tag in yellow,
   then two rows of beat ticks — the master's grid on top, this deck's below
   — with a white playhead line so phase offset between the decks is read as
   horizontal displacement. Bar ticks are taller.

Both views keep the two "Bars" readouts to their right.

### The Bars readouts are cue countdowns

Orange counts bars.beats down to the next **memory cue**; blue to the next
**hot cue**. They show numbers only when such a cue lies ahead of the
playhead; otherwise dashes. Every shot so far has no cues set, hence
`--.-`. Model as `Option<(bars, beats)>` — `None` renders dashes. (The
earlier freedj build showed beats-to-downbeat in blue; that was wrong.)

### Other footer variants seen

- A.HOTCUE (red) and A.CUE can both be shown, stacked, when both modes are on.
- Above the track number: TRACK or SINGLE, following the play-mode setting.
- REMAIN caption appears when the time display counts down.

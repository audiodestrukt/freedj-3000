# XDJ-1000MK2 screens — what the unit shows

Observed from photographs of Dan's unit (local, gitignored:
`reference/pioneer/xdj-1000mk2-{photo.png,perform-photo.jpg,browse-photo.jpg}`).
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

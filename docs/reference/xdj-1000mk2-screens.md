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

**Implemented (2026-08-26)** as a first-version *filesystem* browser
(`crates/app/src/browser.rs` + `screen::draw_browse`): folder = category
header, subfolders + audio files = rows, select encoder / arrows move the
highlight, LOAD opens a folder or loads+plays a track, Back goes up. Loading
swaps the decoded buffer live (`ArcSwap`) and re-uploads the waveform. Still to
come: a rekordbox library (WORKSTREAMS F1/F2), the right-pane preview waveform,
and touch-on-rows selection.

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
   current beat solid — blue, or orange when this deck is master.  freedj's version: top row = bar within the
   4-bar phrase, bottom row = beat within the bar (both from the local grid).
2. **Alignment view** (player 4 shot): a `MASTER PLAYER [n]` tag in yellow,
   then two rows of beat ticks under a fixed white playhead so phase offset
   between the decks reads as horizontal displacement. Bar ticks are taller.
   **Top row = the remote / master deck; bottom row = this (local) deck.**
   Each row's tall downbeat ("one") tick comes from *that deck's own*
   beat-in-bar — the master's from its Link beat packets (which carry beat
   1–4), ours from our grid — so the downbeat marker stays put. (An earlier
   build derived both rows' downbeat from the local beat count while the
   master row scrolled on the master's phase, so the "one" danced and a
   tick's height flickered tall/short.)

   **Four-deck (and larger) setups:** ProDJ Link has exactly one tempo master
   at a time, whatever the deck count. This view always compares *this* deck
   against *the* master — the `MASTER PLAYER [n]` tag names which of the up-to-4
   players that is. It never shows all four at once; the beat-box view shows
   only this deck's own bar/beat. So more decks changes the tag number, not
   the layout.

Both views keep the two "Bars" readouts to their right.

### The Bars readouts are cue countdowns

Orange counts bars.beats down to the next **memory cue**; blue to the next
**hot cue**. They show numbers only when such a cue lies ahead of the
playhead; otherwise dashes. Every shot so far has no cues set, hence
`--.-`. Model as `Option<(bars, beats)>` — `None` renders dashes. (The
earlier freedj build showed beats-to-downbeat in blue; that was wrong.)

The manual is explicit (CDJ-3000X, *Part names* callout 3, "Beat countdown"):
*"Displays the number of bars and beats from the playback point to the closest
saved cue point."* So on the real deck this readout **does not move for a track
with no saved cues** — confirmed live 2026-08-26 on a track with none. It is
not an "advanced rekordbox feature"; it just needs a memory or hot cue ahead
of the playhead (set on the deck, or already present from rekordbox analysis).
Drop a memory cue 8 bars ahead and it reads `8.0 → 7.4 → 7.3 …`.

**freedj already matches this**: both readouts are hardcoded to `--.-` in
`screen.rs` because we have no cue points. When the cue engine lands (below),
the countdown becomes real; until then, dashes are correct parity, not a bug.

> **Deferred, deliberately (2026-08-26):** hot cues / memory cues are **not**
> being implemented yet. That means the Bars countdown stays `--.-`, the
> CUE/LOOP and hot-cue pads stay inert, and the PERFORM screen's pads have
> nothing behind them. This is a scope choice, not an oversight. Implementing
> cues means wiring `crates/engine` into the binary (WORKSTREAMS C2) — a real
> chunk — and there's no pull for it yet. Revisit when cues become worth the
> transport work.

### Other footer variants seen

- A.HOTCUE (red) and A.CUE can both be shown, stacked, when both modes are on.
- Above the track number: TRACK or SINGLE, following the play-mode setting.
- REMAIN caption appears when the time display counts down.

## Downbeat / beat-1 is not truly detected yet

Which beat is "one" (the bar's downbeat) is only as good as the beat grid's
*phase*. freedj's grid (`crates/analysis/src/beat.rs`) anchors on a strong
onset and leaves `downbeat_offset = 0`, so whichever beat the anchor lands on
becomes beat 1 — the bar boundary can be off by 1–3 beats. On a CDJ the
downbeat comes from the rekordbox grid (or the first memory cue you set), which
pins beat 1 deliberately.

So the phase meter's bar row and the (future) Bars countdown inherit an
arbitrary bar phase until either:
- the beat-grid analysis learns a real downbeat (bar-level onset weighting), or
- we implement setting the beat-1 / initial memory cue (part of the cue engine,
  WORKSTREAMS C2, deferred).

Confirmed with a real track 2026-08-26: the detected downbeat did not line up
with the musical "one". Not a rendering bug — the grid phase is unset.
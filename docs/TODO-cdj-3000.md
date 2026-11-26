# CDJ-3000 work: what is left

As of `35e39f6` (2026-11-09), stale rows refreshed 2026-11-14 and 2026-11-17 (after #53 and the 0.3.29 / 0.3.30 fixes). Collected from the CDJ-3000 sessions (HID over
LINK, the native display feed, the CDJ-3000 skin, SYNC/SHIFT GRID) so nothing
lives only in a chat. Detail lives in the linked docs; this is the list.

**Who needs it** says which build a feature matters for:

| Tag | Builds |
|---|---|
| all | every build |
| touch | iPad / iPhone, and desktop without a controller: the screen is the only control surface |
| hw | the hardware deck (ADK-1000 / Pi with physical controls) |
| cdj | freedj driven by a real CDJ-3000 over HID-over-LINK |

## Faceplate and jog display: mostly touch

Yes: the **CDJ-3000 faceplate** (on-screen buttons, jog, tempo fader) is for
the touch builds. With a controller, a hardware deck or a real CDJ, those
controls are physical. The XDJ skin already works this way: its faceplate and
phone CONTROLS page are touch-only, and the deck screen never carries hardware
buttons (the screen-fidelity rule).

The **jog-centre display** (the 320×240 LCD in the platter: artwork, ring
marker, cue marker) is mostly touch too, drawn inside the on-screen jog. Two
exceptions:

- **cdj:** a real CDJ-3000's own jog LCD. We have not seen whether Control
  Mode lets the host draw it (no command for it in the brief so far). Worth one
  look on the emulator (`lcd jog`) while freedj drives the main screen.
- **hw:** only if the hardware deck gets a jog screen.

| Item | Who | Size |
|---|---|---|
| ~~CDJ-3000 faceplate (buttons, jog, tempo), like the XDJ faceplate~~ done: `cdjface.rs`, the whole top panel on the iPad / `--faceplate`, key columns beside the screen on desktop | touch | ✓ |
| ~~Faceplate positions placed by eye~~ done: measured off cdj3k-emu's panel drawing; every key the unit has is on it | touch | ✓ |
| ~~Hot-cue delete, TRACK SEARCH, SEARCH ◀◀ ▶▶, TIME MODE / AUTO CUE, 4 / 8 BEAT LOOP (1/2X, 2X), BEAT JUMP ◀ ▶, TEMPO RANGE / RESET~~ done, checked against the emulator (`docs/testing/cdj-3000-parity.md`) | touch | ✓ |
| Faceplate keys drawn but inert (nothing behind them): PLAY LIST, TRACK FILTER, SHORTCUT, KEY SYNC; DIRECTION, VINYL SPEED ADJ and JOG ADJUST are not drawn | touch | S each |
| The upper Bars row counts to a touched overview point on the unit (freedj: "--.-") | all | S |
| Jog-centre display drawn in the on-screen jog (reference: `jog-artwork.png`, `jog-playing.png`) | touch | M |
| Phone layout for the CDJ skin (the phone keeps the XDJ layout; MENU's SCREEN row says "CDJ-3000: iPad only" there) | touch | M |
| Does Control Mode let the host draw the CDJ's jog LCD? | cdj | S (look) |

## BROWSE and library (`docs/design/cdj-3000-skin.md`, phase 3)

| Item | Who | Size |
|---|---|---|
| Sorting: the sort key and column ▲s are drawn but inert (unit: sort menu) | all | S |
| TRACK FILTER (and its editor) | all | M |
| PLAYLIST bank | all | M |
| MATCHING category (listed, does not open) | all | M |
| ~~Category column for LINK/dbserver sources~~ done in #53: the category column, every category a CDJ-3000 lists (GENRE, BPM ±ranges, KEY with its related-key levels, HISTORY …), BACK into the column | all | ✓ |
| "H" mark by the ♪ (track has hot cues) on OpenDeck's own BROWSE (a CDJ-3000 browsing OpenDeck already shows it) | all | S |
| Green on the category a played track came from; dim green (22,139,22) in the preview pane | all | S |
| Verify covers in the PREVIEW column and HISTORY on Dan's real stick; it is mounted on the dev box now (`/run/media/dan/CDJ2`) | all | S |
| ~~Header extras: PREVIEW toggle, text-size A A A, INFO key~~ done in #53; ← BACK dimmed at the category root still open | all | S |
| SOURCE panel: Date row, Background Color, MY SETTINGS LOAD | all | S |

## Playback screen and deck features (phases 1 and 4)

| Item | Who | Size |
|---|---|---|
| ~~KEY SHIFT panel~~ done: −/+ a semitone, RESET, shifted KEY readouts | all | ✓ |
| ~~Key detection + KEY readout~~ done (0.3.10); green when compatible still open | all | S |
| KEY SYNC | all | M |
| ~~Hot-cue colours from rekordbox (PCO2 in the `.EXT`)~~ done: read, drawn, served. Comments are read and served, not yet shown on our screen | all | S |
| Gate Cue, Smart Cue | all | S |
| ~~Beat-jump value settable (CALL/DELETE + BEAT JUMP); quantize beat value~~ done: CALL/DELETE + BEAT JUMP ◀ / ▶ steps 1/2–64, MENU → QUANTIZE BEAT VALUE (1/8–1) and BEAT JUMP BEAT VALUE, both in the readouts | all | ✓ |
| ~~Slip reverse, reverse~~ done: the lever as SLIP REV / REV keys on the faceplate (and the XDJ panel's REV, `R`), reverse resolved on the audio side like a jump. The CDJ-3000 panel's lever bits are not mapped yet | all | ✓ |
| Active loop, emergency loop, loop slip | all | M |
| Vinyl speed adjust (touch/release) (`varispeed-engine.md`) | all | M |
| Touch Cue on the overview | all | S |
| INFO overlay: EDIT RATING; other players' tabs are in #54 (draft) | all | S |
| SHORTCUT screen; full UTILITY list (phase 6) | all | M |
| ~~Waveform current position CENTER/LEFT~~ done: MENU → WAVEFORM CURRENT POSITION; LEFT puts the playhead a quarter across | all | ✓ |
| ~~Waveform Divisions PHRASE on our own overview~~ done: MENU → WAVEFORM DIVISIONS, rekordbox's phrases from the stick's `.EXT` or our own. A linked player's phrases (its PSSI over dbserver) not yet fetched | all | ✓ |
| ~~6 players on PRO DJ LINK~~ done: PLAYER No. AUTO and 1–6 | all | ✓ |

## LINK with CDJ-3000s: open issues (2026-11-17)

| Item | Who | Size |
|---|---|---|
| 3-band / RGB previews in BROWSE drawn at about half height, one colour per column; the unit stacks the bands (#59). The blue previews match the unit since 0.3.29 | all | S |
| Menu rows are the CDJ-3000's 16 fields for every requester; check an XDJ-1000MK2 / NXS2, else send 12 to older players (#62). Deckbench's `trackinfo` check is a known failure on it | all | S |
| Other players' waveform view, INFO tabs, MASTER release: #54 (contributor, draft). "Other decks can't browse OpenDeck's files" from its notes is still open | all | M |
| The emulator image: small text clipped (#61); CI should boot the real install, and the docs should mention the first-boot pkexec prompt (#63) | cdj3k-emu | S |
| Almost every LINK check so far is against cdj3k-emu (fw 3.22); #53 was the only work also tried on physical CDJ-3000s | all | — |

## Waveform colours and LIGHT MODE (revisit, not now)

LIGHT MODE shipped in `50d3866` as a first cut: every screen colour mirrors
its lightness and keeps its hue (`crates/app/src/theme.rs`, the shader's
`finish`, `waveform_cpu`'s `Palette::px`). Dan's verdict after seeing it on
the desktop (2026-11-16): it works, but the waveform needs a proper pass.
A black waveform core on white looks odd, and mirroring may be the wrong
model for the waveform altogether.

| Item | Who | Size |
|---|---|---|
| Look at a real CDJ-3000 side by side (and photos of the light-mode mods) and sample its waveform colours for BLUE / RGB / 3BAND in each mode. The 3BAND palette came from the manual's pictures, not from a unit | all | M |
| Give LIGHT MODE its own waveform palette instead of mirroring the dark one: 3BAND's white highs mirror to black, BLUE's bright transients go near-black, and the amber mid comes out darker. Likely keep the bands' own colours (saturated blue / amber / a dark-but-not-black high) and only swap the ground, the grid ticks and the text | all | M |
| Decide which parts of the screen should mirror at all. Keys, text and grounds read well mirrored; the waveforms, the overview and possibly the phase meter may want hand-picked colours | all | S |
| Edge anti-aliasing: light mode has slope-aware AA (`cover` / `edge_widths`); dark mode keeps the unit's hard edges. Check against the real unit whether its edges are hard, and whether dark mode should get the AA too | all | S |
| Keep the two backends in step: whatever palette lands goes in the shader and `waveform_cpu` together, `tools/render-parity.sh` (with `OPENDECK_LIGHT=1`) checks them | all | — |

## SYNC and grid (`docs/design/cdj-grid-adjust.md`)

| Item | Who | Size |
|---|---|---|
| Why both bend directions moved the CDJ's own grid left (one-beat wrap?): a slow, larger bend each way, several times | — | S |
| Grids hand-edited during the 48 kHz start-up bug were saved at the wrong rate: reset those tracks' grids, or detect and convert | all | S |
| freedj as the synced deck: SHIFT GRID's visible result on freedj's screen (the e2e test checks phase, not pixels) | all | S |

## CDJ over HID-over-LINK (`docs/design/cdj-hid-display.md`)

| Item | Who | Size |
|---|---|---|
| Phase 4: the real UDP tunnel (`lanhid_net`): discovery, sessions, reassembly, keepalive. Everything so far ran over the emulator's socket | cdj | L |
| Acceptance on a real CDJ-3000 | cdj | M |
| Hot-cue letters above the 0x2C overview do not appear (emulator or a missing field?) | cdj | S |
| 0x26 aux text, multi-deck 0x2A | cdj | S |
| The CDJ's SEARCH, TAG LIST and MENU keys send no bits in Control Mode, so they cannot drive freedj's screens from the CDJ; look for another field, or accept | cdj | S |
| **Release blocker:** the handshake presents as rekordbox (trademarked product identity). Fine for personal testing; settle licensing before shipping | cdj | — |

## Audio

| Item | Who | Size |
|---|---|---|
| Real-time sample-rate conversion (WORKSTREAMS A1). The start-up track is now resampled offline like every LOAD (`8288d03`), so tracks play at the right speed. Streaming SRC would drop the load-time pass and its RAM | all | L |

## Done this round, for reference

Skin phases 0–3 (playback, panels, INFO, BROWSE/TAG LIST/UTILITY, SOURCE,
LOAD PREVIOUS TRACK, GRID ADJUST, rekordbox categories + track table +
PREVIEW, HISTORY, played-green, SEARCH + keyboard). SNAP/SHIFT GRID with SYNC
holding a nudge. The CDJ BROWSE key over HID. Start-up resampling and the
resampler's padded tail. `make test-shift-grid` (end-to-end against the
emulator). The test gaps from these sessions: 117 tests in the app crate.

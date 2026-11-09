# CDJ-3000 work: what is left

As of `35e39f6` (2026-11-09). Collected from the CDJ-3000 sessions (HID over
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
| CDJ-3000 faceplate / controls page (buttons, jog, tempo), like the XDJ faceplate | touch | M |
| Jog-centre display drawn in the on-screen jog (reference: `jog-artwork.png`, `jog-playing.png`) | touch | M |
| Phone layout for the CDJ skin (the phone keeps the XDJ layout today) | touch | M |
| Does Control Mode let the host draw the CDJ's jog LCD? | cdj | S (look) |

## BROWSE and library (`docs/design/cdj-3000-skin.md`, phase 3)

| Item | Who | Size |
|---|---|---|
| Sorting: the sort key and column ▲s are drawn but inert (unit: sort menu) | all | S |
| TRACK FILTER (and its editor) | all | M |
| PLAYLIST bank | all | M |
| MATCHING category (listed, does not open) | all | M |
| Category column for LINK/dbserver sources (only local rekordbox sticks today) | all | M |
| "H" mark by the ♪ (track has hot cues) | all | S |
| Green on the category a played track came from; dim green (22,139,22) in the preview pane | all | S |
| Verify covers in the PREVIEW column and HISTORY on Dan's real stick; it is mounted on the dev box now (`/run/media/dan/CDJ2`) | all | S |
| Header extras: PREVIEW toggle, text-size A A A, INFO key; ← BACK dimmed at the category root | all | S |
| SOURCE panel: Date row, Background Color, MY SETTINGS LOAD | all | S |

## Playback screen and deck features (phases 1 and 4)

| Item | Who | Size |
|---|---|---|
| KEY SHIFT panel (drawn, inert): needs a key-shift engine | all | M |
| Key detection + KEY readout, green when compatible (WORKSTREAMS E1) | all | L |
| KEY SYNC | all | M |
| Hot-cue colours from rekordbox (PCO2 in the `.EXT`), comments | all | M |
| Gate Cue, Smart Cue | all | S |
| Beat-jump value settable (CALL/DELETE + BEAT JUMP); quantize beat value | all | S |
| Slip reverse, reverse | all | M |
| Active loop, emergency loop, loop slip | all | M |
| Vinyl speed adjust (touch/release) (`varispeed-engine.md`) | all | M |
| Touch Cue on the overview | all | S |
| INFO overlay: EDIT RATING, other players' tabs | all | S |
| SHORTCUT screen; full UTILITY list (phase 6) | all | M |
| Waveform current position CENTER/LEFT | all | S |
| 6 players on PRO DJ LINK (4 today; 6 only with all CDJ-3000s) | all | M |

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

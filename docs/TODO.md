# TODO

Dan's open list, newest first.  Topic lists: [TODO-cdj-3000.md](TODO-cdj-3000.md),
[TODO-analysis.md](TODO-analysis.md); ideas not yet work:
[design/product-ideas.md](design/product-ideas.md).

## iOS

- **iPad app silent with the ring/silent switch on: fixed in 0.3.28.**  The
  cause: no audio-session category, so iOS used SoloAmbient and the switch
  muted the app (explained by the developer who fixed it; PR #50, the
  Playback category in `ios/freedj/main.m`).  0.3.10 (live until 0.3.28,
  2026-11-16) did not have it, so reports from 0.3.10 users fit.  If a
  report comes from 0.3.28 or later, ask for the version and
  `opendeck-log.txt`, and look at the hardening below.
- Hardening, lower priority: iOS has no recovery when the output stream
  dies (Linux rebuilds it, `audio::supervise`; iOS builds it once), session
  activation errors only go to `NSLog` (not `opendeck-log.txt`), and
  media-services reset / route changes / becoming active are not handled.
- **CDJ-3000 skin on iPhone.**  The phone keeps the XDJ pages (the skin
  is not offered on a phone: `settings.phone`).  Needs a phone layout for
  the CDJ-3000 screen (the two-page SCREEN / CONTROLS model, 932 x 430
  points), the top-row keys from the Pi panel layout (`cdjface::top_layout`)
  are a starting point.
- 0.3.32 in review (submitted 2026-11-18, manual release).

## Pi deck (appliance)

- **Jog (DJ2Go) in VINYL: waveform moves, no sound** (Dan, 2026-11-18).
  Not reproduced: tests show a turn heard both paused and playing at a
  hand-turned rate (`a_jog_turn_while_paused_scrubs_with_sound`).  It was
  reported on the build with the dropout bug (ring starved every 200 ms,
  fixed bc6a482), which would chop a scrub's small bursts to near silence.
  Retest on the current build; if still muted, capture the log with
  `RUST_LOG=opendeck_app=debug`.
- Speed knob (VINYL SPEED ADJUST, touch / release): not built, see
  TODO-cdj-3000.md.

- **Black gap between the splash and the deck** (a few seconds while the
  deck starts): measure on the Pi, then close it (keep the splash up until
  the first frame, or draw the deck sooner).
- **Power**: the test Pi's supply offers 3 A and logged an undervoltage at
  boot; use the official 27 W.  Consider taking the stems overclock off
  (`/boot/firmware/config.txt`, `arm_freq=2800`) for the appliance.
- HDMI plugged in at boot extends the deck over both screens (not a
  mirror); a real mirror needs the deck to re-layout when outputs change.
- Text and the rotary under the fill squeeze/stretch (SDL path) still
  imperfect; see `design/display-fitting.md`.

## Browsing

- **The deck's own music browsed like a stick**: ARTIST, ALBUM, TRACK, KEY,
  HISTORY, FOLDER with PREVIEW / BPM / KEY from OpenDeck's own analysis
  (background analysis for tracks not yet analysed).  Proposed 2026-11-18,
  not started.
- FOLDER rows, PREVIEW cells and cue marks in list previews: check against
  the CDJ-3000 emulator and match.

## Link / CDJ-3000

- **OpenDeck as master does not appear in other players' WAVEFORM view**:
  CDJs never fetch our analysis; likely needs a rekordbox export
  (`export.pdb` + ANLZ) of our own library.  Findings:
  `reference/cdj-3000-master-and-waveform-view.md`.
- Which playing player a CDJ-3000 hands MASTER to when several play (we
  pick the lowest number).
- PR #54 (waveform view, INFO tabs, MASTER): rebased and updated to the
  unit's behaviour 2026-11-18; review and merge.

## Testing

- **Run the test suite per platform on tagged releases** (Dan, 2026-11-18):
  today only Linux runs it on every push, Windows on `win-v*` tags; the
  macOS, iOS and Android workflows only build.  Add `cargo test` to the
  macOS release workflow, and the suite on the iOS simulator / an Android
  emulator at `ios-v*` / `android-v*` tags (their `cfg(target_os)` paths
  differ: the iOS output is never rebuilt, Linux's is).
- Heard-position estimate ignores a loop's wrap: for the time the ring holds
  (~70 ms at 1x) after a wrap it reads just before the loop's start.

- **UI regression tests per platform**: render each platform's screen
  headless (iPad faceplate, iPhone pages, desktop, Pi 800 x 480, XDJ) with
  a fixed track and compare with reference images in CI.

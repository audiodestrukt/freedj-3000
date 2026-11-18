# TODO

Dan's open list, newest first.  Topic lists: [TODO-cdj-3000.md](TODO-cdj-3000.md),
[TODO-analysis.md](TODO-analysis.md); ideas not yet work:
[design/product-ideas.md](design/product-ideas.md).

## iOS

- **iPad app starts up muted (reports keep coming).**  The deck runs, the
  waveform scrolls, nothing is heard; mechanism not known.  Leads, from
  the code (2026-11-18):
  - *No recovery on iOS when the output stream dies.*  On Linux a failed
    stream is rebuilt (`audio::supervise`); on iOS the stream is built once
    (`StreamOwner::Inline`): if iOS stops the RemoteIO unit (an
    interruption, a route change, a media-services reset, the session not
    yet active), the deck logs `audio stream error` once and stays silent
    until the app is relaunched.
  - *Session activation can fail at launch* (another app holding
    non-mixable audio, launched while not in the foreground) and the error
    only goes to `NSLog`, not to `opendeck-log.txt`.
  - *Interruption without an "ended" notification* (iOS does not promise
    one): the session is never re-activated.
  - Not handled at all: `AVAudioSessionMediaServicesWereResetNotification`,
    route changes, becoming active again after the background.
  - Next: write the session's activation result, category, route, sample
    rate and every interruption / route change / reset into
    `opendeck-log.txt`; ask a reporter for that file.  Then: supervise the
    stream on iOS as on Linux (rebuild on failure), re-activate the session
    and restart the stream on `didBecomeActive`, and handle media-services
    reset.  A test: drive a failed stream through the stand-in device and
    check it comes back.
- **CDJ-3000 skin on iPhone.**  The phone keeps the XDJ pages (the skin
  is not offered on a phone: `settings.phone`).  Needs a phone layout for
  the CDJ-3000 screen (the two-page SCREEN / CONTROLS model, 932 x 430
  points), the top-row keys from the Pi panel layout (`cdjface::top_layout`)
  are a starting point.
- 0.3.32 in review (submitted 2026-11-18, manual release).

## Pi deck (appliance)

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

- **UI regression tests per platform**: render each platform's screen
  headless (iPad faceplate, iPhone pages, desktop, Pi 800 x 480, XDJ) with
  a fixed track and compare with reference images in CI.

# App Store Connect — OpenDeck DJ

Everything App Store Connect asks for at first submission, ready to paste.
Screenshots are in `screenshots/ipad-13/` (regenerate with `make shots-appstore`
or `ios/appstore/capture.sh /path/to/music`).  Bundle `com.audiodestrukt.opendeck`,
SKU `adk-1000`, team `W527ZN3X52`; the app is **iPad-only** (device family 2,
iPadOS 15+), so no iPhone screenshots are required.

## App Information

| Field | Value |
|---|---|
| Name (30) | `OpenDeck DJ` |
| Subtitle (30) | `Pro DJ Link deck for iPad` |
| Primary category | Music |
| Secondary category | Entertainment (optional) |
| Content rights | Does not contain third-party content (the seeded demo track is our own) |
| Age rating | Answer *None* to every question → **4+** |
| Copyright | `2026 AudioDestrukt` |
| License agreement | Apple's standard EULA |

## Pricing and Availability

Free · all territories · no pre-order.

## Version Information (1.0 / 0.1.x)

**Promotional text (170)** — editable without a new build:

```
A single-deck DJ player for iPad that plays your own tracks and syncs with CDJ/XDJ players over Pro DJ Link. No account, no sign-in, nothing collected.
```

**Description (4000):**

```
OpenDeck DJ turns your iPad into a single-deck DJ player modeled on a professional club media player: a large jog wheel with a spinning platter display, a tempo fader, transport controls, a zoomable waveform, hot cues, beat loops, and automatic BPM and beat-grid analysis.

BRING YOUR OWN MUSIC
Drop MP3, AAC/M4A, WAV, AIFF, FLAC or OGG files into the OpenDeck DJ folder in the Files app (On My iPad → OpenDeck DJ), or from a Mac via Finder, and browse them from the deck's BROWSE screen. A demo track is included so you can try everything straight away.

PLAY ALONGSIDE THE CLUB STANDARD
OpenDeck DJ speaks Pro DJ Link. On a local network it appears as another player next to CDJ/XDJ decks: it can follow the shared tempo and beat grid with SYNC, or take MASTER and lead. Link hardware is optional — the app is a complete standalone player on its own.

FEATURES
• Jog wheel with vinyl (scratch) and nudge modes and a live centre display
• Tempo fader with ±6 / ±10 / ±16 / WIDE ranges and MASTER TEMPO key lock
• CUE with AUTO CUE, hot cues A–D, memory points, beat loops from 1/2 to 16 and SLIP
• Zoomable waveform with beat and bar markers, full-track overview and needle search
• Automatic BPM and beat-grid analysis, with GRID ADJUST (snap, shift, reset)
• QUANTIZE so cues and loops land on the beat
• Pro DJ Link: SYNC, MASTER, beat and phase meter, and LINK browsing of other players' media
• TAG LIST, track INFO and a UTILITY menu (player number, tempo range, quantize, auto-cue level)
• Runs entirely on-device: no account, no sign-in, no analytics

OpenDeck DJ is an independent open-source project. Pioneer DJ, CDJ, XDJ, rekordbox and Pro DJ Link are trademarks of AlphaTheta Corporation, which is not affiliated with and does not endorse this app.
```

**Keywords (100, comma-separated, no spaces after commas):**

```
dj,deck,dj player,beat sync,tempo,jog wheel,loop,hot cue,bpm,waveform,dj link,quantize,music player
```

(Deliberately no "pioneer", "cdj", "xdj" or "rekordbox" in the keyword field —
third-party trademarks there are a Guideline 2.3.7 rejection risk; the
description covers compatibility factually, with the disclaimer line.)

| Field | Value |
|---|---|
| Support URL | https://audiodestrukt.com/opendeck |
| Marketing URL | https://audiodestrukt.com/opendeck |
| Privacy Policy URL | https://audiodestrukt.com/opendeck/privacy |
| Version | matches the tag (e.g. `0.1.11`) — pick that build under *Build* |

**What's New (first release):**

```
First release: a single-deck DJ player for iPad with Pro DJ Link sync, your own music via the Files app, hot cues, beat loops, GRID ADJUST and MASTER TEMPO.
```

### Screenshots — iPad 13" Display (2064 × 2752, portrait)

Upload in this order; the first two are what shows in search.

| # | File | Shows |
|---|---|---|
| 1 | `01-playback.png` | Deck playing: waveform, jog, tempo fader, memory points |
| 2 | `02-loop.png` | A 4-beat loop engaged with the phase meter running |
| 3 | `03-perform.png` | PERFORM screen: hot cues A–D and beat-loop pads |
| 4 | `04-browse.png` | BROWSE with a folder of tracks from the Files app |
| 5 | `05-grid.png` | GRID ADJUST keys (RESET / SNAP / SHIFT) |

They are the app's own renderer at the 13" panel's exact pixel size (same draw
code the iPad runs), no alpha channel, sRGB.  App Store Connect reuses the 13"
set for the other iPad sizes.  An app preview video is optional; skip it.

## App Privacy

Answer **"No, we do not collect data from this app"** → the listing shows
*Data Not Collected*.  (No accounts, analytics, ads or crash reporting; the
privacy manifest in the bundle declares the same.)

## App Review Information

| Field | Value |
|---|---|
| Sign-in required | No |
| Contact | your name / phone / email (team contact, not published) |

**Notes for the reviewer:**

```
OpenDeck DJ is a standalone single-deck DJ player; no external hardware or account is needed to review it. A demo track is pre-loaded — tap PLAY/PAUSE, use the jog wheel, tempo fader, CUE, LOOP IN/OUT and the on-screen BROWSE / PERFORM keys.

To load your own music: copy audio files into Files → On My iPad → OpenDeck DJ, then tap BROWSE (or the FILE key), turn the BROWSE knob and press it to LOAD.

On first launch iPadOS asks for Local Network permission. This is used only for Pro DJ Link (UDP on the local network) to beat-sync with Pioneer CDJ/XDJ players if any are present. Denying it leaves every other feature working. Nothing is sent off the device.
```

## Export compliance

Handled by `ITSAppUsesNonExemptEncryption = false` in `Info.plist`; no prompt.

## Before pressing *Add for Review*

- [ ] Build from the intended `ios-v*` tag is selected under *Build*
- [ ] 5 screenshots uploaded to *iPad 13" Display*
- [ ] Privacy answered (Data Not Collected)
- [ ] Age rating completed (4+)
- [ ] Support URL and Privacy Policy URL resolve (both live as of 2026-09-02)
- [ ] `support@audiodestrukt.com` mailbox exists and is read
- [ ] Release option: *Manually release this version* (lets you check the listing before it goes live)

# CDJ-3000(X) — feature inventory for a "CDJ-3000 skin"

Everything the CDJ-3000X instruction manual says the player does: every
screen, on-screen element, touch target, physical control, playback feature and
setting. Purpose: plan a CDJ-3000 skin for freedj / OpenDeck, which already
replicates the XDJ-1000MK2.

**Source:** CDJ-3000X Instruction Manual (AlphaTheta), local copy at
`reference/pioneer/CDJ-3000X_manual.pdf` (gitignored, AlphaTheta's
copyright). Everything here is a summary in our own words. `p.N` = manual page.
Screenshots referenced as "(img p.N)" are where a detail was read off a manual
screen image and not from the text.

**About the manual.** It covers the 3000X, the 2025 refresh. Things that are
3000X-only and not on the original CDJ-3000: Wi-Fi, internet/Wi-Fi status
icons, rekordbox CloudDirectPlay, Beatport/TIDAL streaming, NFC login, OneLibrary-only
USB (p.9), and the Network settings category. Everything else matches the
CDJ-3000. Where the manual doesn't say something (for example the second page of
beat-loop values), this file says so and doesn't guess.

Related: `cdj-3000-playback-screen.md` (layout sketch + callout numbering;
ignore its Status column), `xdj-1000mk2-screens.md` (what freedj already
draws).

---

## 1. Screens

The main display is a 10.1″ touchscreen (p.117). There are two input paths: touch
(tap selects, tap a folder to go down, tap a track for the track menu, flick to
scroll), or the **rotary selector** (turn moves the highlight, press selects). **BACK**
goes up one level, and long-pressing BACK jumps to the top of the hierarchy (p.28).
Unlike the XDJ, every screen is opened by a **dedicated hardware button**
(SOURCE, BROWSE, TAG LIST, PLAYLIST, SEARCH, MENU, SHORTCUT), not by on-screen
touch keys.

**Common furniture.** SOURCE, BROWSE, TAG LIST, PLAYLIST and SEARCH all show a
compressed **playback status display** strip along the bottom: player no.,
REMAIN/time, overview waveform with hot-cue markers, MT, key, tempo range badge,
tempo %, and BPM with a MASTER badge (p.21, p.22, img p.19/21). So the playing deck is
always visible. List screens also have **connection status icons** at top right
(internet, Wi-Fi; 3000X).

### 1.1 Playback (waveform) screen — the home screen

- **Purpose:** the deck's main view.
- **Reached by:** loading a track, or closing other screens.
- **Elements:** 29 numbered callouts, listed in §2 (p.23–26).
- **Touch targets:**

| Target | Action | Page |
|---|---|---|
| Eject icon on device icon (4) | Unloads the track. Blocked during playback if Eject Lock = Lock | p.24 |
| Track info (5) | Shows/hides the title-and-details line | p.24 |
| BEAT LOOP (6) | Opens the beat-loop panel (§1.11) | p.80 |
| KEY SHIFT (7) | Opens the key-shift panel (§1.12) | p.97 |
| BEAT JUMP (8) | Opens the beat-jump panel (§1.11) | p.90 |
| Info ⓘ (9) | Opens the INFO screen (§1.9) | p.59 |
| Waveform/phase meter (11) | Toggles between the phase-meter and waveform view | p.24 |
| GATE CUE (18) | Toggles Gate Cue | p.25 |
| Overall waveform (29) | When paused, or in Vinyl mode with the jog top held: **needle search** (play from the touched point; drag to scrub). While playing: **Touch Cue** (headphone preview via a compatible mixer) and the enlarged waveform + countdown jump to the touched point | p.69, p.72, p.23–24 |
| In Grid Adjust mode: SNAP GRID(CUE), SHIFT GRID, <1/2, 1/2>, ×2, ×1/2, Enlarge, Reduce, RESET | Beat-grid editing (§1.13) | p.95 |

- **Hardware while on this screen:**
  - MENU short press opens the **LOAD PREVIOUS TRACK** list (§1.10).
  - Rotary long press toggles **Zoom ⇄ Grid Adjust**, and turning it zooms or moves the grid (p.24, p.95).
  - TAG TRACK/REMOVE tags the loaded track (p.63).

### 1.2 SOURCE

- **Purpose:** pick the device or library to browse, log in to the cloud, load My Settings, and enter DJ-software control.
- **Reached by:** the SOURCE button (p.15, p.19, p.55).
- **Elements (p.19–21):**

| # | Element | Notes |
|---|---|---|
| 1 | Device icons (left column) | One per device, coloured by the device's Background Color. The player number appears at the top (img p.19) |
| 2 | Device name/status list | e.g. "AlphaTheta USB1" with a ✓ for the selected one, and "Cloud Log In" (img p.19) |
| 3 | ⓘ Info | Shows/hides info for the highlighted device |
| 4 | ⚙ Device settings | Tabs: **Local Library** (Waveform Divisions, Image On-Jog Display, Background Color, MY SETTINGS LOAD), **Cloud Library** (same, plus Display username on-screen and Log Out Time), **Streaming** (TIDAL file format) |
| 5 | Connection status icons | Internet and Wi-Fi |
| 6 | MY SETTINGS LOAD | Loads settings from the selected device (p.115) |
| 7 | Library display | Rows: Local Library (shows free GB), Cloud, Beatport, TIDAL, each with Connected / Not connected and a LOG IN button (img p.19) |
| 8 | Playback status display | The bottom strip |

- **Also on SOURCE:**
  - **SOFTWARE CONTROL** puts the unit in MIDI/HID control mode (p.99).
  - **Cloud Log In → LOG IN** shows a QR code, or you can pick a player to log in on (p.47–48). LOG OUT is on the same screen (p.49).
  - Linked players' media appear here for PRO DJ LINK browsing.

### 1.3 BROWSE (track list)

- **Purpose:** browse the rekordbox library (USB, CloudDirectPlay, Link Export PC/mobile, or streaming). Without a library it shows a plain folder tree (p.21).
- **Reached by:** the BROWSE button. TAG LIST and PLAYLIST open variants of the same list screen (p.21).
- **Elements (p.21–22):**

| # | Element | Notes |
|---|---|---|
| 1 | Category column (left, icons) | Seen in the image: TRACK, ARTIST, ALBUM, KEY, PLAYLIST, HISTORY, MATCHING, FOLDER, DATE ADDED (img p.21), plus HOT CUE (bank lists, p.87). The visible set is chosen in rekordbox Preferences |
| 2 | Device icon | The source picked on SOURCE |
| 3 | ← Back | Up one level |
| 4 | Next-level-up display | Parent folder, device name or nickname |
| 5 | Track list | Columns seen: Preview (mini waveform), artwork, Track (count in brackets), BPM, Key. Tap a column header to **sort**. A header icon opens **sub-column selection** (the choice of sub-columns is set in rekordbox). A key cell turns green when it is harmonically compatible with the master's track (p.58, img p.21) |
| 6 | PREVIEW | Shows/hides the Preview column |
| 7 | Font size | Font size toggle (A/A) |
| 8 | ⓘ Info | Shows/hides details for the highlighted track |
| 9 | Connection icons | |
| 10 | Playback status display | |

- **Behaviours:**
  - **Touch Preview:** with a compatible mixer, touching a row's preview waveform plays that point into the headphones. This works while the deck is playing (p.60).
  - **Played tracks are green** (history) (p.61).
  - **Alphabet Jump:** in an alphabetical list, long-press the rotary, then turn it to step by first letter (A–Z, 0–9, symbols) (p.57).
  - **Page Jump:** in any other list, the same gesture jumps page by page (p.57).
  - Tapping a track opens the **track menu** (p.28).
- **MENU (short press) context menu, by list:**
  - History list: DELETE / ALL DELETE (p.61).
  - Any track list: PLAYED / UNPLAYED (p.67).
  - Playlist list: DELETE / ALL DELETE (p.68).
- **HISTORY** (category):
  - Tracks played for about a minute are logged (p.61).
  - A new history is started whenever a device is connected (p.61).
  - The history's name comes from the History Name utility (p.61).
- **HOT CUE** (category): the Hot Cue Bank list. Choosing a bank and then a track sets hot cues A–H from the bank. Banks are read-only on the unit (p.87).

### 1.4 TAG LIST

- **Purpose:** a scratch queue of up to 100 tracks, with no duplicates (p.62).
- **Reached by:** the TAG LIST button.
- **Elements (p.62–63):**
  - Device icon.
  - Tag list with preview waveform, artwork, serial no., Track, BPM and Key, sortable, with sub-column choice.
  - **GLOBAL** toggle. On: tags go to every source's tag list (USB, cloud, streaming) at once. Off: per source.
  - PREVIEW, font size, ⓘ, and connection icons.
- **Ops:**

| Action | How | Page |
|---|---|---|
| Add a track | TAG TRACK/REMOVE on a highlighted track; a ✓ appears | p.63 |
| Add a whole folder or playlist | Highlight the folder or playlist, then press TAG TRACK/REMOVE | p.63 |
| Tag the loaded track | TAG TRACK/REMOVE on the INFO or waveform screen | p.63 |
| Remove one track | Long-press TAG TRACK/REMOVE on the TAG LIST screen, or a short press in BROWSE/PLAYLIST/SEARCH | p.64 |
| Remove all tracks | MENU → REMOVE ALL TRACKS | p.64 |
| Convert to a playlist | MENU → CREATE PLAYLIST, which makes "TAG LIST XXX". Not available with GLOBAL on, and only rekordbox-managed tracks are converted | p.65 |

### 1.5 PLAYLIST (with Playlist Bank)

- **Purpose:** jump straight to playlists without walking the tree.
- **Reached by:** the PLAYLIST button, or from BROWSE (p.66).
- **Elements (p.66–67):**

| # | Element | Notes |
|---|---|---|
| 1 | **Playlist Bank** column | Bank slots (BANK 1…). A grey icon is empty, a white one is assigned. A trash icon at the bottom enters deregister mode, which puts a × on each slot (p.68, img p.66) |
| 2 | Category | PLAYLIST |
| 3 | Device icon | |
| 4/5 | ← Back, next-level-up | e.g. "PLAYLIST B" |
| 6 | **EDIT** | Reorder or delete tracks in the playlist, by rotary or touch |
| 7–10 | PREVIEW, font size, ⓘ, connection icons | |
| 11 | Track list | Preview waveform, artwork, serial no., Track, BPM, Key. Sortable, with sub-columns |

- **Ops:**
  - Assign a bank: highlight a playlist, then tap a grey bank slot.
  - Open a bank: tap a white slot.
  - Deregister: trash icon, then tap the slot (p.68).
  - MENU → DELETE / ALL DELETE playlists (p.68).
  - MENU → PLAYED / UNPLAYED for a track (p.67).

### 1.6 SEARCH (on-screen keyboard)

- **Purpose:** keyword search, for rekordbox libraries only (p.56).
- **Reached by:** the SEARCH button.
- **Elements (img p.56):**
  - **Top bar:** device icon, a search field with ✕ clear, PREVIEW, font size, ⓘ and connection icons.
  - **Results list:** the same columns as BROWSE, with match count and BPM/Key sort.
  - **QWERTY keyboard** across the lower half of the screen:
    - letter rows;
    - **paste** (top-left);
    - backspace ⌫ and ✕ (right);
    - **space**;
    - **123** (switches to numbers/symbols).
- **Behaviours (p.57):**
  - Results update as you type.
  - Space-separated words are treated as several keywords.
  - Long-press a word in the results to copy it, then **paste** it into the field.
  - With Beatport, MENU picks the search target.

### 1.7 TRACK FILTER

- **Purpose:** narrow the list by BPM, key, rating, colour or rekordbox "My Tag" style tags. rekordbox libraries only (p.58).
- **Reached by:**
  - TRACK FILTER/EDIT **short press** applies the current filter on/off.
  - **Long press** opens the **Track Filter editing screen** (p.58).
- **Edit-screen elements:**
  - A check mark per condition to enable it.
  - **MASTER PLAYER** fills in BPM and key from the sync master's track.
  - Conditions are stored per storage device and can be set up in rekordbox.
- **Related:** key cells turn green for keys related to the master's track. This doesn't work when there is no master (p.58).
- The editing screen isn't pictured in the manual.

### 1.8 SHORTCUT

- **Purpose:** quick settings.
- **Reached by:** the SHORTCUT button; press it again to close (p.110).
- **Elements (p.110, img p.110):**
  1. **Player settings:** a grid of toggles.
  2. **Device information:** the source device's icon and name.
  3. **My Settings:** LOAD / SAVE.
  4. **Device settings:** Waveform Color, Waveform Current Position, Waveform Divisions, Image On-Jog Display.
  - Also: LCD and Jog LCD **brightness sliders** along the bottom, and a **⚙ UTILITY** button at top right that jumps to UTILITY.
  - All items and values are listed in §6.2.
- The bottom playback status strip stays visible.

### 1.9 INFO

- **Purpose:** full details of a loaded track, plus **Information Jump**.
- **Reached by:** ⓘ on the waveform screen (p.59).
- **Elements (p.59, img p.59):**

| # | Element | Notes |
|---|---|---|
| 1 | Player tabs (Player 1–6, left) | Tap to see the track loaded on another linked player |
| 2 | Detail block | Device badge, artwork, title, album, artist, rating stars, BPM, genre, bitrate, year, date added, key, length, comment, other tag lines |
| 3 | EDIT RATING | Change the star rating |
| 4 | ✕ | Back to the waveform screen |

- **Information Jump:** tap a detail (genre, BPM, …) to open BROWSE filtered to that category. It doesn't work on icon-only fields (p.59).
- TAG TRACK/REMOVE works here too (p.63).

### 1.10 LOAD PREVIOUS TRACK

- **Purpose:** list of recently played tracks; select one to reload it.
- **Reached by:** MENU **short press** on the waveform screen. MENU or BACK closes it (p.56).

### 1.11 BEAT LOOP and BEAT JUMP touch panels

Both are overlays that slide in over the lower part of the enlarged waveform.
The waveform screen stays live behind them.

| Panel | Opened by | Contents | Page |
|---|---|---|---|
| **BEAT LOOP** | BEAT LOOP touch key, which lights blue | 8 buttons: **1/4, 1/2, 1, 2, 4, 8, 16, 32**. The active length is lit orange. A **[1][2] page selector** sits at the right; the manual doesn't show page 2. Tapping a value starts a loop of that many beats. With Slip on, **holding** a value is a Slip Beat Loop that ends on release | p.80, p.91, img p.80 |
| **BEAT JUMP** | BEAT JUMP touch key | Four pairs of ◀ / ▶ buttons on page 1: **1/2, 1, 2, 4** beats. Same [1][2] page selector; page 2 not shown. A tap jumps by that many beats, or moves the loop while looping | p.90, img p.90 |

### 1.12 KEY SHIFT panel

- **Reached by:** the KEY SHIFT touch key (p.97).
- **Elements (img p.97):**
  - A **−** button labelled with the key one semitone down (e.g. 7A).
  - The current key plus its semitone offset (e.g. "4A −2"), with **RESET** under it.
  - A **+** button labelled with the key one semitone up (e.g. 9A).
  - Slider-style tick graphics on both sides.
- Each tap moves the key one semitone. The KEY readout (28) shows the shifted key.

### 1.13 Grid Adjust mode

- **Reached by:** long-pressing the rotary on the waveform screen. The indicator (12) switches from ZOOM to GRID. Long-press again to return to Zoom (p.95).
- **Behaviour:** turning the rotary moves the grid.
- **On-screen buttons:**

| Button | Effect |
|---|---|
| SNAP GRID(CUE) | Puts beat 1 on the cue point |
| SHIFT GRID | Writes the pitch-bend offset built up during sync into the grid |
| <1/2, 1/2> | Move the grid by half a beat |
| ×2, ×1/2 | Double or halve the beat count |
| Enlarge, Reduce | Stretch or shrink the grid by 1 ms, anchored on the first beat |
| RESET | Back to the original grid |

- Needs an analysed track.
- The manual doesn't picture this screen.

### 1.14 UTILITY (MENU long press)

- **Reached by:** **long-pressing MENU**; a short press of MENU closes it (p.100–101).
- **Layout (img p.100), three columns:**
  - **Category** tabs: DJ Settings, Display (LCD), Display (Indicator), PRO DJ LINK, Network, System, Info.
  - **Items** with their current values.
  - **Value** list (radio buttons).
- **Header:** UTILITY, version (e.g. Ver.1.00), connection icons.
- **Sub-screens:**
  - **Wi-Fi Setting:**
    - Wi-Fi on/off and the SSID list.
    - Tap an SSID to get a password keyboard, then CONNECT.
    - The network info screen has Auto-Connect.
    - SAVED NETWORKS → FORGET THIS NETWORK (p.108–109).
- All settings are listed in §6.1.

### 1.15 Other screens and overlays

| Screen | What | Page |
|---|---|---|
| Track menu | Opened by tapping a track in any list. Its contents aren't listed in the manual | p.28 |
| Sub-column selection | From a list's title-row icon | p.22 |
| MENU context menus | DELETE / ALL DELETE, PLAYED / UNPLAYED, REMOVE ALL TRACKS, CREATE PLAYLIST, Beatport search target | p.57, 61, 64–68 |
| Cloud login QR / player picker | SOURCE → Cloud Log In | p.47 |
| Streaming login | Beatport username/password entry | p.51–52 |
| Software control mode | SOURCE → SOFTWARE CONTROL | p.99 |
| Screen saver | After 5 min with no track loaded, or 100 min paused / cue-standby / at end of track | p.103 |
| Error displays | E-8302 CANNOT PLAY TRACK; E-8304/8305 UNSUPPORTED FILE FORMAT | p.118 |
| Cloud/stream loading | Load progress is drawn in the overall waveform | p.50, p.53 |

---

## 2. Playback-screen elements (callouts 1–29, p.23–26)

Layout, from top to bottom (img p.23):

1. **Top bar:** device icon, track info, three touch keys, ⓘ, connection icons.
2. **Beat-countdown box and phase meter:** one row.
3. **Enlarged waveform:** grid ticks along the top and bottom edges, beat numbers (8, 16, 24, 32) under it, and the ZOOM|GRID pill at the bottom right.
4. **Info block:** two rows, with the overall waveform in the second row.

| # | Element | What it shows / does |
|---|---|---|
| 1 | Loop beat count | Loop length in beats ("↻ 8") under the playhead, while a loop is set |
| 2 | Enlarged waveform | Scrolling waveform with grid, memory/hot cue and loop markers. The active loop region is shaded. Colour follows Waveform Color. Touching the overview while playing shows that spot here |
| 3 | Beat countdown | Bars.beats from the playhead to the nearest saved cue, or to the touched overview point. The image shows two stacked readouts: a dim "--.-" and a larger blue "93.4" |
| 4 | Device icon (source of the loaded track) | e.g. USB1, with an **eject** touch target; Eject Lock applies |
| 5 | Track information | Artwork thumbnail, title, length, BPM, key, and an eye icon. Tap to show or hide |
| 6 | BEAT LOOP | Touch key, opens §1.11 |
| 7 | KEY SHIFT | Touch key, opens §1.12 |
| 8 | BEAT JUMP | Touch key, opens §1.11 |
| 9 | ⓘ Info | Opens INFO |
| 10 | Connection status icons | Internet and Wi-Fi (3000X) |
| 11 | Waveform / phase meter | Bar and beat offset from the sync master. Tap to switch views (also in SHORTCUT) |
| 12 | Zoom / Grid Adjust indicator | "ZOOM \| GRID". Long-press the rotary to switch modes |
| 13 | Beat Jump beat value | "BEAT JUMP 16" |
| 14 | Quantize beat value | "QUANTIZE 1" in red when quantize is on |
| 15 | Player number | "PLAYER 3" |
| 16 | File-cache icon | Shown once the loaded track is fully cached |
| 17 | Track number | "TRACK 02" |
| 18 | GATE CUE | Touch button (blue) that toggles Gate Cue |
| 19 | SMART CUE | Shown when Smart Cue is on |
| 20 | AUTO CUE | Shown when Auto Cue is on. Lit red when Hot Cue Auto Load is on (p.86) |
| 21 | Time display | mm:ss plus milliseconds in smaller digits, with a REMAIN or TIME caption |
| 22 | CONTINUE / SINGLE | Play mode |
| 23 | Playback speed | Tempo % from the slider (e.g. 0.00 %) |
| 24 | Speed range | ±6 / ±10 / ±16 / WIDE badge |
| 25 | BPM | Current BPM, with one decimal in smaller digits |
| 26 | MASTER / SYNC | MASTER (orange badge) when sync master, SYNC when Beat Sync is on |
| 27 | MT | Master Tempo on |
| 28 | Key | Track key (e.g. 4A). Green when compatible with the master's key |
| 29 | Overall waveform | The whole track, with hot-cue letter flags (A–H in their colours), memory-cue triangles, cue marker, playhead, a time scale under it (−4:00 … −1:00) or phrase data, and slip lines. Touch for needle search or Touch Cue |

Slip on the waveforms (p.92):

- The audible position is a **yellow** line on both the enlarged and overall waveforms.
- The background ("real") position is a **white** line on the overall waveform.

---

## 3. Jog display (LCD in the jog centre, p.27)

A round colour LCD inside the platter. Elements around the ring (img p.27): MODE
at the top, SLIP at upper left, VINYL at upper right, SYNC at lower left, MASTER at
lower right, and BEAT SYNC at the bottom.

| # | Element | Behaviour |
|---|---|---|
| 1 | SLIP indicator | Lit when Slip mode is on |
| 2 | Artwork | The track's artwork (centre square). Can be swapped for a custom image |
| 3 | SYNC indicator | Lit when Beat Sync is on |
| 4 | Playback-point indicator | A marker that rotates during playback and stops on pause |
| 5 | VINYL indicator | Lit in Vinyl mode |
| 6 | Cue / loop / hot-cue point indicator | Marker on the ring showing where the cue point is |
| 7 | MASTER indicator | Lit when this deck is sync master |

**Settings:**

| Setting | Values | Where | Page |
|---|---|---|---|
| Image On-Jog Display | ARTWORK, or CUSTOM IMAGE (a JPEG ≤800×800 from the root of the source device or the cloud) | SHORTCUT, and SOURCE ⚙ | p.112 |
| Jog LCD Brightness | 1–5 (3 by default) | UTILITY Display (LCD) and SHORTCUT | p.103 |
| Jog Ring Brightness | Off / 1 / 2 (default 2). Illumination ring around the jog | UTILITY | p.104 |
| Jog Ring Indicator | Ring blinks as the track nears its end; Off / On (default On) | UTILITY | p.104 |

---

## 4. Physical controls that change UI state (p.13–16)

### Browser and screen block (top right of the screen)

| Control | Effect |
|---|---|
| SOURCE | Opens SOURCE |
| BROWSE | Opens BROWSE |
| TAG LIST | Opens TAG LIST |
| PLAYLIST | Opens PLAYLIST |
| SEARCH | Opens SEARCH |
| MENU | **Short press:** context menu in lists, or LOAD PREVIOUS TRACK on the waveform screen. **Long press:** UTILITY |
| BACK | **Short press:** up one level. **Long press:** to the top |
| TAG TRACK/REMOVE | Short press: tag or untag. Long press: remove (on TAG LIST) |
| Rotary selector | Turn: move the highlight, zoom, or move the grid. Press: select or load. Long press: Alphabet/Page Jump in lists, Zoom⇄Grid on the waveform screen. Has an LED (brightness setting) |
| TRACK FILTER/EDIT | Short press: toggle the filter. Long press: filter editor |
| SHORTCUT | Opens or closes SHORTCUT |

### Transport (left side)

| Control | Effect |
|---|---|
| ▶/❚❚ | Play/pause. In Vinyl mode a pause mutes, in CDJ mode it stutters (p.69) |
| CUE | Set, return or sample the cue (§5) |
| SEARCH ◀◀ / ▶▶ | Scan, or frame search when paused. With the jog: super-fast search |
| TRACK SEARCH ◀◀❘ / ❘▶▶ | Previous/next track. Press once to restart, twice for the previous track. With the jog: super-fast track search |
| SLIP REV / FWD / REV lever | Three-position lever. SLIP REV is momentary slip-reverse; REV latches reverse. It lights during reverse (p.70) |

### Loop, cue and beat column

| Control | Effect |
|---|---|
| BEAT JUMP ◀ / ▶ | Jump or loop-move by the Beat Jump value. **CALL/DELETE + BEAT JUMP** changes the value (p.89) |
| 4 BEAT LOOP (1/2X) | 4-beat loop; while looping, halves it |
| 8 BEAT LOOP (2X) | 8-beat loop; while looping, doubles it |
| LOOP IN/CUE (IN ADJUST) | Loop in, or a real-time cue while playing. Long press: 4-beat loop. While looping: adjust the in point |
| LOOP OUT (OUT ADJUST) | Loop out; while looping: adjust the out point |
| LOOP RELOOP/EXIT | Exit or re-enter the loop; also cancels a loop called from a hot cue |
| HOT CUE A–H (8 pads) | Set, play or gate a hot cue; coloured LEDs |
| CALL/DELETE | Hold and press a pad to delete. Press alone for hot-cue call mode. Also the beat-jump-value modifier |
| SLIP | Slip mode. Lit when on, blinks while slipping |
| QUANTIZE | Quantize on/off; shows QUANTIZE + value on screen. Also lit in power-off standby |
| CUE/LOOP CALL ◀ / ▶ | Call memory cues/loops. While looping: ◀ halves, ▶ doubles |
| MEMORY | Store the current cue or loop |
| DELETE | Delete the memory cue/loop that was called up |

### Jog block

| Control | Effect |
|---|---|
| JOG MODE + VINYL/CDJ indicator | Switch Vinyl ⇄ CDJ |
| JOG FEEL knob | Jog weight |
| VINYL SPEED ADJUST knob | Brake/start speed (§5) |
| Jog wheel with jog display and ring illumination | Pitch bend, scratch, frame search |

### Tempo and sync (right side)

| Control | Effect |
|---|---|
| MASTER | Become sync master, or give up master |
| KEY SYNC | Match key to the master. Press again to reset |
| BEAT SYNC/INST.DOUBLES | Short press: sync on/off. Long press: Instant Doubles |
| TEMPO ±6/±10/±16/WIDE | Cycle the tempo range; the screen badge (24) follows |
| MASTER TEMPO | Key lock; MT on screen. Also resets a key sync |
| TEMPO slider | Speed |
| TEMPO RESET + indicator | Play at the original speed whatever the slider says |

### Other hardware

- USB1/USB2 ports, each with a STOP button and an indicator. Hold STOP until the indicator stops blinking before unplugging (p.54).
- Front: NFC login area and indicator (3000X).
- Rear: analog out, S/PDIF digital out (96 kHz/24-bit), LINK (gigabit), USB-C to PC (p.18, p.116–117).

---

## 5. Playback features

### Transport and search

| Feature | Behaviour | Page |
|---|---|---|
| Play/pause | Vinyl mode: pause mutes. CDJ mode: pause stutters. Play Mode decides whether the next track auto-loads | p.69 |
| Needle search | Touch the overview while paused, or in Vinyl mode with the jog top pressed. Plays from that point; drag to scrub | p.69 |
| Touch Cue | Touch the overview while playing to hear that point in the headphones (needs LINK CUE on the mixer). Main output is unaffected | p.72 |
| Touch Preview | The same, from a list's preview column, before loading | p.60 |
| Fast search, frame search, track search | SEARCH and TRACK SEARCH buttons; the jog adds the super-fast variants | p.70, p.73–74 |
| Reverse | REV side of the lever. Jog bend direction flips too | p.70 |
| Slip reverse | SLIP REV side; normal playback carries on in the background. Works without Slip mode | p.70, p.91 |
| Emergency Loop | If the next track can't start at the end of the track, a beat loop plays automatically and DJ ops are locked. Loading a track cancels it. Not applied if the file is cached | p.82 |
| LOAD PREVIOUS TRACK | MENU on the waveform screen | p.56 |
| Eject / Load lock | See settings | p.101 |

### Tempo

| Feature | Behaviour | Page |
|---|---|---|
| Tempo ranges | ±6 % (0.02 % steps), ±10 % (0.05 %), ±16 % (0.05 %), **WIDE ±100 % (0.5 %)**. −100 % stops the track | p.71 |
| Tempo Reset | Button plus indicator; original speed whatever the slider says | p.71 |
| Master Tempo | Key lock while changing tempo | p.71 |

### Jog

| Feature | Behaviour | Page |
|---|---|---|
| Vinyl mode | Pressing the top stops playback; turning it while pressed scratches | p.73 |
| CDJ mode | No stop or scratch; the top acts as a bend surface | p.73 |
| Pitch bend | Outer ring speeds up or slows down; in CDJ mode the top does too | p.73 |
| JOG FEEL | Weight knob | p.74 |
| Vinyl Speed Adjust | Knob sets brake/start speed (clockwise is slower). The utility chooses what it affects: Touch (stop only; the default), Release (start only), or Touch&Release (both). It also applies to the ▶/❚❚ stop | p.74–75, p.102 |

### Cues

| Feature | Behaviour | Page |
|---|---|---|
| Set cue | CUE while paused (fine-tune with SEARCH or the jog), or LOOP IN/CUE while playing. A new cue replaces the old one | p.76 |
| Back cue | CUE while playing returns to the cue point and enters cue standby | p.76 |
| Cue Point Sampler | Hold CUE at the cue point to play; ▶ while holding keeps it playing | p.77 |
| Memory cues/loops | MEMORY stores; CUE/LOOP CALL ◀/▶ recalls; DELETE removes the called one | p.77–78, p.83 |
| Auto Cue | On load or track search, cue just before the first sound. Turned on in SHORTCUT. The silence threshold is Auto Cue Level, which can instead be set to the first memory cue or first hot cue | p.78, p.102 |
| Smart Cue | Setting a hot cue, or playing from one, also moves the main cue point there | p.76, p.85, p.102 |

### Hot cues and gate cue

| Feature | Behaviour | Page |
|---|---|---|
| Hot cues A–H | 8 per track. Press an empty pad to set one; while looping, a pad stores the loop. An occupied pad must be deleted before it can be reused | p.84 |
| Hot cue colours | Hot Cue Color Off: cue is green, loop is orange, empty is unlit. On: cues use their rekordbox colour, loops stay orange | p.84 |
| Play a hot cue | Press: play from the point. If the cue holds a loop, RELOOP/EXIT leaves it | p.85 |
| Gate Cue | Gate Cue on (UTILITY, SHORTCUT, or the GATE CUE key). Hold a pad while paused to play; release to stop. With a loop, it loops. ▶ during gate latches playback | p.85 |
| Delete a hot cue | CALL/DELETE + pad | p.86 |
| Hot Cue Auto Load | On: hot cues load with the track (AUTO CUE lit red). Off: **manual call**. Press CALL/DELETE and the stored pads blink; press a pad to call it. Call mode ends when all are called, CALL/DELETE is pressed, or the track changes | p.86–87 |
| Hot Cue Bank list | BROWSE → HOT CUE → bank → track sets A–H. Edited only in rekordbox; not on CloudDirectPlay | p.87, p.50 |

### Loops

| Feature | Behaviour | Page |
|---|---|---|
| Manual loop | LOOP IN, then LOOP OUT | p.79 |
| Auto loops | Long-press LOOP IN for a 4-beat loop. 4 BEAT LOOP and 8 BEAT LOOP buttons. BEAT LOOP touch panel: 1/4–32 on page 1 | p.79–80 |
| Loop adjust | IN ADJUST or OUT ADJUST while looping, then SEARCH or the jog to move the point. Press again, or wait 10 s, to exit | p.81 |
| Halve / double | 4 BEAT LOOP (1/2X) or CUE/LOOP CALL ◀ halves. 8 BEAT LOOP (2X) or CUE/LOOP CALL ▶ doubles | p.81 |
| Exit / reloop | RELOOP/EXIT | p.82 |
| Active Loop | A saved loop marked active in rekordbox engages automatically when the playhead passes it. Deleting that memory loop also deletes the active loop | p.82–83 |

### Beat jump and slip

| Feature | Behaviour | Page |
|---|---|---|
| Beat Jump / Loop Move | BEAT JUMP ◀/▶ jumps by the Beat Jump value (1/2–64, default 16), or moves the loop while looping. CALL/DELETE + BEAT JUMP changes the value. The touch panel offers per-value ◀/▶ | p.89–90 |
| Slip mode | SLIP on, then perform; on release, play resumes where it would have been. Variants: **Slip Pause** (Vinyl mode ❚❚), **Slip Scratch**, **Slip Loop**, **Slip Beat Loop** (hold a BEAT LOOP value), **Slip Reverse**, **Slip Hot Cue/Loop** (hold a pad). Turned off by SLIP or by loading a track. Slip Flashing makes the eligible buttons blink | p.91–92, p.104 |

### Quantize, sync and grid

| Feature | Behaviour | Page |
|---|---|---|
| Quantize | Snaps cue, loop in/out and hot cue to the grid; keeps hot cues, loops, reverse and slip on the beat. Beat value 1/8, 1/4, 1/2 or 1. Analysed tracks only | p.88, p.101 |
| Beat Sync | MASTER on one deck, BEAT SYNC on another; BPM and phase follow the master. While synced the slider is ignored. Turning sync off holds the tempo until the slider passes it ("pickup"). Pitch bend on a synced deck drops back to BPM-only sync. rekordbox can be the master | p.93 |
| Master handoff | The master changes when its track changes or pauses, when MASTER is pressed on the master, or when MASTER is pressed on another deck | p.94 |
| Instant Doubles | Long-press BEAT SYNC: loads the master's track and plays it from the same position | p.94 |
| Beat grid adjust | §1.13 | p.95 |

### Key

| Feature | Behaviour | Page |
|---|---|---|
| Key Sync | KEY SYNC shifts to the smallest move onto the master's key family: same, dominant, subdominant, relative, relative-of-dominant, or relative-of-subdominant. KEY SYNC again, or MASTER TEMPO, resets it | p.96 |
| Key Shift | Touch panel, ±1 semitone per tap, RESET | p.97 |
| Key matching display | Keys related to the master's turn green, in lists and in readout (28) | p.26, p.58 |

### Library, cache and integration

| Feature | Behaviour | Page |
|---|---|---|
| File cache | Once a track is cached (icon 16), pulling its USB doesn't unload it; it keeps playing with restrictions | p.25, p.54 |
| History | Tracks played for about 1 min are logged and shown green. PLAYED/UNPLAYED edits the history | p.61, p.67 |
| Information Jump | From INFO | p.59 |
| DJ software control | MIDI/HID over USB-C; SOURCE → SOFTWARE CONTROL. Uses the MIDI channel setting | p.98–99 |
| PRO DJ LINK | Up to 6 players (4 when older players are on the link); 1000BASE-T; player no. shown bottom left | p.32, p.117 |
| Formats | MP3, AAC, WAV, AIFF, ALAC, FLAC; 44.1–96 kHz, 16/24-bit. USB: FAT16/32, exFAT or HFS+, up to 8 folder levels | p.11–12 |

---

## 6. Settings

`*` = factory default. `†` = copied by Duplication and stored in My Settings.

### 6.1 UTILITY (long-press MENU, p.101–107)

**DJ Settings**

| Item | Values | Notes |
|---|---|---|
| Eject/Load Lock† | Eject Lock: Unlock* / Lock. Load Lock: Unlock* / Lock | Blocks unloading or loading during playback |
| Quantize Beat Value† | 1/8, 1/4, 1/2, 1* beat | |
| Beat Jump Beat Value† | 1/2, 1, 2, 4, 8, 16*, 32, 64 beat | |
| Hot Cue Auto Load† | Off, On* | |
| Hot Cue Color† | Off*, On | Pad LED colours |
| Gate Cue† | Off*, On | |
| Overwrite Current Cue with Hot Cue (Smart Cue)† | Off*, On | |
| Auto Cue Level† | Memory Cue*, Hot Cue, −78, −72, −66, −60, −54, −48, −42, −36 dB | |
| Vinyl Speed Adjust† | Touch&Release, Touch*, Release | What the knob affects |
| Play Mode† | Continue, Single* | |

**Display (LCD)**

| Item | Values | Notes |
|---|---|---|
| LCD Brightness† | 1–5 (3*) | |
| Jog LCD Brightness† | 1–5 (3*) | |
| Language† | 13 languages, seen in the image: EN, FR, DE, IT, NL, ES, RU, KO, zh-Hans, zh-Hant, JA, PT, SV (img p.100) | |
| Screen Saver | Off, On* | 5 min with no track loaded; 100 min paused, in cue standby, or at end of track |

**Display (Indicator)**

| Item | Values | Notes |
|---|---|---|
| Slip Flashing† | Off, On* | Slip-capable buttons blink when SLIP is pressed |
| On-Air Display† | Off, On* | On-air state from the mixer |
| Jog Ring Brightness† | Off, 1, 2* | |
| Jog Ring Indicator† | Off, On* | Ring blinks near the end of the track |
| Rotary Selector Brightness† | Off, 1, 2* | |

**PRO DJ LINK**

| Item | Values | Notes |
|---|---|---|
| Player No. | Auto*, 1–6 | Locked while a storage device is connected |
| Duplication | ALL*, PLAYER 1–6 | Pushes the † settings plus TIME MODE, AUTO CUE, JOG MODE, TEMPO RANGE, MASTER TEMPO, QUANTIZE, BEAT SYNC and PHASE METER to other players. Not to a player that is playing |
| MIDI Channel | 1*–16 | |

**Network (3000X)**

| Item | Values | Notes |
|---|---|---|
| Wi-Fi Setting | Off*, On | Opens the SSID list (p.108) |
| Wi-Fi Address | (read-only) | MAC address |
| Ethernet Info | (read-only) | IP and MAC addresses |

**System**

| Item | Values | Notes |
|---|---|---|
| Power Management | Never, 20min* | Auto-off after 20 min unused and unconnected; QUANTIZE lit in standby (p.107) |
| Output Att. | −12, −9, −6, −3, 0* dB | Ignored when DJ software is in use |
| History Name | Up to 32 characters | Renaming restarts the counter at 001 |
| Demo Mode† | Off*, On | All button LEDs on |
| Restore Defaults | Cancel*, Ok | |

**Info**

| Item | Values | Notes |
|---|---|---|
| Serial No. | (read-only) | |
| License | (read-only) | |
| Version No. | (read-only) | |

### 6.2 SHORTCUT (p.111–112, img p.110)

| Group | Item | Values |
|---|---|---|
| Player settings | Waveform/Phase Meter | WAVEFORM*, PHASE METER |
| | Time Mode | REMAIN*, TIME |
| | Auto Cue (labelled "Auto Cue (Memory Cue)" in the image) | OFF, ON* |
| | Hot Cue Auto Load | OFF / ON (as UTILITY) |
| | Smart Cue | OFF / ON |
| | Gate Cue | OFF / ON |
| | Quantize Beat Value | 1/8, 1/4, 1/2, 1 |
| | Beat Jump Beat Value | 1/2, 1, 2, 4, 8, 16, 32, 64 (two rows) |
| | Vinyl Speed Adjust | TOUCH, TOUCH RELEASE, RELEASE |
| | LCD Brightness / Jog LCD Brightness | sliders |
| Device information | Source device icon and name | (display only) |
| Device settings (per device) | Waveform Color | BLUE, RGB, 3BAND |
| | Waveform Current Position | LEFT, CENTER |
| | Waveform Divisions | TIME SCALE (marks every 30 s), PHRASE (rekordbox phrase data; falls back to time scale) |
| | Image On-Jog Display | ARTWORK, CUSTOM IMAGE |
| My Settings | LOAD / SAVE | Not while playing (p.114) |
| (header) | ⚙ UTILITY | Jumps to UTILITY |

### 6.3 Per-device settings (SOURCE → ⚙, p.19–20)

| Tab | Items |
|---|---|
| Local Library | Waveform Divisions, Image On-Jog Display, **Background Color** (device icon and part of the UI colour), MY SETTINGS LOAD |
| Cloud Library | The same, plus **Display username on-screen** and **Log Out Time** (auto-logout when idle) |
| Streaming | TIDAL file format for cloud registration |

### 6.4 My Settings (p.113–115)

- **What's saved:** the † UTILITY items plus TIME MODE, AUTO CUE, JOG MODE, TEMPO RANGE, MASTER TEMPO, QUANTIZE, BEAT SYNC and PHASE METER.
- **Where:** USB, rekordbox, CloudDirectPlay, or a device on another linked player.
- **Save from:** SHORTCUT → SAVE.
- **Load from:** SHORTCUT → LOAD, or SOURCE → MY SETTINGS LOAD (also in its ⚙). Only while paused.

---

## 7. Differences vs the XDJ-1000MK2

The comparison uses the XDJ-1000MK2 manual (`reference/pioneer/XDJ-1000MK2_manual.pdf`) and
`xdj-1000mk2-screens.md`.

### CDJ-3000(X) has, XDJ-1000MK2 doesn't

**Navigation**
- **Hardware navigation buttons:** SOURCE, BROWSE, TAG LIST, PLAYLIST, SEARCH and MENU. The XDJ uses on-screen touch keys instead (BROWSE(SEARCH) / TAG LIST / INFO(LINK INFO) / MENU(UTILITY) / PERFORM, plus USB / LINK / rekordbox source keys).
- **SOURCE screen** with device icons, a library list, cloud and streaming rows, per-device ⚙ settings, and SOFTWARE CONTROL.
- **PLAYLIST screen with Playlist Bank** shortcuts and an EDIT mode for reordering and deleting tracks.
- **List-screen extras:**
  - a preview-waveform column with **Touch Preview**;
  - sortable column headers and sub-column picker;
  - font-size toggle;
  - artwork column;
  - **GLOBAL** tag list.
- **SEARCH as its own screen** with a paste key, multi-keyword search and Beatport target selection. The XDJ also has an on-screen keyboard, but it is reached by holding BROWSE.
- **INFO** with player tabs 1–6 and EDIT RATING.

**Waveform screen**
- **Touch Cue** on the overall waveform during playback.
- Track-info line with artwork and an eye toggle.

**Hot cues, loops and slip**
- **8 physical hot-cue pads A–H** with RGB colours and a dedicated CALL/DELETE button. The XDJ has 4 touch pads per bank on the PERFORM screen, with A–D / E–F–G–H banks.
- **Gate Cue** (setting plus an on-screen GATE CUE toggle).
- **Smart Cue** (hot cue overwrites the main cue).
- **Physical loop and beat controls:** BEAT JUMP ◀▶, 4 BEAT LOOP (1/2X), 8 BEAT LOOP (2X), and a dedicated SLIP button. On the XDJ these are PERFORM-screen pads and a touch key.
- **SLIP REV / FWD / REV lever.** The XDJ has a DIRECTION REV button.
- **BEAT LOOP touch panel** offering 1/4…32 (paged). The XDJ PERFORM pads offer 1/2…16.
- **BEAT JUMP touch panel** with per-value ◀/▶, and a configurable **Beat Jump Beat Value** (1/2–64) for the hardware buttons.

**Key and grid**
- **KEY SYNC** (hardware) and **KEY SHIFT** (semitone panel).
- Key readout and list keys turn **green** when compatible with the master.
- **Grid Adjust touch buttons:** SNAP GRID(CUE), SHIFT GRID, ±1/2, ×2/×1/2, Enlarge/Reduce, RESET.

**Jog**
- **Jog-centre colour LCD** with artwork or a custom image, SLIP/SYNC/VINYL/MASTER indicators, a rotating playback marker and a cue marker.
- **Jog ring illumination**, including blinking near the end of the track.
- **JOG FEEL** weight knob.
- **Vinyl Speed Adjust** can target Touch, Release or both. On the XDJ one TOUCH/RELEASE knob sets both.

**Settings**
- New settings:
  - **Eject Lock**, separate from Load Lock;
  - Jog LCD Brightness;
  - Jog Ring Brightness;
  - Jog Ring Indicator;
  - Rotary Selector Brightness;
  - **Output Att.**;
  - Demo Mode;
  - Restore Defaults.
- **Display settings:**
  - Waveform Current Position (CENTER/LEFT);
  - Waveform Divisions with **PHRASE** data;
  - Waveform Color BLUE/RGB/3BAND;
  - Image On-Jog Display.

**Hardware, network and library**
- **6 players** on PRO DJ LINK (the XDJ supports 4), a gigabit LINK port, 2 USB ports with STOP buttons, and a **file cache** (survives pulling the USB).
- Up to **96 kHz** playback; the XDJ tops out at 48 kHz.
- 3000X only: Wi-Fi, cloud (CloudDirectPlay), Beatport/TIDAL streaming, NFC/QR login, and OneLibrary-only USB.

### XDJ-1000MK2 has, CDJ-3000(X) doesn't (or not like this)

**Screens**
- **PERFORM screen:** touch pads for hot cues (with BANK switch, HOT CUE/BEAT JUMP mode toggle and HOT CUE DELETE(CALL)) and BEAT LOOP pads. On the CDJ these are hardware pads plus touch overlays on the waveform screen.

**On-screen keys and readouts**
- **On-screen MEMORY, DELETE and CUE/LOOP CALL ◀▶ keys.** These are hardware on the CDJ.
- **On-screen BEAT SYNC (INST.D.) and BEAT SYNC MASTER keys.** These are hardware on the CDJ.
- **On-screen SLIP key** in the left column.
- **NEEDLE SEARCH** strip/key and its NEEDLE COUNTDOWN caption. On the CDJ needle search is done on the overview.
- **Separate A.CUE and A.HOTCUE (HOT CUE AUTO LOAD) indicators.** The CDJ folds these into the AUTO CUE caption, which lights red.
- **REMAIN caption and a cache meter on the overview.** The playing-address bar flashes in the last 30 s and 15 s.

**Hardware buttons**
- **TIME MODE/AUTO CUE hardware button.** The CDJ moves Time Mode and Auto Cue to SHORTCUT.

**Settings**
- **UTILITY items the CDJ lacks:**
  - DIGITAL OUT 16/24-bit;
  - TOUCH DISPLAY CALIBRATION;
  - LINK STATUS;
  - HOT CUE AUTO LOAD's third value, "rekordbox SETTING";
  - DUPLICATION "DEFAULT";
  - BACKGROUND COLOR and WAVE FORM COLOR under MENU → MY SETTINGS. The CDJ moves these to SOURCE ⚙ and SHORTCUT.

**Jog**
- **Jog display section:** a simple ring with an operation marker (1.8 s per revolution), a cue/slip marker, an **audio-memory status** indicator, a jog-touch light and a VINYL light. There is no artwork LCD.

**Tempo**
- **WIDE range is −100…+50 % for FLAC/ALAC.** The CDJ manual just says ±100 %.

### Same on both

Most behaviour is the same on both decks:

- **Cueing and loops:**
  - cue, back-cue and cue sampler;
  - real-time cue on LOOP IN;
  - loop in/out with ADJUST and the 10 s timeout;
  - reloop/exit;
  - halve/double via CUE/LOOP CALL;
  - active loop;
  - emergency loop;
  - memory cue/loop store, call and delete.
- **Hot cues:** 8 hot cues A–H with colour rules, hot cue call mode, and Hot Cue Bank lists.
- **Sync and quantize:** quantize 1/8–1; beat sync, master and Instant Doubles.
- **Tempo:** tempo ranges ±6/10/16/WIDE and Master Tempo.
- **Jog and slip:** jog Vinyl/CDJ modes, slip variants, and Zoom/Grid Adjust via a long rotary press.
- **Library:**
  - Track Filter with MASTER PLAYER;
  - Alphabet/Page Jump;
  - history with PLAYED/UNPLAYED;
  - tag list;
  - LOAD PREVIOUS TRACK;
  - INFO Information Jump;
  - My Settings and Duplication.
- **Waveform screen:** beat countdown, the phase meter ⇄ waveform toggle, and on-air display.
- **PRO DJ LINK:** Link Export/USB Export, plus MIDI/HID software control.

---

## Gaps in the manual (not documented; verify on hardware or the emulator)

- BEAT LOOP and BEAT JUMP **page 2** values.
- What the **track menu** (tap on a track) contains.
- The Track Filter editor layout.
- The Grid Adjust screen layout.
- The phase-meter graphic in phase-meter mode.
- The meaning of the two stacked beat-countdown readouts. Our XDJ notes say orange counts to the memory cue and blue to the hot cue; the CDJ manual only says "nearest saved cue".
- Hot-cue **comments**: not mentioned in the 3000X manual.
- Slip auto-release lengths: the XDJ manual gives 8 beats for slip reverse and 4 for slip hot cue; the CDJ manual doesn't say.

# Numark DJ2Go MIDI map

From the Mixxx `Numark DJ2Go.midi.xml` mapping, confirmed against the hardware
2026-08-26. **All messages are on MIDI channel 0**; the two decks are told
apart by note/CC *number*, not by channel. Buttons send Note On (0x90) on
press except Load/Back/Enter, which fire on Note Off (0x80). Faders, jog, and
the browse knob are Control Change (0xB0).

## Per-deck controls

| Control | Deck A (left) | Deck B (right) | Kind | freedj event |
|---|---|---|---|---|
| Play      | note 0x3B | note 0x42 | 0x90 | `PlayPause` |
| Cue       | note 0x33 | note 0x3C | 0x90 | `Cue` |
| Sync      | note 0x40 | note 0x47 | 0x90 | `SyncToggle` |
| Loop In   | note 0x44 | note 0x46 | 0x90 | `LoopIn` |
| Loop Out  | note 0x43 | note 0x45 | 0x90 | `LoopOut` |
| PFL (headphone) | note 0x65 | note 0x66 | 0x90 | — (needs a cue bus) |
| Load      | note 0x4B | note 0x34 | 0x80 | `Load` |
| Shift     | note 0x4B | note 0x34 | 0x90 | — (Load note on press) |
| Pitch fader | CC 0x0D | CC 0x0E | 0xB0 | `TempoFader` |
| Jog wheel   | CC 0x19 | CC 0x18 | 0xB0 | `JogDelta` |
| Channel volume | CC 0x08 | CC 0x09 | 0xB0 | — (no mixer) |

Note: Play and Cue are **not** in note order (Cue 0x33 < Play 0x3B); an
earlier hand-guessed mapping had them swapped. The two small buttons at
0x43/0x44 are **Loop Out/In**, not pitch bend.

## Shared controls

| Control | MIDI | Kind | freedj event |
|---|---|---|---|
| Select knob (browse) | CC 0x1A | 0xB0 relative | `BrowseEncoderDelta` |
| Back  | note 0x59 | 0x80 | `Back` |
| Enter | note 0x5A | 0x80 | `Load` (loads the selection) |
| Crossfader | CC 0x0A | 0xB0 | — (no mixer) |
| Master volume | CC 0x17 | 0xB0 | — |
| Headphone volume | CC 0x0B | 0xB0 | — |

## Selecting a deck

`--deck A` (default) or `--deck B` picks the note table. Two freedj instances
split the controller:

```
./opendeck track1.mp3 --player 1 --deck A
./opendeck track2.mp3 --player 2 --deck B
```

Source: <https://github.com/mixxxdj/mixxx> `res/controllers/Numark DJ2Go.midi.xml`

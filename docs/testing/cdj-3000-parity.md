# CDJ-3000 parity: freedj against the emulated unit

`tools/cdj_parity.py` holds freedj's CDJ-3000 skin against a running
cdj3k-emu slot, feature by feature. One scenario drives both decks: freedj
through `OPENDECK_SCRIPT` (its `press=`, `hold=`, `release=`, `rotary=`,
`lamps=` and `shot=` actions take the emulator's button names) and the
emulator through its control API, on the same timeline. At each `check` it
captures both screens and both sets of lamps, and the report puts the screens
side by side (unit left, freedj right) and lists the lamps that differ.

```sh
cargo build --release -p opendeck-app
tools/cdj_parity.py --track "/path/AC Slater - Bass Face.mp3" --out /tmp/parity
# → /tmp/parity/report.md, one PNG per check
```

The scenario is `tools/cdj_parity/features.txt`: one check per feature the
skin has (lamped modes, readouts, hot cues and their delete, memory points,
4 / 8-beat loops and halve / double, manual loops, BEAT JUMP, TRACK SEARCH,
SEARCH scanning, AUTO CUE, every list screen, SYNC and MASTER alone).

## Before a run

- The emulator slot is up with Developer Mode on (`cdj3k-emu --dev`, or
  `--dev --headless`), with a USB image attached that holds the track. The
  harness loads it from BROWSE (the highlighted row; it loads the next row
  first, since the unit ignores a LOAD of the track it already has).
- The harness then puts the unit in freedj's fresh state: no hot cues,
  QUANTIZE and MASTER TEMPO on, SLIP and SYNC off, JOG MODE CDJ, TEMPO ±16,
  elapsed time, AUTO CUE on. Memory points the unit stored for the track in
  an earlier session stay, and show in the screens.
- freedj runs headless (xvfb, X11) with fresh settings (`XDG_DATA_HOME`
  under `--out`), paused at launch, pinned to loopback as player 2: nothing
  it does, MASTER included, reaches the LAN. Run one harness at a time, and no
  other freedj on loopback meanwhile: a second one shows up as a linked
  player and changes what SYNC and MASTER do.

Nothing Pioneer-owned goes into the repo: the report and images stay under
`--out`.

## What it found (2026-11-09)

Fixed from the first runs: the CDJ faceplate's layout (now measured off the
emulator's panel drawing), the screen keys' dim/full lamps, RELOOP (lit once a
loop is stored), IN / OUT blinking in a loop, QUANTIZE and BEAT JUMP boxes,
A.HOT CUE and AUTO CUE badges, the time caption and ruler in elapsed mode,
the tempo-range badge colours and WIDE's one decimal, hot-cue / memory / cue
markers on the waveforms, the Bars rows (the lower one counts to the next
memory point, red 00.0 on it), SEARCH's speed (about 20× for 1.5 s, then
50×), TRACK SEARCH (reloads to the start cue; wraps round the folder),
hot-cue delete (hold CALL / DELETE, press the pad), the SEARCH key toggling
SEARCH off, BACK doing nothing in SEARCH, and SYNC / MASTER with no other
player (a "No LINKED Players found." pop-up, nothing engaged).

The last run: all 31 checks with matching lamps.

## Known differences

- The upper Bars row: the unit counts to a touched overview point; freedj
  shows "--.-".
- The unit's SEARCH keyboard dims after BACK; freedj's does not.
- The unit keeps memory points per track across loads without rekordbox;
  freedj keeps them for the session.

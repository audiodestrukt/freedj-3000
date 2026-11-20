# CDJ-3000s never drew OpenDeck's waveform when it was master

**Status:** fixed (2026-11-20), on two emulated CDJ-3000s (cdj3k-emu,
firmware 3.22); not yet seen on a real unit.  Issue #66.
**Affected:** every CDJ-3000 on a LINK network where OpenDeck is tempo
master.
**Reported by:** Dan, "How about getting the waveform data in the beat
meter?"

---

## Symptom

A CDJ-3000's WAVEFORM view, the phase-meter slot that Pioneer markets as
the **stacked waveform**, draws the tempo master's waveform and beat grid
thinly above the unit's own.  With a CDJ as master it fills in.  With
OpenDeck as master it showed OpenDeck's orange "4" badge over an empty
strip.

## How a CDJ-3000 gets the master's waveform

From captures between two emulated units:

- The watching unit **fetches once and draws locally** from then on,
  steered by the master's position packets.  Nothing is streamed.
- It fetches the moment the master **loads a track** (even paused), and
  again when a new master starts playing.  With a CDJ master it asked 13 ms
  after the status went to playing+master.
- The fetch is over the master's dbserver, on the session the unit already
  has open: PWV7 (3-band detail, `0x2d04`), PWV5 (colour detail, `0x2c04`),
  the detail waveform (`0x2904`), the beat grid (`0x2204`), PQT2 and the
  cue list (`0x2b04`).  No NFS.
- **Tapping the meter sends nothing.**  The view draws what is already
  there.
- The watcher doesn't need a track of its own: a unit with nothing loaded
  fetched as well.

## The cause

One byte of OpenDeck's CDJ-3000 status packet: **offset `0xcd`**.  A
CDJ-3000 sends `0xf3` there, and OpenDeck sent `0x03`.  The first 0x124
bytes of OpenDeck's status are built from an **XDJ-1000MK2** capture
(`status_template.rs`).  The XDJ has `0x03` at `0xcd`, and a CDJ-3000 will
not fetch analysis from a master that sends it.  Everything else about the
packet was fine, and the dbserver conversation (metadata, menu rows, album
art) was answered correctly all along.

The fix is one line in `build_status_cdj3000`: `p[0xcd] = 0xf3`.  The
XDJ-format status (sent to older players) keeps the XDJ's value.

## Tests

- `cdj3000_status_lets_peers_fetch_our_waveform`: the byte is `0xf3` in
  the CDJ-3000 status, master or not, and stays `0x03` in the XDJ status.
- `cdj3000_status_matches_a_captured_master_byte_for_byte`: OpenDeck's
  whole CDJ-3000 status, built from the live values of a captured master
  (`crates/link/src/testdata/cdj3000_master_status.bin`), must equal that
  capture apart from a written list of known differences, each with its
  reason and whether it was tested.  Without the fix it fails with
  `0x0cd: ours 0x03 cdj 0xf3`.
- On the emulators: plain OpenDeck as master, three tracks loaded in a row.
  Both CDJs fetched each one within 220 ms of the load.

## The same bug, three times

All three came from copying one device's capture into packets that a
different device reads:

| When | Byte | Symptom | Found |
|---|---|---|---|
| Aug 2026 | status `0x1f` (`0x00` from a redacted XDJ capture, real devices send `0x01`) | an XDJ started handing master to OpenDeck, then aborted every time | dd1eee1, after days of handshake theories |
| 2026-11-20 | keep-alive `0x21` / `0x35` (XDJ's `0x02` / `0x00`, CDJ-3000 sends `0x03` / `0x64`) | every CDJ-3000 showed "Linked to player with older firmware" | 4bb0069, by diffing one announce packet |
| 2026-11-20 | status `0xcd` (XDJ's `0x03`, CDJ-3000 sends `0xf3`) | no CDJ-3000 fetched OpenDeck's waveform as master | this fix, by bisecting |

**The lesson:** when a player ignores or refuses OpenDeck, diff OpenDeck's
packets against a real player's in the same role before forming any
theory.  The guard test above now does that for the status packet on every
build.

## Why it took a day

- **Guessing before diffing.**  I tested one idea at a time: the export,
  the model name, the firmware string, `0x9d`, load order, who watches.
  Each run took 2 to 3 minutes and each was a guess.  The method that
  worked was to send a captured master's bytes, confirm the fetch, then
  halve.  That was available from the start.
- **A wrong "ruled out".**  The first full-template run looked like it made
  the CDJs stop even asking for metadata, so I dropped the status packet
  as the cause.  That was a timing artifact of short runs.
- **Worn-out emulators.**  After about 25 OpenDeck restarts the emulated
  units' behaviour drifted: some runs got no metadata request at all, with
  the same inputs as runs that did.  Two successful fetches then "failed to
  reproduce", and the first bisect gave nonsense.  Restarting the firmware
  (`restart` on the control API) cleared it.  Running the bisect inside
  **one** OpenDeck session, reloading the byte ranges from a file and
  loading a fresh track each step, got the answer in 9 steps (about 7
  minutes).
- **Environment snags:** rekordbox under Wine held the LINK and dbserver
  ports twice; a locked desktop stalled OpenDeck's frame-driven script; a
  `pkill -f` matched my own shell.

## How to reproduce the hunt

Two emulated units on `djlink0` (`cdj3k-emu --instance N --headless --dev`,
control API on 4400+N).  Capture a unit's traffic through QEMU's QMP socket
(`object-add filter-dump` on `net0`).  The bisect hook used here
(`OPENDECK_STATUS_TEMPLATE`, `OPENDECK_STATUS_RANGES_FILE`) was not kept;
the guard test replaces it.  Details in
[`../reference/cdj-3000-master-and-waveform-view.md`](../reference/cdj-3000-master-and-waveform-view.md).

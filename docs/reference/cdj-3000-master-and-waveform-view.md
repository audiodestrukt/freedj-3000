# CDJ-3000: the MASTER button and the WAVEFORM view

Observed on two emulated CDJ-3000s (cdj3k-emu, firmware 3.22, slots 1 and 2
on the private bridge `djlink0`) with OpenDeck as a third player, 2026-11-18.
Behaviour only: what the units did and what they asked each other for.
Answers questions raised by PR #54.

## MASTER pressed on the tempo master

- The unit **hands the role on**; it does not leave the network without a
  master.  Its status names the next master (the handoff byte, here
  player 4), that player takes it, and the unit drops its master flag.
- It only hands to a player that is **playing**.  With the only other
  player stopped (its track had ended), the press did nothing: the unit
  stayed master, no handoff in its status.
- Pressed on a unit that is not master, MASTER asks for the role, as
  before.
- Not yet seen: which player it picks when several are playing.

PR #54 makes our deck release the role to nobody; to match the unit it
should hand it to a playing player and keep it when no one else plays.

## The WAVEFORM view in the phase-meter slot

The manual's item 11 on the waveform screen ("Waveform/phase meter: toggles
between the phase-meter and waveform view", p.24); UTILITY → Waveform/Phase
Meter picks the default (WAVEFORM).  Tapping the slot toggles.

What it draws: the **tempo master's** detail waveform, thin, in the master's
waveform colour (3-band here), with the master's beat ticks along both
edges (red on the first beat of a bar), above this unit's own enlarged
waveform and sharing its white playhead line.  The master's player number
shows in an orange badge at the left.  On the master itself the slot reads
**MASTER PLAYER** in an orange box.

How it gets the data: ahead of time, not when the view opens.

| When (slot 1 watching slot 2) | Slot 2's status | Slot 1 asks slot 2's dbserver |
|---|---|---|
| 49.7 s | track 68 loaded (flags 0x84) | `0x2002` metadata, track 68 |
| 59.0 s | flags 0xe4: playing + master | `0x2d04` PWV7 (.2EX), `0x2c04` PWV5, `0x2904` detail waveform, `0x2204` beat grid, `0x2c04` PQT2, `0x2b04` cues |

Opening the view later sent no requests: it draws what it already has.  Both
flag bits turned on together, so whether master or playing triggers the
fetch is open (master is the likely one).

PR #54's view: same idea and nearly the same data (detail waveform, beat
grid, PWV7, PWV5), but it follows the player whose beat packets feed the
phase meter (the last beat sender), not the master.  To match the unit it
should follow the master.

## OpenDeck as the master

With OpenDeck (player 4) as master and playing, a CDJ-3000 shows the "4"
badge but no waveform: it never fetches OpenDeck's analysis.  What was
tried (2026-11-18, both emulated units watching, traffic captured):

| OpenDeck serving | The CDJs did | Analysis fetched |
|---|---|---|
| a plain folder | NFS LOOKUP `PIONEER` / `.PIONEER` (not found); dbserver setup + `0x3007`; at most one `0x2002` metadata | no |
| a folder with an empty `PIONEER/rekordbox` | looked inside for `PIONEER/djprofile.nxs` and `PIONEER/CDP`; both asked `0x2002` metadata for the loaded track | no |
| the same, with the title row's flag set to `0x0100_0000` as a CDJ sends it | the same | no |

OpenDeck's status for its track has the layout a CDJ's has (source player
4, USB slot, rekordbox type, track id, flags `0xe4` playing + master), and
the CDJs connect to its dbserver the moment it becomes master, as they do
to a master CDJ.  The difference left is the rekordbox export itself: a
CDJ most likely decides from `PIONEER/rekordbox/export.pdb` (and its ANLZ
files) whether a player's track has analysis to fetch.  Serving that, an
export OpenDeck writes for its own library, is the likely fix, and a bigger
job.  The title row's flag is kept (it is what a CDJ sends).

## Reproducing

Two slots on `djlink0` (`cdj3k-emu --instance N --headless --dev`, the
control API on port 4400+N), a stick image attached to one
(`media attach`), a track loaded and playing on each (`press PLAY`), MASTER
on one, then `touch 0.16 0.2` on the other's phase meter and `lcd main`.
Capture without root through QEMU's QMP socket (`object-add filter-dump`
on `net0`, `/tmp/cdj3k-emu-1000/instance-N/qmp-fd.sock`).  The CDJ's
dbserver database port is dynamic (33511 here), not 1051.

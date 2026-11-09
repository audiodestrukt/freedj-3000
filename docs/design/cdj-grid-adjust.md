# CDJ-3000 Grid Adjust: SNAP GRID and SHIFT GRID

Status: measured on cdj3k-emu 2026-11-09; implemented in freedj (CDJ-3000
skin). These two keys look simple, but neither one lights when you would guess
it does. This page records what the unit does, how it was measured, and the
loose ends.

## Summary

| Key | Lights when | Tapping it |
|---|---|---|
| SNAP GRID (CUE) | a cue point has been **set by hand** since the track loaded | puts beat 1 on the cue point |
| SHIFT GRID | the deck is **synced and holding a phase offset** that a jog bend left | moves the grid by that offset and clears it |
| ◀◀ 1/2, ▶▶ 1/2 | always (with a grid) | moves the grid half a beat |
| RESET | the grid has been edited | restores the analysed grid |

The emulated firmware shows **only these five keys**. The manual also lists
×2, ×1/2, Enlarge and Reduce; they appear neither for rekordbox-analysed nor for
unanalysed tracks.

## SNAP GRID (CUE)

### What the unit does

- **Dim** right after a load, while the only cue is the one the deck placed
  itself (auto cue / the start of the track). This holds whether the track is
  playing or paused, and for rekordbox-analysed tracks too.
- **Lit** as soon as you set a cue yourself: play, pause somewhere else, press
  CUE.
- After that it **stays lit**: while playing, while paused, on the cue or away
  from it, and even after a snap. Snapping does not dim it, so the rule is not
  "would snapping change anything".

### How it was found

On a stick with no rekordbox database, SNAP stayed dim in every state tried. It
looked like "needs an analysed track", which is what the manual says. With
Dan's rekordbox stick (226 tracks) it was **still dim** on an analysed track.
The difference was the test, not the track: the first attempts pressed CUE
while the track was at its end, which does not set a new cue. Pause mid-track,
jog away, press CUE: lit.

| Step (analysed track "Actin' Up") | SNAP |
|---|---|
| loaded, playing, GRID mode | dim |
| PLAY (pause) | dim |
| jog away while paused | dim |
| CUE (sets a new cue) | **lit** |
| PLAY / CUE / jog / CUE / play-pause | lit throughout |
| tap SNAP (RESET lights) | lit |
| tap RESET | lit |

### freedj

`DeckSnapshot::cue_set` is set when the transport reports a new cue point
(`Outcome::cue_point`) or a memory point is called, and cleared on load. The
SNAP key lights on `cue_set` (and a grid). Calling a memory point on the unit
was not tested; freedj counts it as hand-set.

## SHIFT GRID

### What it is for

On a CDJ, SYNC matches tempo and phase to the master. If you then bend with the
jog, the CDJ **keeps** the offset you made: the deck stays synced but sits off
the master's beat by that much. This is how a DJ corrects a grid that is a bit
off by ear. SHIFT GRID then writes that offset into the track's grid. The
grid now matches what you hear, and the offset goes back to zero. After that,
SYNC alone lines the track up correctly.

### What the unit does

- **Dim** while synced with no bend, playing or paused.
- **Lit** after a jog-side bend while **synced and playing**. A bend while
  paused is a frame search, not a bend, and does not light it.
- The offset is **held**: SHIFT stays lit through a 5 s wait and a pause.
- **Tap:** the grid moves, SHIFT dims, RESET lights.
- **The amount is the net bend:** a bend forward and straight back moves the
  grid by about nothing.

### Measurements

Zoom about 17.6 px per beat at 126 BPM; grid position read from the red
downbeat ticks in the top tick row (y 216). The deck was paused before each tap
so the waveform stood still.

| Bend while synced + playing | SHIFT | Grid move on tap |
|---|---|---|
| none | dim | n/a |
| +90° 600 ms, +90° 600 ms, +180° 800 ms | lit | 6 px left (≈0.34 beat) |
| −180° 800 ms (after RESET) | lit | 4 px left (≈0.23 beat) |
| +180° 800 ms then −180° 800 ms | lit | 1 px (≈0) |
| +360° 1500 ms | lit | 16 px left (≈0.9 beat) |

`jog <deg> <ms> side` is the emulator's pitch-bend gesture: turning the
platter by its side. The amount a bend shifts depends on how fast you turn, not
only how far, so the degrees do not map linearly onto beats.

### Loose end: the direction

A forward bend (clockwise) and a backward bend both moved the grid **left**. The
net-zero test shows the amount is the net bend, so the sign has to come from
somewhere else. Two likely explanations, not yet separated:

1. The held offset is wrapped to one beat (0…1, not ±½). A small backward bend
   then reads as almost a whole beat forward, so the move is drawn as a short
   hop the same way.
2. The emulator's side-jog sign, or how the offset is applied, differs from the
   assumption.

What is invariant, and what freedj keeps: **after SHIFT GRID, what you hear
does not move**. The offset is folded into the grid, and SYNC then holds the
track in the same place with no offset. A whole-beat difference changes only
which tick is drawn as a downbeat. To settle the sign, repeat the test with
freedj as the **synced** deck and the CDJ as master, and compare freedj's grid
move with its known nudge direction (see Reproducing).

### freedj

- `LinkState::sync_offset` (beats, f32 bits) is the held offset;
  `LinkState::jog_nudge_ms` says the jog is nudging a synced deck.
- **The SYNC loop** (`prodj.rs`, SYNC follow; decisions in `SyncPhase`) corrects toward the held offset
  (`phase_err(frac, offset)`, wrapped ±½ beat) instead of toward 0.
  - While the jog nudges, the loop leaves the phase alone.
  - At the first master beat after the wheel settles (`JOG_SETTLE_MS` = 300),
    it stores where the phase landed as the new offset.
  - SYNC off clears it.
- **Before this change** freedj's SYNC pulled the phase back to 0 every master
  beat, so a nudge while synced simply vanished. That is the opposite of the
  CDJ, and it is why SHIFT GRID had nothing to act on.
- **SHIFT GRID** (`GridAdjust::ShiftSync`) moves the grid by `offset × period`
  (later beats for a positive offset: we were past our beat when the master's
  landed) and zeroes the offset. The key lights while |offset| > 0.005 beat
  (`DeckSnapshot::sync_offset`).

## Reproducing

The rig puts the emulator and freedj on a private Link segment on one machine.

1. **Bridge** (root, once per boot). A bridge, not a macvtap: a macvtap cannot
   reach its own host, so freedj on the same machine would never see the CDJ.
   ```
   sudo ip link add djlink0 type bridge
   sudo ip addr add 169.254.77.1/16 dev djlink0
   sudo ip link set djlink0 up
   ```
   Without `brd +` the address reports no broadcast; freedj now derives it
   from the netmask (it used to broadcast to its own address).
2. **Emulator onto the bridge:** control API `setting network djlink0`. It
   restarts, asks for the password once (pkexec), enslaves its tap `cdj3k1`
   and gives itself a 169.254.x.x address.
3. **freedj as master**, pinned to the bridge:
   ```
   OPENDECK_LINK_IFACE=djlink0 OPENDECK_LINK_SEND=1 OPENDECK_PLAYER=2 \
   OPENDECK_MASTER=1 OPENDECK_PLAY=1 ./target/debug/opendeck <track>
   ```
   `OPENDECK_LINK_IFACE` keeps Link on that interface and ignores players
   outside its subnet.
4. **On the CDJ:** load a track, then press PLAY and `BEAT_SYNC`. The orange
   "2" shows that freedj is master. Enter GRID with a long rotary press
   (`press ROTARY_PRESS 1500`), bend with `jog <deg> <ms> side`, pause, and
   tap SHIFT at (0.398, 0.59) in touch coordinates.
5. **Reading the keys:** sample the key's background at y 450 (lit: grey 50,
   dim: 15) and the red downbeat ticks at y 216.

Captures from these runs are in `reference/pioneer/cdj3000-emu/` (local only):
`grid-adjust.png`, `browse-rekordbox-*.png` and `source-rekordbox.png`.

## Tests

The logic behind both keys is in pure functions with unit tests, so a
regression does not need the emulator to show up:

| What | Test |
|---|---|
| Phase error wraps around the held offset | `prodj::sync_offset_tests::phase_err_wraps_around_the_held_offset` |
| No nudge: SYNC pulls to the beat | `…::without_a_nudge_sync_pulls_to_the_beat` |
| A nudge while synced is held, and drift corrects toward it, not 0 | `…::a_nudge_while_synced_is_held_not_undone` |
| A backward nudge holds a negative offset | `…::a_backward_nudge_holds_a_negative_offset` |
| SYNC off clears the hold and a pending nudge | `…::sync_off_clears_the_hold_and_forgets_a_pending_nudge` |
| Closed loop settles on the held offset, from either side, with drift | `…::closed_loop_settles_on_the_held_offset` |
| SHIFT GRID puts the heard point on a beat and keeps the bar | `grids::tests::shift_by_beats_puts_the_heard_point_on_the_beat` |
| Which keys light | `cdj3000::grid_key_tests::grid_keys_follow_the_units_rules` |
| Only a new CUE reports a cue point (SNAP's signal) | `transport::tests::only_setting_a_new_cue_reports_cue_point` |
| Broadcast derived without `brd`; the pin hears only its subnet | `prodj::sync_offset_tests::broadcast_is_derived_…`, `…::a_pinned_interface_hears_only_its_subnet` |

Still manual: the live run against the emulator (Reproducing), and freedj as
the synced deck.

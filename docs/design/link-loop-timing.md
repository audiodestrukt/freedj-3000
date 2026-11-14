# Loops over Pro DJ Link

Fixed 2026-11-14. Code: `BeatClock` and `PlayheadEstimate` in
`crates/app/src/prodj.rs`; tests in `beat_clock_tests` and
`playhead_estimate_tests` there; live check `tools/deckbench/loop_beats.py`.

## The bug

The deck looped, but what OpenDeck told the other players did not. A
player, mixer or lighting tool following OpenDeck over Link saw the song
run on, or saw beats stop, while the deck itself was looping.

## How the sender knows where the deck is

The Link sender thread ticks once a millisecond. The decoder's cursor
(`position`) runs ~85 ms ahead of what is heard and moves in 512-frame
blocks, so the sender keeps its own estimate of the heard playhead: it
free-runs at the playback rate and is pulled gently (τ ≈ 200 ms) toward
`position − in_flight`. Beat packets go out when the estimate crosses a
grid beat line; the status packet's beat number and the CDJ-3000 Absolute
Position packet (playhead ms, every 30 ms) come from the same estimate.

That estimate knew nothing about loops. Four faults followed, each found by
watching OpenDeck from Deckbench:

| # | Fault | What a follower saw | Fix |
|---|---|---|---|
| 1 | The estimate only jumped back on a jump of more than 0.5 s, so a loop shorter than that (½ beat, 1 beat at > 120 BPM) never wrapped it; it ran on and was dragged back by the pull. | Link position running through the song during short loops. | The estimate wraps at the loop's end itself, however short the loop, and the pull goes the short way round the loop. |
| 2 | The reference `position − in_flight` reads before the loop start just after the cursor wraps (the loop's tail is still queued). | The position dipping before the loop start on every wrap. | The reference walks back modulo the loop, as the screen's playhead already did (`heard_pos`). |
| 3 | A jump back of one or two beats was not treated as a jump: a beat went out only when the beat index rose past the last one sent, or fell more than two. | No beat packets at all during a 1–2 beat loop. | A wrap counts as a seek: the beat the loop lands on goes out each time round. |
| 4 | Loop points are whole frames; grid beat lines are not. A loop's start can sit a fraction of a frame before its beat, and its end a fraction past the next. | Each wrap sent the beat before the loop ("1") then the loop's beat ("2") a millisecond later, or the beat after the loop ("3") just before wrapping. | Within ≈2 ms of a loop's end is its start (`LOOP_EDGE`); a position within 0.001 beat of a line is on it (`on_beat_line`); the spin to a beat line that is also the loop's end lands on the loop's start (`PlayheadEstimate::advance`). |

Faults 3 and 4 only became visible once 1 and 2 were fixed: Deckbench
showed `2222…` with one stray `3`, then `1212…`, then `2222…` clean.

## Deckbench on one machine

[Deckbench](https://github.com/commit-collective/deckbench) runs simulated
players on `127.0.0.N` and listens on the Link ports. Two things kept it
from hearing OpenDeck on the loopback:

- With `OPENDECK_LINK_IFACE=lo` OpenDeck broadcast to `127.0.0.1`, which
  only one socket bound to the port receives. It now broadcasts to
  `127.255.255.255` (`loopback_link`), which Linux delivers to every socket
  on the port: Deckbench's decks and a second OpenDeck both hear it.
- OpenDeck's listeners set `SO_REUSEPORT`. On Linux that spreads unicast
  datagrams for a shared port across the sockets instead of copying them,
  so OpenDeck's own listener can take packets meant for Deckbench (the
  CDJ-3000 Absolute Position packets, which are unicast). Not changed yet:
  the beat and status checks do not need it.

## Tests

- `beat_clock_tests` drive `BeatClock` exactly as the sender thread does,
  at 1 ms ticks, with the cursor 85 ms ahead in 512-frame blocks, loop
  points rounded to whole frames and a 127 BPM grid (beat lines between
  frames): plain play sends each beat once in order; a 1-beat loop sends
  only its beat, once per wrap, never two together; a ½-beat loop keeps
  sending its beat; a 4-beat loop counts 40 41 42 43 40 …
- `playhead_estimate_tests`: the estimate stays inside short loops, never
  dips before a long loop's start, follows the track without a loop, lands
  on the loop start when stepping onto its end (also with the end a few
  frames past the beat), and a hair before a beat line is on it.
- `loopback_tests`: the loopback broadcast address.
- Each fix was checked by reverting it: every revert fails a test.
- `tools/deckbench/loop_beats.py`: the live check. Deckbench observes on the
  loopback, OpenDeck plays headless as player 4, sets a 1-beat loop with
  LOOP IN / OUT and exits it; the script passes when every beat sent in the
  loop is the same beat of the bar, one each time round, never two
  together. Before these fixes it fails (no beats reach Deckbench; with only
  the broadcast fixed, `1212…` or a stray beat).

## Open

- Absolute Position packets over the loopback (the `SO_REUSEPORT` point).
- A LOOP OUT pressed within half a beat of LOOP IN with QUANTIZE on rounds
  back onto the IN point and is ignored ("before the in point"); a CDJ makes
  the loop at least one beat. Not changed yet.
- The spin to a beat line advances the estimate, and the next tick's `dt`
  counts that time again; the pull absorbs it (≤ 1.5 ms a beat). Older than
  these fixes; harmless so far.

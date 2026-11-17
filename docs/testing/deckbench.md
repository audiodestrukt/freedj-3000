# Testing with Deckbench

[Deckbench](https://github.com/commit-collective/deckbench) simulates Pro
DJ Link players and a mixer on one machine and records everything on the
link. OpenDeck runs beside it on the loopback.

## Setup

```sh
git clone https://github.com/commit-collective/deckbench ~/sandbox/thirdparty/deckbench
cargo build --release -p opendeck-app
```

Python 3.8+, `xvfb-run`. Linux: OpenDeck with `OPENDECK_LINK_IFACE=lo`
broadcasts to `127.255.255.255`, which Deckbench's sockets hear.

## Checks

```sh
make deckbench                                  # build, then every check
tools/deckbench/suite.py --only sync,master     # some of them
tools/deckbench/suite.py --keep target/tmp/db   # keep the logs and the bench's view
```

`tools/deckbench/suite.py` starts Deckbench with two decks on the loopback
(players 2 and 3; 2 playing at 126 BPM, the tempo master) and OpenDeck
headless beside them as player 4 on `lo`, driven by an `OPENDECK_SCRIPT`.
Each check reads the bench's own view of OpenDeck, as another player sees it:

| Check | Passes when |
|---|---|
| `sync` | after SYNC, OpenDeck reports 126 BPM, synced, its beats within 0.05 beat of the master's |
| `trackinfo` | the bench reads OpenDeck's loaded track from its dbserver. **Known failure (XFAIL)**: Deckbench reads 12 arguments a message at most, our rows carry a CDJ-3000's 16 ([#62](https://github.com/audiodestrukt/freedj-3000/issues/62)) |
| `waveform` | the bench reads OpenDeck's overview, detail and beat grid, the grid at the track's tempo |
| `master` | after MASTER, OpenDeck is the tempo master and player 2 has let go |
| `loopbeats` | `tools/deckbench/loop_beats.py`: a 1-beat loop sends its beat once each time round, on the same beat of the bar ([link-loop-timing.md](../design/link-loop-timing.md)) |

A known failure prints XFAIL and does not fail the run; one that starts
passing prints XPASS, and its entry in `KNOWN` should go.

**Ports.** The bench runs on the real Pro DJ Link ports (OpenDeck uses
them), with each of its decks bound to its own address. OpenDeck pinned to
an interface (`OPENDECK_LINK_IFACE`) binds its dbserver to that interface's
address only, so the two share 12523 on one machine. Unpinned, OpenDeck
binds every address and the bench's decks would hold the port first.

Run it before and after any Link change; it is loopback-only, and takes
MASTER, which never belongs on a network with real decks.

## By hand

```sh
cd ~/sandbox/thirdparty/deckbench && python3 -m deckbench --numbers 2,3 --on
OPENDECK_LINK_IFACE=lo target/release/opendeck --player 4 --link-send track.mp3
```

Open http://127.0.0.1:8750/: OpenDeck shows as a player card with its
strip, beats and loop.

## Limits

- OpenDeck's listeners use `SO_REUSEPORT`, so unicast packets to Deckbench
  (CDJ-3000 Absolute Position) can land on OpenDeck's own socket instead.
- Never run a master-taking OpenDeck on the real LAN; the loopback is fine.

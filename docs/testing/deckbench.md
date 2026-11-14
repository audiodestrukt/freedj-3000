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

| Script | Checks |
|---|---|
| `tools/deckbench/loop_beats.py` | A 1-beat loop sends its beat once each time round over Link, on the same beat of the bar ([link-loop-timing.md](../design/link-loop-timing.md)) |

Each starts Deckbench as an observer (`--observe --capture`), runs OpenDeck
headless with an `OPENDECK_SCRIPT`, then reads the capture. `--keep DIR`
keeps the capture (JSON lines, one packet each, with its hex) and both logs.

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

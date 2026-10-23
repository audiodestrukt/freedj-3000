# Testing OpenDeck against a real CDJ-3000

OpenDeck has been exercised against an XDJ-1000MK2 only. CDJ-3000s speak the
same Pro DJ Link, but a 3000 has never been on this network, and its status
packet is a newer generation than the one the parser was pinned on. If you
own one and want to help, this page is the whole ask. Three tiers; the first
needs nothing but Python.

Report back with a [CDJ-3000 report issue](https://github.com/audiodestrukt/freedj-3000/issues/new?template=cdj-3000-report.md).

## Tier 0: a packet capture (10 minutes, no build)

What it gives us: the first CDJ-3000 packets we have ever seen, replayable
through the parser offline, so we know before anyone links a deck whether
the status layout parses.

1. Ethernet from the CDJ-3000's LINK port to your computer. A direct cable
   is fine; the decks auto-negotiate. Give the computer an address on the
   deck's subnet if it does not get one (the decks self-assign 169.254.x.x
   when nothing hands out addresses; a laptop set to DHCP ends up there too).
2. Load a track on the deck and play it.
3. Run
   ```
   python3 tools/link_capture.py
   ```
   It prints one decoded line per packet (announce, beat, status) and writes
   `link-capture-<date>.log` with every packet's bytes.
4. While it runs: press MASTER on the deck, turn SYNC on and off, change the
   pitch, stop and start the track, load another track. Each of those should
   show up as a changed status line.
5. Ctrl-C. Attach the `.log` to the report. It contains the deck's name,
   player number, firmware string and link-local addresses, nothing else.

If you have the repo built, this tells you what we will see:
```
cargo run -p opendeck-link --example replay_capture -- link-capture-*.log
```
It prints what the parser understood and exits non-zero if a beat or status
packet was rejected. Either result is useful; a rejection is the finding.

## Tier 1: the iPad app on the same wire (App Store build)

What it gives us: whether a 3000 lists OpenDeck, syncs to it, and hands it
MASTER, with no build at all.

1. Install [OpenDeck DJ](https://apps.apple.com/app/id6807472453). A USB-C
   to ethernet adapter on the iPad, Wi-Fi off, cable to the deck's LINK
   port (or both into the same switch). Plug the adapter in **before**
   launching the app; it binds its Link interface at startup.
2. In the app: MENU → PERF LOG → ON. That writes
   `Documents/opendeck-perf.log` (Files app → On My iPad → OpenDeck DJ) and,
   from 0.2.10, the Link packets go into the same file: the first packet of
   each kind from each player as hex, then every status change.
3. Load a track in the app (the demo track is fine) and play it.
4. Checklist, with what to look at on each side:
   - The CDJ-3000's LINK / player list shows "OpenDeck" (or the name you set)
     as player N. App side: the INFO page lists the 3000.
   - SYNC on the app: its BPM snaps to the deck's and the phase meter settles.
   - SYNC on the deck with the app as MASTER (press MASTER in the app): the
     deck's tempo follows the app. This is the handoff the XDJ needed one
     status byte fixed for (0x1f); the 3000 may want something else.
   - Press MASTER on the deck: the app yields. Press MASTER in the app again:
     the deck yields.
   - Pitch the app ±5 % while the deck is synced: the deck follows.
   - LINK on the deck, browse: does the app appear as a source? Loading from
     it is **not expected to work** (the 3000 moves files differently; see the
     README status table). Tell us what the deck shows anyway.
5. Attach `opendeck-perf.log` and a photo of the deck's LINK screen.

## Tier 2: a desktop or Pi build with send on

Same checklist, with the full log. Linux or macOS:
```
make build
RUST_LOG=opendeck=debug,wgpu=off,naga=off ./target/release/opendeck techno.mp3 --link-send --player 2 2>&1 | tee cdj3000-session.log
```
Pick a `--player` number the deck is not using (1 to 4 on a mixed network).
Press `M` to take master, `S` for sync. Attach `cdj3000-session.log`; it has
every packet both ways at debug level.

Please do not leave a test instance running as MASTER and walk away: a
master that stops sending beats can leave a synced deck unable to re-elect
one until it is unsynced (we have seen the XDJ do this).

## What we will do with it

- A status packet of a new length gets pinned as a unit test in
  `crates/link/src/prodj.rs`, next to the XDJ's.
- The handoff and sync observations update the README's status table from
  "compatible by protocol" to tested, or open an issue for what differs.
- A capture from a CDJ-3000 **as master with a second player synced** is the
  one we want most: it shows the 3000's sync counter and handoff bytes in
  motion.

## Running OpenDeck *on* a CDJ-3000

Not this page. That is a firmware replacement and is gated on things no
pushed change can supply; the assessment is in
[docs/reference/cdj-3000-target.md](../reference/cdj-3000-target.md).

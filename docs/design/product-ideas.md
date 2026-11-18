# Product ideas

Ideas Dan has raised, kept here so they are not lost in a chat.  Not
decided, not scheduled.  Each says what already exists that it would build
on, and what it would take.  The shipped product's framing is
[`adk-1000-vision.md`](adk-1000-vision.md).

## 1. A media server with light decks (2026-11-18)

One powerful box holds the music and does the heavy work; the decks on the
table are light (a Pi, an iPad, an ESP32 build) and stream from it.  The
server splits stems in real time and streams processed audio out.  It might
be, or sit inside, a **mixer**.

Why it fits: stem separation is the one job a deck-sized computer can't do
in real time (a Pi 5 splits at about 1x on all four cores with an active
cooler; see `docs/reference/raspberry-pi.md`), while a desktop GPU or a
Mac does it many times faster.  A mixer is already the box every deck is
wired into.

What exists:
- OpenDeck already serves its library to other players over Pro DJ Link
  (dbserver + NFS: metadata, beat grids, waveforms, the audio), and loads
  from other players while the file is still arriving
  (`opendeck-media-source-works`, the streaming loader).
- Stems split on the device, from the playhead, and stems are read from a
  stick or another player (`docs/design/stems-on-device.md`).
- The proposal for serving split stems to other players:
  [`serving-split-stems.md`](serving-split-stems.md).

What it would take: stems split on the server ahead of where each deck
plays and streamed to it (the "ready map" of stems-from-the-playhead is
already per stretch); a deck that plays streamed stems with the same mixer
it uses for prepared ones; the server's own UI (library, analysis, which
deck has what).  As a mixer: four or more channels of audio in from the
decks, or the decks sending no audio at all and the mixer mixing their
streams itself.

Open: whether the decks stay standalone players (they work without the
server) or become controllers of the server's players; latency budget over
Wi-Fi vs. wired; what a DJM-style mixer box costs to build.

## 2. One deck, banks A/B/C/D (2026-11-18)

A deck format where a button switches which deck you are playing, like an
Akai MPC's pad banks: A, B, C, D each a loaded deck, ready to go, and the
one surface (jog, pads, transport) drives whichever is selected.  Maybe
with a separate output per deck, so a mixer still gets four channels.

What exists:
- The player-crate plan lifts the deck engine into Player / Output /
  Clock crates so one app runs N decks
  ([`player-crate.md`](player-crate.md)).  That plan said "never a 4-deck
  layout on one screen"; banks are a different answer to the same wish:
  four decks, one screen and surface, one at a time.
- Pro DJ Link supports six player numbers; each bank could be its own
  player on the network (its own sync, its own LOADED state on the
  mixer).
- Multiple outputs: OpenDeck opens one output device today; per-deck
  outputs need the Output crate and an interface with 4+ channels (or
  several USB cards).

What it would take: the Player crate; a bank key per deck on the faceplate
and on controllers.toml; the screen showing which bank is live (and the
others' state at a glance); deciding what happens to a playing bank when
you switch away (keeps playing, as the MPC does).

## 3. OpenDeck on iOS as a stem source for an OverCue-modded CDJ (2026-11-18)

The iPad splits stems on the device; a CDJ-3000 running the OverCue mod
plays stems from a stick or another player.  So the iPad could serve its
splits to a modded CDJ over Pro DJ Link, the way a stick with prepared
stems does today.

This is [`serving-split-stems.md`](serving-split-stems.md): serve the
splits as the files a stick carries, over the NFS OpenDeck already runs, no
new protocol.  Dan put it on hold on 2026-11-15 ("ahead of our skis") to
keep iOS correct first.

Constraint: clean room.  We read what a stick with prepared stems holds,
not OverCue's binaries, and we don't probe its private channel or phone
protocol.

## 4. A battery-powered deck (2026-11-18)

The 5" Pi deck (`hardware/cad/deck5/`) running on a battery for a few
hours: no wall power at a park or a house party.

Numbers from the test Pi 5 (2026-11-18): the board's own rails draw about
4.7 W with the deck idle; add the 7" panel at full brightness (about 2 to
2.5 W), the USB audio interface and the fan, and conversion loss, and the
deck uses roughly 8 to 9 W playing, 14 to 15 W while splitting stems at
2.8 GHz.  A power bank's rated capacity is at 3.7 V and loses about 15 %
getting to 5 V:

| Bank | Usable at 5 V | Playing (~9 W) | Splitting (~15 W) |
|---|---|---|---|
| 10,000 mAh (37 Wh) | ~31 Wh | ~3.5 h | ~2 h |
| 20,000 mAh (74 Wh) | ~63 Wh | ~7 h | ~4 h |

The Pi 5 wants 5 V at 5 A (USB-C PD, 27 W).  Most banks give 5 V at 3 A:
the Pi runs, but warns, caps its USB ports at 600 mA (enough for a UCA202)
and may dip during a split.  Pick a bank that does 5 V / 5 A PD, or lower
the overclock and keep splits off.  Measure on the deck before trusting
the table: `vcgencmd pmic_read_adc` gives the board's rails, a USB-C
meter the whole draw.

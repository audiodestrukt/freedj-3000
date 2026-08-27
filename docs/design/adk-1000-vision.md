# ADK-1000 — a drop-in Pro DJ Link deck (product vision)

Status: **vision + status** (2026-08-27). The product framing for the iPad build
(**ADK-1000**, by Audiodestrukt) and an honest map of what's proven vs. what's
left. Pairs with [`faceplate-and-hardware.md`](faceplate-and-hardware.md) and
[`mobile-tablet-port.md`](mobile-tablet-port.md).

## The pitch

The ADK-1000 turns an iPad into a real CDJ-class deck that **joins an existing
Pro DJ Link rig**. Bring it to the club, set it next to the CDJs, and it works
with the **USB drive already inserted in the other decks** — no config, no
separate media, no laptop.

- A **3rd / 4th deck you already own**, for the price of an app, instead of a
  ~$2k CDJ for an occasional extra channel.
- Joins the Link network, **syncs tempo/beat**, and can **take master**.
- **Browses and loads tracks from the shared rekordbox USB** in whichever deck
  has it (Pro DJ Link's shared-library / LINK EXPORT, read over NFS) — it needs
  no USB of its own.
- Photorealistic XDJ-style faceplate; touch-driven.

## Why it's credible: the magic part already works

The most compelling beat of the demo — *walk up, load off the stick that's in
someone else's deck* — is **already proven on device**: on the iPad, loading a
track from the XDJ over Link *is* Pro DJ Link's shared-library feature (one USB
in one deck, read by everyone over NFS). That's the existing `opendeck-nfs` LINK
transport, running on iOS. Also proven: it renders the full deck, plays cleanly,
syncs to a real XDJ, and can take tempo master (the `0x1f` status-byte fix).

## What's left for true out-of-the-box club use

1. **Device-number claim — the #1 gap.** With 2+ real CDJs already on the network
   as players 1–2, the ADK-1000 must claim **the next free player number without
   conflict**. Today freedj *skips* the Pro DJ Link claim/defend handshake and
   asserts a hardcoded number — fine solo against one XDJ, wrong in a populated
   rig. Needs the real arbitration so it slots in as player 3/4 automatically.
2. **Standalone Link broadcast — PROVEN (2026-08-27).** Sync is UDP broadcast;
   on iPad it now works standalone over a wired USB-C ethernet dongle, Wi-Fi off,
   no Mac and **no multicast entitlement** required. The earlier failures were a
   startup interface-binding issue (freedj latches its Link interface once at
   launch; if the dongle isn't up yet it binds nothing) — a restart with ethernet
   already present fixes it. Remaining work is only robustness: re-evaluate/re-bind
   the interface on network change so no restart is needed (issue #36).
3. **Robust shared-library discovery.** Find which deck/USB is exporting the
   library (USB vs rekordbox source; any player, not just a hardcoded XDJ). See
   the LINK source issues (#30 rekordbox dbserver, #31 pipeline NFS reads, #32
   browse UI + source model).
4. **Feature completeness for "pro":** loops, hot cues, beat jump, SLIP (#28).

## Audio, to be explicit

Pro DJ Link carries **sync + metadata, not audio**. The ADK-1000's audio reaches
the mixer physically — USB-C → a small audio interface → a mixer channel. freedj
just needs to output cleanly; the routing is hardware. Worth stating so the "it
just works" story doesn't imply audio-over-Link (it isn't).

## The line

Native (iPad) buys real sockets + real audio + the shared-USB library; the
faceplate makes it read as a deck, not an app. The remaining work is
**arbitration (claim a player number) + standalone broadcast**, on top of the
library/sync/master/deck that already run. That's a short list for "a third CDJ
in your bag."

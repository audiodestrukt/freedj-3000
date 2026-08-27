# Pro DJ Link — player numbers, and the XDJ menu settings around them

Status: **reference** (2026-08-27). Field notes from testing freedj against a real
XDJ-1000MK2, plus the protocol facts behind them. Pairs with the master-handoff
work in `crates/app/src/prodj.rs` / `crates/link/src/prodj.rs`.

## How many players? 1–4, or 6 on an all-CDJ-3000 network

- **1–4 is the universal player space.** Every Pro DJ Link generation (CDJ-2000/
  NXS/NXS2, XDJ-1000/1000MK2, XDJ-XZ, mixed rigs) numbers **CDJ-class players
  1–4** — one per deck channel, matching a 4-channel DJM.
- **The CDJ-3000 raised the ceiling to 6 — but only if _every_ linked device is a
  CDJ-3000.** Put a single non-3000 (an XDJ, a CDJ-2000, freedj today) on the
  network and it falls back to the 4-player limit. So 5 and 6 are valid numbers
  **only** on an all-CDJ-3000 setup.
- Non-player devices (the DJM mixer, rekordbox, a laptop) use their own higher
  device numbers and don't consume a player slot.

### What this means for freedj

- freedj accepts `--player 1..6` (`OPENDECK_PLAYER`, or the iOS default of 3),
  clamped to 1–6 in `run()`. **But picking 5 or 6 only works on an all-CDJ-3000
  network.** Against an XDJ/mixed rig, an XDJ **refuses master handoff to player
  5/6** because it isn't running 6-player mode — this is correct hardware
  behaviour, not a freedj bug.
- **Observed:** with the XDJ set to player 4, freedj hands off master fine at
  players **1, 2, 3** and **fails at 5** — exactly the 4-player-limit rule above.
- The ADK-1000 is a drop-in **deck 3** next to CDJs 1–2, so the iOS default of
  **3** is the right choice; a mixed rig can only go 1–4 anyway.

## Changing the XDJ PLAYER No. — TWO conditions must both be met

Changing a CDJ/XDJ's **PLAYER No.** (Utility → hold MENU >1s) is blocked unless
**both** of these are true. Verified on an XDJ-1000MK2, 2026-08-27:

1. **No other device on the LINK network.** The XDJ errors ("remove all devices
   from the linked players to change PLAYER No.") if anything else is present —
   another CDJ/XDJ, a **LINK-connected DJM mixer**, a laptop running rekordbox,
   **or freedj itself** (a running freedj is a linked player). Easiest: **unplug
   the XDJ's LINK ethernet cable** so it's guaranteed alone.
2. **No USB/SD medium loaded.** Per the manual, "the player number cannot be
   changed when a medium is loaded" — you must **eject the USB drive** first.
   This is the one that's easy to miss: even with the network unplugged and the
   deck showing *not linked*, the setting stays locked until the stick is out.

So the full recipe: unplug LINK (or quit every other device) **and** eject the
USB → change PLAYER No. → reinsert USB / reconnect LINK.

Player numbers are a manual, deliberate assignment, and the network won't let two
devices race to the same slot. This is why freedj uses a manually-set number (not
automatic negotiation yet — see below), matching how you'd set a real deck.

**Note:** you usually don't need to touch the XDJ at all — leave it at its number
and just set *freedj's* (`--player`, or the iPad default 3) to something free.

## The "Duplication" Utility setting (unrelated to sync)

The XDJ's **Duplication** menu copies **Utility/preference settings** from one
linked player to the others over Pro DJ Link — it has **nothing to do** with
tempo master, SYNC, or track loading. Options:

- **DEFAULT** — reset the target's Utility settings to factory defaults.
- **PLAYER 1…4** — copy the Utility settings **from that source player** to the
  others (the "(PLAYER 4)" you see is just the current source context).
- **ALL** — apply to all linked players.

Duplicated items include QUANTIZE, HID SETTING, AUTO CUE LEVEL, LANGUAGE,
ARTWORK, TRACK INFORMATION, PLAYLIST VIEW, ON AIR DISPLAY, JOG BRIGHTNESS, JOG
INDICATOR, LCD BRIGHTNESS. freedj does **not** need to implement Duplication for
the demo; it's a convenience for setting one deck's preferences and pushing them
to the rest.

## Master handoff is timing-critical — use a Release build

The handoff completes only when the outgoing master's status packet naming the
successor (Mh) arrives promptly. A **debug build** (`opt-level = 1`) runs the R3
timestretch and audio path slowly enough to starve the `prodj-tx` thread, so the
completing status is late and the XDJ aborts the handoff. **Release works** (the
whole reason `Cargo.toml` notes "the audio engine binary must never be a debug
build in prod"). Build the iPad demo Release (`make ios-device IOS_CONFIG=Release`)
— it also fixes the debug-build heat.

## Related / roadmap

- **#37** — automatic device-number claim/defend (CDJ "Auto" mode). Deferred; we
  set the number manually for now.
- **#38** — on-device (Utility-style) player-number config for the iPad.

## Sources

- Pioneer CDJ Utility "Duplication" — operating instructions (ManualsLib);
  Pioneer DJ forums on changing player number / duplication while linked.
- XDJ-1000MK2 PLAYER No. (medium-loaded lock): XDJ-1000MK2 Operating Instructions
  (ManualsLib p.35 / the kuvo.com PDF).
- Pro DJ Link overview (player counts): community protocol guides + the CDJ-3000
  6-player announcement.

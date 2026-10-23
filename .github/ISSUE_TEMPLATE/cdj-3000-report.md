---
name: CDJ-3000 interop report
about: You put OpenDeck (or just the capture tool) on a network with a CDJ-3000. Tell us what happened.
title: "CDJ-3000 interop: <firmware version> — <one line>"
labels: link, cdj-3000
---

<!-- Thanks. The steps are in docs/testing/cdj-3000-interop.md. Fill what you did; skip the rest. -->

**Deck**: CDJ-3000 / CDJ-3000X, firmware ___ (UTILITY → VERSION)
**OpenDeck**: App Store 0.2.__ on iPad / desktop build at commit ___ / Pi
**Wiring**: direct cable / switch; anything else on the network (mixer, rekordbox, other players): ___

**Tier 0 capture**: attached `link-capture-*.log` — yes / no
Output of `cargo run -p opendeck-link --example replay_capture -- <log>` if you ran it:
```
```

**Tier 1 / 2 checklist** (works / doesn't / didn't try, one word each, plus what you saw):
- Deck lists OpenDeck as a player:
- App SYNC follows the deck:
- Deck SYNC follows the app as MASTER:
- MASTER handoff deck → app:
- MASTER handoff app → deck:
- Pitch follow while synced:
- Deck's LINK browse shows the app:

**Attachments**: `opendeck-perf.log` / `cdj3000-session.log`, a photo of the deck's LINK screen.

**Anything odd**: freezes, the deck's screen complaining, the app's INFO page, timing feel.

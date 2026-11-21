#!/usr/bin/env python3
"""check_meters.py METER_LOG [DECK_LOG] — check a make reaper-mixer run.

DECK_LOG (REAPER's stdout: the decks' own log) is read first, for the cause:
two decks started, each loaded a track, each started playing.  Then the meter
log (OPENDECK_MIXER_LOG), for the result: both decks sound, and the Mixer
follows the crossfader, A alone at 0 and B alone at 1.  Exits non-zero,
saying why, if not."""
import re
import sys


def check_decks(path):
    """The decks' log: what went wrong before any sound could."""
    text = open(path, errors="replace").read()
    up = len(re.findall(r"deck loop: deck \d+ up", text))
    loaded = re.findall(r"\] loaded (.+?) in [\d.]+s", text)
    playing = len(re.findall(r"autoload ok|\] playing", text))
    bad = []
    if up < 2:
        bad.append(f"only {up} deck(s) started (want 2): is the plugin installed (make plugin)?")
    if len(loaded) < 2:
        asked = re.findall(r"plugin deck \d+: loading (.+)", text)
        bad.append(f"only {len(loaded)} deck(s) loaded a track (asked for: {asked or 'none'}): "
                   "a wrong or split path in TRACK_A / TRACK_B?")
    elif playing < 2:
        bad.append(f"only {playing} deck(s) started playing")
    return bad, loaded


def check_meters(path):
    """The meter log: what the host heard."""
    rows = []
    for line in open(path):
        m = re.match(r"t=\s*([\d.]+) pos=\s*([\d.]+) xf=([\d.]+) A=([\d.]+) B=([\d.]+) mix=([\d.]+)", line)
        if m:
            rows.append(tuple(map(float, m.groups())))
    live = [r for r in rows if r[1] > 3.0]           # past the first seconds (both decks loading)
    bad = []
    if len(live) < 30:
        bad.append(f"only {len(live)} meter rows past 3 s")
    for name, col in (("A", 3), ("B", 4)):
        sounding = sum(r[col] > 0.05 for r in live) / max(1, len(live))
        if sounding < 0.9:
            bad.append(f"deck {name} sounds in only {sounding:.0%} of the rows")
    for xf, col, name in ((0.0, 3, "A"), (1.0, 4, "B")):
        at = [r for r in live if abs(r[2] - xf) < 1e-6 and r[col] > 0.05]
        off = [r for r in at if abs(r[5] - r[col]) > 0.1 * r[col] + 0.01]
        if not at:
            bad.append(f"no rows with the crossfader at {name}")
        elif len(off) > len(at) // 10:
            bad.append(f"crossfader at {name}: the Mixer is not deck {name} in {len(off)} of {len(at)} rows")
    return bad, len(live)


bad = []
if len(sys.argv) > 2:
    deck_bad, loaded = check_decks(sys.argv[2])
    if deck_bad:
        print("decks: " + "; ".join(deck_bad))
        sys.exit(1)                                  # no point reading meters for decks that never played
    print("decks: 2 started, loaded " + ", ".join(p.rsplit("/", 1)[-1] for p in loaded[:2]) + ", playing")
bad, rows = check_meters(sys.argv[1])
print(f"meters: {rows} rows: " + ("ok" if not bad else "; ".join(bad)))
sys.exit(1 if bad else 0)

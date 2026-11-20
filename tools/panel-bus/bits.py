#!/usr/bin/env python3
"""Bit-level events in an XDJ-1000MK2 panel-link capture, in time order.

    tools/panel-bus/bits.py capture.sr                # both directions
    tools/panel-bus/bits.py capture.sr --dir miso     # panel -> main only
    tools/panel-bus/bits.py capture.sr --bytes        # byte timelines for the noisy fields too

Frames as documented in docs/reference/xdj-1000mk2-panel-bus.md: 32 bytes each
way per transfer, sampled on the rising SCK edge, LSB first, byte 30 a ones'-
complement sum of 0..29, byte 31 = 0x8F.  Frames failing the checksum are
counted and dropped.  Each bit that changes is printed as its intervals at 1
(e.g. a button's presses), ordered by when it first changed, so a set of
controls pressed in a known order lines up with that order.  Fields that move
continuously (analog, counters, the jog ring) are skipped unless --bytes.
"""
import argparse, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from decode import load_sr, bit, edges, CH

FRAME = 32
# Fields that change on their own; their bits are noise for press-matching.
NOISY = {"mosi": {9, 30}, "miso": {2, 4, 5, 6, 7, 8, 9, 10, 11, 23, 25, 30}}

def oc_sum(b):
    s = 0
    for x in b:
        s += x
        s = (s & 0xFF) + (s >> 8)
    return s

def frames(path):
    """[(t_seconds, mosi bytes, miso bytes)] of the checksum-valid frames, and the bad count."""
    rate, s = load_sr(path)
    rise, _ = edges(bit(s, CH["SCK"]))
    mo = bit(s, CH["MOSI"])[rise]; mi = bit(s, CH["MISO"])[rise]
    gap = np.flatnonzero(np.diff(rise) > 100e-6 * rate)
    starts = np.r_[0, gap + 1]; ends = np.r_[gap + 1, len(rise)]
    weights = 1 << np.arange(8)                          # LSB first
    out, bad = [], 0
    for a, b in zip(starts, ends):
        if b - a != FRAME * 8:
            bad += 1; continue
        m = bytes((mo[a:b].reshape(-1, 8) * weights).sum(1).astype(np.uint8))
        i = bytes((mi[a:b].reshape(-1, 8) * weights).sum(1).astype(np.uint8))
        if oc_sum(m[:30]) != m[30] or oc_sum(i[:30]) != i[30]:
            bad += 1; continue
        out.append((rise[a] / rate, m, i))
    return out, bad

def bit_events(fr, idx, skip):
    """{(byte, bit): (initial value, [(t_on, t_off)…])} for every bit that changed."""
    ev = {}
    for byte in range(30):
        if byte in skip: continue
        col = [f[idx][byte] for f in fr]
        for b in range(8):
            v = [(c >> b) & 1 for c in col]
            if all(x == v[0] for x in v): continue
            spans, on = [], (fr[0][0] if v[0] else None)
            for k in range(1, len(v)):
                if v[k] and not v[k - 1]: on = fr[k][0]
                if not v[k] and v[k - 1]: spans.append((on, fr[k][0])); on = None
            if on is not None: spans.append((on, None))
            ev[(byte, b)] = (v[0], spans)
    return ev

def show_bits(fr, idx, name, skip):
    ev = bit_events(fr, idx, skip)
    first = lambda item: min([s for s, _ in item[1][1] if s is not None] + [e for _, e in item[1][1] if e is not None] or [0])
    print(f"{name}: {len(ev)} bits changed")
    for (byte, b), (v0, spans) in sorted(ev.items(), key=first):
        txt = ' '.join(f"{'…' if s is None else f'{s:.2f}'}–{'…' if e is None else f'{e:.2f}'}" for s, e in spans)
        if len(spans) > 10:                     # a blink or a flicker: summarise
            done = [(s, e) for s, e in spans if s is not None and e is not None]
            on = np.median([e - s for s, e in done]) if done else 0
            per = np.median(np.diff([s for s, _ in done])) if len(done) > 1 else 0
            txt = (f"{len(spans)} pulses {done[0][0]:.2f}–{done[-1][1]:.2f} s, "
                   f"on {on * 1e3:.0f} ms every {per * 1e3:.0f} ms (median)")
        print(f"  [{byte:2d}].{b} (0x{1 << b:02x}) starts {v0}, high: {txt}")

def show_bytes(fr, idx, name, which):
    print(f"{name} byte timelines (every 0.5 s):")
    nxt = 0
    for t, *f in fr:
        if t >= nxt:
            print(f"  {t:6.2f}s " + ' '.join(f"{i}:{f[idx - 1][i]:02x}" for i in sorted(which)))
            nxt = t + 0.5

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("capture")
    ap.add_argument("--dir", choices=["mosi", "miso", "both"], default="both")
    ap.add_argument("--bytes", action="store_true", help="also print the noisy fields as byte timelines")
    a = ap.parse_args()
    fr, bad = frames(a.capture)
    print(f"{len(fr)} frames, {bad} dropped (length or checksum), {fr[-1][0] - fr[0][0]:.1f} s" if fr else "no frames")
    if not fr: return
    for idx, name in ((1, "mosi"), (2, "miso")):
        if a.dir in (name, "both"):
            show_bits(fr, idx, f"{'main -> panel' if idx == 1 else 'panel -> main'} ({name})", NOISY[name])
            if a.bytes: show_bytes(fr, idx, name, NOISY[name] - {30})

if __name__ == "__main__":
    main()

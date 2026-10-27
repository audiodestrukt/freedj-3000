#!/usr/bin/env python3
"""Decode a sigrok capture of the XDJ-1000MK2 panel link (PNLB CN8002).

    tools/panel-bus/decode.py capture.sr                 # survey + frames
    tools/panel-bus/decode.py capture.sr --edge falling  # sample on the other edge
    tools/panel-bus/decode.py capture.sr --lsb           # bit order
    tools/panel-bus/decode.py capture.sr --gap 20        # inter-frame gap, us

The link is the M16C's UART1 in clock-synchronous mode with the main CPU as
clock master, i.e. SPI-like without a chip select.  Frames are therefore found
by *gaps* in the clock (and BUSY transitions), not by CS.  The survey prints
what is needed to pick the sampling edge, bit order and gap: clock frequency,
bits per burst (8 = one byte per burst, 8n = n bytes), BUSY timing relative to
the clock bursts.  Frames are printed as hex, MOSI (main->panel) and MISO
(panel->main) side by side, with a timestamp and the BUSY state.

Channel order is the one capture.sh records: SCK RXD(MOSI) TXD(MISO) BUSY RST CNVSS.
"""
import argparse, io, sys, zipfile, configparser
import numpy as np

CH = dict(SCK=0, MOSI=1, MISO=2, BUSY=3, RST=4, CNVSS=5)

def load_sr(path):
    """Return (samplerate_hz, samples as uint8/uint16 array of channel bits)."""
    z = zipfile.ZipFile(path)
    meta = configparser.ConfigParser()
    meta.read_string(z.read("metadata").decode())
    dev = meta["device 1"]
    unitsize = int(dev.get("unitsize", "1"))
    rate_s = dev.get("samplerate", "0").split()
    mult = {"Hz": 1, "kHz": 1e3, "MHz": 1e6, "GHz": 1e9}
    rate = int(float(rate_s[0]) * mult.get(rate_s[1] if len(rate_s) > 1 else "Hz", 1))
    chunks = sorted((n for n in z.namelist() if n.startswith("logic-1-")),
                    key=lambda n: int(n.rsplit("-", 1)[1]))
    raw = b"".join(z.read(n) for n in chunks)
    dt = {1: np.uint8, 2: np.uint16, 4: np.uint32}[unitsize]
    return rate, np.frombuffer(raw, dtype=dt)

def bit(samples, ch):
    return ((samples >> ch) & 1).astype(np.uint8)

def edges(line):
    d = np.diff(line.astype(np.int8))
    return np.flatnonzero(d == 1) + 1, np.flatnonzero(d == -1) + 1   # rising, falling

def survey(rate, s):
    sck = bit(s, CH["SCK"]); busy = bit(s, CH["BUSY"])
    rise, fall = edges(sck)
    n = len(s)
    print(f"samples {n}  rate {rate/1e6:g} MHz  duration {n/rate:.3f} s")
    for name in CH:
        l = bit(s, CH[name]); r, f = edges(l)
        print(f"  {name:5s} idle={'H' if l.mean() > .5 else 'L'} duty={l.mean():.3f} rising={len(r)} falling={len(f)}")
    if len(rise) < 2:
        print("no clock activity"); return None
    per = np.diff(rise)
    short = per[per < np.percentile(per, 90)]
    if len(short):
        f = rate / np.median(short)
        print(f"clock: median period {np.median(short)/rate*1e6:.3f} us -> ~{f/1e3:.1f} kHz, "
              f"{np.median(short):.1f} samples/bit")
    # bursts: split where the clock pauses for > 4 bit periods
    bitlen = np.median(short) if len(short) else per.min()
    gap = np.flatnonzero(per > 4 * bitlen)
    starts = np.r_[0, gap + 1]; ends = np.r_[gap, len(rise) - 1]
    sizes = ends - starts + 1
    print(f"bursts: {len(sizes)}  clocks/burst histogram:")
    vals, cnt = np.unique(sizes, return_counts=True)
    for v, c in sorted(zip(vals, cnt), key=lambda t: -t[1])[:8]:
        print(f"    {v:5d} clocks x {c}")
    gaps_us = per[gap] / rate * 1e6
    if len(gaps_us):
        print(f"inter-burst gaps: min {gaps_us.min():.1f} us  median {np.median(gaps_us):.1f} us  max {gaps_us.max():.1f} us")
    # BUSY relative to clock
    br, bf = edges(busy)
    if len(br) or len(bf):
        print(f"BUSY: {len(br)} rising, {len(bf)} falling; first rising at {br[0]/rate*1e3 if len(br) else float('nan'):.3f} ms")
    return bitlen

def frames(rate, s, edge, lsb, gap_us, bits_per_word):
    sck = bit(s, CH["SCK"])
    rise, fall = edges(sck)
    samp = rise if edge == "rising" else fall
    if len(samp) == 0: return
    mosi = bit(s, CH["MOSI"])[samp]; miso = bit(s, CH["MISO"])[samp]
    busy = bit(s, CH["BUSY"])
    per = np.diff(samp)
    gap = np.flatnonzero(per > gap_us * 1e-6 * rate)
    starts = np.r_[0, gap + 1]; ends = np.r_[gap + 1, len(samp)]
    def words(bits):
        out = []
        for i in range(0, len(bits) - bits_per_word + 1, bits_per_word):
            w = bits[i:i + bits_per_word]
            if lsb: w = w[::-1]
            out.append(int("".join(map(str, w)), 2))
        return out, len(bits) % bits_per_word
    for a, b in zip(starts, ends):
        t = samp[a] / rate
        mo, rem = words(mosi[a:b]); mi, _ = words(miso[a:b])
        bz = busy[samp[a]]
        extra = f" (+{rem} bits)" if rem else ""
        print(f"{t*1e3:10.3f} ms  busy={bz}  n={b-a:4d}{extra}\n"
              f"      >> {' '.join(f'{x:02x}' for x in mo)}\n"
              f"      << {' '.join(f'{x:02x}' for x in mi)}")

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("capture")
    ap.add_argument("--edge", choices=["rising", "falling"], default="rising")
    ap.add_argument("--lsb", action="store_true", help="LSB-first words (default MSB-first)")
    ap.add_argument("--gap", type=float, default=None, help="inter-frame clock gap in us (default: 4 bit periods)")
    ap.add_argument("--bits", type=int, default=8, help="bits per word")
    ap.add_argument("--no-frames", action="store_true")
    a = ap.parse_args()
    rate, s = load_sr(a.capture)
    bitlen = survey(rate, s)
    if bitlen is None or a.no_frames: return
    gap = a.gap if a.gap is not None else 4 * bitlen / rate * 1e6
    print(f"\nframes (edge={a.edge}, {'LSB' if a.lsb else 'MSB'}-first, gap>{gap:.1f} us):")
    frames(rate, s, a.edge, a.lsb, gap, a.bits)

if __name__ == "__main__":
    main()

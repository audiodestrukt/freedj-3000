#!/usr/bin/env python3
"""Send ProDJ Link beat packets to freedj for testing, in the real wire format.

One packet per beat at the given BPM, 96 bytes, laid out per the Deep Symmetry
analysis (https://djl-analysis.deepsymmetry.org/djl-analysis/beats.html) and
verified against prolink_virtual_cdj traffic.  Real CDJ/XDJ hardware sends
these on UDP 50001.

For a full virtual deck (announces, status, master handoff) use the
prolink-cpp harness instead — see docs/reference/link-test-harness.md.

Usage:  python3 send_beat.py [bpm] [host] [port] [player]
  bpm     — beats per minute (default 123.0)
  host    — destination (default 127.0.0.1)
  port    — destination port (default 50001)
  player  — device number 1–6 (default 2)
"""

import socket
import struct
import sys
import time

MAGIC    = b"Qspt1WmJOL"
PKT_BEAT = 0x28
PITCH_UNITY = 0x00100000


def make_beat_packet(bpm: float, beat_in_bar: int, player: int, name: bytes = b"send_beat") -> bytes:
    pkt = bytearray(0x60)
    pkt[0:10]   = MAGIC
    pkt[0x0a]   = PKT_BEAT
    pkt[0x0b:0x0b + len(name)] = name[:20]
    pkt[0x1f]   = 0x01
    pkt[0x21]   = player
    struct.pack_into(">H", pkt, 0x22, 0x3c)          # remaining length

    beat_ms = 60000.0 / bpm
    to_bar  = 5 - beat_in_bar                          # beats until next downbeat
    counts  = [1, 2, to_bar, 4, to_bar + 4, 8]         # next, 2nd, bar, 4th, 2nd bar, 8th
    for i, n in enumerate(counts):
        struct.pack_into(">I", pkt, 0x24 + 4 * i, int(round(n * beat_ms)))

    pkt[0x3c:0x54] = b"\xff" * 24
    struct.pack_into(">I", pkt, 0x54, PITCH_UNITY)     # +0%
    struct.pack_into(">H", pkt, 0x5a, int(round(bpm * 100)))
    pkt[0x5c]   = beat_in_bar
    pkt[0x5f]   = player
    return bytes(pkt)


def main():
    bpm    = float(sys.argv[1]) if len(sys.argv) > 1 else 123.0
    host   = sys.argv[2]        if len(sys.argv) > 2 else "127.0.0.1"
    port   = int(sys.argv[3])   if len(sys.argv) > 3 else 50001
    player = int(sys.argv[4])   if len(sys.argv) > 4 else 2

    beat_s = 60.0 / bpm
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    print(f"Sending {bpm} BPM beat packets as player {player} to {host}:{port}  (Ctrl-C to stop)")

    beat = 1
    try:
        # Sleep to an absolute deadline so scheduling jitter does not accumulate.
        next_beat = time.monotonic()
        while True:
            sock.sendto(make_beat_packet(bpm, beat, player), (host, port))
            beat = beat % 4 + 1
            next_beat += beat_s
            delay = next_beat - time.monotonic()
            if delay > 0:
                time.sleep(delay)
    except KeyboardInterrupt:
        print("\nStopped.")


if __name__ == "__main__":
    main()

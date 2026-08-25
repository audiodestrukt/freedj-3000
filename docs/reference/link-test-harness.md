# ProDJ Link test harness

How to put a second deck on the network without owning a second deck.

## The harness: prolink-cpp

Checked out at `~/sandbox/thirdparty/prolink-cpp` (grantHarris/prolink-cpp,
MIT, C++17), built March 2026. Binaries in `build/`:

| Binary | What it does |
|---|---|
| `prolink_virtual_cdj` | A full virtual CDJ: announces on 50000, sends beats on 50001 and status on 50002, acts as tempo master |
| `prolink_virtual_cdj_interactive` | Same, with a menu to change BPM, beat position, beat-in-bar, master/sync at runtime |
| `prolink_listener` | Prints every beat and status packet it sees — an independent decoder to compare ours against |
| `prolink_control_demo` | Discovers devices and exercises sync/master commands |

Rebuild if needed:

```bash
cd ~/sandbox/thirdparty/prolink-cpp && mkdir -p build && cd build && cmake .. && cmake --build .
```

## Running it against freedj on one machine

```bash
make virtual-cdj            # default: device 5, 128 BPM, on eno1
make run                    # in another terminal
```

`make virtual-cdj` runs

```
prolink_virtual_cdj <device_ip> <broadcast_ip> <mac> <device> <name> <bpm>
```

with the IP and broadcast taken from `IFACE` (default `eno1`). Two traps in
its argument parser, learned the hard way:

- **Do not pass device number 7.** Its default is 7 and it detects "unset" by
  comparing against 7, so `7` leaves it unset and the *name* gets parsed as
  the device number → "device_number must be non-zero". Use 1–6.
- **It exits on stdin EOF** ("Press Enter to stop"). Backgrounding it with no
  stdin kills it immediately. The Makefile pipes a `sleep` into it.

Verified 2026-08-25: with freedj listening on 50001 and 50002 with
`SO_REUSEADDR|SO_REUSEPORT`, both processes coexist on one machine and freedj
receives everything — no port forwarding needed. (The earlier note in
`prodj.rs` about `socat UDP4-RECV:50002,reuseport,fork UDP4-SENDTO:127.0.0.1:50052`
predates the reuse-port bind and the 50001 listener; it is obsolete.)

## What freedj should log

```
ProDJ sniffer: port 50000 rx 48 bytes ...     every 1.5 s — announce
ProDJ rx :50001 96 bytes from <ip>:50001 ...  every beat  — beat packet, full hex
ProDJ beat: player 5 @ 128.00 BPM ...          the parser decoding it
ProDJ rx :50002 ...                            ~5/s        — status, not yet decoded
```

If the `rx :50001` lines appear but no `ProDJ beat:` lines follow, the parser
is rejecting real packets — which is exactly what happened before 2026-08-25,
when it only understood `send_beat.py`'s private layout.

## The lightweight alternative: send_beat.py

`tools/send_beat.py` sends only beat packets, in the real 96-byte format, to
50001 by default. No announces, no status, no master handoff — enough to
drive the B2 strip and the phase meter, not enough to test discovery.

```bash
make two-deck BPM=130       # freedj + send_beat.py together
```

## Beat packet layout

Verified byte-for-byte against the harness capture that is pinned in
`crates/link/src/prodj.rs` tests:

```
0x00–0x09  "Qspt1WmJOL"
0x0a       0x28
0x0b–0x1e  device name, NUL padded
0x1f       0x01
0x21       device number
0x22–0x23  remaining length, 0x003c
0x24–0x3b  six u32 BE, ms until: next beat, 2nd beat, next bar, 4th beat, 2nd bar, 8th beat
0x3c–0x53  0xFF × 24
0x54–0x57  pitch, u32 BE, 0x00100000 = +0%   (the harness sets a flag in the top byte)
0x5a–0x5b  BPM × 100, u16 BE
0x5c       beat within bar, 1–4
0x5f       device number again
           96 bytes
```

Source: <https://djl-analysis.deepsymmetry.org/djl-analysis/beats.html>

## With the real XDJ-1000MK2

Ethernet from the deck's LINK port to this machine (direct cable is fine —
the decks auto-negotiate crossover), play a track, `make dev`. Same log lines
as above, from the real thing. Press MASTER on the deck and watch the status
packets change: that handshake is what Link *send* will eventually have to
speak (WORKSTREAMS §B2).

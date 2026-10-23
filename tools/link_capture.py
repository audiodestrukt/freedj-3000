#!/usr/bin/env python3
"""Capture Pro DJ Link traffic from the players on this network.

For anyone with a CDJ-3000 (or any CDJ/XDJ) who wants to help: plug the
deck's LINK port into the same ethernet as this computer, run this, play a
track, press MASTER / SYNC a few times, stop it with Ctrl-C, and attach the
.log it wrote to a report (docs/testing/cdj-3000-interop.md). No build, no
dependencies: Python 3 only, works on macOS, Linux and Windows.

It listens on UDP 50000 (announce), 50001 (beat) and 50002 (status), writes
every packet with its full bytes to link-capture-<date>.log, and prints a
decoded one-liner per packet so you can see it working.  Nothing is sent.

The log is replayed through OpenDeck's parser offline with
    cargo run -p opendeck-link --example replay_capture -- link-capture-*.log

Usage:  python3 tools/link_capture.py [--quiet] [--out FILE]
"""
import datetime, select, socket, struct, sys

MAGIC = b"Qspt1WmJOL"
PORTS = (50000, 50001, 50002)
TYPES = {0x06: "announce", 0x0a: "status", 0x28: "beat", 0x26: "master-req",
         0x27: "master-rsp", 0x2a: "sync-ctrl", 0x05: "media-query", 0x29: "absolute-pos"}

def name_at(d, off):
    return d[off:off + 20].split(b"\0", 1)[0].decode("ascii", "replace")

def decode(port, d):
    """One line of human-readable meaning, best effort."""
    if len(d) < 0x0b or d[:10] != MAGIC:
        return "not Pro DJ Link"
    t = d[0x0a]
    kind = TYPES.get(t, f"type {t:#04x}")
    try:
        if t == 0x06 and len(d) >= 0x30:
            mac = ":".join(f"{b:02x}" for b in d[0x26:0x2c])
            ip = ".".join(str(b) for b in d[0x2c:0x30])
            return f"{kind} {name_at(d, 0x0c)!r} player {d[0x24]} ip {ip} mac {mac}"
        if t == 0x28 and len(d) >= 0x60:
            bpm = struct.unpack(">H", d[0x5a:0x5c])[0] / 100
            pitch = (struct.unpack(">I", d[0x54:0x58])[0] - 0x100000) / 0x100000 * 100
            return f"{kind} player {d[0x21]} bpm {bpm:.2f} pitch {pitch:+.2f}% beat {d[0x5c]}/4"
        if t == 0x0a and len(d) >= 0xcc:
            f = d[0x89]
            flags = "".join(s for s, m in (("PLAY ", 0x40), ("MASTER ", 0x20), ("SYNC ", 0x10), ("ONAIR ", 0x08)) if f & m)
            bpm = struct.unpack(">H", d[0x92:0x94])[0]
            bpm_s = "none" if bpm == 0xffff else f"{bpm / 100:.2f}"
            pitch = (struct.unpack(">I", d[0x8c:0x90])[0] - 0x100000) / 0x100000 * 100
            fw = d[0x7c:0x80].decode("ascii", "replace")
            return (f"{kind} player {d[0x21]} len {len(d):#x} fw {fw} P1 {d[0x7b]:#04x} {flags}"
                    f"pitch {pitch:+.2f}% bpm {bpm_s} Mm {d[0x9e]} Mh {d[0x9f]:#04x} bar-beat {d[0xa6]}")
        if t in (0x26, 0x27, 0x2a) and len(d) >= 0x28:
            return f"{kind} from player {d[0x21]} payload {d[0x24:].hex()}"
    except (struct.error, IndexError):
        pass
    return f"{kind} player {d[0x21] if len(d) > 0x21 else '?'} len {len(d):#x}"

def main(argv):
    quiet = "--quiet" in argv
    out = None
    if "--out" in argv:
        out = argv[argv.index("--out") + 1]
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    out = out or f"link-capture-{stamp}.log"
    socks = []
    for p in PORTS:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if hasattr(socket, "SO_REUSEPORT"):
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        try:
            s.bind(("", p))
        except OSError as e:
            sys.exit(f"cannot bind UDP {p}: {e} (is another Link app or OpenDeck running? close it, or run with --quiet after stopping it)")
        socks.append(s)
    seen = {}        # (player) -> dict of what we know
    counts = {}
    print(f"listening on UDP {PORTS}; writing {out}; Ctrl-C to stop")
    with open(out, "w") as fh:
        fh.write("# opendeck link capture v1\n# time port src len hex  -- decoded\n")
        try:
            while True:
                ready, _, _ = select.select(socks, [], [], 1.0)
                for s in ready:
                    data, (ip, sport) = s.recvfrom(4096)
                    port = s.getsockname()[1]
                    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")
                    line = decode(port, data)
                    fh.write(f"{now} {port} {ip}:{sport} {len(data)} {data.hex()}  -- {line}\n")
                    fh.flush()
                    key = (port, data[0x0a] if len(data) > 0x0a else -1, len(data))
                    counts[key] = counts.get(key, 0) + 1
                    if len(data) > 0x21 and data[:10] == MAGIC:
                        pl = data[0x24] if data[0x0a] == 0x06 else data[0x21]
                        info = seen.setdefault(pl, {"ip": ip, "names": set(), "status_len": set(), "fw": set()})
                        if data[0x0a] == 0x06:
                            info["names"].add(name_at(data, 0x0c))
                        elif data[0x0a] == 0x0a and len(data) >= 0x80:
                            info["status_len"].add(len(data))
                            info["fw"].add(data[0x7c:0x80].decode("ascii", "replace"))
                    if not quiet and (counts[key] <= 3 or counts[key] % 50 == 0):
                        print(f"{port} {ip} {line}")
        except KeyboardInterrupt:
            pass
    print(f"\nwrote {out}")
    print("players seen:")
    for pl, info in sorted(seen.items()):
        print(f"  player {pl}: {sorted(info['names'])} at {info['ip']} status len {sorted(info['status_len'])} fw {sorted(info['fw'])}")
    print("packets:", ", ".join(f"port {p} type {t:#04x} len {l}: {n}" for (p, t, l), n in sorted(counts.items())))

if __name__ == "__main__":
    main(sys.argv[1:])

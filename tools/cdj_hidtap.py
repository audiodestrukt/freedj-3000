#!/usr/bin/env python3
"""Tap the emulated CDJ's PC Link (USB-B) traffic on Linux.

Connects to QEMU's cdj3k.usb-link socket (the emulator's own PcLink only
exists on macOS), does the HELLO/IDENTITY handshake, then prints every HID
report, highlighting the bytes that changed since the previous one.
Frame: type(1) len(2, BE) payload.  1 HID, 2 MIDI, 3 HELLO, 4 IDENTITY.

  hidtap.py [sock] [--send HEX]   (--send writes one host->deck HID report)
"""
import socket, struct, sys, time, select

SOCK = "/tmp/cdj3k-emu-1000/instance-1/usb-link.sock"
args = [a for a in sys.argv[1:]]
send_hex = None
if "--send" in args:
    i = args.index("--send"); send_hex = args[i + 1]; del args[i:i + 2]
if args: SOCK = args[0]

s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(SOCK)
buf = b""
last = {}
t0 = time.time()
next_hello = 0
have_id = False

def frame(t, p=b""):
    return bytes([t]) + struct.pack(">H", len(p)) + p

while True:
    now = time.time()
    if not have_id and now >= next_hello:
        s.sendall(frame(3)); next_hello = now + 0.3
    r, _, _ = select.select([s], [], [], 0.1)
    if not r: continue
    d = s.recv(65536)
    if not d: print("EOF"); break
    buf += d
    while len(buf) >= 3:
        t = buf[0]; n = struct.unpack(">H", buf[1:3])[0]
        if len(buf) < 3 + n: break
        p = buf[3:3 + n]; buf = buf[3 + n:]
        ts = f"{time.time() - t0:8.3f}"
        if t == 4:
            have_id = True
            print(ts, "IDENTITY:", p.decode(errors="replace").replace("\n", " | "), flush=True)
            if send_hex:
                s.sendall(frame(1, bytes.fromhex(send_hex))); print(ts, "sent", send_hex)
        elif t == 1:
            prev = last.get(1)
            if prev == p: continue
            hx = []
            for i, b in enumerate(p):
                ch = prev is None or i >= len(prev) or prev[i] != b
                hx.append(f"\x1b[1;33m{b:02x}\x1b[0m" if ch and prev is not None else f"{b:02x}")
            diff = "" if prev is None else " changed@" + ",".join(
                str(i) for i in range(len(p)) if i >= len(prev) or prev[i] != p[i])
            print(ts, f"HID[{n}]", " ".join(hx), diff, flush=True)
            last[1] = p
        elif t == 2:
            print(ts, "MIDI", p.hex(" "), flush=True)
        else:
            print(ts, f"type {t}", p.hex(" "), flush=True)

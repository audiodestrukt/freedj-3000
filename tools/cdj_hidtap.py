#!/usr/bin/env python3
"""Tap the emulated CDJ's PC Link (USB-B) traffic on Linux.

Connects to QEMU's cdj3k.usb-link socket (the emulator's own PcLink only
exists on macOS), does the HELLO/IDENTITY handshake, then prints every HID
report, highlighting the bytes that changed since the previous one.
Frame: type(1) len(2, BE) payload.  1 HID, 2 MIDI, 3 HELLO, 4 IDENTITY.
The payload is the bare report (no hidraw report-number byte), 64 bytes on
the CDJ-3000.

  cdj_hidtap.py [sock] [--decode] [--log FILE] [--send HEX]

  --decode   label the fields of type-0x20 input reports per the public
             analysis (swiftb0y.github.io/CDJHidProtocol, control.html) so
             the emulator can confirm or correct it; handshake stages
             0x11-0x15 are named too
  --log FILE append every frame as a JSON line (t, type, hex) for later
             diffing; keep it in the scratchpad, not the repo
  --send HEX write one host->deck HID report after IDENTITY

The bridge in the guest must be running: Developer Mode on, then
  printf 'sh systemctl start cdj3k-pc-link-bridge\\n' | nc 127.0.0.1 4401
"""
import json, socket, struct, sys, time, select

SOCK = "/tmp/cdj3k-emu-1000/instance-1/usb-link.sock"
args = [a for a in sys.argv[1:]]
send_hex = None
log_path = None
decode = False
if "--send" in args:
    i = args.index("--send"); send_hex = args[i + 1]; del args[i:i + 2]
if "--log" in args:
    i = args.index("--log"); log_path = args[i + 1]; del args[i:i + 2]
if "--decode" in args:
    args.remove("--decode"); decode = True
if args: SOCK = args[0]

# Input report 0x20, byte -> (bit -> name).  Bits are 1 while pressed.
BUTTONS = {
    0x02: {7: "PLAY", 6: "CUE", 5: "SEARCH>", 4: "<SEARCH", 3: "TRACK>", 2: "CALL 1/2X", 1: "CALL 2X"},
    0x03: {7: "IN/CUE", 6: "OUT", 5: "RELOOP", 2: "TIME/AUTOCUE", 1: "MEMORY", 0: "DELETE"},
    0x04: {7: "JOG MODE", 4: "JOG TOUCH", 3: "TEMPO RANGE", 2: "MASTER TEMPO", 1: "TEMPO RESET", 0: "NEEDLE TOUCH"},
    0x05: {7: "LIB VIEW", 6: "QUANTIZE", 5: "MASTER", 4: "SYNC", 3: "ROTARY PUSH", 2: "BACK", 1: "TAG", 0: "EJECT"},
    0x06: {7: "SLIP", 6: "REV", 5: "SLIP REV", 3: "TRACK FILTER", 2: "CALL/DELETE"},
    0x08: {7: "LOOP 32", 6: "LOOP 16", 5: "LOOP 8", 4: "LOOP 4", 3: "LOOP 2", 2: "LOOP 1"},
    0x09: {4: "LOOP 1/4", 3: "LOOP 1/2", 2: "4/8 BEAT"},
    0x0D: {7: "JUMP+1", 6: "JUMP+2", 5: "JUMP+4", 4: "JUMP+8", 3: "JUMP+16"},
    0x0E: {7: "JUMP-1", 6: "JUMP-2", 5: "JUMP-4", 4: "JUMP-8", 3: "JUMP-16"},
    0x0F: {7: "HOTCUE A", 6: "HOTCUE B", 5: "HOTCUE C", 4: "HOTCUE D", 3: "HOTCUE E", 2: "HOTCUE F", 1: "HOTCUE G", 0: "HOTCUE H"},
}
JOG_DIR = {0b11: "fwd", 0b10: "rev", 0b01: "still", 0b00: "still"}
STAGES = {0x11: "HANDSHAKE 1 CDJ fw version", 0x12: "HANDSHAKE 2 host seedA", 0x13: "HANDSHAKE 3 CDJ hashA/seedE",
          0x14: "HANDSHAKE 4 host hashE/devid", 0x15: "HANDSHAKE 5 ok", 0x20: "INPUT", 0x21: "OUTPUT"}

def u16(p, o):
    return p[o] | (p[o + 1] << 8) if o + 1 < len(p) else None

def decode_input(p):
    if len(p) < 0x1D or p[1] != 0x20:
        return None
    pressed = [name for off, bits in BUTTONS.items() for bit, name in bits.items() if p[off] >> bit & 1]
    return (f"jog pos={u16(p, 0x17):5d} speed={u16(p, 0x19):5d} dir={JOG_DIR[p[4] >> 5 & 3]}"
            f" tempo={u16(p, 0x15):5d} needle={u16(p, 0x1B):4d} rotary={u16(p, 0x13):5d}"
            f" vinyl={p[0x11]:3d}/{p[0x12]:3d}" + (" | " + " ".join(pressed) if pressed else ""))

s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(SOCK)
log = open(log_path, "a") if log_path else None
buf = b""
last = {}
t0 = time.time()
next_hello = 0
have_id = False
n_in = 0

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
        now = time.time()
        ts = f"{now - t0:8.3f}"
        if log:
            log.write(json.dumps({"t": round(now - t0, 4), "type": t, "hex": p.hex()}) + "\n"); log.flush()
        if t == 4:
            have_id = True
            print(ts, "IDENTITY:", p.decode(errors="replace").replace("\n", " | "), flush=True)
            if send_hex:
                s.sendall(frame(1, bytes.fromhex(send_hex))); print(ts, "sent", send_hex)
        elif t == 1:
            n_in += 1
            prev = last.get(1)
            if prev == p: continue
            hx = []
            for i, b in enumerate(p):
                ch = prev is None or i >= len(prev) or prev[i] != b
                hx.append(f"\x1b[1;33m{b:02x}\x1b[0m" if ch and prev is not None else f"{b:02x}")
            diff = "" if prev is None else " changed@" + ",".join(
                str(i) for i in range(len(p)) if i >= len(prev) or prev[i] != p[i])
            kind = STAGES.get(p[1], f"type 0x{p[1]:02x}") if len(p) > 1 else "?"
            print(ts, f"HID[{n}] {kind}", " ".join(hx), diff, flush=True)
            if decode:
                dec = decode_input(p)
                if dec: print(ts, "   ", dec, flush=True)
            last[1] = p
        elif t == 2:
            print(ts, "MIDI", p.hex(" "), flush=True)
        else:
            print(ts, f"type {t}", p.hex(" "), flush=True)

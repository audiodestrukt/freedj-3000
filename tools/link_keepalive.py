#!/usr/bin/env python3
"""Unicast Pro DJ Link keep-alives (0x06 announce) to one device.

Normally announces are broadcast on UDP 50000 and never cross a routed link
(Tailscale, VPN).  Sending the same packet unicast to the target lands in the
same socket, so a rekordbox / player on the far side lists us as a member and
will then serve NFS / dbserver to our address.

    tools/link_keepalive.py <target-ip> <our-ip> [device-number] [name]
"""
import socket, struct, sys, time, random

target = sys.argv[1]; me = sys.argv[2]
device = int(sys.argv[3]) if len(sys.argv) > 3 else 3
name = (sys.argv[4] if len(sys.argv) > 4 else "freedj-3000").encode()[:20]
mac = bytes([0x02, 0x0d, 0xec, 0x00, 0x00, device])          # locally administered

pkt = bytearray(0x36)
pkt[0:10] = b"Qspt1WmJOL"; pkt[0x0a] = 0x06
pkt[0x0c:0x0c + len(name)] = name
pkt[0x20] = 0x01; pkt[0x21] = 0x02
pkt[0x22:0x24] = struct.pack(">H", 0x36)
pkt[0x24] = device; pkt[0x25] = 0x01                          # device type CDJ
pkt[0x26:0x2c] = mac
pkt[0x2c:0x30] = socket.inet_aton(me)
pkt[0x30] = 0x01; pkt[0x34] = 0x01

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind((me, 50000))
print(f"keep-alive: device {device} {name.decode()} {me} -> {target}:50000 every 1.5s", flush=True)
while True:
    s.sendto(pkt, (target, 50000))
    time.sleep(1.5)

#!/usr/bin/env python3
"""Ask a Pro DJ Link device's portmapper where its mountd / nfsd live.

CDJ/XDJ players run portmap on the standard UDP 111.  rekordbox (laptop) runs
it on UDP **50111** (a laptop can't bind 111 unprivileged), which is why an
earlier probe of :111 concluded rekordbox had no NFS.  Usage:

    tools/portmap_probe.py 192.168.68.60          # tries 111 and 50111
    tools/portmap_probe.py 192.168.68.58 111
"""
import os, socket, struct, sys

def getport(host, pmport, prog, vers, proto=17, timeout=2.0):
    xid = int.from_bytes(os.urandom(4), "big")
    # RPC v2 CALL: xid, CALL, rpcvers, prog=portmap, vers=2, proc=GETPORT, AUTH_NULL cred+verf
    hdr = struct.pack(">IIIIII", xid, 0, 2, 100000, 2, 3) + struct.pack(">IIII", 0, 0, 0, 0)
    args = struct.pack(">IIII", prog, vers, proto, 0)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(timeout)
    try:
        s.sendto(hdr + args, (host, pmport)); d, _ = s.recvfrom(512)
        astat = struct.unpack(">I", d[20:24])[0]
        port = struct.unpack(">I", d[24:28])[0]
        return f"accept={astat} port={port}"
    except socket.timeout:
        return "no answer"
    finally:
        s.close()

if __name__ == "__main__":
    host = sys.argv[1]
    ports = [int(p) for p in sys.argv[2:]] or [111, 50111]
    for pm in ports:
        print(f"{host} portmap:{pm}  mountd(100005 v1): {getport(host, pm, 100005, 1)}"
              f"  |  nfsd(100003 v2): {getport(host, pm, 100003, 2)}")

"""Read bootlog.c's RAM log back over USB (the FX2's own 0xA0 upload request).

The CPU is halted first: while it runs, 0xA0 reads return junk on this chip.
Reload the firmware afterwards to run it again."""
import usb.core, usb.backend.libusb1
be = usb.backend.libusb1.get_backend(find_library=lambda _: "/opt/homebrew/lib/libusb-1.0.dylib")
dev = usb.core.find(idVendor=0x0925, idProduct=0x3881, backend=be)
dev.ctrl_transfer(0x40, 0xA0, 0xE600, 0, b"\x01", timeout=1000)   # CPU held in reset
def peek(addr, n):
    out = b""
    while n:
        k = min(n, 1024)
        out += bytes(dev.ctrl_transfer(0xC0, 0xA0, addr, 0, k, timeout=1000))
        addr += k; n -= k
    return out
TICK_MS = 65536 / 4e6 * 1e3
h = peek(0x2000, 16)
print(f"magic {h[:4]!r} stage {h[4]} events {h[5]} frames logged {h[7]} "
      f"sent {h[8] | h[9] << 8} BUSY rose after {h[10] | h[11] << 8}")
w = h[12] | h[13] << 8
print(f"SCK rise after a test release: {'never (no pull-up?)' if w == 0xffff else f'{w} polls'}")
ev = peek(0x2010, 4 * h[5])
for i in range(h[5]):
    t = (ev[4 * i] | ev[4 * i + 1] << 8) * TICK_MS + ev[4 * i + 2] * 256 / 4e3
    s = ev[4 * i + 3]
    what = "frame, BUSY did not rise; now" if s & 0x80 else "line change:"
    print(f"  {t:8.1f} ms after TXD rose: {what} TXD={s & 1} BUSY={s >> 1 & 1}")
fl = peek(0x2110, 36 * h[7])
for i in range(h[7]):
    e = fl[36 * i:36 * i + 36]
    at = (e[2] | e[3] << 8) * TICK_MS
    print(f"{i:2d} @{at:6.0f} ms waited {e[0] * TICK_MS:5.0f} ms  BUSY rose {e[1]}  << {e[4:].hex(' ')}")

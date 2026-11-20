"""Load an Intel HEX image into an FX2LP's RAM with the chip's own 0xA0 request."""
import sys, usb.core, usb.backend.libusb1
be = usb.backend.libusb1.get_backend(find_library=lambda _: "/opt/homebrew/lib/libusb-1.0.dylib")
dev = usb.core.find(idVendor=0x0925, idProduct=0x3881, backend=be)
if dev is None: sys.exit("no 0925:3881 device")
def poke(addr, data): dev.ctrl_transfer(0x40, 0xA0, addr, 0, bytes(data), timeout=1000)
recs = []
for line in open(sys.argv[1]):
    line = line.strip()
    if not line.startswith(":"): continue
    b = bytes.fromhex(line[1:]); n, addr, typ = b[0], (b[1] << 8) | b[2], b[3]
    if typ == 0: recs.append((addr, b[4:4 + n]))
poke(0xE600, [1])                       # CPU held in reset
for addr, data in recs: poke(addr, data)
poke(0xE600, [0])                       # run
print(f"loaded {sum(len(d) for _, d in recs)} bytes in {len(recs)} records")

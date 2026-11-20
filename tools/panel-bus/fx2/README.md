# fx2: the logic analyser as the panel-link master

The 8-channel fx2 analyser is a Cypress CY7C68013A whose firmware is loaded
into RAM on every plug-in, so it can run our own code instead of sigrok's.
The one used here (a "Lonely Binary" 24 MHz 8CH) has no input buffer: its
D0-D7 pins can drive outputs (checked with `blink.c`, D7 toggling).

- `blink.c`: drives D7 only, toggling about once a second. Output check.
- `bootlog.c`: master in the power-on window only (panel ready to 4 s, then
  released by a hardware timer), logging the start-up and every reply to RAM;
  `readlog.py` halts the CPU and reads the log (0xA0 reads return junk while
  the CPU runs). Results in `docs/reference/xdj-1000mk2-panel-bus.md`.
- `ring.c`: panel-link master, standalone (no USB): waits for BUSY low,
  clocks one 32-byte frame (SPI mode 3, LSB first, checksum), and steps the
  jog ring position (byte 9, 1..135) about every 4 frames. D0 = SCK out,
  D1 = RXD out, D2 = TXD in, D3 = BUSY in. The frame is the stock main
  board's idle frame. **Not yet run against the panel.**
- `load.py`: loads an Intel HEX file with the chip's 0xA0 request.

    make ring.ihx && python3 load.py ring.ihx     # pyusb + Homebrew libusb

**Only with the main board cut off from SCK and RXD** (pins 19 and 17): two
masters on one bus fight. The non-invasive way is a passthrough built from
two 20-pin 1.0 mm FFC-to-2.54 mm adapters joined by jumper wires, leaving out
17 and 19, with the analyser on the panel side through ~330 Ω on D0/D1. Do
the continuity check in `../README.md` first. PNL_RST (13) may be left
unconnected: the panel has its own reset IC and Q8026 holds it out of reset
(`docs/reference/xdj-1000mk2-panel-bus.md`).

After loading, the analyser no longer answers sigrok; unplug and replug it.

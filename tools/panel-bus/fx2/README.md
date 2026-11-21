# fx2: the logic analyser as the panel-link master

The 8-channel fx2 analyser is a Cypress CY7C68013A whose firmware is loaded
into RAM on every plug-in, so it can run our own code instead of sigrok's.
The one used here (a "Lonely Binary" 24 MHz 8CH) has no input buffer: its
D0-D7 pins (port B) can drive outputs.

Wiring (CN8002 legs on PNLB): D0 = 19 SCK, D1 = 17 RXD, D2 = 15 TXD,
D3 = 18 BUSY, D4 = 13 PNL_RST, GND = 20. For master mode the main board is cut
off SCK, RXD and BUSY by masking CN2602 contacts 1-4 at the LCDB end of the
ribbon. The full procedure, and why each step is there, is in
`docs/reference/xdj-1000mk2-panel-bus.md`, "Driving the panel".

## The map page (what to use)

    make bridge.ihx
    python3 bridge.py        # deck off; it loads bridge.ihx and waits; switch the deck on
                             # then open http://127.0.0.1:8765

`bridge.c` holds SCK/RXD high and PNL_RST low from the panel's power-up,
fakes the main board's BUSY pull-down, sends the page's frame every ~3 ms at
the stock timing and hands back each valid reply over the FX2's default bulk
EP1 (no USB code; needs a freshly plugged-in analyser). `bridge.py` serves the
page: every output bit as a toggle, the jog ring slider, the panel's inputs
decoded live. `bridge.py --no-load` restarts the page only. Needs pyusb and
Homebrew's libusb.

## The rest, in the order they were written

| File | What it is |
|---|---|
| `blink.c` | D7 toggling: the check that the pins can drive |
| `ring.c` | first standalone master (jog ring sweep); never worked as is |
| `bootlog.c` | master in the power-on window with the main board still connected, logging to RAM: one valid reply, then silent |
| `runwrite.c` | writes into a running deck, every 4th frame for 2 s: two masters scramble each other |
| `master.c` | after-boot master with the SCK interlock, holding and reading PNL_RST |
| `master_fixed.c` | as `master.c`, frames on a fixed schedule ignoring BUSY |
| `master_busylow.c` | holds BUSY low through an RST pulse (that pulse reboots the deck) |
| `master_boot.c` | from power-on, SCK/RXD held high, RST held low |
| `master_pd.c` | as `master_boot.c`, BUSY held low 0.6 s and its rise timed: found the pull-down |
| `master_pd2.c` | the faked BUSY pull-down: the first master the panel followed |
| `bridge.c` | `master_pd2.c` + EP1 to the Mac, 3 ms pacing |
| `load.py` | loads an Intel HEX file with the chip's 0xA0 request |
| `readlog.py` | halts the CPU (0xA0 reads are junk while it runs) and prints a RAM log |

Never pulse PNL_RST: a panel reset glitches PWR_SW and the deck reboots.
"All on" in the page (bytes 2-29 = 0xFF) puts the deck into standby; a
control wakes it.

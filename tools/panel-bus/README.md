# Panel-bus capture kit (XDJ-1000MK2, PNLB CN8002)

Observe the link between the XDJ's main CPU and its panel CPU on our own unit.
Background and pinout: `docs/reference/xdj-1000mk2-panel-bus.md`.

1. `sudo apt install sigrok-cli sigrok-firmware-fx2lafw pulseview`, then
   replug the analyser (the package installs the udev rule that makes it
   usable without root).  `sigrok-cli --scan` must list `fx2lafw`.
   macOS: `brew install sigrok-cli` (no udev step).  The clone shows up as
   0925:3881 ("2341:14465" in `ioreg`, which prints decimal).  After a replug
   the first capture uploads the firmware and then fails with "Device has been
   disconnected"; run it again and it captures.  An unwired fx2 clone reads
   every channel as constant high.
2. Fit the breakout between PNLB CN8002 and LCDB CN2602, **deck unplugged**,
   and check continuity before any power: header 20 and 11 beep to chassis,
   header 1 and 3 (V+5R2) do not, and 1 does not beep to 20.  The stock
   DDD1787 is *reverse face*; the wrong face on either breakout cable puts
   pin n on pin 21-n, which shorts V+5R2 to GND and breaks PWR_SW (pin 12),
   so the deck will not even power on.  Then wire the analyser as in
   `capture.sh`, GND last, and check with a meter that pin 19 (SCK) idles at
   0 or 3.3 V.
3. Captures, deck powered, in this order:
   - `capture.sh idle 5` : nothing touched.  Is the bus polled continuously or
     only on events?  Clock rate, burst length, BUSY behaviour.
   - `capture.sh play 5` : press PLAY at ~2 s, again at ~4 s.
   - `capture.sh cue 5`, `capture.sh jog-cw 5` (one slow turn), `capture.sh
     jog-ccw 5`, `capture.sh tempo 5` (fader end to end), `capture.sh browse
     5` (encoder a few clicks + press), `capture.sh touch 5` (one tap on the
     LCD), `capture.sh boot 20` (started before power-on: the reset/handshake
     and any LED init).
4. `decode.py captures/panel-bus/idle.sr` prints a survey (clock, bursts, BUSY)
   then frames.  Try `--edge falling` / `--lsb` until the bytes look
   structured (constant headers, counters, checksums).

Captures stay out of git for now (`captures/` is ignored).

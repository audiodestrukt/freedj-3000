# Panel-bus capture kit (XDJ-1000MK2, PNLB CN8002)

Observe the link between the XDJ's main CPU and its panel CPU on our own unit.
Background and pinout: `docs/reference/xdj-1000mk2-panel-bus.md`.

1. `sudo apt install sigrok-cli sigrok-firmware-fx2lafw pulseview`, then
   replug the analyser (the package installs the udev rule that makes it
   usable without root).  `sigrok-cli --scan` must list `fx2lafw`.
2. Wire the analyser to the breakout on CN8002 as in `capture.sh`.
   Check with a meter first that pin 19 (SCK) idles at 0 or 3.3 V.
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

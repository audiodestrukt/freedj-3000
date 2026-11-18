# Boot straight into OpenDeck

Turn a Raspberry Pi with the desktop (Raspberry Pi OS Bookworm, labwc or
wayfire) into a deck that comes up playing-ready at power on: the desktop
logs itself in and OpenDeck starts fullscreen.

```bash
deploy/pi/autostart/install.sh                  # from a checkout or the release tarball's pi/autostart
deploy/pi/autostart/install.sh ~/OpenDeck/opendeck   # or name the binary
```

Run it as the user who plays, not with sudo (it asks for sudo itself, for
two settings). It:

- installs `~/.local/bin/opendeck-session` and starts it at login through
  `~/.config/autostart/opendeck.desktop`;
- writes `~/.config/opendeck/autostart.env` (the binary's path,
  `OPENDECK_FULLSCREEN=1`; add any other `OPENDECK_*` setting there);
- turns on desktop autologin and turns off screen blanking
  (`raspi-config nonint do_boot_behaviour B4`, `do_blanking 1`).

Then `sudo reboot`.

**Q** quits to the desktop. A crash restarts the deck after 2 s; five
crashes within 10 s of starting in a row stop the restarts (a missing
library or no display would otherwise loop). The last run's output is
`~/.local/state/opendeck/session.log`, the one before it `session.log.1`.

USB sticks: the desktop's file manager mounts them under `/media/<user>`
already, so do not also install `../usb-automount` (that one is for
images without a desktop).

To undo: `deploy/pi/autostart/install.sh --remove`, and
`sudo raspi-config nonint do_boot_behaviour B3` to boot to the login
screen again.

Without a desktop (Pi OS Lite, the Zero W's fast KMSDRM path) this does
not apply: there the deck would run from a systemd unit with
`SDL_VIDEODRIVER=kmsdrm OPENDECK_RENDERER=sdl`; not shipped yet.

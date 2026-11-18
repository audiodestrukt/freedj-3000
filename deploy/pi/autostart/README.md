# Boot straight into OpenDeck

Make a Raspberry Pi come up as a deck at power on: a boot splash, then
OpenDeck fullscreen on the display, with no desktop and no login.

```bash
deploy/pi/autostart/install.sh                      # from a checkout, or the release tarball's pi/autostart
deploy/pi/autostart/install.sh --audio CODEC ~/OpenDeck/opendeck   # name the sound card / binary
sudo reboot
```

Run it as the user who plays, not with sudo (it asks for sudo itself).
Made for Raspberry Pi OS Lite (no desktop); on a desktop image it boots
past the desktop the same way.

What it sets up:

- **The deck:** `opendeck.service` runs OpenDeck in
  [cage](https://github.com/cage-kiosk/cage), a kiosk compositor that
  shows one app fullscreen (installed if missing), on the Pi's screen at
  boot, as your user without a login. If the deck stops (a crash, or Q)
  it is back 2 s later; five stops within a minute leave it stopped. The
  Pi boots to the console target, so a desktop does not start.
- **Sound:** OpenDeck plays to ALSA's default device; the installer
  points that at a USB interface in `/etc/asound.conf`. `--audio CARD`
  names it (the name in brackets in `aplay -l` / `cat /proc/asound/cards`;
  a Behringer UCA202 is `CODEC`); without it, the first USB sound card
  plugged in at install time. A USB card's name stays the same whichever
  port it is in.
- **Boot splash:** `splash.png` (the app icon for now; replace it and
  re-run to change it) from power on until the deck's first frame,
  instead of the rainbow square and the boot text: Plymouth, `quiet
  splash` on the kernel command line, `disable_splash=1` in `config.txt`
  (both backed up as `*.before-opendeck`). The service takes the screen
  from the splash without a black gap. `--no-splash` leaves boot as it was.
- **Settings:** `/etc/default/opendeck` (`KEY=value`; the binary's path,
  `OPENDECK_FULLSCREEN=1`, and any other `OPENDECK_*` setting). Then
  `sudo systemctl restart opendeck`.

Over the network: `journalctl -u opendeck -f` for the log,
`sudo systemctl stop opendeck` to free the screen, `start` to bring it
back. Ctrl+Alt+F2 on a keyboard reaches a console login.

USB sticks: with no desktop nothing mounts them; install
`../usb-automount` too. Real-time audio threads: `../rt-audio` (the
service's session applies its limits).

**Keep the desktop instead:** `install.sh --desktop` logs the desktop in
automatically and starts OpenDeck fullscreen in it (Q quits to the
desktop; settings in `~/.config/opendeck/autostart.env`, log in
`~/.local/state/opendeck/session.log`).

**Undo:** `install.sh --remove` takes either out and restores
`asound.conf`, `cmdline.txt` and `config.txt`.

Not tested on a Pi yet: the installer's steps, the restart loop and the
splash image were checked on a desktop Linux; the service, cage and the
splash handoff need a first boot on the real thing.

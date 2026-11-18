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
  points that at a USB interface in `/etc/asound.conf` (`pcm.!default` to
  `hw:CARD` through `plug`). `--audio CARD`
  names it (the name in brackets in `aplay -l` / `cat /proc/asound/cards`;
  a Behringer UCA202 is `CODEC`); without it, the first USB sound card
  plugged in at install time. A USB card's name stays the same whichever
  port it is in.
- **Boot splash:** audiodestrukt's wordmark (`splash.png`, drawn by
  `make_splash.py` in the brand of audiodestrukt.com: the transistor mark,
  "audio" grey and "destrukt" lime, OPENDECK under it) from power on until
  the deck's first frame, instead of the rainbow square and the boot text:
  Plymouth, `quiet splash` on the kernel command line, `disable_splash=1`
  in `config.txt` (both backed up as `*.before-opendeck`). The service
  takes the screen from the splash without a black gap. Edit
  `make_splash.py` (or drop in any `splash.png`) and re-run the installer to
  change it; `--no-splash` leaves boot as it was.
- **The hardware deck:** `OPENDECK_APPLIANCE=1` in `/etc/default/opendeck`
  tells OpenDeck it is the deck, not a window on a desktop: fullscreen,
  filling the panel (laid out in its own shape and squeezed to the panel's,
  sharp), the CDJ-3000's keys in one row along the top as on the unit (no
  rotary: lists are worked by touch), and no mouse pointer.  cage also gets a transparent cursor theme
  (`XCURSOR_THEME=opendeck-blank`), since it shows its own pointer until the
  mouse first moves over the deck.  A mouse still works, unseen.
- **One screen:** cage spreads the deck over every display connected at
  boot. `--screen DSI-1` keeps it on that one (the official 7" panel) by
  switching the others off on the kernel command line; leave it out if the
  others are only plugged in now and then.
- **Music on the deck:** `~/Music` of the user it runs as is the deck's
  own library (SOURCE's folder row, and what other players can load from it
  over LINK); `OPENDECK_MUSIC=/path` in the settings picks another folder.
- **Settings:** `/etc/default/opendeck` (`KEY=value`; the binary's path,
  `OPENDECK_FULLSCREEN=1`, and any other `OPENDECK_*` setting). Then
  `sudo systemctl restart opendeck`.

See the screen over the network (no cost when unused):
`XDG_RUNTIME_DIR=/run/user/$(id -u) WAYLAND_DISPLAY=wayland-0 grim -o DSI-1 shot.png`
on the Pi (grim is in Raspberry Pi OS).

An HDMI monitor plugged in at boot is not a mirror: cage lays it beside
the panel and stretches the deck over both (the panel shows a corner, and
the deck renders every pixel of both, 4640 × 2160 with a 4K monitor). Boot
with it unplugged, or use `--screen`.

Over the network: `journalctl -u opendeck -f` for the log,
`sudo systemctl stop opendeck` to free the screen, `start` to bring it
back. Ctrl+Alt+F2 on a keyboard reaches a console login.

USB sticks: with no desktop nothing mounts them, so the installer also
installs `../usb-automount` (read-only mounts at `/media/usb/<LABEL>`;
`journalctl -t opendeck-usb -f` to watch). Real-time audio threads: `../rt-audio` (the
service's session applies its limits).

**Keep the desktop instead:** `install.sh --desktop` logs the desktop in
automatically and starts OpenDeck fullscreen in it (Q quits to the
desktop; settings in `~/.config/opendeck/autostart.env`, log in
`~/.local/state/opendeck/session.log`).

**Undo:** `install.sh --remove` takes either out and restores
`asound.conf`, `cmdline.txt` and `config.txt`.

Booted on a Pi 5 (Raspberry Pi OS trixie, the 7" DSI panel, a Behringer
UCA202): about 15 s from power to the deck, 1 % of a core idle, sound at
44.1 kHz on the UCA202, real-time audio threads.  Between the splash and the
deck's first frame the screen is black for a few seconds (cage takes the
display before the deck has drawn).

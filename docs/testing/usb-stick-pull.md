# Pulling the USB stick

What the deck must do when a USB stick is pulled out (or plugged back in)
at any moment, how to test it without touching the hardware, and the last
results.  Applies to the Pi appliance (`deploy/pi/autostart`, with
`deploy/pi/usb-automount`) and to any Linux desktop that mounts sticks.

## What must happen

| When the stick is pulled | The deck |
|---|---|
| Idle, not browsing it | Its `USB: <label>` row leaves SOURCE within about a second. Nothing else changes. |
| Browsing it (BROWSE inside the stick) | Goes back to SOURCE, as a CDJ does, not an empty BROWSE. |
| A track from it loaded (fully read) | Keeps playing: the track is in memory. Cues, loops, waveform all stay. |
| A track from it still reading in (the first second or two after LOAD) | Plays up to where it was read, then ends there (log: `load: decoding stopped early, track ends at …s: … Input/output error`). Before anything is playable: `COULD NOT LOAD THE TRACK` and the previous track stays. |
| Always | The app keeps running: no crash, no restart (`NRestarts=0`), the frame rate holds. |

| When it goes back in | The deck |
|---|---|
| Any time | Mounted again (read-only, `/media/usb/<LABEL>`) in about a second, `USB: <label>` back in SOURCE. |

Only a clean end of file ends a track normally; a read error (`EIO` from
a vanished device) is a failure, handled as above.  The code:
`Browser::poll_media` / `take_browsed_gone` (browser.rs), `LoadMsg::Failed`
(lib.rs), `opendeck-usb-mount@.service` (bound to the device, so systemd
unmounts on removal).

## Automated (runs in CI with every `cargo test`)

`crates/app/src/lib.rs`, module `stick_pull_tests`, on a whole test deck:

| Test | Pulls the stick | Checks |
|---|---|---|
| `a_track_still_reading_plays_to_where_it_got` | while the deck's real loader is reading `amplificate.mp3` from it | the track closes off where reading stopped (about 68 s of 527), no COULD NOT LOAD, it plays on |
| `a_track_pulled_before_it_plays_leaves_the_old_one` | before the first byte | COULD NOT LOAD THE TRACK; the loaded track is unchanged |
| `pulling_the_browsed_stick_goes_to_source` | while BROWSE is inside it (a fake stick from the demo export) | SOURCE, the USB row gone; no jump when BROWSE is elsewhere |

The stick is a `PulledStick` reader: every byte is there to open and seek
(the MP3 decoder reads the end of the file at open), and once pulled,
reads at or past a cut fail with EIO, as a vanished USB device's do.  It
goes through `Prep::complete`, the path every load takes.  Plus
`browser::tests::pulling_the_browsed_stick_is_reported_once` and
`devscript::tests::parses_source`.

What the automated tests do not reach: the automount (udev, systemd,
the mount script) and a real device's behaviour.  That is the next
section, on the Pi.

## Pulling it without touching it

Switching the stick's USB port off in sysfs is, to the system, the stick
being pulled: the device disappears, udev fires `remove`, reads on open
files fail with `EIO`.  Switching it back on is plugging it in again.

```bash
# The stick's USB device (here: sda; use lsblk to find yours).
U=$(readlink -f /sys/block/sda/device | sed 's|/host.*||')/authorized
# On the Pi 5 with the stick in the top USB 3 port that is:
#   /sys/devices/platform/axi/1000120000.pcie/1f00300000.usb/xhci-hcd.1/usb3/3-1/authorized
echo 0 | sudo tee $U     # pull
echo 1 | sudo tee $U     # plug back in
```

`sda` only exists while the stick is in, so read the path before pulling.

## Scripted: load from the stick, play, pull mid-load

`OPENDECK_SCRIPT` drives the deck with no hands (`crates/app/src/devscript.rs`).
`source=` opens a SOURCE row by its label; `open=` enters the BROWSE row
whose name starts with the text (a track row loads).  Folder names are
predictable on any stick, so walk the folders rather than rekordbox's
categories (which reopen wherever the stick was last left).

On the Pi, swap the service's binary for a launcher that sets the script,
then restart it and pull at the right moment:

```bash
cat > ~/opendeck/pulltest.sh <<'EOF'
#!/bin/sh
export OPENDECK_SCRIPT="3:source=USB: CDJ2; 3.5:open=Contents; 4:open=ACRAZE; 4.5:open=Heard It Like This; 6:open=067; 6.1:play; 9:screen=playback; 9.5:shot=/tmp/s1.png; 16:shot=/tmp/s2.png"
exec /home/raspberry/opendeck/opendeck
EOF
chmod +x ~/opendeck/pulltest.sh
sudo sed -i 's|^OPENDECK_BIN=.*|OPENDECK_BIN=/home/raspberry/opendeck/pulltest.sh|' /etc/default/opendeck
sudo systemctl restart opendeck
sleep 6.6; echo 0 | sudo tee $U      # ~0.5 s after the LOAD at 6 s: mid-read
sleep 12; systemctl show opendeck -p NRestarts
echo 1 | sudo tee $U
# put the real binary back
sudo sed -i 's|^OPENDECK_BIN=.*|OPENDECK_BIN=/home/raspberry/opendeck/opendeck|' /etc/default/opendeck
sudo systemctl restart opendeck
```

Change the folder and track names to ones on your stick.  A whole track
reads in about 1.6 s on a Pi 5, so the pull has to land within that to
test the mid-read case; pull later to test "loaded and playing".  Script
times count from the app's start, about 0.3 s after `systemctl restart`.

What to read afterwards:

```bash
journalctl --since "-1min" -o short-precise | grep -E \
  'opendeck_app(::browser)?\] (script|load|media|loading|decoding)|opendeck-usb\[|stopped early|COULD|frames:'
```

The `frames:` lines carry the playhead: it has to keep moving after the
pull.  `/tmp/s1.png`, `/tmp/s2.png` are the screen (fetch with scp).  To
see the live screen instead: `XDG_RUNTIME_DIR=/run/user/$(id -u)
WAYLAND_DISPLAY=wayland-0 grim -c shot.png`.

Without a script: BROWSE into the stick and pull it (must land on
SOURCE); load a track and pull at once (plays to where it got); load,
wait, pull (plays on).

## By hand, once per hardware change

The software pull does not cover a stick yanked while the port is
drawing current, or a stick with a bad contact (in and out several times
in a second).  Before a release on new hardware: yank a stick mid-load,
mid-play, and while browsing it; wiggle one in and out; check the log as
above.  A stick pulled during a supply dip (the Pi 5 on less than its 5 A
supply) can take the USB sound card with it; the deck then runs silent
until that comes back (see `deploy/pi/autostart/README.md`).

## Results

**2026-11-18, Pi 5, Raspberry Pi OS trixie, appliance (cage), stick CDJ2
(vfat, rekordbox export), software pulls:**

- Browsing the stick, pulled: unmounted in 40 ms, `media removed` within a
  second, deck switched to SOURCE without the USB row; deck up,
  0 restarts.  Back in: remounted, `USB: CDJ2` back in about 1 s.
- Mid-read: LOAD at 22:46:53.868, playable in 54 ms, play; stick pulled
  0.4 s later; `decoding stopped early, track ends at 17.2s: decode: codec
  error: Input/output error (os error 5)`; playhead 4.1 s → 9.0 s → 13.9 s
  over the next 10 s at 60 fps; 0 restarts; replug picked up.
- Automated: the `stick_pull_tests` above, all passing.

Not yet tested by hand on the real port (see above).

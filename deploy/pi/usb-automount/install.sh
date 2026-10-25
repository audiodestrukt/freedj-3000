#!/bin/sh
# Install (or --remove) OpenDeck's appliance USB automount on a Raspberry Pi
# (or any systemd + udev Linux).  Needs root; re-execs under sudo.
#
#   sudo deploy/pi/usb-automount/install.sh            # install + mount what is plugged in now
#   sudo deploy/pi/usb-automount/install.sh --remove   # take it out again
#
# For headless / appliance images (Raspberry Pi OS Lite).  On a desktop image
# the file manager already automounts under /media/<user>; installing this too
# mounts every stick twice (two USB rows in OpenDeck) — pick one.
set -eu
[ "$(id -u)" = 0 ] || exec sudo "$0" "$@"
here=$(dirname "$(readlink -f "$0")")

if [ "${1-}" = "--remove" ]; then
    rm -f /etc/udev/rules.d/99-opendeck-usb.rules /etc/systemd/system/opendeck-usb-mount@.service /usr/local/bin/opendeck-usb-mount
    systemctl daemon-reload; udevadm control --reload-rules
    echo "removed (kept /etc/default/opendeck-usb and /media/usb)"; exit 0
fi

install -m 755 "$here/opendeck-usb-mount"          /usr/local/bin/opendeck-usb-mount
install -m 644 "$here/opendeck-usb-mount@.service" /etc/systemd/system/opendeck-usb-mount@.service
install -m 644 "$here/99-opendeck-usb.rules"       /etc/udev/rules.d/99-opendeck-usb.rules
[ -e /etc/default/opendeck-usb ] || install -m 644 "$here/opendeck-usb.default" /etc/default/opendeck-usb
. /etc/default/opendeck-usb
mkdir -p "${OPENDECK_MEDIA_ROOT:-/media/usb}"

systemctl daemon-reload
udevadm control --reload-rules
# Mount anything already plugged in.
udevadm trigger --subsystem-match=block --action=add --settle 2>/dev/null || udevadm trigger --subsystem-match=block --action=add
echo "installed: USB sticks mount under ${OPENDECK_MEDIA_ROOT:-/media/usb}/<LABEL> (read-only unless OPENDECK_USB_RW=1 in /etc/default/opendeck-usb)"
echo "watch:     journalctl -t opendeck-usb -f      list: findmnt -R ${OPENDECK_MEDIA_ROOT:-/media/usb}"

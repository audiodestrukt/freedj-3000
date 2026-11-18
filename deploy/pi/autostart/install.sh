#!/bin/sh
# Boot a Raspberry Pi straight into OpenDeck: log the desktop in
# automatically and start the deck fullscreen in it (restarted if it
# crashes).  Run as the user who will play, NOT with sudo; it asks for sudo
# only for the system settings.
#
#   deploy/pi/autostart/install.sh [path/to/opendeck]   # install
#   deploy/pi/autostart/install.sh --remove              # take it out again
#
# Without a path it uses the opendeck next to this folder (the release
# tarball's layout), else target/release/opendeck in a checkout, else the
# one on PATH.  Needs Raspberry Pi OS with the desktop (Bookworm: labwc or
# wayfire; both run ~/.config/autostart at login).
set -eu
[ "$(id -u)" != 0 ] || { echo "run as your user, not root (it uses sudo where it must)"; exit 1; }
here=$(dirname "$(readlink -f "$0")")
cfg="${XDG_CONFIG_HOME:-$HOME/.config}"
bindir="$HOME/.local/bin"

if [ "${1-}" = "--remove" ]; then
    rm -f "$cfg/autostart/opendeck.desktop" "$bindir/opendeck-session"
    echo "removed the autostart (kept $cfg/opendeck/autostart.env)."
    echo "to boot to the desktop login again: sudo raspi-config nonint do_boot_behaviour B3"
    exit 0
fi

bin="${1-}"
if [ -z "$bin" ]; then
    for c in "$here/../../opendeck" "$here/../../../target/release/opendeck" "$(command -v opendeck || true)"; do
        [ -n "$c" ] && [ -x "$c" ] && { bin=$c; break; }
    done
fi
[ -n "$bin" ] && [ -x "$bin" ] || { echo "no opendeck binary found; pass its path: $0 /path/to/opendeck"; exit 1; }
bin=$(readlink -f "$bin")
"$bin" --version >/dev/null 2>&1 || { echo "$bin does not run here (missing libraries? sudo apt install libasound2 libsdl2-2.0-0 mesa-vulkan-drivers)"; exit 1; }

mkdir -p "$bindir" "$cfg/autostart" "$cfg/opendeck"
install -m 755 "$here/opendeck-session" "$bindir/opendeck-session"
if [ ! -e "$cfg/opendeck/autostart.env" ]; then
    cat >"$cfg/opendeck/autostart.env" <<EOF
# Read by opendeck-session at every start.  Any OPENDECK_* setting works here.
OPENDECK_BIN=$bin
OPENDECK_FULLSCREEN=1
EOF
else
    sed -i "s|^OPENDECK_BIN=.*|OPENDECK_BIN=$bin|" "$cfg/opendeck/autostart.env"
fi
cat >"$cfg/autostart/opendeck.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=OpenDeck
Comment=Start the deck at login
Exec=$bindir/opendeck-session
X-GNOME-Autostart-enabled=true
EOF

# Desktop autologin (B4) and no screen blanking: a deck must not go dark
# mid-set.  raspi-config is Raspberry Pi OS only; elsewhere say what to do.
if command -v raspi-config >/dev/null; then
    sudo raspi-config nonint do_boot_behaviour B4
    sudo raspi-config nonint do_blanking 1
    auto="desktop autologin on, screen blanking off"
else
    auto="NOT Raspberry Pi OS: turn on desktop autologin for $USER yourself"
fi
echo "installed: $bin starts fullscreen at login ($auto)."
echo "settings: $cfg/opendeck/autostart.env   log: ~/.local/state/opendeck/session.log"
echo "Q quits to the desktop; a crash restarts it.  Reboot to try: sudo reboot"

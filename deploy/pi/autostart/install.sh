#!/bin/sh
# Boot a Raspberry Pi straight into OpenDeck.  Run as the user who will play,
# NOT with sudo; it asks for sudo itself.
#
#   deploy/pi/autostart/install.sh [--audio CARD] [--screen CONNECTOR] [--no-splash] [path/to/opendeck]
#       The appliance (the default; Raspberry Pi OS Lite, no desktop): a
#       systemd service runs OpenDeck fullscreen in cage, a one-app kiosk
#       compositor, on the screen at boot, and restarts it if it stops.
#       Sound goes to CARD (an ALSA card name, e.g. CODEC for a Behringer
#       UCA202); without --audio, the first USB sound card plugged in.
#       A boot splash (splash.png) shows from power on until the deck draws.
#       --screen DSI-1 puts the deck on that display only (the official
#       7" touch panel), switching the others off at boot; without it cage
#       spreads the deck over every connected display.
#
#   deploy/pi/autostart/install.sh --desktop [path/to/opendeck]
#       Keep the desktop: log it in automatically and start OpenDeck
#       fullscreen in it (Q quits to the desktop).
#
#   deploy/pi/autostart/install.sh --remove      take either out again
#
# Without a path it uses the opendeck next to this folder (the release
# tarball's layout), else target/release/opendeck in a checkout, else the
# one on PATH.
set -eu
[ "$(id -u)" != 0 ] || { echo "run as your user, not root (it uses sudo where it must)"; exit 1; }
here=$(dirname "$(readlink -f "$0")")
cfg="${XDG_CONFIG_HOME:-$HOME/.config}"
bindir="$HOME/.local/bin"
user=$(id -un)

mode=kiosk; card=""; bin=""; splash=1; screen=""
while [ $# -gt 0 ]; do
    case "$1" in
        --desktop) mode=desktop ;;
        --remove)  mode=remove ;;
        --no-splash) splash=0 ;;
        --screen)  screen="${2:?--screen needs a connector (ls /sys/class/drm: DSI-1, HDMI-A-1, ...)}"; shift ;;
        --audio)   card="${2:?--audio needs a card name (see: aplay -l)}"; shift ;;
        -*)        echo "unknown option $1"; exit 1 ;;
        *)         bin=$1 ;;
    esac
    shift
done

if [ "$mode" = remove ]; then
    rm -f "$cfg/autostart/opendeck.desktop" "$bindir/opendeck-session"
    if [ -e /etc/systemd/system/opendeck.service ]; then
        sudo systemctl disable opendeck.service 2>/dev/null || true
        sudo rm -f /etc/systemd/system/opendeck.service /etc/pam.d/opendeck
        sudo systemctl daemon-reload
        for b in /boot/firmware /boot; do
            for f in cmdline.txt config.txt; do
                [ -e $b/$f.before-opendeck ] && sudo mv $b/$f.before-opendeck $b/$f
            done
        done
        [ -e /etc/asound.conf.before-opendeck ] && sudo mv /etc/asound.conf.before-opendeck /etc/asound.conf \
            || sudo rm -f /etc/asound.conf
        [ -e /etc/systemd/system/display-manager.service ] && sudo systemctl set-default graphical.target
    fi
    echo "removed (kept $cfg/opendeck/autostart.env and /etc/default/opendeck)."
    command -v raspi-config >/dev/null && echo "boot to the desktop login again: sudo raspi-config nonint do_boot_behaviour B3"
    exit 0
fi

if [ -z "$bin" ]; then
    for c in "$here/../../opendeck" "$here/../../../target/release/opendeck" "$(command -v opendeck || true)"; do
        [ -n "$c" ] && [ -x "$c" ] && { bin=$c; break; }
    done
fi
[ -n "$bin" ] && [ -x "$bin" ] || { echo "no opendeck binary found; pass its path: $0 /path/to/opendeck"; exit 1; }
bin=$(readlink -f "$bin")
"$bin" --version >/dev/null 2>&1 || { echo "$bin does not run here (missing libraries? sudo apt install libasound2 libsdl2-2.0-0 mesa-vulkan-drivers)"; exit 1; }

# ── The desktop: autologin + XDG autostart ──────────────────────────────────
if [ "$mode" = desktop ]; then
    mkdir -p "$bindir" "$cfg/autostart" "$cfg/opendeck"
    install -m 755 "$here/opendeck-session" "$bindir/opendeck-session"
    if [ ! -e "$cfg/opendeck/autostart.env" ]; then
        printf '# Read by opendeck-session at every start.  Any OPENDECK_* setting works here.\nOPENDECK_BIN=%s\nOPENDECK_FULLSCREEN=1\n' "$bin" >"$cfg/opendeck/autostart.env"
    else
        sed -i "s|^OPENDECK_BIN=.*|OPENDECK_BIN=$bin|" "$cfg/opendeck/autostart.env"
    fi
    printf '[Desktop Entry]\nType=Application\nName=OpenDeck\nComment=Start the deck at login\nExec=%s\nX-GNOME-Autostart-enabled=true\n' \
        "$bindir/opendeck-session" >"$cfg/autostart/opendeck.desktop"
    # Desktop autologin (B4) and no screen blanking: a deck must not go dark mid-set.
    if command -v raspi-config >/dev/null; then
        sudo raspi-config nonint do_boot_behaviour B4
        sudo raspi-config nonint do_blanking 1
        auto="desktop autologin on, screen blanking off"
    else
        auto="NOT Raspberry Pi OS: turn on desktop autologin for $user yourself"
    fi
    echo "installed: $bin starts fullscreen at login ($auto)."
    echo "settings: $cfg/opendeck/autostart.env   log: ~/.local/state/opendeck/session.log"
    echo "Q quits to the desktop; a crash restarts it.  Reboot to try: sudo reboot"
    exit 0
fi

# ── The appliance: cage on tty1 from systemd ────────────────────────────────
command -v cage >/dev/null || { echo "installing cage (the kiosk compositor)"; sudo apt-get install -y cage; }

# Sound: OpenDeck plays to ALSA's default device, so point the default at
# the interface.  A USB card's ALSA name is stable across reboots and ports
# (a UCA202 is CODEC); its number is not.
if [ -z "$card" ]; then
    card=$(awk '/USB-Audio/ { sub(/^ *[0-9]+ \[/, ""); sub(/ *\].*/, ""); print; exit }' /proc/asound/cards 2>/dev/null || true)
fi
if [ -n "$card" ]; then
    [ -e /etc/asound.conf ] && [ ! -e /etc/asound.conf.before-opendeck ] && sudo cp /etc/asound.conf /etc/asound.conf.before-opendeck
    # The default device itself, not defaults.pcm.card: alsa-lib on trixie
    # refuses a card NAME there ("card is not a string"), and a number moves.
    # plug converts the rate if a track's differs from what the card takes.
    printf '# Written by OpenDeck deploy/pi/autostart/install.sh: the deck plays to this card.\npcm.!default { type plug; slave.pcm "hw:%s" }\nctl.!default { type hw; card %s }\n' "$card" "$card" \
        | sudo tee /etc/asound.conf >/dev/null
    sound="sound to ALSA card $card"
else
    sound="NO USB sound card found: sound goes to the Pi's default (HDMI); plug the interface in and re-run, or pass --audio CARD"
fi

# Settings the service reads (systemd's KEY=value; any OPENDECK_* works).
if [ ! -e /etc/default/opendeck ]; then
    printf '# Read by opendeck.service at every start (KEY=value, no quotes needed).\nOPENDECK_BIN=%s\nOPENDECK_FULLSCREEN=1\nOPENDECK_HIDE_CURSOR=1\n' "$bin" \
        | sudo tee /etc/default/opendeck >/dev/null
else
    sudo sed -i "s|^OPENDECK_BIN=.*|OPENDECK_BIN=$bin|" /etc/default/opendeck
    grep -q '^OPENDECK_HIDE_CURSOR=' /etc/default/opendeck || echo OPENDECK_HIDE_CURSOR=1 | sudo tee -a /etc/default/opendeck >/dev/null
fi

# A logind session for the service (cage needs a seat: the screen, the
# input devices) without anyone logging in; pam_limits so the real-time
# audio limits (../rt-audio) apply as they would at a login.
printf 'auth    required pam_unix.so nullok\naccount required pam_unix.so\nsession required pam_unix.so\nsession required pam_limits.so\nsession required pam_systemd.so\n' \
    | sudo tee /etc/pam.d/opendeck >/dev/null
sed "s|@USER@|$user|" "$here/opendeck.service" | sudo tee /etc/systemd/system/opendeck.service >/dev/null

# One screen: cage has no option to pick an output, so switch the others off
# for the kernel (video=<connector>:d on the command line).
if [ -n "$screen" ]; then
    [ -e /sys/class/drm/card*-"$screen" ] 2>/dev/null || ls /sys/class/drm/card*-"$screen" >/dev/null 2>&1 \
        || { echo "no display connector $screen; there are: $(ls /sys/class/drm | sed -n 's/^card[0-9]*-//p' | grep -v Writeback | tr '\n' ' ')"; exit 1; }
    boot=/boot/firmware; [ -e $boot/cmdline.txt ] || boot=/boot
    [ -e $boot/cmdline.txt.before-opendeck ] || sudo cp $boot/cmdline.txt $boot/cmdline.txt.before-opendeck
    line=$(sed 's/ video=[^ ]*:d//g' $boot/cmdline.txt)
    for c in $(ls /sys/class/drm | sed -n 's/^card[0-9]*-//p' | grep -v -e Writeback -e "^$screen\$"); do
        line="$line video=$c:d"
    done
    echo "$line" | sudo tee $boot/cmdline.txt >/dev/null
fi

# Boot splash: splash.png from power on until the deck's first frame, in
# place of the rainbow square and the boot text (Plymouth; --no-splash skips).
if [ "$splash" = 1 ]; then
    command -v plymouth >/dev/null || sudo apt-get install -y plymouth
    sudo install -d /usr/share/plymouth/themes/opendeck
    sudo install -m 644 "$here/plymouth/opendeck.plymouth" "$here/plymouth/opendeck.script" "$here/splash.png" \
        /usr/share/plymouth/themes/opendeck/
    boot=/boot/firmware; [ -e $boot/cmdline.txt ] || boot=/boot
    if [ -e $boot/cmdline.txt ]; then
        [ -e $boot/cmdline.txt.before-opendeck ] || sudo cp $boot/cmdline.txt $boot/cmdline.txt.before-opendeck
        line=$(cat $boot/cmdline.txt)
        for w in quiet splash plymouth.ignore-serial-consoles logo.nologo vt.global_cursor_default=0; do
            case " $line " in *" $w "*) ;; *) line="$line $w" ;; esac
        done
        echo "$line" | sudo tee $boot/cmdline.txt >/dev/null
        [ -e $boot/config.txt.before-opendeck ] || sudo cp $boot/config.txt $boot/config.txt.before-opendeck
        grep -q '^disable_splash=1' $boot/config.txt || printf '\n[all]\ndisable_splash=1\n' | sudo tee -a $boot/config.txt >/dev/null
    fi
    sudo plymouth-set-default-theme -R opendeck   # -R rebuilds the initramfs (a minute)
fi

sudo systemctl daemon-reload
sudo systemctl enable opendeck.service
# No desktop at boot: it would fight the deck for the screen.
sudo systemctl set-default multi-user.target
echo "installed: opendeck.service runs $bin fullscreen at boot ($sound${screen:+, on $screen only})."
echo "settings: /etc/default/opendeck   log: journalctl -u opendeck -f   stop for maintenance: sudo systemctl stop opendeck"
echo "Q or a crash restarts the deck.  Reboot to try: sudo reboot"

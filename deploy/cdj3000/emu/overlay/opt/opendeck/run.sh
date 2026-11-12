#!/bin/sh
# OpenDeck in cdj3k-emu: the CDJ-3000 screen on the virtio-gpu, the panel
# from the sub-CPU device, Pro DJ Link on the virtio NIC.
export HOME=/root
mkdir -p /root
# Display: SDL's software renderer off-screen (no X, no GL; the guest has
# virtio-gpu without virgl and runs under TCG), each frame copied to the
# framebuffer. SDL's KMSDRM page flips do not reach cdj3k-emu's scanout;
# the kernel's fbdev emulation does.
export OPENDECK_RENDERER=sdl
export SDL_VIDEODRIVER=dummy
export SDL_RENDER_DRIVER=software
export OPENDECK_FBDEV=/dev/fb0
export OPENDECK_PRESENT=immediate
export OPENDECK_MAX_FPS=${OPENDECK_MAX_FPS:-30}
# The console cursor would blink over the deck.
echo 0 > /sys/class/graphics/fbcon/cursor_blink 2>/dev/null
printf '\033[?25l' > /dev/tty1 2>/dev/null
# The CDJ-3000 UI, bare 1280x720 screen (OPENDECK_SUBUCOM = the real panel).
export OPENDECK_SKIN=cdj3000
export OPENDECK_SUBUCOM=/dev/subucom_spi1.0
export OPENDECK_STRETCH=wsola
export OPENDECK_LINK_IFACE=${OPENDECK_LINK_IFACE:-eth0}
export RUST_LOG=${RUST_LOG:-info,wgpu=warn,naga=warn}
# Tracks come from the emulator's USB stick, which mounts under
# /media/usb (scanned by media.rs). The deck boots empty, as a CDJ does
# (decoding and analysing a track before the first frame took ~25 s under
# TCG); OPENDECK_TRACK=<file> preloads one anyway.
cd /opt/opendeck
exec ./opendeck ${OPENDECK_TRACK:+"$OPENDECK_TRACK"}

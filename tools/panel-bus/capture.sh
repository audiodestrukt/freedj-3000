#!/usr/bin/env bash
# Capture the XDJ-1000MK2 panel link (PNLB CN8002) with a Saleae-Logic-class
# fx2lafw analyser through sigrok-cli.  Our own hardware, bus observation only.
#
#   tools/panel-bus/capture.sh <name> [seconds] [samplerate]
#
# Wiring (CN8002 pin -> analyser channel), see docs/reference/xdj-1000mk2-panel-bus.md:
#   19 PNL_SCK   -> D0      17 PNL_RXD (main->panel, MOSI) -> D1
#   15 PNL_TXD (panel->main, MISO) -> D2      18 PNL_BUSY -> D3
#   13 PNL_RST   -> D4      14 PNL_CNVSS -> D5      20 or 11 GND -> GND
#
# Output: captures/panel-bus/<name>.sr (sigrok session, open in PulseView or
# feed to tools/panel-bus/decode.py).  Defaults: 5 s at 16 MHz.  The 8-channel
# fx2 clones drop samples at 24 MHz over a busy USB bus; 16 MHz is reliable
# and still gives >=4 samples per bit for a clock up to 4 MHz.
set -euo pipefail
name=${1:?usage: capture.sh <name> [seconds] [samplerate]}
secs=${2:-5}
rate=${3:-16M}
out=captures/panel-bus
mkdir -p "$out"
exec sigrok-cli -d fx2lafw \
  --config "samplerate=$rate" \
  --channels D0=SCK,D1=RXD,D2=TXD,D3=BUSY,D4=RST,D5=CNVSS \
  --time "$((secs * 1000))" \
  -o "$out/$name.sr"

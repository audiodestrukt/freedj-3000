# Panel-bus kit (XDJ-1000MK2, PNLB CN8002)

Tools for the XDJ-1000MK2's main board ↔ panel CPU link, used with an fx2
logic analyser. **The full bench setup (hardware, wiring, software, the
listen and drive procedures, troubleshooting) is in
[docs/hardware/xdj-1000mk2-analyser.md](../../docs/hardware/xdj-1000mk2-analyser.md)**;
the protocol is in
[docs/reference/xdj-1000mk2-panel-bus.md](../../docs/reference/xdj-1000mk2-panel-bus.md).

| Tool | Use |
|---|---|
| `capture.sh <name> [s] [rate]` | record the link to `captures/panel-bus/<name>.sr` (sigrok, D0=SCK D1=RXD D2=TXD D3=BUSY D4=RST D5=CNVSS); keep runs ≤ 30 s at 4 MHz |
| `decode.py <capture.sr>` | survey (clock, bursts, BUSY) and frames; `--lsb` with the rising edge is the link's format |
| `bits.py <capture.sr>` | every bit that changed, as intervals in time order, checksums verified |
| `fx2/` | firmware for the analyser as the panel's master, the map page (`bridge.py`), loader and RAM-log reader; see `fx2/README.md` |

Captures stay out of git (`captures/` is ignored); the power-on capture is
kept in `reference/panel-bus/`.

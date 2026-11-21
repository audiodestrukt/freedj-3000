#!/usr/bin/env python3
"""XDJ-1000MK2 panel map: drive the deck's lamps and read its controls from a browser.

    python3 bridge.py            # deck off: loads bridge.ihx, then switch the deck on;
                                 # serves http://127.0.0.1:8765
    python3 bridge.py --no-load  # the bridge firmware is already running

Needs pyusb and Homebrew's libusb, and the analyser freshly plugged in (the
FX2's default descriptors provide the EP1 bulk pair bridge.c uses).  The deck
must be set up for master mode: the main board kept off SCK, RXD and BUSY
(docs/reference/xdj-1000mk2-panel-bus.md, "Driving the panel").

Outputs: every bit of the main-board -> panel frame (bytes 0-29) as a toggle,
the jog ring position (byte 9), presets.  Inputs: the panel's reply decoded
with the map in the reference doc (keys, jog, fader, rotary, touch), raw bytes.
"""
import argparse, json, os, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import usb.core, usb.util, usb.backend.libusb1

HERE = os.path.dirname(os.path.abspath(__file__))
VID, PID = 0x0925, 0x3881
_BREW_LIBUSB = "/opt/homebrew/lib/libusb-1.0.dylib"   # macOS (Homebrew); Linux finds the system libusb
BACKEND = usb.backend.libusb1.get_backend(find_library=lambda _: _BREW_LIBUSB) if os.path.exists(_BREW_LIBUSB) else usb.backend.libusb1.get_backend()

# Panel -> main keys, (byte, mask): name.  From the captures (reference doc, "Buttons").
KEYS = {
    (16, 0x01): "PLAY", (16, 0x02): "CUE", (16, 0x04): "RELOOP", (16, 0x08): "LOOP OUT",
    (16, 0x10): "LOOP IN", (16, 0x80): "REV", (17, 0x01): "ROTARY PUSH",
    (18, 0x01): "TRACK ◀", (18, 0x02): "TRACK ▶", (18, 0x04): "SEARCH ◀◀", (18, 0x08): "SEARCH ▶▶",
    (19, 0x01): "QUANTIZE", (19, 0x02): "TIME MODE", (19, 0x04): "BACK", (19, 0x08): "TAG TRACK",
    (19, 0x10): "SHORTCUT", (19, 0x20): "FILTER", (20, 0x01): "VINYL", (20, 0x02): "TEMPO RANGE",
    (20, 0x04): "MASTER TEMPO",
}
STOCK_IDLE = [0x30, 0xdf, 0, 0, 0, 0, 0, 0, 0x58, 0x01, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x02, 0x02, 0x02, 0, 0, 0, 0, 0]
LAMP_TEST = [0x73, 0x18, 0, 0, 0, 0, 0, 0x33, 0x40, 0x3f, 0x28, 0x10, 0x01, 0, 0, 0, 0, 0, 0x20, 0, 0, 0, 0x02, 0x02, 0x02, 0, 0, 0, 0, 0]


def load_firmware(path):
    dev = usb.core.find(idVendor=VID, idProduct=PID, backend=BACKEND)
    if dev is None:
        sys.exit("no analyser (0925:3881) on USB")
    poke = lambda a, d: dev.ctrl_transfer(0x40, 0xA0, a, 0, bytes(d), timeout=1000)
    recs = []
    for line in open(path):
        b = bytes.fromhex(line.strip()[1:]) if line.startswith(":") else b""
        if b and b[3] == 0:
            recs.append(((b[1] << 8) | b[2], b[4:4 + b[0]]))
    poke(0xE600, [1])
    for a, d in recs:
        poke(a, d)
    poke(0xE600, [0])
    usb.util.dispose_resources(dev)
    time.sleep(0.5)


class Link:
    """The latest frame to send and the latest reply, shuttled over EP1."""

    def __init__(self):
        self.dev = usb.core.find(idVendor=VID, idProduct=PID, backend=BACKEND)
        if self.dev is None:
            sys.exit("no analyser (0925:3881) on USB")
        try:
            self.dev.set_configuration()
        except usb.core.USBError:
            pass
        usb.util.claim_interface(self.dev, 0)
        self.dev.set_interface_altsetting(interface=0, alternate_setting=1)
        self.tx = list(STOCK_IDLE)
        self.dirty = True
        self.rx = None
        self.frames = self.bad = 0
        self.err = ""
        self.lock = threading.Lock()
        threading.Thread(target=self.run, daemon=True).start()

    def set_tx(self, frame):
        with self.lock:
            self.tx = [int(x) & 0xff for x in frame[:30]] + [0] * (30 - len(frame[:30]))
            self.dirty = True

    def run(self):
        while True:
            try:
                with self.lock:
                    send, self.dirty = (bytes(self.tx) if self.dirty else None), False
                if send is not None:
                    self.dev.write(0x01, send, timeout=200)
                r = bytes(self.dev.read(0x81, 64, timeout=200))
                if len(r) >= 36:
                    with self.lock:
                        self.rx = r[:32]
                        self.frames = r[32] | r[33] << 8
                        self.bad = r[34] | r[35] << 8
                        self.err = ""
            except usb.core.USBError as e:
                with self.lock:
                    self.err = str(e)
                time.sleep(0.2)
            time.sleep(0.02)

    def state(self):
        with self.lock:
            rx, tx = self.rx, list(self.tx)
            frames, bad, err = self.frames, self.bad, self.err
        out = {"tx": tx, "frames": frames, "bad": bad, "err": err, "rx": None}
        if rx:
            be = lambda o: rx[o] << 8 | rx[o + 1]
            jog = be(8)
            out["rx"] = list(rx)
            out["valid"] = rx[0] == 0x81 and rx[31] == 0x8F
            out["keys"] = [{"name": n, "down": bool(rx[b] & m)} for (b, m), n in KEYS.items()]
            out["values"] = {
                "fader (4-5)": f"0x{be(4):04x}",
                "centre detent (6-7)": f"0x{be(6):04x}",
                "jog position (8-9)": jog - 0x10000 if jog & 0x8000 else jog,
                "jog speed (10-11)": "stopped" if be(10) == 0xFFFF else be(10),
                "jog moving": bool(rx[15] & 0x80),
                "jog direction": "up" if rx[15] & 0x40 else "down",
                "jog touch bits (15.4-5)": f"{(rx[15] >> 4) & 3:02b}",
                "rotary (14)": rx[14],
                "touch x (22-23)": be(22), "touch y (24-25)": be(24),
                "vinyl speed adjust (2)": rx[2],
            }
        return out


PAGE = r"""<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>XDJ panel map</title>
<style>
:root { --bg:#101215; --fg:#e6e6e6; --dim:#8a8f98; --card:#191c21; --line:#2a2f37; --on:#ff8a3d; --key:#3ddc84; }
* { box-sizing:border-box; } body { margin:0; background:var(--bg); color:var(--fg); font:14px/1.4 -apple-system, system-ui, sans-serif; }
header { padding:12px 16px; border-bottom:1px solid var(--line); display:flex; gap:16px; align-items:baseline; flex-wrap:wrap; }
h1 { font-size:16px; margin:0; } .stat { color:var(--dim); font-variant-numeric:tabular-nums; }
main { display:grid; grid-template-columns:minmax(0,1.2fr) minmax(0,1fr); gap:16px; padding:16px; }
@media (max-width:900px) { main { grid-template-columns:1fr; } }
section { background:var(--card); border:1px solid var(--line); border-radius:8px; padding:12px; }
h2 { font-size:13px; margin:0 0 8px; color:var(--dim); text-transform:uppercase; letter-spacing:.05em; }
.row { display:grid; grid-template-columns:44px repeat(8, 1fr) 52px; gap:4px; align-items:center; margin:2px 0; }
.row .lbl, .row .hex { color:var(--dim); font:12px ui-monospace, monospace; } .row .hex { text-align:right; }
.bit { height:22px; border:1px solid var(--line); border-radius:4px; background:transparent; color:var(--dim); font:10px ui-monospace, monospace; cursor:pointer; }
.bit.on { background:var(--on); border-color:var(--on); color:#111; }
.hdr .bit { cursor:default; border:none; } .tools { display:flex; flex-wrap:wrap; gap:6px; margin-bottom:10px; align-items:center; }
button.p { background:#232831; color:var(--fg); border:1px solid var(--line); border-radius:6px; padding:5px 10px; cursor:pointer; }
button.p:hover { border-color:var(--dim); } input[type=range] { flex:1; min-width:160px; }
.keys { display:grid; grid-template-columns:repeat(auto-fill, minmax(110px,1fr)); gap:6px; }
.k { padding:6px 8px; border:1px solid var(--line); border-radius:6px; color:var(--dim); font-size:12px; }
.k.down { background:var(--key); border-color:var(--key); color:#08130c; font-weight:600; }
table { width:100%; border-collapse:collapse; font-variant-numeric:tabular-nums; } td { padding:3px 0; border-bottom:1px solid var(--line); }
td:last-child { text-align:right; font-family:ui-monospace, monospace; }
.raw { font:12px ui-monospace, monospace; color:var(--dim); word-break:break-all; margin-top:8px; }
.bad { color:#ff5d5d; }
</style></head><body>
<header><h1>XDJ-1000MK2 panel map</h1><span class="stat" id="stat">connecting…</span></header>
<main>
<section><h2>Main board → panel (what we send)</h2>
<div class="tools">
  <button class="p" onclick="preset('off')">All off</button>
  <button class="p" onclick="preset('idle')">Stock idle</button>
  <button class="p" onclick="preset('test')">Lamp test</button>
  <button class="p" onclick="preset('on')" title="one of these bits reboots the deck">All on (2–29) ⚠ reboots</button>
  <span class="stat">click a byte number to set / clear all 8 bits</span>
  <label class="stat">ring <span id="ringv"></span></label>
  <input type="range" id="ring" min="0" max="138" oninput="setRing(this.value)">
</div>
<div class="row hdr"></div>
<div id="grid"></div>
</section>
<section><h2>Panel → main board (what it reports)</h2>
<div class="keys" id="keys"></div>
<h2 style="margin-top:14px">Values</h2><table id="vals"></table>
<div class="raw" id="raw"></div>
</section>
</main>
<script>
const IDLE = __IDLE__, TEST = __TEST__;
let tx = IDLE.slice(), busy = false;
document.querySelector('.hdr').innerHTML = '<span></span>' + [7,6,5,4,3,2,1,0].map(b => `<span class="bit">${b}</span>`).join('') + '<span></span>';
function hex(v) { return v.toString(16).padStart(2, '0'); }
function draw() {
  const g = document.getElementById('grid'); let h = '';
  for (let i = 0; i < 30; i++) {
    h += `<div class="row"><span class="lbl" style="cursor:pointer" onclick="wholeByte(${i})">${i}</span>`;
    for (let b = 7; b >= 0; b--) h += `<button class="bit ${tx[i] >> b & 1 ? 'on' : ''}" onclick="flip(${i},${b})"></button>`;
    h += `<span class="hex">0x${hex(tx[i])}</span></div>`;
  }
  g.innerHTML = h;
  document.getElementById('ring').value = tx[9]; document.getElementById('ringv').textContent = tx[9];
}
function send() { fetch('/tx', {method:'POST', body:JSON.stringify(tx)}); draw(); }
function flip(i, b) { tx[i] ^= 1 << b; send(); }
function setRing(v) { tx[9] = +v; send(); }
function wholeByte(i) { tx[i] = tx[i] === 255 ? 0 : 255; send(); }
function preset(p) {
  if (p === 'off') tx = Array(30).fill(0);
  if (p === 'idle') tx = IDLE.slice();
  if (p === 'test') tx = TEST.slice();
  if (p === 'on') { tx = IDLE.slice(0, 2).concat(Array(28).fill(255)); tx[9] = 136; }
  send();
}
async function poll() {
  try {
    const s = await (await fetch('/state')).json();
    const st = document.getElementById('stat');
    st.innerHTML = s.err ? `<span class="bad">${s.err}</span>` : `frames ${s.frames} · bad replies ${s.bad}` + (s.rx && !s.valid ? ' · <span class="bad">reply not valid</span>' : '');
    if (s.rx) {
      document.getElementById('keys').innerHTML = s.keys.map(k => `<div class="k ${k.down ? 'down' : ''}">${k.name}</div>`).join('');
      document.getElementById('vals').innerHTML = Object.entries(s.values).map(([k, v]) => `<tr><td>${k}</td><td>${v}</td></tr>`).join('');
      document.getElementById('raw').textContent = s.rx.map(hex).join(' ');
    }
  } catch (e) { document.getElementById('stat').textContent = 'bridge not answering'; }
  setTimeout(poll, 50);
}
draw(); poll();
</script></body></html>"""


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--no-load", action="store_true", help="the bridge firmware is already running")
    ap.add_argument("--port", type=int, default=8765)
    a = ap.parse_args()
    if not a.no_load:
        load_firmware(os.path.join(HERE, "bridge.ihx"))
    link = Link()
    page = PAGE.replace("__IDLE__", json.dumps(STOCK_IDLE)).replace("__TEST__", json.dumps(LAMP_TEST))

    class H(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def reply(self, code, body, ctype):
            data = body.encode() if isinstance(body, str) else body
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path == "/":
                self.reply(200, page, "text/html; charset=utf-8")
            elif self.path == "/state":
                self.reply(200, json.dumps(link.state()), "application/json")
            else:
                self.reply(404, "not found", "text/plain")

        def do_POST(self):
            if self.path == "/tx":
                n = int(self.headers.get("Content-Length", 0))
                link.set_tx(json.loads(self.rfile.read(n)))
                self.reply(204, b"", "text/plain")
            else:
                self.reply(404, "not found", "text/plain")

    print(f"panel map on http://127.0.0.1:{a.port}")
    ThreadingHTTPServer(("127.0.0.1", a.port), H).serve_forever()


if __name__ == "__main__":
    main()

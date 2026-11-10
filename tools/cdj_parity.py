#!/usr/bin/env python3
"""Hold freedj's CDJ-3000 skin against the emulated unit, feature by feature.

One scenario drives both: freedj through OPENDECK_SCRIPT (its `press=`,
`hold=`, `lamps=` and `shot=` actions use cdj3k-emu's button names) and a
running cdj3k-emu slot through its control API, on the same timeline. At
each `check` both screens are captured and both sets of lamps read; the
report puts the screens side by side and lists the lamps that differ.

    tools/cdj_parity.py --track "/path/AC Slater - Bass Face.mp3" \\
        --scenario tools/cdj_parity/features.txt --out /tmp/parity

Before a run:

- the emulator slot is up with Developer Mode on (`cdj3k-emu --dev`), and
  the same track is the one `--emu-load` loads (default: BROWSE, then the
  rotary's push on the highlighted row, then CUE to stop it);
- freedj is built (`cargo build --release -p opendeck-app`).

freedj runs headless (xvfb, X11), pinned to the loopback interface as
player 2, so nothing it does (MASTER included) reaches the real LAN.

Scenario lines (`#` starts a comment):

    press <BUTTON>[:<ms>]   press a button; 1000 ms or more is a long press
    hold <BUTTON>           hold until `release <BUTTON>`
    release <BUTTON>
    rotary <n>              turn the rotary n detents
    wait <seconds>
    check <name>            capture both screens and lamps

Nothing Pioneer-owned is written into the repo: the report and its images
go to --out, outside it.
"""
import argparse, base64, json, os, socket, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EMU = Path.home() / "sandbox/thirdparty/cdj3k-emu/dist/linux/opt/cdj3k-emu/bin/cdj3k-emu"
# The screen inside freedj's desktop window (cdjface::side_layout at 1620x720).
FD_WINDOW, FD_SCREEN = (1620, 720), (170, 0, 1450, 720)
PRESS_GAP, CHECK_SETTLE = 1.0, 1.4


class Emu:
    """One control-API connection to a cdj3k-emu slot (holds last as long as it)."""
    def __init__(self, slot):
        self.s = socket.create_connection(("127.0.0.1", 4400 + slot), timeout=30)
        self.f = self.s.makefile("rw")

    def req(self, line):
        self.f.write(line + "\n"); self.f.flush()
        while True:
            r = json.loads(self.f.readline())
            if "ok" in r:
                if not r["ok"]: print(f"  emu: {line}: {r.get('error')}", file=sys.stderr)
                return r

    def lamps(self):
        return self.req("lamps")["lamps"]

    def screen(self):
        from PIL import Image
        import io
        return Image.open(io.BytesIO(base64.b64decode(self.req("lcd main")["png"]))).convert("RGB")

    def normalise(self):
        """Put the unit in freedj's fresh state: no hot cues, QUANTIZE and
        MASTER TEMPO on, SLIP and SYNC off, JOG MODE CDJ, TEMPO ±16, time
        elapsed, AUTO CUE on (a stored loop or the unit's settings are left)."""
        lit = lambda v: v not in (False, "off", 0, [6, 12, 12], [0, 0, 0])
        for _ in range(3):          # the unit's own hot cues can arrive after the load
            time.sleep(1.0)
            l = self.lamps()
            cues = [k for k in l if k.startswith("HOT_") and lit(l[k])]
            if not cues: break
            self.req("hold CALL_DELETE")
            for k in cues: self.req(f"press {k} 150"); time.sleep(0.4)
            self.req("release CALL_DELETE")
        for lamp, want, button in [("QUANTIZE", "full", "QUANTIZE"), ("SLIP", "medium", "SLIP"),
                                   ("MASTER_TEMPO", True, "MASTER_TEMPO"), ("JOG_MODE_CDJ", True, "JOG_MODE"),
                                   ("BEAT_SYNC", False, "BEAT_SYNC")]:
            if self.lamps().get(lamp) != want: self.req(f"press {button}"); time.sleep(0.6)
        white = lambda p: min(p) > 150
        for _ in range(4):          # TEMPO range badge: white = ±16
            if self.screen().getpixel((952, 492)) == (255, 255, 255): break
            self.req("press TEMPO_RANGE"); time.sleep(0.8)
        im = self.screen()          # REMAIN caption present = remain mode
        if sum(white(im.getpixel((x, y))) for x in range(440, 500) for y in range(487, 499)) > 20:
            self.req("press TIME_MODE"); time.sleep(0.8)
        im = self.screen()          # the AUTO CUE box's top edge
        if not any(white(im.getpixel((x, 537))) for x in range(290, 380)):
            self.req("press TIME_MODE 1500"); time.sleep(0.8)

    def run(self, steps):
        for st in steps.split(";"):
            st = st.strip()
            if st.startswith("sleep "): time.sleep(float(st[6:]))
            elif st: self.req(st)


def parse(path):
    """Scenario → [(t, kind, arg)] with t in seconds from the start."""
    t, out = 3.0, []          # both decks get a few seconds to settle first
    for raw in Path(path).read_text().splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line: continue
        kind, _, arg = line.partition(" ")
        arg = arg.strip()
        if kind == "press":
            ms = int(arg.split(":")[1]) if ":" in arg else 200
            out.append((t, "press", arg)); t += PRESS_GAP + ms / 1000
        elif kind in ("hold", "release", "rotary"):
            out.append((t, kind, arg)); t += 0.5
        elif kind == "wait":
            t += float(arg)
        elif kind == "check":
            t += CHECK_SETTLE; out.append((t, "check", arg)); t += 0.6
        else:
            sys.exit(f"{path}: unknown step {line!r}")
    return out, t + 1.0


def freedj_script(steps, end, out):
    acts = ["0.50:jogmode=cdj"]       # the unit's default; freedj's is VINYL
    for t, kind, arg in steps:
        if kind == "check":
            acts += [f"{t:.2f}:lamps={out}/{arg}.fd.json", f"{t:.2f}:shot={out}/{arg}.fd.png"]
        else:
            acts.append(f"{t:.2f}:{kind}={arg}")
    acts.append(f"{end:.2f}:quit")
    return "; ".join(acts)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--track", required=True)
    ap.add_argument("--scenario", default=str(ROOT / "tools/cdj_parity/features.txt"))
    ap.add_argument("--out", required=True)
    ap.add_argument("--slot", type=int, default=1)
    # The unit ignores a LOAD of the track it already has, so load the next
    # row first, then the track (the highlighted row in BROWSE).
    ap.add_argument("--emu-load", default="press BROWSE; sleep 1.5; rotary 1; sleep 0.5; press ROTARY_PRESS; sleep 4; "
                                          "press BROWSE; sleep 1.5; rotary -1; sleep 0.5; press ROTARY_PRESS; sleep 5; "
                                          "press CUE; sleep 1")
    ap.add_argument("--freedj", default=str(ROOT / "target/release/opendeck"))
    a = ap.parse_args()
    out = Path(a.out).resolve(); out.mkdir(parents=True, exist_ok=True)
    steps, end = parse(a.scenario)

    emu = Emu(a.slot)
    # Back to the playback screen: a lit screen key closes its own screen.
    for lamp, button in [("BROWSE", "BROWSE"), ("TAG_LIST", "TAG_LIST"), ("SEARCH", "SEARCH_MENU"),
                         ("SOURCE", "SOURCE"), ("MENU", "MENU")]:
        if emu.lamps().get(lamp) == "full": emu.req(f"press {button}"); time.sleep(1.2)
    print("emulator: loading the track"); emu.run(a.emu_load)
    print("emulator: matching freedj's starting state"); emu.normalise()

    xdg = out / "xdg"                 # fresh settings: freedj's defaults, not yours
    xdg.mkdir(exist_ok=True)
    env = dict(os.environ, OPENDECK_SKIN="cdj3000", OPENDECK_AUTOPLAY="0", XDG_DATA_HOME=str(xdg), OPENDECK_WINDOW=f"{FD_WINDOW[0]}x{FD_WINDOW[1]}",
               OPENDECK_LINK_IFACE="lo", OPENDECK_PLAYER="2", OPENDECK_SCRIPT=freedj_script(steps, end, out),
               OPENDECK_MEDIA_DIRS=str(Path(a.track).parent), WINIT_UNIX_BACKEND="x11", WINIT_X11_SCALE_FACTOR="1")
    env.pop("WAYLAND_DISPLAY", None)
    log = open(out / "freedj.log", "w")
    fd = subprocess.Popen(["xvfb-run", "-a", "-s", "-screen 0 1700x800x24", a.freedj, a.track],
                          env=env, stdout=log, stderr=subprocess.STDOUT)
    # freedj's script clock starts when it logs "script: N actions".
    t_wait = time.time()
    while "script:" not in (out / "freedj.log").read_text():
        if fd.poll() is not None or time.time() - t_wait > 60: sys.exit("freedj did not start; see freedj.log")
        time.sleep(0.05)
    t0 = time.time()
    print(f"freedj: up; {len(steps)} steps over {end:.0f} s")

    held = set()
    for t, kind, arg in steps:
        time.sleep(max(0.0, t0 + t - time.time()))
        if kind == "press":
            b, _, ms = arg.partition(":")
            emu.req(f"press {b} {ms or 200}")
        elif kind == "hold": emu.req(f"hold {arg}"); held.add(arg)
        elif kind == "release": emu.req(f"release {arg}"); held.discard(arg)
        elif kind == "rotary": emu.req(f"rotary {arg}")
        elif kind == "check":
            r = emu.req("lcd main")
            (out / f"{arg}.emu.png").write_bytes(base64.b64decode(r["png"]))
            (out / f"{arg}.emu.json").write_text(json.dumps(emu.req("lamps")["lamps"]))
            print(f"  check {arg}")
    fd.wait(timeout=60)
    report(out, [arg for _, k, arg in steps if k == "check"])


def report(out, checks):
    from PIL import Image
    rows, bad = [], 0
    for name in checks:
        fdj, emj = out / f"{name}.fd.json", out / f"{name}.emu.json"
        if not fdj.exists():
            rows.append(f"## {name}\n\nfreedj wrote no lamps (did it crash? see freedj.log)\n"); bad += 1; continue
        f, e = json.loads(fdj.read_text()), json.loads(emj.read_text())
        diff = [f"| {k} | {e.get(k)} | {v} |" for k, v in f.items()
                if k in e and e[k] != v and not (v == "blink" and isinstance(e[k], bool))]
        bad += bool(diff)
        # Side by side: the unit left, freedj right, at the unit's 1280x720.
        img = out / f"{name}.png"
        try:
            em = Image.open(out / f"{name}.emu.png").convert("RGB")
            fd = Image.open(out / f"{name}.fd.png").convert("RGB").crop(FD_SCREEN).resize(em.size)
            both = Image.new("RGB", (em.width * 2 + 8, em.height), (60, 60, 60))
            both.paste(em, (0, 0)); both.paste(fd, (em.width + 8, 0)); both.save(img)
        except FileNotFoundError as err:
            img = None; print(f"  {name}: {err}", file=sys.stderr)
        rows.append(f"## {name}\n\n" + (f"![{name}]({img.name})\n\n" if img else "")
                    + ("| lamp | unit | freedj |\n|---|---|---|\n" + "\n".join(diff) + "\n" if diff else "Lamps match.\n"))
    (out / "report.md").write_text(f"# CDJ-3000 parity: freedj against cdj3k-emu\n\nUnit left, freedj right. "
                                   f"{len(checks) - bad} of {len(checks)} checks with matching lamps.\n\n" + "\n".join(rows))
    print(f"report: {out / 'report.md'} ({bad} checks with lamp differences)")


if __name__ == "__main__":
    main()

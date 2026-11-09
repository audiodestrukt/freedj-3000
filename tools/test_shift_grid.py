#!/usr/bin/env python3
"""End-to-end test: SYNC holds a jog nudge, and SHIFT GRID folds it into the grid.

freedj runs as player 2, synced to an emulated CDJ-3000 (cdj3k-emu) that is
master on a private Link bridge. freedj is driven by OPENDECK_SCRIPT and logs
its phase on every master beat; this checks the log:

  A. synced, before the nudge: on the beat (phase ~ 0)
  B. after a jog nudge: SYNC holds the offset the nudge left (not pulled to 0)
  C. after SHIFT GRID: the offset is 0 and the phase is ~ 0 straight away,
     i.e. the grid moved and what is heard did not

Needs the rig in docs/design/cdj-grid-adjust.md ("Reproducing"): the djlink0
bridge, the emulator on it (`setting network djlink0`) with a rekordbox stick,
and its control API on 127.0.0.1:4401. Without it the test is skipped (exit 77).

  tools/test_shift_grid.py [--track FILE] [--no-setup] [--keep-log]

--no-setup skips loading a track on the CDJ (one is already loaded); the
runner still makes sure it is playing and master.
"""
import argparse, base64, json, os, re, socket, subprocess, sys, time
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CTL = ("127.0.0.1", 4401)
SKIP = 77
# Phase tolerances, in beats. The Link estimate jitters by ~0.01-0.02.
ON_BEAT = 0.05
MIN_NUDGE = 0.05

# freedj's timeline (seconds after start).
SCRIPT = "1:play; 3:sync; 3:jogmode=cdj; 14:jog=3x20; 26:shiftsync; 36:quit"
NUDGE_AT, SHIFT_AT = 14.0, 26.0


def ctl(cmd, timeout=10):
    s = socket.create_connection(CTL, timeout=timeout)
    s.settimeout(timeout)
    s.sendall((cmd + "\n").encode())
    buf = b""
    while b"\n" not in buf:
        d = s.recv(1 << 20)
        if not d:
            break
        buf += d
    s.close()
    return json.loads(buf.split(b"\n")[0])


def skip(why):
    print(f"SKIP: {why}")
    sys.exit(SKIP)


def preflight():
    if not Path("/sys/class/net/djlink0").exists():
        skip("no djlink0 bridge (see docs/design/cdj-grid-adjust.md)")
    try:
        net = ctl("setting").get("network")
    except OSError as e:
        skip(f"emulator control API not reachable on {CTL}: {e}")
    if net != "djlink0":
        skip(f"emulator network is {net!r}, want djlink0 (`setting network djlink0`)")


def cdj_state():
    """(playing, master), read off the CDJ's screen: the playhead is white
    while playing (red when paused), and MASTER shows an orange tag in the
    BPM box."""
    import io
    from PIL import Image
    im = Image.open(io.BytesIO(base64.b64decode(ctl("lcd main")["png"]))).convert("RGB")
    playhead = im.getpixel((640, 300))
    tag = im.getpixel((1235, 497))
    return playhead[1] > 200, tag[0] > 200 and 60 < tag[1] < 160


def ensure_playing_master():
    for _ in range(4):
        playing, master = cdj_state()
        if playing and master:
            return
        if not playing:
            ctl("press PLAY")
            time.sleep(1.5)
        if not master:
            ctl("press MASTER")
            time.sleep(2.5)
    sys.exit(f"CDJ setup: could not get it playing and master (playing, master = {cdj_state()})")


def load_track_on_cdj():
    """Load the second TRACK row from the rekordbox stick."""
    steps = [
        "press SOURCE", "press ROTARY_PRESS",          # SOURCE → the USB row → BROWSE
        "touch 0.035 0.364", "touch 0.3 0.3125",       # TRACK category, second row
        "touch 0.92 0.31",                             # its LOAD key
    ]
    for s in steps:
        r = ctl(s)
        if not r.get("ok"):
            sys.exit(f"CDJ setup failed at {s!r}: {r}")
        time.sleep(2.5)
    time.sleep(3)


TS = re.compile(r"^\[(\S+)Z")
BEAT = re.compile(r"beat phase ([+-]\d\.\d+) held ([+-]\d\.\d+)( jog)?")
HOLD = re.compile(r"holding phase offset ([+-]\d\.\d+)")
SHIFT = re.compile(r"SHIFT GRID by ([+-]\d\.\d+)")


def wrap(x):
    return x - round(x)


def run_freedj(track, log_path):
    env = dict(os.environ,
               OPENDECK_LINK_IFACE="djlink0", OPENDECK_LINK_SEND="1", OPENDECK_PLAYER="2",
               OPENDECK_SCRIPT=SCRIPT, OPENDECK_SKIN="cdj3000",
               RUST_LOG="opendeck_app=info,opendeck_app::prodj=debug",
               WINIT_UNIX_BACKEND="x11", WINIT_X11_SCALE_FACTOR="1")
    env.pop("WAYLAND_DISPLAY", None)
    exe = ROOT / "target" / "debug" / "opendeck"
    if not exe.exists():
        sys.exit(f"build first: {exe} missing (cargo build -p opendeck-app)")
    with open(log_path, "w") as f:
        p = subprocess.run([str(exe), str(track)], env=env, stdout=f, stderr=subprocess.STDOUT, timeout=90)
    return p.returncode


def parse(log_path):
    t0, beats, holds, shifts = None, [], [], []
    for line in open(log_path, errors="replace"):
        m = TS.match(line)
        if not m:
            continue
        t = datetime.fromisoformat(m.group(1)).timestamp()
        if t0 is None and "script:" in line:
            t0 = t
        if t0 is None:
            continue
        rel = t - t0
        if (b := BEAT.search(line)):
            beats.append((rel, float(b.group(1)), float(b.group(2)), bool(b.group(3))))
        elif (h := HOLD.search(line)):
            holds.append((rel, float(h.group(1))))
        elif (s := SHIFT.search(line)):
            shifts.append((rel, float(s.group(1))))
    return beats, holds, shifts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--track", default=str(Path.home() / "Music/dj-library/Crossnaders - About It.mp3"))
    ap.add_argument("--no-setup", action="store_true")
    ap.add_argument("--keep-log", action="store_true")
    a = ap.parse_args()

    preflight()
    if not a.no_setup:
        load_track_on_cdj()
    ensure_playing_master()
    log_path = Path(os.environ.get("TMPDIR", "/tmp")) / "test_shift_grid.log"
    rc = run_freedj(a.track, log_path)
    beats, holds, shifts = parse(log_path)

    fails = []
    def check(ok, what):
        print(("  ok   " if ok else "  FAIL ") + what)
        if not ok:
            fails.append(what)

    print(f"freedj exit {rc}; {len(beats)} master beats, holds {holds}, shifts {shifts}")
    check(rc == 0, "freedj ran the script to quit")
    check(len(beats) > 40, "master beats arrived (CDJ master and playing, freedj synced)")

    # A: settled on the beat before the nudge.
    a_ph = [p for t, p, h, j in beats if NUDGE_AT - 5 < t < NUDGE_AT]
    check(len(a_ph) >= 3 and all(abs(p) < ON_BEAT for p in a_ph[-4:]),
          f"A: on the beat before the nudge {['%+.3f' % p for p in a_ph[-4:]]}")

    # B: a hold was taken after the nudge and SYNC keeps it.
    hold = next((h for t, h in holds if NUDGE_AT < t < SHIFT_AT), None)
    check(hold is not None and abs(hold) > MIN_NUDGE, f"B: nudge left a held offset ({hold})")
    if hold is not None:
        b_ph = [(p, h) for t, p, h, j in beats if SHIFT_AT - 6 < t < SHIFT_AT and not j]
        check(len(b_ph) >= 3 and all(abs(h - hold) < 1e-3 for p, h in b_ph),
              "B: the offset stays held until SHIFT GRID")
        check(all(abs(wrap(p - hold)) < ON_BEAT for p, h in b_ph[-4:]),
              f"B: phase sits on the held offset, not 0 {['%+.3f' % p for p, h in b_ph[-4:]]}")

    # C: SHIFT GRID moved the grid by the offset; on the beat at once.
    sh = next((s for t, s in shifts if t >= SHIFT_AT - 1), None)
    check(sh is not None and hold is not None and abs(sh - hold) < 1e-3, f"C: SHIFT GRID by the held offset ({sh})")
    c = [(p, h) for t, p, h, j in beats if t > SHIFT_AT + 0.2]
    check(len(c) >= 4 and all(h == 0.0 for p, h in c), "C: offset cleared")
    check(len(c) >= 4 and all(abs(p) < ON_BEAT for p, h in c[:3]),
          f"C: on the new grid's beat straight away (no jump) {['%+.3f' % p for p, h in c[:3]]}")

    if not a.keep_log and not fails:
        log_path.unlink(missing_ok=True)
    else:
        print(f"log: {log_path}")
    print("PASS" if not fails else f"FAIL ({len(fails)})")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()

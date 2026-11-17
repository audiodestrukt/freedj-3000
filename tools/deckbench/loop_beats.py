#!/usr/bin/env python3
"""Loop beats over Link, checked on Deckbench.

Runs Deckbench as an observer on the loopback, runs OpenDeck headless as
player 4 (full Link send, OPENDECK_LINK_IFACE=lo), plays, sets a one-beat
loop with LOOP IN / LOOP OUT, exits it, and checks the beat packets OpenDeck
sent while it looped: one each time the loop goes round, all on the same
beat of the bar, never two together.

    tools/deckbench/loop_beats.py [--opendeck target/release/opendeck]
                                  [--deckbench ~/sandbox/thirdparty/deckbench]
                                  [--track music.mp3] [--keep DIR]

Needs xvfb-run and Deckbench (github.com/commit-collective/deckbench).
Exit status 0 = pass.  Linux only (loopback broadcast).
"""
import argparse, json, os, subprocess, sys, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = "3:play; 8:press=LOOP_IN; 8.45:press=LOOP_OUT; 16:press=RELOOP; 20:quit"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--opendeck", default=str(ROOT / "target/release/opendeck"))
    ap.add_argument("--deckbench", default=os.path.expanduser("~/sandbox/thirdparty/deckbench"))
    ap.add_argument("--track", default=str(ROOT / "amplificate.mp3"))
    ap.add_argument("--keep", help="keep the capture and logs in this folder")
    a = ap.parse_args()

    work = Path(a.keep or tempfile.mkdtemp(prefix="deckbench-loop-")).resolve()   # deckbench runs from its own folder
    work.mkdir(parents=True, exist_ok=True)
    cap, ready = work / "capture.jsonl", work / "ready.json"
    for f in (cap, ready):
        f.unlink(missing_ok=True)

    bench = subprocess.Popen(
        [sys.executable, "-m", "deckbench", "--observe", "--capture", str(cap), "--ready-file", str(ready), "--http-port", "0"],
        cwd=a.deckbench, stdout=open(work / "bench.log", "w"), stderr=subprocess.STDOUT)
    try:
        for _ in range(100):
            if ready.exists():
                break
            time.sleep(0.1)
        else:
            sys.exit("deckbench did not come up (see bench.log)")
        env = {k: v for k, v in os.environ.items() if k != "WAYLAND_DISPLAY"}
        env.update(XDG_DATA_HOME=str(work / "xdg"), XDG_CONFIG_HOME=str(work / "xdg"), OPENDECK_LINK_IFACE="lo",
                   OPENDECK_STEMS="0", OPENDECK_SCRIPT=SCRIPT, RUST_LOG="info")
        with open(work / "opendeck.log", "w") as log:
            subprocess.run(["timeout", "60", "xvfb-run", "-a", "-s", "-screen 0 1700x1000x24", a.opendeck,
                            "--player", "4", "--link-send", a.track], env=env, stdout=log, stderr=subprocess.STDOUT)
    finally:
        bench.terminate()
        bench.wait(10)

    loop = [l for l in open(work / "opendeck.log") if "loop: " in l and "(in/out)" in l]
    if not loop:
        sys.exit(f"FAIL: OpenDeck set no loop (see {work}/opendeck.log)")
    beats = []
    for line in open(cap):
        j = json.loads(line)
        if j.get("kind") == "beat" and j.get("src") == "127.0.0.1":
            beats.append((j["t"], bytes.fromhex(j["hex"])[0x5C]))
    if len(beats) < 20:
        sys.exit(f"FAIL: only {len(beats)} beat packets reached Deckbench")
    # The loop runs ~5 s to ~13 s of the bench's clock; judge the middle.
    t0 = beats[0][0]
    mid = [b for b in beats if t0 + 6.5 < b[0] < t0 + 11.5]
    seq = "".join(str(b[1]) for b in mid)
    gaps = [round(y[0] - x[0], 3) for x, y in zip(mid, mid[1:])]
    problems = []
    if len(set(b[1] for b in mid)) != 1:
        problems.append(f"beats in the loop are not all one beat of the bar: {seq}")
    if gaps and min(gaps) < 0.3:
        problems.append(f"two beats together (gap {min(gaps)} s)")
    if not 8 <= len(mid) <= 12:
        problems.append(f"{len(mid)} beats in 5 s of a 0.47 s loop, want one each time round")
    print(loop[0].strip().split("] ")[-1])
    print(f"beats in the loop: {seq}  gaps {min(gaps) if gaps else '-'}..{max(gaps) if gaps else '-'} s")
    if problems:
        print("FAIL:", "; ".join(problems), f"(capture: {cap})")
        sys.exit(1)
    print("PASS")


if __name__ == "__main__":
    main()

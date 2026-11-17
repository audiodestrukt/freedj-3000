#!/usr/bin/env python3
"""The Deckbench suite: what players on the link see of OpenDeck.

    tools/deckbench/suite.py [--opendeck target/release/opendeck]
                             [--deckbench ~/sandbox/thirdparty/deckbench]
                             [--track amplificate.mp3] [--keep DIR] [--only NAME,...]
    make deckbench

Deckbench (github.com/commit-collective/deckbench) runs two simulated
CDJs on the loopback (players 2 and 3; 2 playing at 126 BPM and tempo
master). OpenDeck runs headless beside them as player 4 on `lo`, with a
timed OPENDECK_SCRIPT, and every check reads the bench's own view of
OpenDeck, as another player on the network would see it:

  sync       SYNC: OpenDeck reports the master's tempo, synced, and its
             beats land on the master's (phase within 0.05 beat).
  trackinfo  The bench reads OpenDeck's loaded track from its dbserver
             (title, tempo), as a CDJ does for its INFO screen.  Known to
             fail (XFAIL) until #62: Deckbench reads at most 12 arguments a
             message, and our rows carry a CDJ-3000's 16.
  waveform   The bench reads OpenDeck's waveform and beat grid from its
             dbserver: the overview, the detail, a grid at the track's
             tempo.
  master     MASTER: OpenDeck takes the tempo master role from player 2,
             and player 2 lets it go.
  loopbeats  A one-beat loop sends its beat once each time round
             (tools/deckbench/loop_beats.py, its own run).

Loopback only: OpenDeck takes MASTER here, which must never happen on a
network with real decks. Linux (loopback broadcast). Needs xvfb-run.
Exit status 0 = every check passed.
"""
import argparse, base64, json, os, subprocess, sys, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ME = "127.0.0.1"          # OpenDeck on lo
PLAYER = 4
MASTER_TEMPO = 126.0
# 3 s: play. 5 s: SYNC to player 2. 16 s: MASTER. 26 s: quit.
SCRIPT = "3:play; 5:sync; 16:press=MASTER; 26:quit"
SYNC_AT, MASTER_AT = 12.0, 22.0   # when the bench's view is read for each
# Known failures, by check: the reason and the issue.  The suite passes with
# them failing (XFAIL) and says so when one starts passing (XPASS: drop it).
KNOWN = {
    "trackinfo": "Deckbench reads 12 arguments at most; our rows have a CDJ-3000's 16 (#62)",
}


def beat_at(phase, t):
    """Beats into the track at time t, from a [time, beats, bpm] phase."""
    t0, beats, bpm = phase
    return beats + (t - t0) * bpm / 60.0


class Suite:
    def __init__(self, a):
        self.a, self.results = a, []

    def check(self, name, ok, detail):
        known = KNOWN.get(name)
        word = ("XPASS" if ok else "XFAIL") if known else ("PASS" if ok else "FAIL")
        self.results.append((name, bool(ok) or bool(known), detail))
        print(f"{word:5s} {name:10s} {detail}" + (f"\n      {'':10s} known: {known}" if known else ""), flush=True)

    def wanted(self, name):
        return not self.a.only or name in self.a.only

    def run_link(self, work):
        sys.path.insert(0, self.a.deckbench)
        from deckbench.testing import Bench
        # The real ports (shift 0): OpenDeck uses them. Each bench deck binds
        # its own address (127.0.0.2, .3), OpenDeck its own (127.0.0.1).
        with Bench(numbers=[2, 3], on=True, init={2: "play,master"}, shift=0) as bench:
            bench.post(f"api/deck/2/tempo/{MASTER_TEMPO:g}")
            env = {k: v for k, v in os.environ.items() if k != "WAYLAND_DISPLAY"}
            env.update(XDG_DATA_HOME=str(work / "xdg"), XDG_CONFIG_HOME=str(work / "xdg"), OPENDECK_LINK_IFACE="lo",
                       OPENDECK_STEMS="0", OPENDECK_SCRIPT=SCRIPT, RUST_LOG="info,wgpu=off,naga=off")
            log = open(work / "opendeck.log", "w")
            od = subprocess.Popen(["timeout", "60", "xvfb-run", "-a", "-s", "-screen 0 1700x1000x24", self.a.opendeck,
                                   "--player", str(PLAYER), "--link-send", self.a.track],
                                  env=env, stdout=log, stderr=subprocess.STDOUT)
            t0 = time.time()
            snaps = {}
            meta_asked = False
            try:
                while od.poll() is None:
                    el = time.time() - t0
                    s = bench.state()
                    me = self.device(s)
                    for mark in (SYNC_AT, MASTER_AT):
                        if el >= mark and mark not in snaps:
                            snaps[mark] = s
                    # Ask for the track's metadata once OpenDeck has a track loaded.
                    if me and (me.get("status") or {}).get("rbid") and not meta_asked:
                        meta_asked = True
                        try:
                            bench.post(f"api/player/{ME}/metadata/refresh?number={PLAYER}")
                        except Exception as e:                      # the bench says why
                            snaps["metadata_error"] = str(e).split(": ", 1)[-1]
                    if me:
                        snaps["last"] = s
                    if el > 24 and "waveform_data" not in snaps and (me or {}).get("waveform", {}).get("state") == "ready":
                        snaps["waveform_data"] = bench.get(f"api/player/{ME}/waveform?number={PLAYER}")
                    time.sleep(0.25)
            finally:
                if od.poll() is None:
                    od.kill()
                json.dump({str(k): v for k, v in snaps.items()}, open(work / "bench-states.json", "w"), indent=1, default=str)
        return snaps

    @staticmethod
    def device(state):
        for d in state.get("protocols", {}).get("prolink", {}).get("devices", []):
            if d.get("number") == PLAYER and d.get("ip") == ME:
                return d
        return None

    def check_sync(self, s):
        me = self.device(s) if s else None
        if not me:
            return self.check("sync", False, "the bench never saw OpenDeck")
        st, deck = me["status"], s["decks"]["2"]
        if not (st.get("sync") and abs(st.get("bpm", 0) - MASTER_TEMPO) < 0.05):
            return self.check("sync", False, f"synced={st.get('sync')} bpm={st.get('bpm')} (master {MASTER_TEMPO})")
        if not (me.get("phase") and deck.get("phase")):
            return self.check("sync", False, "no beat phase to compare")
        t = max(me["phase"][0], deck["phase"][0])
        d = (beat_at(me["phase"], t) - beat_at(deck["phase"], t)) % 1.0
        d = min(d, 1.0 - d)
        self.check("sync", d < 0.05, f"{st['bpm']:.2f} BPM, synced, beats {d:.3f} beat from the master's")

    def check_master(self, s):
        me = self.device(s) if s else None
        if not me:
            return self.check("master", False, "the bench never saw OpenDeck")
        mine, theirs = me["status"].get("master"), s["decks"]["2"].get("master")
        self.check("master", bool(mine) and not theirs, f"OpenDeck master={mine}, player 2 master={theirs}")

    def check_trackinfo(self, s, error=None):
        me = self.device(s) if s else None
        m = (me or {}).get("metadata")
        if not m:
            reason = error or (me or {}).get("metadata_reason") or "no metadata read"
            return self.check("trackinfo", False, reason)
        title = m.get("title") or ""
        bpm = m.get("bpm") or m.get("tempo")
        self.check("trackinfo", bool(title), f"title {title!r}, tempo {bpm}")

    def check_waveform(self, snaps):
        me = self.device(snaps.get("last") or {})
        w = (me or {}).get("waveform") or {}
        data = snaps.get("waveform_data")
        if w.get("state") != "ready" or not data:
            return self.check("waveform", False, f"state {w.get('state')} {w.get('reason', '')}".strip())
        ov = base64.b64decode(data.get("overview") or "")
        det = base64.b64decode(data.get("detail") or "")
        grid = data.get("grid") or []
        track_bpm = me["status"].get("track_bpm") or 0
        grid_bpm = grid[0][2] / 100.0 if grid else 0
        ok = len(ov) > 0 and len(det) > len(ov) and len(grid) > 8 and abs(grid_bpm - track_bpm) < 0.5 and max(ov) > 0
        self.check("waveform", ok, f"overview {len(ov)}, detail {len(det)} ({data.get('rate')}/s), "
                                   f"{len(grid)} beats at {grid_bpm:g} BPM (track {track_bpm:g})")

    def check_loopbeats(self, work):
        r = subprocess.run([sys.executable, str(ROOT / "tools/deckbench/loop_beats.py"), "--opendeck", self.a.opendeck,
                            "--deckbench", self.a.deckbench, "--track", self.a.track, "--keep", str(work / "loopbeats")],
                           capture_output=True, text=True)
        last = (r.stdout.strip().splitlines() or [r.stderr.strip()])[-1]
        self.check("loopbeats", r.returncode == 0, last)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--opendeck", default=str(ROOT / "target/release/opendeck"))
    ap.add_argument("--deckbench", default=os.path.expanduser("~/sandbox/thirdparty/deckbench"))
    ap.add_argument("--track", default=str(ROOT / "amplificate.mp3"))
    ap.add_argument("--keep", help="keep the logs and the bench's view in this folder")
    ap.add_argument("--only", type=lambda v: v.split(","), help="run only these checks (sync,trackinfo,waveform,master,loopbeats)")
    a = ap.parse_args()
    for p, what in ((a.opendeck, "OpenDeck (cargo build --release -p opendeck-app)"), (a.deckbench, "Deckbench"), (a.track, "the track")):
        if not os.path.exists(p):
            sys.exit(f"no {what} at {p}")
    work = Path(a.keep or tempfile.mkdtemp(prefix="deckbench-suite-")).resolve()   # deckbench runs from its own folder
    work.mkdir(parents=True, exist_ok=True)

    s = Suite(a)
    if any(s.wanted(n) for n in ("sync", "trackinfo", "waveform", "master")):
        snaps = s.run_link(work)
        if s.wanted("sync"):
            s.check_sync(snaps.get(SYNC_AT))
        if s.wanted("trackinfo"):
            s.check_trackinfo(snaps.get("last"), snaps.get("metadata_error"))
        if s.wanted("waveform"):
            s.check_waveform(snaps)
        if s.wanted("master"):
            s.check_master(snaps.get(MASTER_AT))
    if s.wanted("loopbeats"):
        s.check_loopbeats(work)

    failed = [n for n, ok, _ in s.results if not ok]
    known = [n for n, _, _ in s.results if n in KNOWN]
    passed = len(s.results) - len(failed) - len(known)
    print(f"\n{passed} passed" + (f", {len(known)} known failure ({', '.join(known)})" if known else "")
          + (f"; FAILED: {', '.join(failed)}" if failed else "")
          + f"\nlogs and the bench's view: {work}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

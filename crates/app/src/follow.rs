//! FOLLOW: a deck that mirrors another player on Link instead of playing.
//! It loads whatever that player loads (the track's analysis and audio come
//! from the player's database and its stick, as a LINK load does), never
//! plays it, and draws the playhead where that player's is.
//!
//! Where that is: a CDJ-3000 says it outright (an Absolute Position packet,
//! ~33 a second).  Older players (XDJ-1000, CDJ-2000NXS2) only say which
//! beat they are on (status, every 200 ms) and when each beat lands (a beat
//! packet), so [`FollowClock`] puts the playhead on the grid at every beat
//! and runs it on at the player's pitch between beats.  A cue or a scratch
//! inside one beat shows at the next; paused, the playhead is on the beat
//! the player reports, not inside it.

use opendeck_types::BeatGrid;
use opendeck_types::time::Instant;

/// The followed player's track, as a list of beat times in ms (beat 1 first:
/// Link counts beats from 1).
#[derive(Debug, Clone, Default)]
pub struct Grid { pub beats_ms: Vec<f64>, period_ms: f64, anchor_ms: f64 }

impl Grid {
    /// From a deck's grid (`sample_rate` frames a second).
    pub fn from_beat_grid(g: &BeatGrid, sample_rate: u32) -> Grid {
        let sr = sample_rate.max(1) as f64;
        Grid {
            beats_ms: g.beats.iter().map(|&f| f as f64 * 1000.0 / sr).collect(),
            period_ms: if g.bpm > 0.0 { 60_000.0 / g.bpm } else { 500.0 },
            anchor_ms: g.anchor_sample as f64 * 1000.0 / sr,
        }
    }

    /// A constant-tempo grid: beat 1 at `first_ms`.
    #[cfg(test)]
    pub fn constant(first_ms: f64, bpm: f64) -> Grid { Grid { beats_ms: Vec::new(), period_ms: 60_000.0 / bpm, anchor_ms: first_ms } }

    /// When beat `n` (1-based) lands, in track ms.
    pub fn ms_of(&self, n: u32) -> f64 {
        let i = n.max(1) as usize - 1;
        match self.beats_ms.get(i) {
            Some(&t) => t,
            None => match self.beats_ms.last() {
                // Past the grid's last beat: run on at the last beat's length.
                Some(&last) => last + (i + 1 - self.beats_ms.len()) as f64 * self.period_at(self.beats_ms.len() as u32 - 1),
                None => self.anchor_ms + i as f64 * self.period_ms,
            },
        }
    }

    /// How long beat `n` lasts, in track ms.
    pub fn period_at(&self, n: u32) -> f64 {
        let n = n.max(1) as usize;
        match (self.beats_ms.get(n - 1), self.beats_ms.get(n)) {
            (Some(a), Some(b)) if b > a => b - a,
            _ if self.beats_ms.len() >= 2 => { let l = self.beats_ms.len(); self.beats_ms[l - 1] - self.beats_ms[l - 2] }
            _ => self.period_ms,
        }
    }

    /// Where `ms` is in beats: 3.25 = a quarter of the way into beat 3.
    pub fn pos_beats(&self, ms: f64) -> f64 {
        let n = self.beat_of(ms);
        n as f64 + ((ms - self.ms_of(n)) / self.period_at(n)).max(0.0)
    }

    /// The beat (1-based) that `ms` falls in.
    pub fn beat_of(&self, ms: f64) -> u32 {
        if self.beats_ms.is_empty() {
            return ((ms - self.anchor_ms) / self.period_ms).floor().max(0.0) as u32 + 1;
        }
        self.beats_ms.partition_point(|&t| t <= ms).max(1) as u32
    }
}

/// What Link last said about the followed player.
#[derive(Debug, Clone, Copy)]
pub struct Heard {
    /// Its latest status: beat number, PLAY, pitch (1.0 = +0%), and when it came.
    pub beat:      Option<u32>,
    pub playing:   bool,
    pub pitch:     f64,
    pub status_at: Instant,
    /// Where in the bar the status's beat is (1-4).
    pub status_bib: Option<u8>,
    /// When its latest beat packet came, and where in the bar that beat is.
    pub beat_at:   Option<Instant>,
    pub beat_bib:  Option<u8>,
}

/// The followed player's playhead, from its beats and status.
#[derive(Debug, Default)]
pub struct FollowClock {
    /// Where the playhead was (track ms) at an instant, and whether it runs on from there.
    base:        Option<(f64, Instant)>,
    playing:     bool,
    pitch:       f64,
    /// The beat we believe it is on.
    beat:        Option<u32>,
    last_beat:   Option<Instant>,
    last_status: Option<Instant>,
    /// Statuses in a row whose beat disagreed with ours: one alone is a
    /// status sent before its number moved, not a jump.
    mismatch:    u8,
    /// How this player's beat numbers sit in the bar: beat n is at
    /// `(n - 1 + offset) % 4 + 1`.  Each status votes for an offset (its
    /// beat and that beat's place in the bar); a status sent as the beat
    /// changes can pair the new number with the old place, so it takes a
    /// clear majority.  A beat packet names its place in the bar, so with
    /// this it names its beat exactly.
    bar_votes:   [f32; 4],
    /// Before the grid's first beat (a player says beat 0 anywhere in the
    /// lead-in): the playhead runs on, but not past beat 1 until its beat
    /// packet says it got there.
    cap:         Option<f64>,
}

/// Of the beats next to `n`, the one at `bib` in the bar.
fn on_bar(n: u32, bib: Option<u8>, offset: Option<u32>) -> u32 {
    let (Some(bib), Some(off)) = (bib, offset) else { return n };
    [n, n + 1, n.saturating_sub(1).max(1)].into_iter()
        .find(|&m| (m - 1 + off) % 4 + 1 == bib as u32).unwrap_or(n)
}

impl FollowClock {
    /// Start over (a new track).
    pub fn reset(&mut self) { *self = FollowClock::default(); }

    /// The bar offset most statuses agree on (at least three quarters of the vote).
    fn bar_offset(&self) -> Option<u32> {
        let total: f32 = self.bar_votes.iter().sum();
        let (i, best) = self.bar_votes.iter().enumerate().fold((0, 0.0f32), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
        (total >= 3.0 && best >= total * 0.75).then_some(i as u32)
    }

    /// The playhead at `now`, in track ms.
    pub fn at(&self, now: Instant) -> Option<f64> {
        let (ms, at) = self.base?;
        let run = if self.playing { now.saturating_duration_since(at).as_secs_f64() * 1000.0 * self.pitch } else { 0.0 };
        Some(self.cap.map_or(ms + run, |cap| (ms + run).min(cap)))
    }

    /// Take what is new in `h` and return the playhead at `now`.
    pub fn update(&mut self, h: &Heard, grid: &Grid, now: Instant) -> Option<f64> {
        if h.pitch > 0.0 { self.pitch = h.pitch; }
        if let (Some(n @ 1..), Some(bib), true) = (h.beat, h.status_bib, Some(h.status_at) != self.last_status) {
            let off = ((bib as u32 + 7 - (n - 1) % 4) % 4) as usize;
            for (i, v) in self.bar_votes.iter_mut().enumerate() { *v = *v * 0.95 + if i == off { 1.0 } else { 0.0 }; }
        }
        // A beat packet: the playhead is on the next beat's line, now.
        if let Some(tb) = h.beat_at.filter(|t| Some(*t) != self.last_beat) {
            self.last_beat = Some(tb);
            let n = match (self.playing, self.beat, self.base) {
                (true, Some(b), Some((ms, at))) => {
                    // How far into beat b this packet came; beats missed in
                    // between (a lost packet) count too.
                    let into = ms - grid.ms_of(b) + tb.saturating_duration_since(at).as_secs_f64() * 1000.0 * self.pitch;
                    Some(b + (into / grid.period_at(b)).round().max(1.0) as u32)
                }
                // Beats while we hold it paused: it plays (its status says so
                // a few hundred ms later), from the next beat line.
                // Held before the first beat (cued at beat 0), the next is beat 1.
                (false, _, Some((ms, _))) => { self.playing = true; Some(if ms < grid.ms_of(1) - 1.0 { 1 } else { grid.beat_of(ms) + 1 }) }
                _ => None,
            };
            if let Some(n) = n {
                let n = on_bar(n, h.beat_bib, self.bar_offset());
                log::debug!(target: "follow", "beat packet: {:?} → {n}", self.beat);
                self.cap = None;
                self.beat = Some(n);
                self.base = Some((grid.ms_of(n), tb));
            }
        }
        if Some(h.status_at) != self.last_status {
            let prev = self.last_status.replace(h.status_at);
            self.status(h, grid, prev);
        }
        self.at(now)
    }

    /// A status: PLAY or pause, and whether its beat number says the
    /// playhead jumped.
    ///
    /// Players number the beat differently.  An XDJ-1000MK2's status moves
    /// to beat n with n's beat packet; a CDJ-3000's (3.22) halfway through
    /// beat n-1, the nearest beat.  So a status agrees with us if its beat
    /// is the one we are in or the nearest one, and a jump is taken only
    /// from a status in the first half of a beat, where both say the same.
    fn status(&mut self, h: &Heard, grid: &Grid, prev: Option<Instant>) {
        let ts = h.status_at;
        // A paused status just after a beat packet is one sent before the
        // player started: the beats are newer news.
        let stale_pause = !h.playing && self.playing && h.beat_at.is_some_and(|tb| tb > ts.checked_sub(std::time::Duration::from_millis(50)).unwrap_or(ts)) && prev.is_none_or(|p| p < ts);
        if h.playing != self.playing && !stale_pause {
            // It stopped somewhere since the last status, and a player's status
            // reports a change late (a CDJ-3000's beats start ~300 ms before its
            // status says PLAY): hold the playhead where it was at that last
            // status.  (Starting is taken from the first beat packet instead.)
            let at = match prev { Some(p) if self.playing && p < ts => p, _ => ts };
            if let Some(ms) = self.at(at) { self.base = Some((ms, ts)); }
            self.playing = h.playing;
        }
        let Some(n) = h.beat else { return };
        let here = self.at(ts);
        if n == 0 {
            // In the lead-in: anywhere before beat 1 agrees.  Past it, one
            // such status is one sent before beat 1's packet moved its number;
            // two are a cue back to the start, which begins at the top.
            let first = grid.ms_of(1);
            if here.is_some_and(|ms| ms < first) { self.cap = Some(first); self.mismatch = 0; return; }
            self.mismatch = self.mismatch.saturating_add(1);
            if here.is_some() && self.mismatch < 2 { return; }
            self.mismatch = 0;
            self.beat = Some(0);
            self.base = Some(((first - grid.period_at(1)).max(0.0), ts));
            self.cap = Some(first);
            return;
        }
        let agrees = here.is_some_and(|ms| {
            let (pos, n) = (grid.pos_beats(ms), n as f64);
            n > pos - 1.15 && n <= pos + 0.65   // its own beat (just after a beat line, still the last) or the nearest
        });
        if agrees { self.mismatch = 0; return; }
        self.mismatch = self.mismatch.saturating_add(1);
        log::debug!(target: "follow", "status: beat {n}, ours {:?}, mismatch {}", here.map(|ms| grid.pos_beats(ms)), self.mismatch);
        if here.is_some() && self.mismatch < 2 { return; }
        // A jump (a cue, a search, a beat jump, the first status): on beat n.
        // Playing with a recent beat packet, beat n began at that packet; in
        // the second half of a beat wait for the next status.  Otherwise we
        // only know the beat, so it starts now.
        let period = grid.period_at(n);
        // Just after a beat line a status may still name the last beat, and
        // past the middle the next: take a jump only from one in between.
        let into = match h.beat_at {
            Some(tb) if self.playing && tb <= ts => {
                let d = ts.duration_since(tb).as_secs_f64() * 1000.0 * self.pitch;
                if d >= period { 0.0 } else if (period * 0.12..period * 0.45).contains(&d) { d } else { return }
            }
            _ => 0.0,
        };
        self.mismatch = 0;
        self.cap = None;
        self.beat = Some(n);
        self.base = Some((grid.ms_of(n) + into, ts));
    }
}

/// The playhead as drawn: runs on at the pitch every frame and is pulled
/// gently onto the clock's, so a beat's correction of a few ms doesn't jerk
/// the waveform.  A jump of more than a beat or so is taken at once.
#[derive(Debug, Default)]
pub struct Smooth { est: Option<(f64, Instant)> }

impl Smooth {
    pub fn reset(&mut self) { self.est = None; }

    pub fn next(&mut self, target_ms: f64, pitch: f64, playing: bool, now: Instant) -> f64 {
        let est = match self.est {
            Some((e, at)) => {
                let dt = now.saturating_duration_since(at).as_secs_f64().min(0.25);
                let e = if playing { e + dt * 1000.0 * pitch } else { e };
                let err = target_ms - e;
                if err.abs() > 300.0 || !playing { target_ms } else { e + err * (1.0 - (-dt / 0.15).exp()) }
            }
            None => target_ms,
        };
        self.est = Some((est, now));
        est
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// An XDJ playing a 120 BPM track (a beat every 500 ms, beat 1 at
    /// 100 ms) from beat `from`, at pitch 1, heard by a FollowClock: beat
    /// packets on every beat and a status every 200 ms (carrying the beat
    /// number the XDJ is on at that moment).  Returns the clock's playhead
    /// against the truth at every 10 ms.
    fn run(from: u32, secs: f64, pitch: f64, status_lag_ms: u64) -> Vec<(f64, f64)> { run_as(from, secs, pitch, status_lag_ms, false) }

    /// `nearest`: the status names the nearest beat (a CDJ-3000), not the
    /// one the playhead is in (an XDJ).
    fn run_as(from: u32, secs: f64, pitch: f64, status_lag_ms: u64, nearest: bool) -> Vec<(f64, f64)> {
        let grid = Grid::constant(100.0, 120.0);
        let t0 = Instant::now();
        let start_ms = grid.ms_of(from) + 120.0;   // inside the beat
        let truth = |t: f64| start_ms + t * pitch;
        let wall_of = |ms: f64| (ms - start_ms) / pitch;   // ms of wall time when the track is at `ms`
        let mut c = FollowClock::default();
        let mut h = Heard { beat: None, playing: true, pitch, status_at: t0, status_bib: None, beat_at: None, beat_bib: None };
        let mut out = Vec::new();
        let mut next_status = 0.0;
        let mut next_beat = from + 1;
        let mut t = 0.0;
        while t < secs * 1000.0 {
            let now = t0 + Duration::from_secs_f64(t / 1000.0);
            if wall_of(grid.ms_of(next_beat)) <= t {
                h.beat_at = Some(now);
                h.beat_bib = Some(((next_beat + 2) % 4 + 1) as u8);   // beat 1 is the bar's 3rd
                next_beat += 1;
            }
            if t >= next_status {
                // A status sent `status_lag_ms` after a beat may still carry the old beat number.
                let ms = truth(t - status_lag_ms as f64);
                let on = if nearest { grid.beat_of(ms + 250.0) } else { grid.beat_of(ms) };
                h.beat = Some(on);
                h.status_bib = Some(((on + 2) % 4 + 1) as u8);
                h.status_at = now;
                next_status += 200.0;
            }
            if let Some(p) = c.update(&h, &grid, now) { out.push((p, truth(t))); }
            t += 10.0;
        }
        out
    }

    #[test]
    fn a_playing_xdj_is_followed_to_within_a_frame_once_a_beat_has_landed() {
        for pitch in [1.0, 1.06, 0.92] {
            let v = run(10, 6.0, pitch, 0);
            // Before the first beat packet the clock only knows the beat (up to a beat off).
            let after: Vec<_> = v.iter().skip(60).collect();
            let worst = after.iter().map(|(p, t)| (p - t).abs()).fold(0.0, f64::max);
            assert!(worst < 15.0, "pitch {pitch}: worst error {worst:.1} ms after the first beat");
        }
    }

    #[test]
    fn a_status_one_beat_stale_does_not_jump_the_playhead() {
        // The XDJ's status lags its beat by 30 ms: right after each beat it still says the old one.
        let v = run(10, 6.0, 1.0, 30);
        let after: Vec<_> = v.iter().skip(60).collect();
        let worst = after.iter().map(|(p, t)| (p - t).abs()).fold(0.0, f64::max);
        assert!(worst < 15.0, "worst error {worst:.1} ms");
    }

    #[test]
    fn a_player_naming_the_nearest_beat_is_followed_without_slipping_a_beat() {
        // From inside the second half of a beat, where the two conventions differ.
        for from in [10, 11] {
            let v = run_as(from, 8.0, 1.0, 0, true);
            let worst = v.iter().skip(60).map(|(p, t)| (p - t).abs()).fold(0.0, f64::max);
            assert!(worst < 15.0, "from beat {from}: worst error {worst:.1} ms");
        }
    }

    #[test]
    fn pause_holds_and_a_cue_jumps_to_its_beat() {
        let grid = Grid::constant(0.0, 120.0);
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut c = FollowClock::default();
        // Playing on beat 9 (4000 ms), a beat packet at its start.
        let mut h = Heard { beat: Some(9), playing: true, pitch: 1.0, status_at: at(100), status_bib: Some(1), beat_at: Some(at(0)), beat_bib: Some(1) };
        let p = c.update(&h, &grid, at(100)).unwrap();
        assert!((p - 4100.0).abs() < 1.0, "{p}");
        // The pause status at 300 says it stopped since the one at 100, and
        // statuses come late: it holds where the one at 100 had it (4100).
        h.playing = false; h.status_at = at(300);
        assert!((c.update(&h, &grid, at(300)).unwrap() - 4100.0).abs() < 1.0);
        assert!((c.update(&h, &grid, at(2000)).unwrap() - 4100.0).abs() < 1.0, "holds while paused");
        // CUE back to beat 1: the status's beat moves (twice, to be believed) and so does the playhead.
        h.beat = Some(1); h.status_at = at(2200);
        c.update(&h, &grid, at(2200));
        h.status_at = at(2400);
        assert_eq!(c.update(&h, &grid, at(2400)), Some(0.0));
        // PLAY: it runs from there.
        h.playing = true; h.status_at = at(2600);
        let p = c.update(&h, &grid, at(2700)).unwrap();
        assert!((p - 100.0).abs() < 1.0, "{p}");
    }

    /// A track whose first beat is 2 s in: the player says beat 0 all
    /// through the lead-in.  The playhead starts at the top (a beat before
    /// beat 1, here), runs on, waits at beat 1's line rather than run past
    /// it, takes beat 1's packet, and a late status still saying 0 after it
    /// doesn't send it back.  (Beat 0 used to overflow the bar vote.)
    #[test]
    fn a_lead_in_before_the_first_beat_is_followed_into_beat_1() {
        let grid = Grid::constant(2000.0, 120.0);
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut c = FollowClock::default();
        let mut h = Heard { beat: Some(0), playing: false, pitch: 1.0, status_at: at(0), status_bib: Some(4), beat_at: None, beat_bib: None };
        assert_eq!(c.update(&h, &grid, at(0)), Some(1500.0), "cued in the lead-in: at the top of it");
        h.playing = true; h.status_at = at(200);
        assert!((c.update(&h, &grid, at(400)).unwrap() - 1700.0).abs() < 1.0, "runs on");
        assert_eq!(c.update(&h, &grid, at(900)), Some(2000.0), "but not past beat 1 without its packet");
        h.beat_at = Some(at(950)); h.beat_bib = Some(1);
        assert_eq!(c.update(&h, &grid, at(950)), Some(2000.0), "beat 1's packet: on its line");
        h.status_at = at(960);
        assert!((c.update(&h, &grid, at(1000)).unwrap() - 2050.0).abs() < 1.0, "a status still saying 0 is late, not a cue");
        h.beat = Some(1); h.status_at = at(1100);
        assert!((c.update(&h, &grid, at(1200)).unwrap() - 2250.0).abs() < 1.0);
    }

    /// A real XDJ-1000MK2 playing at 126 BPM (reference/link-captures, the
    /// app's own debug log of what it heard), replayed through the clock at
    /// the packets' own times: once its first beat lands, the clock counts
    /// the same beats the XDJ reports, puts each beat packet on its grid
    /// line, and never runs backwards.
    #[test]
    fn a_recorded_xdj_is_followed_beat_for_beat() {
        let r = replay(include_str!("../../../reference/link-captures/xdj-1000mk2-playing-2026-08-26.log"), 126.0, 10.0);
        assert!(r.beats >= 8 && r.statuses >= 15, "the capture has the XDJ playing: {} beats, {} statuses", r.beats, r.statuses);
    }

    /// An XDJ-1000MK2 (firmware 1.45) running in cdj-nxs2-qemu, its packets
    /// stamped with the deck's own guest time: it plays a 126.05 BPM click
    /// track, pauses, plays, goes to +2.5 %, pauses, plays, then -2.5 %.  The
    /// clock follows every beat through all of it (the emulated deck's beats
    /// wobble about ±10 ms, so the run-on bound is looser than the real XDJ's).
    #[test]
    fn an_emulated_xdj_is_followed_through_pauses_and_pitch_changes() {
        let r = replay(include_str!("../../../reference/link-captures/xdj-1000mk2-emulated-play-pause-pitch-2026-11-23.log"), 126.05, 25.0);
        assert!(r.beats >= 150, "{} beats", r.beats);
        assert_eq!(r.pauses, 2, "both pauses heard");
        assert!(r.pitches.iter().any(|p| (p - 1.025).abs() < 0.002) && r.pitches.iter().any(|p| (p - 0.975).abs() < 0.002), "{:?}", r.pitches);
        let mut e = r.runon_ms.clone();
        e.sort_by(f64::total_cmp);
        let p95 = e[e.len() * 95 / 100];
        eprintln!("emulated XDJ: {} beats, run-on error median {:.1} ms, p95 {p95:.1} ms, max {:.1} ms", r.beats, e[e.len() / 2], e[e.len() - 1]);
        assert!(p95 < 15.0, "p95 {p95:.1} ms");
    }

    struct Replayed { beats: usize, statuses: usize, pauses: usize, pitches: Vec<f64>, runon_ms: Vec<f64> }

    /// Replay a capture log (the app's debug lines of what it heard) through
    /// the clock at the packets' own times.  Once its first beat lands, the
    /// clock must count the same beats the player reports, put each beat
    /// packet on its grid line having run on to within `runon` ms of it, hold
    /// still while paused, and never run backwards.
    fn replay(log: &str, bpm: f64, runon: f64) -> Replayed {
        use opendeck_link::prodj::ProDjLink;
        let t0 = Instant::now();
        let mut first: Option<f64> = None;
        let mut packets = Vec::new();
        for line in log.lines() {
            let (Some(ts), Some(open), Some(close)) = (line.find('T'), line.find("— ["), line.rfind(']')) else { continue };
            if !line.contains("ProDJ rx :") { continue; }
            let hms: Vec<f64> = line[ts + 1..].split(['Z', ' ']).next().unwrap().split(':').map(|v| v.parse().unwrap()).collect();
            let secs = hms[0] * 3600.0 + hms[1] * 60.0 + hms[2];
            let t = secs - *first.get_or_insert(secs);
            let bytes: Vec<u8> = line[open + "— [".len()..close].split(',').map(|h| u8::from_str_radix(h.trim(), 16).unwrap()).collect();
            packets.push((t0 + Duration::from_secs_f64(t), bytes));
        }
        let grid = Grid::constant(0.0, bpm);
        let mut c = FollowClock::default();
        let mut h: Option<Heard> = None;
        let mut r = Replayed { beats: 0, statuses: 0, pauses: 0, pitches: Vec::new(), runon_ms: Vec::new() };
        let (mut prev, mut was_playing, mut since_play) = (f64::MIN, false, 0);
        for (at, b) in &packets {
            if let Some(beat) = ProDjLink::parse_beat(b) {
                if beat.player != 1 { continue; }
                let Some(hh) = h.as_mut() else { continue };
                // Just before the packet, the clock should be about to reach a grid line.
                let before = c.at(*at).unwrap();
                hh.beat_at = Some(*at);
                hh.beat_bib = Some(beat.beat_in_bar);
                let p = c.update(hh, &grid, *at).unwrap();
                if r.beats > 0 {
                    let n = grid.beat_of(p + 1.0);
                    assert!((p - grid.ms_of(n)).abs() < 0.5, "a beat packet is on a grid line");
                    // The first beat after a PLAY has nothing to have run on from.
                    if since_play > 0 {
                        assert!((before - p).abs() < runon, "and the clock had run on to within {runon} ms of it: {:.1} (beat {n})", before - p);
                        r.runon_ms.push((before - p).abs());
                    }
                }
                r.beats += 1;
                since_play += 1;
            } else if let Some(st) = ProDjLink::parse_status(b) {
                if st.player != 1 { continue; }
                let hh = h.get_or_insert(Heard { beat: None, playing: false, pitch: 1.0, status_at: *at, status_bib: None, beat_at: None, beat_bib: None });
                hh.beat = st.beat; hh.status_bib = st.beat_in_bar; hh.playing = st.playing; hh.pitch = st.pitch as f64; hh.status_at = *at;
                let held = c.at(*at);
                // A player that has loaded but not cued names no beat yet.
                let Some(p) = c.update(hh, &grid, *at) else { assert!(st.beat.is_none(), "a beat named, no position"); continue };
                if !st.playing && !was_playing { if let Some(held) = held { assert!((p - held).abs() < 1.0, "holds while paused: {held:.1} → {p:.1}"); } }
                if was_playing && !st.playing { r.pauses += 1; }
                if !was_playing && st.playing { since_play = 0; }
                was_playing = st.playing;
                if r.pitches.last().is_none_or(|q| (q - st.pitch as f64).abs() > 1e-4) { r.pitches.push(st.pitch as f64); }
                if r.beats > 1 && st.playing {
                    let ours = grid.beat_of(p + 1.0);
                    assert!(st.beat == Some(ours) || st.beat == Some(ours - 1), "status says beat {:?}, the clock is on {ours}", st.beat);
                }
                r.statuses += 1;
            } else { continue }
            let p = c.at(*at).unwrap();
            // Backwards only by a cue (a status jump while paused); the captures have none.
            assert!(p >= prev - 1.0, "never backwards: {prev:.1} → {p:.1}");
            prev = p;
        }
        r
    }

    #[test]
    fn grid_counts_beats_from_one_and_runs_on_past_its_end() {
        let g = Grid { beats_ms: vec![100.0, 600.0, 1100.0], ..Grid::constant(0.0, 120.0) };
        assert_eq!(g.ms_of(1), 100.0);
        assert_eq!(g.ms_of(3), 1100.0);
        assert_eq!(g.ms_of(5), 2100.0);
        assert_eq!(g.beat_of(650.0), 2);
        assert_eq!(g.beat_of(50.0), 1);
    }
}

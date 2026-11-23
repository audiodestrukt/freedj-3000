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
use std::time::Instant;

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
    /// When its latest beat packet came.
    pub beat_at:   Option<Instant>,
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
    /// Statuses in a row whose beat disagreed with ours (a status sent just
    /// before its beat number moved is one beat behind: not a jump).
    mismatch:    u8,
}

impl FollowClock {
    /// Start over (a new track).
    pub fn reset(&mut self) { *self = FollowClock::default(); }

    /// The playhead at `now`, in track ms.
    pub fn at(&self, now: Instant) -> Option<f64> {
        let (ms, at) = self.base?;
        let run = if self.playing { now.saturating_duration_since(at).as_secs_f64() * 1000.0 * self.pitch } else { 0.0 };
        Some(ms + run)
    }

    /// Take what is new in `h` and return the playhead at `now`.
    pub fn update(&mut self, h: &Heard, grid: &Grid, now: Instant) -> Option<f64> {
        if h.pitch > 0.0 { self.pitch = h.pitch; }
        // A beat packet: the playhead is on the next beat's line, now.
        if let Some(tb) = h.beat_at.filter(|t| Some(*t) != self.last_beat) {
            self.last_beat = Some(tb);
            if let (true, Some(b), Some((ms, at))) = (self.playing, self.beat, self.base) {
                // How far into beat b this packet came; beats missed in
                // between (a lost packet) count too.
                let into = ms - grid.ms_of(b) + tb.saturating_duration_since(at).as_secs_f64() * 1000.0 * self.pitch;
                let k = (into / grid.period_at(b)).round().max(1.0) as u32;
                let n = b + k;
                self.beat = Some(n);
                self.base = Some((grid.ms_of(n), tb));
            }
        }
        if Some(h.status_at) != self.last_status {
            self.last_status = Some(h.status_at);
            self.status(h, grid);
        }
        self.at(now)
    }

    fn status(&mut self, h: &Heard, grid: &Grid) {
        let ts = h.status_at;
        let here = self.at(ts);
        if h.playing != self.playing {
            // PLAY or pause: hold the playhead where it was, and run on (or not) from there.
            if let Some(ms) = here { self.base = Some((ms, ts)); }
            self.playing = h.playing;
        }
        let Some(n) = h.beat else { return };
        let ours = match (self.playing, here) {
            (true, _) => self.beat,
            (false, Some(ms)) => Some(grid.beat_of(ms)),
            (false, None) => None,
        };
        if ours == Some(n) { self.mismatch = 0; return; }
        self.mismatch += 1;
        if ours.is_some() && self.mismatch < 2 { return; }
        self.mismatch = 0;
        // A jump (a cue, a search, a new track, the first status): on beat n.
        // Playing, beat n started at its last beat packet if that came inside
        // this beat; otherwise we only know the beat, so start it now.
        let into = match h.beat_at {
            Some(tb) if self.playing && tb <= ts => {
                let d = ts.duration_since(tb).as_secs_f64() * 1000.0 * self.pitch;
                if d < grid.period_at(n) { d } else { 0.0 }
            }
            _ => 0.0,
        };
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
    fn run(from: u32, secs: f64, pitch: f64, status_lag_ms: u64) -> Vec<(f64, f64)> {
        let grid = Grid::constant(100.0, 120.0);
        let t0 = Instant::now();
        let start_ms = grid.ms_of(from) + 120.0;   // inside the beat
        let truth = |t: f64| start_ms + t * pitch;
        let wall_of = |ms: f64| (ms - start_ms) / pitch;   // ms of wall time when the track is at `ms`
        let mut c = FollowClock::default();
        let mut h = Heard { beat: None, playing: true, pitch, status_at: t0, beat_at: None };
        let mut out = Vec::new();
        let mut next_status = 0.0;
        let mut next_beat = from + 1;
        let mut t = 0.0;
        while t < secs * 1000.0 {
            let now = t0 + Duration::from_secs_f64(t / 1000.0);
            if wall_of(grid.ms_of(next_beat)) <= t {
                h.beat_at = Some(now);
                next_beat += 1;
            }
            if t >= next_status {
                // A status sent `status_lag_ms` after a beat may still carry the old beat number.
                let on = grid.beat_of(truth(t - status_lag_ms as f64));
                h.beat = Some(on);
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
    fn pause_holds_and_a_cue_jumps_to_its_beat() {
        let grid = Grid::constant(0.0, 120.0);
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut c = FollowClock::default();
        // Playing on beat 9 (4000 ms), a beat packet at its start.
        let mut h = Heard { beat: Some(9), playing: true, pitch: 1.0, status_at: at(100), beat_at: Some(at(0)) };
        let p = c.update(&h, &grid, at(100)).unwrap();
        assert!((p - 4100.0).abs() < 1.0, "{p}");
        // Paused at 4300: it holds there.
        h.playing = false; h.status_at = at(300);
        assert!((c.update(&h, &grid, at(300)).unwrap() - 4300.0).abs() < 1.0);
        assert!((c.update(&h, &grid, at(2000)).unwrap() - 4300.0).abs() < 1.0, "holds while paused");
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

    /// A real XDJ-1000MK2 playing at 126 BPM (reference/link-captures, the
    /// app's own debug log of what it heard), replayed through the clock at
    /// the packets' own times: once its first beat lands, the clock counts
    /// the same beats the XDJ reports, puts each beat packet on its grid
    /// line, and never runs backwards.
    #[test]
    fn a_recorded_xdj_is_followed_beat_for_beat() {
        use opendeck_link::prodj::ProDjLink;
        let log = include_str!("../../../reference/link-captures/xdj-1000mk2-playing-2026-08-26.log");
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
        let grid = Grid::constant(0.0, 126.0);
        let mut c = FollowClock::default();
        let mut h: Option<Heard> = None;
        let (mut beats, mut statuses, mut prev) = (0, 0, f64::MIN);
        for (at, b) in &packets {
            if let Some(beat) = ProDjLink::parse_beat(b) {
                if beat.player != 1 { continue; }
                let Some(hh) = h.as_mut() else { continue };
                // Just before the packet, the clock should be about to reach a grid line.
                let before = c.at(*at).unwrap();
                hh.beat_at = Some(*at);
                let p = c.update(hh, &grid, *at).unwrap();
                if beats > 0 {
                    let n = grid.beat_of(p + 1.0);
                    assert!((p - grid.ms_of(n)).abs() < 0.5, "a beat packet is on a grid line");
                    assert!((before - p).abs() < 10.0, "and the clock had run on to within 10 ms of it: {:.1}", before - p);
                }
                beats += 1;
            } else if let Some(st) = ProDjLink::parse_status(b) {
                if st.player != 1 { continue; }
                let hh = h.get_or_insert(Heard { beat: None, playing: false, pitch: 1.0, status_at: *at, beat_at: None });
                hh.beat = st.beat; hh.playing = st.playing; hh.pitch = st.pitch as f64; hh.status_at = *at;
                let p = c.update(hh, &grid, *at).unwrap();
                if beats > 1 {
                    let ours = grid.beat_of(p + 1.0);
                    assert!(st.beat == Some(ours) || st.beat == Some(ours - 1), "status says beat {:?}, the clock is on {ours}", st.beat);
                }
                statuses += 1;
            } else { continue }
            let p = c.at(*at).unwrap();
            assert!(p >= prev - 1.0, "never backwards: {prev:.1} → {p:.1}");
            prev = p;
        }
        assert!(beats >= 8 && statuses >= 15, "the capture has the XDJ playing: {beats} beats, {statuses} statuses");
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

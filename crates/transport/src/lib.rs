//! The CUE / PLAY rules, as a pure state machine.
//!
//! These are the behaviours a CDJ player is judged on, and they are all about
//! *when* something happens rather than what it sounds like: hold CUE to preview,
//! tap PLAY while holding to latch, press CUE while playing to return and pause,
//! press CUE after pausing to set a new cue there. Getting one of them subtly
//! wrong is the kind of thing you notice on stage and not in a code review.
//!
//! So they live here, away from the audio thread, the atomics and the UI:
//! nothing in this crate knows what a sample is. It takes an event plus the
//! current audible position, updates its own small state, and returns an
//! [`Outcome`] describing what the deck should do. The caller owns the playhead.
//!
//! That makes them verifiable. `tests/specs.rs` runs the behaviour specs from
//! `specs/xdj-1000mk2/` against this crate through the `panelspec` protocol, so
//! the same specs can also be run against a rewrite, a plugin, or a JS port.

/// A position in source samples, as the listener hears it.
pub type Pos = u64;

/// What the transport remembers between events.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Transport {
    /// Where CUE returns to.
    pub cue_point: Pos,
    /// The playhead is sitting on the cue point, rather than played or searched
    /// away from it.
    ///
    /// This decides whether a CUE press *previews the existing cue* or *sets a
    /// new one here*, which is why every path that moves the playhead has to keep
    /// it honest.
    pub cued: bool,
    /// CUE is held from a paused deck: releasing it returns to the cue.
    pub preview: bool,
}

/// What the deck should do about an event. `None` means "leave it alone".
///
/// Applied in a fixed order — pause, then seek, then play — which reproduces the
/// ordering each case needs: returning to the cue stops before seeking, while a
/// preview seeks before starting.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Outcome {
    pub playing: Option<bool>,
    pub seek_to: Option<Pos>,
    /// The cue point moved to here.
    pub cue_point: Option<Pos>,
}

impl Outcome {
    fn playing(playing: bool) -> Self {
        Self { playing: Some(playing), ..Self::default() }
    }

    fn with_seek(mut self, to: Pos) -> Self {
        self.seek_to = Some(to);
        self
    }

    fn with_cue(mut self, at: Pos) -> Self {
        self.cue_point = Some(at);
        self
    }
}

impl Transport {
    pub fn new(cue_point: Pos) -> Self {
        // A freshly loaded track sits on its cue.
        Self { cue_point, cued: true, preview: false }
    }

    /// Latch continuous playback, cancelling any momentary CUE preview.
    ///
    /// After this, releasing CUE no longer jumps back to the cue — the deck just
    /// keeps playing. This is the XDJ "hold CUE, tap PLAY to lock in" gesture, and
    /// also what a plain PLAY means.
    pub fn play(&mut self) -> Outcome {
        self.preview = false;
        // Playback leaves the cue behind.
        self.cued = false;
        Outcome::playing(true)
    }

    /// PLAY/PAUSE. During a preview it latches instead of toggling.
    pub fn play_pause(&mut self, playing: bool) -> Outcome {
        if self.preview {
            return self.play();
        }
        if playing {
            Outcome::playing(false)
        } else {
            // Starting playback leaves the cue, so pausing somewhere else and
            // pressing CUE sets a new cue *there* rather than previewing the old
            // one. Forgetting this is a real bug: it makes the standard
            // play → pause → CUE way of setting a cue point silently do nothing.
            self.cued = false;
            Outcome::playing(true)
        }
    }

    /// Momentary CUE. `audible` is the displayed playhead, already quantised to a
    /// frame boundary by the caller — not the decoder's read-ahead cursor, which
    /// sits ahead of what is actually heard.
    pub fn cue(&mut self, pressed: bool, playing: bool, audible: Pos) -> Outcome {
        if !pressed {
            // Releasing only matters if this press started a preview.
            if !self.preview {
                return Outcome::default();
            }
            self.preview = false;
            self.cued = true;
            return Outcome::playing(false).with_seek(self.cue_point);
        }
        if playing {
            // Playing → return to the cue and pause.
            self.cued = true;
            Outcome::playing(false).with_seek(self.cue_point)
        } else if self.cued {
            // Paused on the cue → preview from it while held, and do *not* move
            // it. Rapid re-taps therefore always retrigger the same point rather
            // than adopting a position the preview raced forward to.
            self.preview = true;
            Outcome::playing(true).with_seek(self.cue_point)
        } else {
            // Paused after playing or searching away → set a new cue here, then
            // preview it while held.
            self.cue_point = audible;
            self.preview = true;
            self.cued = true;
            Outcome::playing(true).with_cue(audible)
        }
    }

    /// The playhead was moved by hand — a jog, a needle drop, a beat jump — so it
    /// is no longer on the cue.
    pub fn searched(&mut self) {
        self.cued = false;
    }

    /// The playhead was placed *on* a cue: a memory point recalled, a track
    /// loaded, auto-cue applied.
    pub fn arrived_at_cue(&mut self, cue_point: Pos) {
        self.cue_point = cue_point;
        self.cued = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUE: Pos = 10_000;

    fn deck() -> (Transport, bool, Pos) {
        (Transport::new(CUE), false, CUE)
    }

    /// Apply an outcome the way a deck does: pause, seek, play.
    fn apply(outcome: Outcome, playing: &mut bool, position: &mut Pos) {
        if outcome.playing == Some(false) {
            *playing = false;
        }
        if let Some(to) = outcome.seek_to {
            *position = to;
        }
        if outcome.playing == Some(true) {
            *playing = true;
        }
    }

    #[test]
    fn hold_cue_previews_and_release_returns() {
        let (mut t, mut playing, mut position) = deck();
        apply(t.cue(true, playing, position), &mut playing, &mut position);
        assert!(playing && position == CUE);
        position = CUE + 5_000; // it played on a little
        apply(t.cue(false, playing, position), &mut playing, &mut position);
        assert!(!playing);
        assert_eq!(position, CUE, "release returns to the cue");
    }

    #[test]
    fn tapping_play_during_a_preview_latches() {
        let (mut t, mut playing, mut position) = deck();
        apply(t.cue(true, playing, position), &mut playing, &mut position);
        apply(t.play_pause(playing), &mut playing, &mut position);
        assert!(playing, "PLAY must latch here, not toggle to pause");
        position = CUE + 5_000;
        apply(t.cue(false, playing, position), &mut playing, &mut position);
        assert!(playing, "releasing CUE keeps playing once latched");
        assert_eq!(position, CUE + 5_000, "and does not jump back");
    }

    /// The regression this crate exists for: play, pause where you want the cue,
    /// press CUE. The cue must move there.
    #[test]
    fn pausing_then_pressing_cue_sets_the_cue_there() {
        let (mut t, mut playing, mut position) = deck();
        apply(t.play_pause(playing), &mut playing, &mut position);
        position = CUE + 30_000; // played on
        apply(t.play_pause(playing), &mut playing, &mut position);
        assert!(!playing);

        apply(t.cue(true, playing, position), &mut playing, &mut position);
        assert_eq!(t.cue_point, CUE + 30_000, "the cue moves to where you paused");
        assert!(playing, "and previews from it");
        apply(t.cue(false, playing, position), &mut playing, &mut position);
        assert_eq!(position, CUE + 30_000);
    }

    #[test]
    fn cue_while_playing_returns_and_pauses() {
        let (mut t, mut playing, mut position) = deck();
        apply(t.play_pause(playing), &mut playing, &mut position);
        position = CUE + 30_000;
        apply(t.cue(true, playing, position), &mut playing, &mut position);
        assert!(!playing);
        assert_eq!(position, CUE);
        assert_eq!(t.cue_point, CUE, "returning must not move the cue");
    }

    #[test]
    fn re_tapping_cue_never_moves_it() {
        let (mut t, mut playing, mut position) = deck();
        for _ in 0..3 {
            apply(t.cue(true, playing, position), &mut playing, &mut position);
            position += 3_000; // the preview races forward
            apply(t.cue(false, playing, position), &mut playing, &mut position);
        }
        assert_eq!(t.cue_point, CUE);
        assert_eq!(position, CUE);
    }

    #[test]
    fn searching_away_then_cue_sets_a_new_point() {
        let (mut t, mut playing, _) = deck();
        let mut position = 60_000;
        t.searched();
        apply(t.cue(true, playing, position), &mut playing, &mut position);
        assert_eq!(t.cue_point, 60_000);
    }

    #[test]
    fn releasing_cue_when_no_preview_started_does_nothing() {
        let (mut t, mut playing, mut position) = deck();
        apply(t.play_pause(playing), &mut playing, &mut position);
        position = CUE + 30_000;
        // CUE pressed while playing returns and pauses; the release must not then
        // start a preview or move anything.
        apply(t.cue(true, playing, position), &mut playing, &mut position);
        let before = (t, playing, position);
        apply(t.cue(false, playing, position), &mut playing, &mut position);
        assert_eq!((t, playing, position), before);
    }
}

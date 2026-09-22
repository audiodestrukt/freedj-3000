//! The deck's behaviour specs, run against the real transport rules.
//!
//! `specs/xdj-1000mk2/` describes the CUE/PLAY behaviours as physical inputs and
//! observable results — the only things that survive a rewrite. This test wires
//! [`transport::Transport`] to that runner, supplying the one thing the rules
//! don't own: a playhead.
//!
//! Time is virtual. Nothing sleeps, no audio device is opened, and a full run
//! takes milliseconds, so this belongs in the ordinary test suite rather than in
//! a manual check by ear.
//!
//! The same specs can be run against anything else that implements these rules —
//! a plugin, a rewrite, a JS port — by writing an adapter that speaks the
//! protocol. See the `panelspec` docs.

use panelspec::adapter::{Adapter, Hello, Input, PROTOCOL};
use panelspec::load::load;
use panelspec::runner::{Filter, Outcome, run};
use panelspec::spec::Advance;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use transport::{Pos, Transport};

/// The deck, with the audio replaced by two numbers.
struct Deck {
    rules: Transport,
    playing: bool,
    /// Where the listener would hear the playhead, in source samples.
    position: Pos,
    length: Pos,
    /// Samples per second of wall time, so specs can talk in milliseconds.
    per_ms: f64,
}

impl Default for Deck {
    fn default() -> Self {
        // A nominal 44.1 kHz stereo track: the specs work in milliseconds, so the
        // exact rate only has to be consistent.
        let per_ms = 44_100.0 * 2.0 / 1000.0;
        Self { rules: Transport::new(0), playing: false, position: 0, length: 0, per_ms }
    }
}

impl Deck {
    fn samples(&self, ms: f64) -> Pos {
        (ms * self.per_ms) as Pos
    }

    fn millis(&self, samples: Pos) -> f64 {
        samples as f64 / self.per_ms
    }

    /// Pause, seek, play — the order the rules are written against.
    fn apply(&mut self, outcome: transport::Outcome) {
        if outcome.playing == Some(false) {
            self.playing = false;
        }
        if let Some(to) = outcome.seek_to {
            self.position = to.min(self.length);
        }
        if outcome.playing == Some(true) {
            self.playing = true;
        }
    }
}

impl Adapter for Deck {
    fn hello(&mut self) -> anyhow::Result<Hello> {
        Ok(Hello {
            device: "xdj-1000mk2".into(),
            profile: "xdj-1000mk2".into(),
            implementation: Some("opendeck-transport (crates/transport)".into()),
            protocol: PROTOCOL,
        })
    }

    fn reset(&mut self, fixture: &str, params: &Map<String, Value>) -> anyhow::Result<()> {
        if fixture != "loaded-paused" {
            anyhow::bail!("unknown fixture `{fixture}`");
        }
        let number = |name: &str| -> anyhow::Result<f64> {
            params.get(name).and_then(Value::as_f64).ok_or_else(|| anyhow::anyhow!("`{name}` must be a number"))
        };
        let mut deck = Deck::default();
        deck.length = deck.samples(number("track_ms")?);
        let cue = deck.samples(number("cue_ms")?);
        deck.rules = Transport::new(cue);
        deck.position = cue;
        *self = deck;
        Ok(())
    }

    fn input(&mut self, input: &Input) -> anyhow::Result<()> {
        match input {
            Input::Down { control } if control == "play" => {
                let outcome = self.rules.play_pause(self.playing);
                self.apply(outcome);
            }
            Input::Down { control } if control == "cue" => {
                let (playing, position) = (self.playing, self.position);
                let outcome = self.rules.cue(true, playing, position);
                self.apply(outcome);
            }
            Input::Up { control } if control == "cue" => {
                let (playing, position) = (self.playing, self.position);
                let outcome = self.rules.cue(false, playing, position);
                self.apply(outcome);
            }
            // A key release that means nothing is not an error.
            Input::Up { .. } => {}
            // NEEDLE SEARCH: a touch position on the overview waveform.
            Input::Set { control, value } if control == "needle_search" => {
                let at = value.as_f64().ok_or_else(|| anyhow::anyhow!("needle_search takes 0..1"))?;
                self.position = (at.clamp(0.0, 1.0) * self.length as f64) as Pos;
                self.rules.searched();
            }
            other => anyhow::bail!("unsupported input {other:?}"),
        }
        Ok(())
    }

    fn advance(&mut self, by: Advance) -> anyhow::Result<()> {
        let Advance::Ms(ms) = by else {
            anyhow::bail!("this deck counts time in ms");
        };
        if self.playing {
            self.position = (self.position + self.samples(ms)).min(self.length);
        }
        Ok(())
    }

    fn observe(&mut self, paths: &[String]) -> anyhow::Result<Map<String, Value>> {
        Ok(paths
            .iter()
            .map(|path| {
                let value = match path.as_str() {
                    "deck.playing" => json!(self.playing),
                    "deck.position_ms" => json!(self.millis(self.position)),
                    "deck.cue_ms" => json!(self.millis(self.rules.cue_point)),
                    // An unknown path reads as absent, per the protocol.
                    _ => Value::Null,
                };
                (path.clone(), value)
            })
            .collect())
    }

    fn events(&mut self) -> anyhow::Result<Vec<Value>> {
        // The transport rules emit no events; the deck's output is its state.
        Ok(vec![])
    }
}

fn specs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs")
}

#[test]
fn the_transport_rules_satisfy_every_deck_spec() {
    let library = load(&[specs_dir()]);
    assert!(library.errors.is_empty(), "unreadable spec files: {:?}", library.errors);

    let report = run(&library, &mut Deck::default(), &Filter::default()).expect("the adapter says hello");

    let failures: Vec<String> = report
        .results
        .iter()
        .filter_map(|case| match &case.outcome {
            Outcome::Pass | Outcome::Skip { .. } => None,
            Outcome::Fail { at, detail, .. } => Some(format!("{}: {at} — {detail}", case.spec)),
            Outcome::Error { at, detail } => Some(format!("{}: {at} — {detail}", case.spec)),
        })
        .collect();
    assert!(failures.is_empty(), "\n  {}", failures.join("\n  "));

    let passed = report.count(|outcome| matches!(outcome, Outcome::Pass));
    assert!(passed >= 9, "only {passed} spec cases ran — did the specs directory move?");
}

/// The specs have to be checked against the device vocabulary too, or a typo in a
/// control name quietly tests nothing.
#[test]
fn the_specs_are_well_formed() {
    let library = load(&[specs_dir()]);
    let errors: Vec<String> = panelspec::lint::lint(&library)
        .into_iter()
        .filter(|diagnostic| diagnostic.level == panelspec::lint::Level::Error)
        .map(|diagnostic| format!("{}: {}", diagnostic.file, diagnostic.msg))
        .collect();
    assert!(errors.is_empty(), "\n  {}", errors.join("\n  "));
}

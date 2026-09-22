# Transport rules: CUE / PLAY as a verifiable state machine

`crates/transport` holds the deck's CUE and PLAY behaviour as a pure state
machine, and `specs/xdj-1000mk2/` describes that behaviour as physical inputs and
observable results. `cargo test -p opendeck-transport` runs the specs against the
rules in milliseconds, with no audio device.

## Why it was extracted

These behaviours are all about *when* something happens: hold CUE to preview, tap
PLAY while holding to latch, press CUE while playing to return and pause, press
CUE after pausing to set a new cue there. They were inline in `DeckApp::apply`,
which meant they could only be checked by playing a track and listening — and
`DeckApp` can't be constructed in a test (it needs a waveform cache, an audio
handle, a browser, a tag list, Link state).

A bug that had been there for a while makes the case. `cued` means *the playhead is
sitting on the cue point*, and it decides whether a CUE press previews the existing
cue or sets a new one. A plain `PlayPause` toggle never cleared it, so:

1. Press PLAY, let the track run.
2. Press PLAY again to pause somewhere you want a cue.
3. Press CUE — and the deck previewed the **old** cue instead of setting one here.

That is the standard CDJ way to set a cue point, and it silently did nothing.
Pioneer's manual is explicit that the paused position becomes the cue point.

The deeper problem was structural: `cued` is a *cached derivation* of
`playhead == cue_point`, and it was written at eight separate sites (jog, needle,
memory recall, auto-cue, load, `lock_in_play`, and three inside the CUE handler).
Every new path that moves the playhead is one more place that has to remember —
which is why editing the lock-play path is what tends to desync it.

## The shape

```
event + audible position ──▶ Transport ──▶ Outcome { playing, seek_to, cue_point }
                             (the rules)     │
                                             ▼
                                      DeckApp::apply_transport
                                      (atomics, seek, logging)
```

`Transport` owns `cue_point`, `cued` and `preview` — the three fields that used to
be on `DeckApp`. It knows nothing about samples, atomics or the audio thread: it
takes the current audible position as an argument and returns what should happen.
`DeckApp::apply_transport` carries that out in a fixed order — **pause, then seek,
then play** — which reproduces the ordering each case needs (returning to the cue
stops before seeking; a preview seeks before starting).

The paths that move the playhead now say so in the rules' own words:

| Instead of | Now |
|---|---|
| `self.cued = false` after a jog or needle | `self.transport.searched()` |
| setting `cue_point` + `cued = true` on memory recall, auto-cue, load | `self.transport.arrived_at_cue(pos)` |
| clearing `cue_preview` and `cued` by hand | `self.transport.play()` |

The cue is still captured at `quantized(smoothed_pos)` — the displayed playhead,
not the decoder cursor, which sits ~93 ms ahead of what is heard. That correction
stays in `DeckApp`, where the audio knowledge lives; the rules just receive the
number.

## The specs

`specs/xdj-1000mk2/` is seven behaviours plus a device vocabulary, in the
[panelspec](https://github.com/audiodestrukt/hexatrack) format:

| Spec | Pins |
|---|---|
| `cue-play-locks-in` | hold CUE + tap PLAY latches; releasing CUE keeps playing |
| `cue-preview-returns` | releasing CUE *without* PLAY returns to the cue and pauses |
| `cue-retap-keeps-point` | rapid re-taps always restart from the same point |
| `cue-while-playing` | CUE during playback returns and pauses |
| `cue-sets-point-after-search` | paused and searched away, CUE sets the cue there |
| `pause-then-cue-sets-point` | the bug above |
| `play-toggles` | plain PLAY toggles when CUE isn't held |

A spec states inputs and expected observable state, never internals:

```yaml
steps:
  - hold:
      keys: cue
      then:
        - expect: {deck.playing: true}
        - wait: 2s
        - press: play          # latches
  - expect:
      deck.playing: true       # releasing CUE no longer returns
      deck.position_ms: {$approx: 12000, $tol: 50}
```

`crates/transport/tests/specs.rs` implements the runner's `Adapter` trait against
`Transport`, supplying the one thing the rules don't own: a playhead. Time is
virtual — `advance` moves it, nothing sleeps — so a full run is milliseconds and
belongs in the normal test suite.

Two tests: `the_transport_rules_satisfy_every_deck_spec`, and
`the_specs_are_well_formed`, which lints the specs against the vocabulary so a
typo'd control name can't quietly test nothing.

Because the specs are a process-level contract, the same suite can be run against
anything else implementing these rules — a plugin, a rewrite, a JS/Web MIDI port —
by writing an adapter that speaks the protocol.

## Notes for whoever touches this next

- **Add a behaviour by writing the spec first.** If a rule can't be stated as
  inputs and observables, that's usually a sign it's reaching into internals.
- **The `panelspec` dev-dependency is pinned by revision**, because `Cargo.lock`
  is gitignored here and an unpinned git dependency would follow that repo's
  default branch. Bump it deliberately.
- **The specs exist in two repos** right now: authoritative here (this is the
  implementation they describe), with a copy in hexatrack used as a demo that two
  independent implementations satisfy the same suite. That will drift — a submodule
  or moving hexatrack's copy to test fixtures would fix it.
- **`cued` could be derived** rather than tracked:
  `(smoothed_pos - cue_point).abs() < tolerance`. That deletes this bug class
  instead of centralising it, and is now a one-function change. The tolerance is
  needed because of the in-flight offset above.
- **Not covered:** hot cues, memory points, loops, slip and the jog's own
  behaviour. They live in `DeckApp` still. The specs and vocabulary extend to them
  the same way — `docs/spec-format.md` in the hexatrack repo is the reference.

# Analysis: what to improve next

Where OpenDeck's own analysis stands against rekordbox, and what to dig
into later. Measuring tools are in `crates/app/examples/*_vs_rekordbox*`.

## Phrases

Measured on 31 house tracks from rekordbox over LINK
(`phrase_vs_rekordbox_link <rekordbox-ip> all`; cases cached in
`target/phrase-link-cache`, `-` reruns offline, `PHRASE_SWEEP=1` grid-searches
`PhraseConfig`): phrase starts within a bar precision 0.70 / recall 0.73,
mood 25/31, coarse phrase type 65 %.

- [ ] Check the settings on tracks they weren't tuned on (the rest of the
      playlist Dan analysed) before retuning.
- [ ] Mood: calibrate on Mid / Low material (vocal, downtempo, non-house).
      On house the bass features can't separate rekordbox's Mid tracks.
- [ ] Phrase types: 65 % agreement; intro/outro and up/down/chorus rules.
- [ ] Short phrases (1–5 bars, ~20 % of rekordbox's) are never produced.
- [ ] Use rekordbox's downbeats (`detect_with(.., true)`) wherever the grid
      came from rekordbox (stick or LINK loads).

## Beats, key, waveforms

- [ ] Downbeat (bar 1) detection in the beat analyzer.
- [ ] Drum & bass at full tempo; 4:5 / 2:3 tempo mistakes.
- [ ] Key: 72 % exact on a house stick; check relative/fifth confusions.
- [ ] RGB waveform green band fit is weak (0.45).

//! A track's phrases for the screen: rekordbox's phrase analysis (a PSSI
//! section, from a stick's `.EXT` or OpenDeck's own analysis) turned into
//! coloured, labelled spans of the track, which the CDJ-3000 screen draws
//! under the overall waveform when WAVEFORM DIVISIONS is PHRASE.

use opendeck_analysis::phrase::{parse_pssi, Mood, Phrase};

/// One phrase on the track: where it starts and ends (interleaved sample
/// positions, as every other position on the deck), its label as rekordbox
/// names it, and how it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct PhraseSpan {
    pub start: u64,
    pub end:   u64,
    pub label: String,
    pub rgb:   [u8; 3],
    /// Labels are black on the lighter mid / low mood colours, white on the
    /// high mood's.
    pub dark_text: bool,
    /// A fill-in ends the phrase, from here to its end.
    pub fill: Option<u64>,
}

/// Where the phrases come from, for which wins: the ones that came with the
/// load (a stick's rekordbox analysis, or a cached analysis of the file)
/// stay; otherwise OpenDeck's own arrive with the whole track's analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source { Rekordbox, OpenDeck }

/// The spans of a PSSI section.  `beat_ms(n)` is the time of beat `n`
/// (1-based, as the section counts them) in the grid the analysis was made
/// on; `track_end` closes the last phrase when its end beat is past the
/// grid.  Positions come out in interleaved samples at `sr` / `ch`.  An
/// unreadable section, or one with no phrases, gives no spans.
pub fn spans(pssi: &[u8], beat_ms: impl Fn(u16) -> Option<u32>, track_end: u64, sr: u32, ch: u16) -> Vec<PhraseSpan> {
    let Ok(song) = parse_pssi(pssi) else { return Vec::new() };
    let at = |beat: u16| beat_ms(beat).map(|ms| ms as u64 * sr as u64 / 1000 * ch as u64);
    let mut out = Vec::new();
    for (i, p) in song.phrases.iter().enumerate() {
        let Some(start) = at(p.beat) else { continue };
        let end_beat = song.phrases.get(i + 1).map_or(song.end_beat, |n| n.beat);
        let end = at(end_beat).unwrap_or(track_end).min(track_end.max(start));
        if end <= start { continue; }
        let (rgb, dark_text) = look(p, song.mood);
        let fill = (p.fill != 0).then(|| at(p.beat_fill)).flatten().filter(|f| *f > start && *f < end);
        out.push(PhraseSpan { start, end, label: p.label(song.mood), rgb, dark_text, fill });
    }
    out
}

/// A phrase's colour, as rekordbox paints it: the palette Beat Link uses
/// (Deep Symmetry's `Util.phraseColor`, EPL-2.0; the values only), by mood
/// and kind.  Text is white on the high mood's colours, black otherwise.
pub fn look(p: &Phrase, mood: Mood) -> ([u8; 3], bool) {
    let label = p.label(mood);
    let rgb = match (mood, label.as_str()) {
        (Mood::High, "Intro 1")  => [200, 0, 0],
        (Mood::High, "Intro 2")  => [200, 50, 0],
        (Mood::High, "Up 1")     => [140, 50, 255],
        (Mood::High, "Up 2")     => [105, 50, 255],
        (Mood::High, "Up 3")     => [90, 50, 255],
        (Mood::High, "Down")     => [155, 115, 45],
        (Mood::High, "Chorus 1" | "Chorus 2") => [15, 170, 0],
        (Mood::High, "Outro 1")  => [80, 135, 195],
        (Mood::High, "Outro 2")  => [95, 135, 175],
        (Mood::Mid, "Intro")     => [225, 70, 70],
        (Mood::Mid, "Verse 1")   => [80, 110, 255],
        (Mood::Mid, "Verse 2")   => [80, 85, 255],
        (Mood::Mid, "Verse 3")   => [100, 80, 255],
        (Mood::Mid, "Verse 4")   => [120, 80, 255],
        (Mood::Mid, "Verse 5")   => [140, 80, 255],
        (Mood::Mid, "Verse 6")   => [160, 80, 255],
        (Mood::Mid, "Bridge")    => [225, 215, 65],
        (Mood::Mid, "Chorus")    => [120, 195, 125],
        (Mood::Mid, "Outro")     => [115, 130, 150],
        (Mood::Low, "Intro")     => [255, 170, 180],
        (Mood::Low, "Verse 1")   => [165, 160, 255],
        (Mood::Low, "Verse 2")   => [190, 160, 255],
        (Mood::Low, "Bridge")    => [255, 250, 165],
        (Mood::Low, "Chorus")    => [185, 225, 185],
        (Mood::Low, "Outro")     => [145, 160, 180],
        _                        => [255, 255, 255],
    };
    (rgb, mood != Mood::High)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeck_analysis::phrase::{encode_pssi, SongStructure};

    fn phrase(beat: u16, kind: u16, k1: u8) -> Phrase { Phrase { beat, kind, k1, ..Default::default() } }

    /// A high-mood track at 120 BPM (a beat every 500 ms, beat 1 at 0):
    /// Intro 1 for 32 beats, Up 1, Chorus 1 with a fill-in at its last
    /// 4 beats, Outro 1 to beat 129.  Each span runs from its beat to the
    /// next phrase's, in interleaved samples at 48 kHz stereo, with the
    /// rekordbox colour and label; masked as rekordbox 6+ writes it.
    #[test]
    fn a_high_mood_track_becomes_coloured_spans() {
        let mut chorus = phrase(65, 5, 1);
        chorus.fill = 1;
        chorus.beat_fill = 93;
        let song = SongStructure {
            mood: Mood::High, end_beat: 129,
            phrases: vec![phrase(1, 1, 1), phrase(33, 2, 0), chorus, phrase(97, 6, 1)],
            ..Default::default()
        };
        let beat_ms = |n: u16| (n >= 1 && n <= 200).then(|| (n as u32 - 1) * 500);
        let s = spans(&encode_pssi(&song, true), beat_ms, u64::MAX, 48_000, 2);
        let at = |beat: u32| (beat - 1) as u64 * 500 * 48 * 2;   // ms → interleaved samples
        assert_eq!(s.len(), 4);
        assert_eq!(s.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["Intro 1", "Up 1", "Chorus 1", "Outro 1"]);
        assert_eq!((s[0].start, s[0].end), (0, at(33)));
        assert_eq!((s[3].start, s[3].end), (at(97), at(129)));
        assert_eq!(s[0].rgb, [200, 0, 0]);
        assert_eq!(s[2].rgb, [15, 170, 0]);
        assert_eq!(s[2].fill, Some(at(93)), "the fill-in from beat 93");
        assert!(s.iter().all(|p| !p.dark_text), "white labels on the high mood");
    }

    /// The last phrase stops at the track's end when its end beat is past
    /// the grid; a section that does not parse gives nothing.
    #[test]
    fn the_last_phrase_stops_at_the_track_end_and_junk_gives_nothing() {
        let song = SongStructure { mood: Mood::Low, end_beat: 999, phrases: vec![phrase(1, 1, 0), phrase(17, 9, 0)], ..Default::default() };
        let beat_ms = |n: u16| (n >= 1 && n <= 64).then(|| (n as u32 - 1) * 500);
        let end = 40 * 48_000 * 2;
        let s = spans(&encode_pssi(&song, false), beat_ms, end, 48_000, 2);
        assert_eq!(s.last().map(|p| (p.label.as_str(), p.end)), Some(("Chorus", end)));
        assert!(s.iter().all(|p| p.dark_text), "black labels on the low mood's light colours");
        assert_eq!(s[0].rgb, [255, 170, 180]);
        assert!(spans(b"not a pssi section at all......................", beat_ms, end, 48_000, 2).is_empty());
    }
}

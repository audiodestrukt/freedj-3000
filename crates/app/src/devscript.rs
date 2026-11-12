//! `OPENDECK_SCRIPT`: drive the deck from a timed list of actions, for
//! end-to-end tests that cannot reach the window (no keyboard, no touch).
//!
//! `OPENDECK_SCRIPT="4:sync; 4:jogmode=cdj; 12:jog=2x30; 22:shiftsync; 32:quit"`
//!
//! Each entry is `<seconds after start>:<action>`:
//!
//! | action | does |
//! |---|---|
//! | `sync` / `unsync` | SYNC on / off |
//! | `play` | PLAY |
//! | `jogmode=cdj` / `jogmode=vinyl` | JOG MODE (not saved) |
//! | `jog=<delta>x<frames>` | one jog delta per frame for that many frames |
//! | `shiftsync` / `snapgrid` / `gridreset` | the GRID ADJUST actions |
//! | `screen=browse` (`taglist`, `menu`, `source`, `history`, `perform`, `perform-stems`, `playback`) | open that screen (`perform-stems`: PERFORM with the STEMS pads) |
//! | `stems=<drums>,<vocal>,<harmonics>` | set the stem levels (0..1) |
//! | `open=<row name>` | BROWSE: highlight the row with that name (or the first starting with it) and enter it |
//! | `select=<n>` | BROWSE: highlight row n |
//! | `category=<n>` | BROWSE on rekordbox: open category n (0 = ARTIST) |
//! | `search=<text>` | open SEARCH and type the text |
//! | `press=<BUTTON>` / `press=<BUTTON>:<ms>` | press a CDJ-3000 button by cdj3k-emu's name (`HOT_A`, `BEATLOOP_HALF`, `MENU`…), released after `ms` (200); 1000 ms or more is a long press |
//! | `hold=<BUTTON>` / `release=<BUTTON>` | hold a button until released (CALL_DELETE + a pad deletes it) |
//! | `rotary=<n>` | turn the rotary `n` detents (positive = down the list) |
//! | `lamps=<path>` | write the faceplate's lamps as cdj3k-emu reports them (JSON) |
//! | `shot=<path>` | save the next frame as a PNG and carry on |
//! | `quit` | exit the process (status 0) |
//!
//! The button actions use the emulator's names on purpose: one scenario
//! drives freedj and the emulated unit alike (`tools/cdj_parity.py`).

/// One parsed action.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Sync(bool),
    Play,
    JogVinyl(bool),
    Jog { delta: i32, frames: u32 },
    ShiftSync,
    SnapGrid,
    GridReset,
    Screen(String),
    Open(String),
    Select(usize),
    Category(usize),
    Search(String),
    Press { button: String, ms: u32 },
    Hold(String),
    Release(String),
    Rotary(i32),
    Lamps(String),
    Shot(String),
    /// Stem levels: drums, vocal, harmonics (0..1).
    Stems([f32; 3]),
    Quit,
}

/// Parse a script; entries are sorted by time. Errors name the bad entry.
pub fn parse(src: &str) -> Result<Vec<(f64, Action)>, String> {
    let mut out = Vec::new();
    for entry in src.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let (t, act) = entry.split_once(':').ok_or_else(|| format!("{entry:?}: want <secs>:<action>"))?;
        let t: f64 = t.trim().parse().map_err(|_| format!("{entry:?}: bad time"))?;
        let act = act.trim();
        let a = match act {
            "sync" => Action::Sync(true),
            "unsync" => Action::Sync(false),
            "play" => Action::Play,
            "jogmode=cdj" => Action::JogVinyl(false),
            "jogmode=vinyl" => Action::JogVinyl(true),
            "shiftsync" => Action::ShiftSync,
            "snapgrid" => Action::SnapGrid,
            "gridreset" => Action::GridReset,
            "quit" => Action::Quit,
            _ if act.starts_with("screen=") => Action::Screen(act[7..].to_string()),
            _ if act.starts_with("open=") => Action::Open(act[5..].to_string()),
            _ if act.starts_with("search=") => Action::Search(act[7..].to_string()),
            _ if act.starts_with("select=") => Action::Select(act[7..].parse().map_err(|_| format!("{entry:?}: bad row"))?),
            _ if act.starts_with("press=") => {
                let spec = &act[6..];
                let (button, ms) = match spec.split_once(':') {
                    Some((b, ms)) => (b, ms.trim().parse().map_err(|_| format!("{entry:?}: bad hold time"))?),
                    None => (spec, 200),
                };
                Action::Press { button: button.trim().to_ascii_uppercase(), ms }
            }
            _ if act.starts_with("hold=") => Action::Hold(act[5..].trim().to_ascii_uppercase()),
            _ if act.starts_with("release=") => Action::Release(act[8..].trim().to_ascii_uppercase()),
            _ if act.starts_with("rotary=") => Action::Rotary(act[7..].trim().parse().map_err(|_| format!("{entry:?}: bad detents"))?),
            _ if act.starts_with("lamps=") => Action::Lamps(act[6..].trim().to_string()),
            _ if act.starts_with("shot=") => Action::Shot(act[5..].trim().to_string()),
            _ if act.starts_with("stems=") => {
                let g: Vec<f32> = act[6..].split(',').map(|v| v.trim().parse::<f32>()).collect::<Result<_, _>>()
                    .map_err(|_| format!("{entry:?}: want stems=<drums>,<vocal>,<harmonics>"))?;
                let g: [f32; 3] = g.try_into().map_err(|_| format!("{entry:?}: want three stem levels"))?;
                Action::Stems(g)
            }
            _ if act.starts_with("category=") => Action::Category(act[9..].parse().map_err(|_| format!("{entry:?}: bad category"))?),
            _ => {
                let spec = act.strip_prefix("jog=").ok_or_else(|| format!("{entry:?}: unknown action"))?;
                let (d, n) = spec.split_once('x').ok_or_else(|| format!("{entry:?}: want jog=<delta>x<frames>"))?;
                Action::Jog {
                    delta: d.trim().parse().map_err(|_| format!("{entry:?}: bad delta"))?,
                    frames: n.trim().parse().map_err(|_| format!("{entry:?}: bad frame count"))?,
                }
            }
        };
        out.push((t, a));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(out)
}

/// The script plus the jog deltas still to feed, one per frame.
#[derive(Default)]
pub struct Script {
    pending: std::collections::VecDeque<(f64, Action)>,
    jog: Option<(i32, u32)>,
}

impl Script {
    pub fn from_env() -> Option<Self> {
        let src = std::env::var("OPENDECK_SCRIPT").ok()?;
        match parse(&src) {
            Ok(v) => { log::info!("script: {} actions", v.len()); Some(Self { pending: v.into(), jog: None }) }
            Err(e) => { log::error!("OPENDECK_SCRIPT: {e}"); None }
        }
    }

    /// Queue an action for `at` seconds after the start (a press's release).
    pub fn later(&mut self, at: f64, a: Action) {
        let i = self.pending.iter().position(|(t, _)| *t > at).unwrap_or(self.pending.len());
        self.pending.insert(i, (at, a));
    }

    /// The actions due by `elapsed` seconds, plus this frame's jog delta.
    pub fn step(&mut self, elapsed: f64) -> (Vec<Action>, Option<i32>) {
        let mut due = Vec::new();
        while self.pending.front().is_some_and(|(t, _)| *t <= elapsed) {
            let (_, a) = self.pending.pop_front().unwrap();
            if let Action::Jog { delta, frames } = a { self.jog = Some((delta, frames)); }
            due.push(a);
        }
        let jog = match self.jog {
            Some((d, n)) if n > 0 => { self.jog = if n > 1 { Some((d, n - 1)) } else { None }; Some(d) }
            _ => None,
        };
        (due, jog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sorts() {
        let v = parse("12:jog=-3x4; 4:sync ;4:jogmode=cdj;30:quit").unwrap();
        assert_eq!(v, vec![
            (4.0, Action::Sync(true)), (4.0, Action::JogVinyl(false)),
            (12.0, Action::Jog { delta: -3, frames: 4 }), (30.0, Action::Quit),
        ]);
        assert!(parse("4 sync").is_err());
        assert!(parse("4:dance").is_err());
        assert!(parse("4:jog=2").is_err());
    }

    #[test]
    fn parses_the_browse_and_grid_actions() {
        let v: Vec<Action> = parse("1:screen=browse; 2:open=rekordbox; 3:select=4; 4:category=2; 5:search=ALO B; \
                                    6:shiftsync; 7:snapgrid; 8:gridreset; 9:unsync; 10:play; 11:jogmode=vinyl")
            .unwrap().into_iter().map(|(_, a)| a).collect();
        assert_eq!(v, vec![
            Action::Screen("browse".into()), Action::Open("rekordbox".into()), Action::Select(4),
            Action::Category(2), Action::Search("ALO B".into()), Action::ShiftSync, Action::SnapGrid,
            Action::GridReset, Action::Sync(false), Action::Play, Action::JogVinyl(true),
        ]);
        assert!(parse("1:select=x").is_err());
        assert!(parse("1:category=").is_err());
    }

    #[test]
    fn parses_the_button_actions() {
        let v: Vec<Action> = parse("1:press=hot_a; 2:press=MENU:1500; 3:hold=CALL_DELETE; 4:release=call_delete; \
                                    5:rotary=-3; 6:lamps=/tmp/l.json; 7:shot=/tmp/s.png")
            .unwrap().into_iter().map(|(_, a)| a).collect();
        assert_eq!(v, vec![
            Action::Press { button: "HOT_A".into(), ms: 200 }, Action::Press { button: "MENU".into(), ms: 1500 },
            Action::Hold("CALL_DELETE".into()), Action::Release("CALL_DELETE".into()), Action::Rotary(-3),
            Action::Lamps("/tmp/l.json".into()), Action::Shot("/tmp/s.png".into()),
        ]);
        assert!(parse("1:press=PLAY:soon").is_err());
        assert!(parse("1:rotary=x").is_err());
        let mut s = Script { pending: parse("1:play; 3:quit").unwrap().into(), jog: None };
        s.later(2.0, Action::Release("CUE".into()));
        let order: Vec<f64> = s.pending.iter().map(|(t, _)| *t).collect();
        assert_eq!(order, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn jog_feeds_one_delta_per_frame() {
        let mut s = Script { pending: parse("1:jog=2x3").unwrap().into(), jog: None };
        assert_eq!(s.step(0.5), (vec![], None));
        let (due, j) = s.step(1.0);
        assert_eq!((due.len(), j), (1, Some(2)));
        assert_eq!(s.step(1.1).1, Some(2));
        assert_eq!(s.step(1.2).1, Some(2));
        assert_eq!(s.step(1.3).1, None);
    }
}

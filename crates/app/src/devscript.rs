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
//! | `screen=browse` (`taglist`, `menu`, `source`, `history`, `playback`) | open that screen |
//! | `open=<row name>` | BROWSE: highlight the row with that name and enter it |
//! | `select=<n>` | BROWSE: highlight row n |
//! | `category=<n>` | BROWSE on rekordbox: open category n (0 = ARTIST) |
//! | `search=<text>` | open SEARCH and type the text |
//! | `quit` | exit the process (status 0) |

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

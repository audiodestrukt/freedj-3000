//! CDJ HID control carried over Pro DJ Link — the "HID via LINK" path a
//! CDJ-3000 uses in CONTROL MODE (DJM USB), as opposed to the rear USB-B
//! cable. The host (freedj, e.g. on the iPhone) advertises a mixer role on
//! the wired LAN; each CDJ tunnels its 32-byte HID input reports to the host
//! over UDP, and the host tunnels 64-byte status/display reports back.
//!
//! This module is the wire layer only, written from the CDJ-3000 / DJM-A9
//! integration brief: it shares the `Qspt1WmJOL` magic and ports 50000/50001
//! with [`crate::prodj`]. It decodes to the neutral [`CdjControl`] events in
//! this crate; the app-side surface maps those onto `ControlEvent`, the same
//! split as the MIDI and I2C surfaces.
//!
//! Three things live elsewhere: the session handshake (stages 0x11–0x15, its
//! own module), the status/display encoders (0x21–0x30), and the iOS/desktop
//! socket glue. This file is the receive half: framing, discovery, decode.

use std::net::Ipv4Addr;

/// Every Pro DJ Link packet, tunnel frames included, opens with these 10 bytes.
pub const MAGIC: [u8; 10] = *b"Qspt1WmJOL";
/// Device announcements and number inquiries.
pub const PORT_DISCOVERY: u16 = 50000;
/// The HID tunnel: routed input reports from each CDJ, reports to each CDJ.
pub const PORT_TUNNEL: u16 = 50001;

/// Low nibble of a tunnel payload's route byte; the high nibble is the player.
pub mod route {
    /// Host → CDJ: a status/display report (`[(p<<4)|0x01, HID 1..63]`).
    pub const HOST_REPORT: u8 = 0x01;
    /// Host → CDJ: USB-A availability, makes the host selectable as a source.
    pub const AVAILABILITY: u8 = 0x05;
    /// CDJ → Host: an input report (`[(p<<4)|0x09, HID 1..31]`).
    pub const CDJ_INPUT: u8 = 0x09;
}

const NAME_LEN: usize = 20;
/// UDP-payload offset at which both tunnel directions place their routed HID.
const TUNNEL_PAYLOAD_OFF: usize = 36;

fn name_field(name: &str) -> [u8; NAME_LEN] {
    let mut n = [0u8; NAME_LEN];
    let b = name.as_bytes();
    let k = b.len().min(NAME_LEN);
    n[..k].copy_from_slice(&b[..k]);
    n
}

fn has_magic(p: &[u8]) -> bool {
    p.len() >= 11 && p[..10] == MAGIC
}

// ── Discovery (port 50000) ───────────────────────────────────────────────

/// A peer learned from a type-0x06 announcement on the discovery port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub player: u8,
    pub name: String,
    pub mac: [u8; 6],
    pub ip: Ipv4Addr,
}

/// Parse a player (CDJ) announcement: type 0x06, name at 12, player at 36,
/// MAC at 38, IPv4 at 44. Returns `None` for anything else, so the caller can
/// pass every datagram through. Duplicate player numbers and source-address
/// mismatches are the caller's to reject.
pub fn parse_announcement(p: &[u8]) -> Option<Peer> {
    if !has_magic(p) || p[10] != 0x06 || p.len() < 48 {
        return None;
    }
    let end = 12 + NAME_LEN;
    let name = String::from_utf8_lossy(&p[12..end])
        .trim_end_matches('\0')
        .to_string();
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&p[38..44]);
    let ip = Ipv4Addr::new(p[44], p[45], p[46], p[47]);
    Some(Peer { player: p[36], name, mac, ip })
}

/// Build the host's own announcement (role 0x21 = 33), sent to the wired
/// subnet broadcast every ~2 s. 54 bytes.
pub fn build_host_announcement(host_name: &str, mac: [u8; 6], ip: Ipv4Addr) -> Vec<u8> {
    let mut v = Vec::with_capacity(54);
    v.extend_from_slice(&MAGIC);
    v.extend_from_slice(&[0x06, 0x00]);
    v.extend_from_slice(&name_field(host_name));
    v.extend_from_slice(&[0x01, 0x02, 0x00, 0x36, 0x21, 0x02]);
    v.extend_from_slice(&mac);
    v.extend_from_slice(&ip.octets());
    v.extend_from_slice(&[0x03, 0x00, 0x00, 0x00, 0x03, 0x31]);
    debug_assert_eq!(v.len(), 54);
    v
}

/// Is this a number inquiry (type 0x04) from a peer? Returns the counter byte
/// (offset 37) to echo back in [`build_number_reply`].
pub fn parse_number_inquiry(p: &[u8]) -> Option<u8> {
    (has_magic(p) && p[10] == 0x04 && p.len() > 37).then(|| p[37])
}

/// Reply to a number inquiry, echoing its counter. 38 bytes.
pub fn build_number_reply(host_name: &str, counter: u8) -> Vec<u8> {
    let mut v = Vec::with_capacity(38);
    v.extend_from_slice(&MAGIC);
    v.extend_from_slice(&[0x05, 0x00]);
    v.extend_from_slice(&name_field(host_name));
    v.extend_from_slice(&[0x01, 0x02, 0x00, 0x26, 0x21, counter]);
    debug_assert_eq!(v.len(), 38);
    v
}

// ── Tunnel (port 50001) ──────────────────────────────────────────────────

/// Extract the routed 32-byte HID payload of a CDJ→host tunnel frame
/// (68 bytes: magic, 0x04, name, `01 00`, player, `00 20`, payload32).
/// Returns the sender's player number and the payload, whose byte 0 is the
/// route and byte 1 the command (0x20 input, 0xF0 fragmented display ack).
pub fn parse_cdj_tunnel(p: &[u8]) -> Option<(u8, &[u8])> {
    if !has_magic(p) || p[10] != 0x04 || p.len() < TUNNEL_PAYLOAD_OFF + 32 {
        return None;
    }
    let player = p[33]; // magic(10)+0x04(1)+name(20)+01 00 => player at 33
    let payload = &p[TUNNEL_PAYLOAD_OFF..TUNNEL_PAYLOAD_OFF + 32];
    Some((player, payload))
}

/// Wrap a 64-byte host report in a host→CDJ tunnel frame addressed to
/// `host_name` (the advertised mixer role). 100 bytes. The caller sets the
/// payload's route byte; this only frames it.
pub fn build_host_tunnel(host_name: &str, payload64: &[u8; 64]) -> Vec<u8> {
    let mut v = Vec::with_capacity(100);
    v.extend_from_slice(&MAGIC);
    v.push(0x04);
    v.extend_from_slice(&name_field(host_name));
    v.extend_from_slice(&[0x01, 0x00, 0x21, 0x00, 0x40]);
    v.extend_from_slice(payload64);
    debug_assert_eq!(v.len(), 100);
    v
}

/// The low-rate frames the host repeats to keep a session up and selectable.
pub mod host_frames {
    use super::route;

    /// `[(p<<4)|0x05, 0x21, 62 zero]` — advertise USB-A availability (~2 s).
    pub fn availability(player: u8) -> [u8; 64] {
        let mut r = [0u8; 64];
        r[0] = (player << 4) | route::AVAILABILITY;
        r[1] = 0x21;
        r
    }

    /// `[(p<<4)|0x01, 63 zero]` — empty keepalive (~400 ms).
    pub fn keepalive(player: u8) -> [u8; 64] {
        let mut r = [0u8; 64];
        r[0] = (player << 4) | route::HOST_REPORT;
        r
    }
}

// ── Input report (command 0x20) decode ───────────────────────────────────

/// A momentary control that reports as one bit. Offsets/masks per the brief's
/// page-4 table, inside the 32-byte routed payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdjButton {
    Play,
    Cue,
    LoopIn,
    LoopOut,
    Reloop,
    JogTouch,
    Vinyl,
    MasterTempo,
    TempoRange,
    TempoReset,
    Back,
    /// Rotary encoder push (0x05 bit 0x08).
    RotaryPush,
    Quantize,
    Shift,
    BeatJumpFwd,
    BeatJumpRev,
}

/// (byte offset, bit mask, button) for every single-bit control.
const BUTTON_BITS: &[(usize, u8, CdjButton)] = &[
    (0x02, 0x80, CdjButton::Play),
    (0x02, 0x40, CdjButton::Cue),
    (0x03, 0x80, CdjButton::LoopIn),
    (0x03, 0x40, CdjButton::LoopOut),
    (0x03, 0x20, CdjButton::Reloop),
    (0x04, 0x10, CdjButton::JogTouch),
    (0x04, 0x80, CdjButton::Vinyl),
    (0x04, 0x04, CdjButton::MasterTempo),
    (0x04, 0x08, CdjButton::TempoRange),
    (0x04, 0x02, CdjButton::TempoReset),
    (0x05, 0x04, CdjButton::Back),
    (0x05, 0x08, CdjButton::RotaryPush),
    (0x05, 0x40, CdjButton::Quantize),
    (0x06, 0x04, CdjButton::Shift),
    (0x07, 0x80, CdjButton::BeatJumpFwd),
    (0x07, 0x40, CdjButton::BeatJumpRev),
];

/// Loop-length popover, byte 0x08: 0x80..0x04 = 32,16,8,4,2,1 beats; plus
/// byte 0x09 0x10/0x08 = 1/4, 1/2.
const LOOP_LEN_08: &[(u8, f32)] =
    &[(0x80, 32.0), (0x40, 16.0), (0x20, 8.0), (0x10, 4.0), (0x08, 2.0), (0x04, 1.0)];
const LOOP_LEN_09: &[(u8, f32)] = &[(0x10, 0.25), (0x08, 0.5)];

/// Touchscreen beat-jump popover, bytes 0x0D (forward) / 0x0E (backward):
/// 0x80..0x02 = 1,2,4,8,16,32,64 beats.
const JUMP_SIZE: &[(u8, f32)] =
    &[(0x80, 1.0), (0x40, 2.0), (0x20, 4.0), (0x10, 8.0), (0x08, 16.0), (0x04, 32.0), (0x02, 64.0)];

fn u16le(p: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([p[o], p[o + 1]])
}
fn i16le(p: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([p[o], p[o + 1]])
}

/// A neutral, edge-detected control event decoded from a CDJ input report.
/// The app's surface layer maps these onto `ControlEvent`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CdjControl {
    /// A momentary button changed state.
    Button { btn: CdjButton, pressed: bool },
    /// A hot-cue pad A..H (slot 0..7) changed state (byte 0x0F).
    HotCue { slot: u8, pressed: bool },
    /// A loop-length control was pressed (not released).
    LoopLength { beats: f32 },
    /// A beat-jump-size control was pressed; `forward` picks the direction.
    BeatJumpSize { beats: f32, forward: bool },
    /// The deck's own browse list opened or closed (0x05 bit 0x80). This is a
    /// level, not a key: on cdj3k-emu in Control Mode the BROWSE key flips it,
    /// BACK from the playback screen sets it, SOURCE clears it.
    BrowseView { on: bool },
    /// Browse encoder moved; wrapping signed delta of the 0x13 counter.
    Browse { delta: i32 },
    /// Tempo fader raw signed value (0x15); span about -1000..1000, caller
    /// turns it into a rate with the deck's range.
    Tempo { raw: i16 },
    /// Jog moved; `delta` is the wrapping difference of the 0x17 position
    /// (9728 ticks/rev, wraps at 65536), `motion` the signed 0x19 field.
    Jog { delta: i32, motion: i16, position: u16 },
}

fn wrapping_u16_delta(prev: u16, cur: u16) -> i32 {
    (cur.wrapping_sub(prev) as i16) as i32
}

/// Stateful decoder: holds the previous report so button edges and rotary
/// deltas are reported once. Feed each command-0x20 routed payload.
#[derive(Default)]
pub struct InputDecoder {
    prev: Option<[u8; 32]>,
}

impl InputDecoder {
    pub fn new() -> Self {
        Self { prev: None }
    }

    /// Decode one 32-byte routed input payload (byte 1 must be 0x20) into the
    /// events that changed since the previous report. The first report after
    /// construction only seeds state (no spurious press edges), except analog
    /// values, which are reported so the UI can initialise.
    pub fn decode(&mut self, p: &[u8]) -> Vec<CdjControl> {
        let mut out = Vec::new();
        if p.len() < 32 || p[1] != 0x20 {
            return out;
        }
        let cur: [u8; 32] = p[..32].try_into().unwrap();
        let first = self.prev.is_none();
        let prev = self.prev.unwrap_or([0u8; 32]);

        // Single-bit buttons: emit on an edge.
        for &(off, mask, btn) in BUTTON_BITS {
            let now = cur[off] & mask != 0;
            let was = prev[off] & mask != 0;
            if now != was && !(first && !now) {
                out.push(CdjControl::Button { btn, pressed: now });
            }
        }
        // Browse-view level: report every change, and the initial state so
        // the app starts in step with the deck.
        let view = cur[0x05] & 0x80 != 0;
        if (first && view) || (!first && view != (prev[0x05] & 0x80 != 0)) {
            out.push(CdjControl::BrowseView { on: view });
        }
        // Hot cues A..H, byte 0x0F, 0x80 down to 0x01.
        for slot in 0..8u8 {
            let mask = 0x80u8 >> slot;
            let now = cur[0x0F] & mask != 0;
            let was = prev[0x0F] & mask != 0;
            if now != was && !(first && !now) {
                out.push(CdjControl::HotCue { slot, pressed: now });
            }
        }
        // Loop length + physical 4/8-beat: fire on the press edge only.
        for &(mask, beats) in LOOP_LEN_08 {
            if cur[0x08] & mask != 0 && prev[0x08] & mask == 0 {
                out.push(CdjControl::LoopLength { beats });
            }
        }
        for &(mask, beats) in LOOP_LEN_09 {
            if cur[0x09] & mask != 0 && prev[0x09] & mask == 0 {
                out.push(CdjControl::LoopLength { beats });
            }
        }
        if cur[0x09] & 0x04 != 0 && prev[0x09] & 0x04 == 0 {
            out.push(CdjControl::LoopLength { beats: 4.0 });
        }
        if cur[0x0A] & 0x04 != 0 && prev[0x0A] & 0x04 == 0 {
            out.push(CdjControl::LoopLength { beats: 8.0 });
        }
        // Touchscreen beat-jump popovers, press edge.
        for (off, forward) in [(0x0D, true), (0x0E, false)] {
            for &(mask, beats) in JUMP_SIZE {
                if cur[off] & mask != 0 && prev[off] & mask == 0 {
                    out.push(CdjControl::BeatJumpSize { beats, forward });
                }
            }
        }
        // Analog fields.
        let browse = u16le(&cur, 0x13);
        if first {
            // seed only; deltas start from here
        } else {
            let d = wrapping_u16_delta(u16le(&prev, 0x13), browse);
            if d != 0 {
                out.push(CdjControl::Browse { delta: d });
            }
        }
        let tempo = i16le(&cur, 0x15);
        if first || tempo != i16le(&prev, 0x15) {
            out.push(CdjControl::Tempo { raw: tempo });
        }
        let pos = u16le(&cur, 0x17);
        let motion = i16le(&cur, 0x19);
        if first {
            // seed
        } else {
            let d = wrapping_u16_delta(u16le(&prev, 0x17), pos);
            if d != 0 || motion != 0 {
                out.push(CdjControl::Jog { delta: d, motion, position: pos });
            }
        }

        self.prev = Some(cur);
        out
    }
}

/// Tempo raw (-1000..1000) to a playback rate for the given range percent,
/// per the brief: `rate = 1 + raw/1000 * rangePercent/100`.
pub fn tempo_rate(raw: i16, range_percent: f32) -> f32 {
    1.0 + (raw as f32 / 1000.0) * (range_percent / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_report(fill: impl Fn(&mut [u8; 32])) -> [u8; 32] {
        let mut r = [0u8; 32];
        r[0] = (1 << 4) | route::CDJ_INPUT;
        r[1] = 0x20;
        fill(&mut r);
        r
    }

    #[test]
    fn browse_key_is_a_level_and_rotary_push_is_a_button() {
        // Read off cdj3k-emu in Control Mode: the rotary push is 0x05/0x08,
        // the BROWSE key flips 0x05/0x80 and leaves it set.
        let mut d = InputDecoder::new();
        d.decode(&input_report(|_| {})); // seeds state (and reports tempo)
        assert_eq!(
            d.decode(&input_report(|r| r[0x05] = 0x08)),
            vec![CdjControl::Button { btn: CdjButton::RotaryPush, pressed: true }]
        );
        d.decode(&input_report(|_| {}));
        assert_eq!(d.decode(&input_report(|r| r[0x05] = 0x80)), vec![CdjControl::BrowseView { on: true }]);
        // Holding the level reports nothing more; BACK on top is just BACK.
        assert!(d.decode(&input_report(|r| r[0x05] = 0x80)).is_empty());
        assert_eq!(
            d.decode(&input_report(|r| r[0x05] = 0x84)),
            vec![CdjControl::Button { btn: CdjButton::Back, pressed: true }]
        );
        d.decode(&input_report(|r| r[0x05] = 0x80));
        assert_eq!(d.decode(&input_report(|_| {})), vec![CdjControl::BrowseView { on: false }]);
        // A session that starts with the deck already browsing says so.
        let mut d = InputDecoder::new();
        assert!(d.decode(&input_report(|r| r[0x05] = 0x80)).contains(&CdjControl::BrowseView { on: true }));
    }

    #[test]
    fn host_announcement_shape() {
        let a = build_host_announcement("DJM-A9", [1, 2, 3, 4, 5, 6], Ipv4Addr::new(169, 254, 1, 2));
        assert_eq!(a.len(), 54);
        assert_eq!(&a[..10], &MAGIC);
        assert_eq!(a[10], 0x06);
        assert_eq!(&a[12..18], b"DJM-A9");
        assert_eq!(a[36], 0x21); // host role
    }

    #[test]
    fn round_trip_announcement_parse() {
        // A player announcement we build by hand, then parse.
        let mut p = vec![0u8; 48];
        p[..10].copy_from_slice(&MAGIC);
        p[10] = 0x06;
        p[12..18].copy_from_slice(b"CDJ-30");
        p[36] = 3;
        p[38..44].copy_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        p[44..48].copy_from_slice(&[169, 254, 9, 9]);
        let peer = parse_announcement(&p).unwrap();
        assert_eq!(peer.player, 3);
        assert_eq!(peer.name, "CDJ-30");
        assert_eq!(peer.mac, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert_eq!(peer.ip, Ipv4Addr::new(169, 254, 9, 9));
    }

    #[test]
    fn number_reply_echoes_counter() {
        let r = build_number_reply("DJM-A9", 0x7a);
        assert_eq!(r.len(), 38);
        assert_eq!(*r.last().unwrap(), 0x7a);
    }

    #[test]
    fn tunnel_extracts_player_and_payload() {
        let payload = input_report(|r| r[0x02] = 0x80);
        let mut frame = Vec::new();
        frame.extend_from_slice(&MAGIC);
        frame.push(0x04);
        frame.extend_from_slice(&name_field("CDJ-3000"));
        frame.extend_from_slice(&[0x01, 0x00, 3, 0x00, 0x20]);
        frame.extend_from_slice(&payload);
        let (player, got) = parse_cdj_tunnel(&frame).unwrap();
        assert_eq!(player, 3);
        assert_eq!(got, &payload);
    }

    #[test]
    fn host_tunnel_frames_64_byte_report() {
        let mut rep = [0u8; 64];
        rep[0] = (1 << 4) | route::HOST_REPORT;
        rep[1] = 0x21;
        let f = build_host_tunnel("DJM-A9", &rep);
        assert_eq!(f.len(), 100);
        assert_eq!(&f[36..100], &rep);
    }

    #[test]
    fn button_edges_once() {
        let mut d = InputDecoder::new();
        assert!(d.decode(&input_report(|_| {})).iter().all(|e| !matches!(
            e,
            CdjControl::Button { .. }
        )));
        let play = d.decode(&input_report(|r| r[0x02] = 0x80));
        assert!(play.contains(&CdjControl::Button { btn: CdjButton::Play, pressed: true }));
        // held: no repeat
        let held = d.decode(&input_report(|r| r[0x02] = 0x80));
        assert!(!held.iter().any(|e| matches!(e, CdjControl::Button { btn: CdjButton::Play, .. })));
        let rel = d.decode(&input_report(|_| {}));
        assert!(rel.contains(&CdjControl::Button { btn: CdjButton::Play, pressed: false }));
    }

    #[test]
    fn hot_cue_slots() {
        let mut d = InputDecoder::new();
        d.decode(&input_report(|_| {}));
        let a = d.decode(&input_report(|r| r[0x0F] = 0x80));
        assert!(a.contains(&CdjControl::HotCue { slot: 0, pressed: true }));
        let h = d.decode(&input_report(|r| r[0x0F] = 0x01));
        assert!(h.contains(&CdjControl::HotCue { slot: 7, pressed: true }));
    }

    #[test]
    fn jog_and_tempo_deltas() {
        let mut d = InputDecoder::new();
        d.decode(&input_report(|r| {
            r[0x17..0x19].copy_from_slice(&100u16.to_le_bytes());
        }));
        let ev = d.decode(&input_report(|r| {
            r[0x17..0x19].copy_from_slice(&110u16.to_le_bytes());
            r[0x19..0x1B].copy_from_slice(&10i16.to_le_bytes());
            r[0x15..0x17].copy_from_slice(&200i16.to_le_bytes());
        }));
        assert!(ev.iter().any(|e| matches!(e, CdjControl::Jog { delta: 10, motion: 10, .. })));
        assert!(ev.contains(&CdjControl::Tempo { raw: 200 }));
    }

    #[test]
    fn jog_wraps() {
        let mut d = InputDecoder::new();
        d.decode(&input_report(|r| r[0x17..0x19].copy_from_slice(&65530u16.to_le_bytes())));
        let ev = d.decode(&input_report(|r| r[0x17..0x19].copy_from_slice(&4u16.to_le_bytes())));
        assert!(ev.iter().any(|e| matches!(e, CdjControl::Jog { delta: 10, .. })));
    }

    #[test]
    fn tempo_rate_formula() {
        assert!((tempo_rate(0, 10.0) - 1.0).abs() < 1e-6);
        assert!((tempo_rate(1000, 10.0) - 1.10).abs() < 1e-6);
        assert!((tempo_rate(-1000, 6.0) - 0.94).abs() < 1e-6);
    }
}

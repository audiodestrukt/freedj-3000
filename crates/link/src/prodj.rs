//! ProDJ Link — Pioneer CDJ/XDJ network sync protocol.
//!
//! Protocol documentation: https://djl-analysis.deepsymmetry.org/
//! Reference implementation: beat-link (Java) by Deep Symmetry
//!
//! Packet layouts below were verified byte-for-byte against traffic from
//! `prolink_virtual_cdj` (grantHarris/prolink-cpp) on 2026-08-25 and from a
//! real XDJ-1000MK2 (firmware 1.44) on 2026-08-26; both captures are pinned
//! as tests.  See docs/reference/link-test-harness.md.
//!
//! We appear on the network as player number 1–4.
//! Pioneer mixers (DJM-900NXS2 etc.) and CDJs see us as a peer.

use opendeck_types::EngineSnapshot;
use std::net::Ipv4Addr;

/// UDP ports used by ProDJ Link (per the Deep Symmetry analysis):
///   50000 — device announce / keep-alive, every 1.5 s
///   50001 — beat packets (0x28), one per beat, from every playing deck
///   50002 — status packets (0x0a), ~5/s, play state / pitch / sync flags
pub const PORT_ANNOUNCE:  u16 = 50000;
pub const PORT_BEAT:      u16 = 50001;
pub const PORT_STATUS:    u16 = 50002;

/// Packet type byte, at offset 0x0a in every packet.
pub const PKT_ANNOUNCE:   u8 = 0x06;
pub const PKT_BEAT:       u8 = 0x28;
pub const PKT_STATUS:     u8 = 0x0A;
/// Tempo-master handoff: request (to the current master, port 50001) and
/// its yield response (back to the requester, port 50001).
pub const PKT_MASTER_REQ: u8 = 0x26;
pub const PKT_MASTER_RSP: u8 = 0x27;
/// Sync control (port 50001): tell a player to sync, unsync, or become master.
pub const PKT_SYNC_CTRL:  u8 = 0x2A;
pub const SYNC_ON:        u32 = 0x10;
pub const SYNC_OFF:       u32 = 0x20;
pub const BECOME_MASTER:  u32 = 0x01;

/// Magic header present in all ProDJ Link packets: "Qspt1WmJOL".
pub const MAGIC: &[u8; 10] = b"Qspt1WmJOL";

/// Pitch field value meaning +0% (1.0×).
const PITCH_UNITY: u32 = 0x0010_0000;

/// Offsets within a beat packet (0x60 bytes).
mod beat {
    pub const NAME:      usize = 0x0b;   // 20 bytes, NUL padded
    pub const DEVICE:    usize = 0x21;
    pub const LEN:       usize = 0x22;   // u16 BE, remaining length = 0x3c
    pub const NEXT_BEAT: usize = 0x24;   // six u32 BE, ms until: next beat, 2nd beat,
                                         // next bar, 4th beat, 2nd bar, 8th beat
    pub const FILL:      usize = 0x3c;   // 24 × 0xFF
    pub const PITCH:     usize = 0x54;   // u32 BE, 0x00100000 = +0%
    pub const BPM:       usize = 0x5a;   // u16 BE, ×100
    pub const BEAT:      usize = 0x5c;   // beat within bar, 1–4
    pub const DEVICE2:   usize = 0x5f;
    pub const SIZE:      usize = 0x60;
}

/// Offsets within a CDJ status packet (0x0a).  Length varies by generation
/// (0xd0 / 0xd4 / 0x11c / 0x124); an XDJ-1000MK2 on firmware 1.44 sends 0x124.
mod status {
    pub const DEVICE:    usize = 0x21;
    pub const LEN:       usize = 0x22;   // u16 BE, remaining length
    pub const ACTIVITY:  usize = 0x27;   // 0 idle, 1 active
    pub const SLOT:      usize = 0x29;   // 1 CD, 2 SD, 3 USB, 4 rekordbox, 6 streaming
    pub const TRACK_TYPE: usize = 0x2a;  // 0 none, 1 rekordbox, 2 unanalysed, 5 CD
    pub const PLAY:      usize = 0x7b;   // P1, see PlayState
    pub const FIRMWARE:  usize = 0x7c;   // 4 ASCII bytes
    pub const SYNC_N:    usize = 0x84;   // u32 BE sync counter: a new master sets largest-seen + 1
    pub const FLAGS:     usize = 0x89;   // bit6 play, bit5 master, bit4 sync, bit3 on-air
    pub const PITCH:     usize = 0x8c;   // u32 BE, 0x00100000 = +0%
    pub const BPM:       usize = 0x92;   // u16 BE ×100, 0xffff = no track
    pub const MASTER:    usize = 0x9e;   // Mm: 0 not master, 1 master, 2 master (tempo invalid)
    pub const HANDOFF:   usize = 0x9f;   // Mh: player being handed master to, else 0xff
    pub const BEAT:      usize = 0xa0;   // u32 BE, 0xffffffff = none
    pub const CUE_CD:    usize = 0xa4;   // u16 BE bars to next cue, 0x1ff = none
    pub const BEAT_BAR:  usize = 0xa6;   // 1–4, 0 = none
    pub const COUNTER:   usize = 0xc8;   // u32 BE packet counter
    pub const MIN_SIZE:  usize = 0xcc;
}

/// Offsets within an announce / keep-alive packet (0x36 bytes).
mod announce {
    pub const NAME:    usize = 0x0c;   // 20 bytes
    pub const LEN:     usize = 0x22;   // u16 BE = 0x36
    pub const DEVICE:  usize = 0x24;
    pub const MAC:     usize = 0x26;
    pub const IP:      usize = 0x2c;
    pub const SIZE:    usize = 0x36;
}

/// Play state byte P1 of a status packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    NoTrack, Loading, Playing, Looping, Paused, CuedPaused, CuePlay, CueScratch,
    Searching, Ended, EmergencyLoop, Other(u8),
}

impl PlayState {
    fn from_byte(b: u8) -> Self {
        use PlayState::*;
        match b {
            0x00 => NoTrack, 0x02 => Loading, 0x03 => Playing, 0x04 => Looping,
            0x05 => Paused, 0x06 => CuedPaused, 0x07 => CuePlay, 0x08 => CueScratch,
            0x09 => Searching, 0x11 => Ended, 0x12 => EmergencyLoop, o => Other(o),
        }
    }
}

/// Everything a status packet tells us about a deck.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Status {
    pub player:       u8,
    pub play:         PlayState,
    pub playing:      bool,     // F bit 6
    pub master:       bool,     // F bit 5
    pub sync:         bool,     // F bit 4
    pub on_air:       bool,     // F bit 3
    /// 1.0 = +0%.
    pub pitch:        f32,
    /// Track BPM ×1 (not pitched); None when no track is loaded.
    pub bpm:          Option<f32>,
    pub beat:         Option<u32>,
    pub beat_in_bar:  Option<u8>,
    /// Player master is being handed to, if a handoff is in progress.
    pub handoff_to:   Option<u8>,
    pub track_loaded: bool,
    pub firmware:     [u8; 4],
    pub counter:      u32,
    /// Sync counter: a device taking master sets this to the largest value
    /// it has seen on the network plus one; the old master yields only to a
    /// claim with a higher value.
    pub sync_counter: u32,
}

/// What we put in our own status packets.
#[derive(Debug, Clone, Copy)]
pub struct StatusFields {
    pub playing:      bool,
    pub track_loaded: bool,
    pub master:       bool,
    pub sync:         bool,
    pub on_air:       bool,
    /// 1.0 = +0%.
    pub pitch:        f32,
    /// Track BPM, unpitched.
    pub bpm:          Option<f32>,
    /// Cumulative beat number from the start of the track.
    pub beat:         Option<u32>,
    pub beat_in_bar:  Option<u8>,
    /// Player we are handing master to, during a handoff.
    pub handoff_to:   Option<u8>,
    pub counter:      u32,
    pub sync_counter: u32,
}

pub struct ProDjLink {
    player_num:  u8,
    device_name: [u8; 20],
}

/// Everything a beat packet tells us about the sender.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beat {
    pub player:       u8,
    /// Effective tempo, already including the sender's pitch.
    pub bpm:          f32,
    /// Pitch multiplier, 1.0 = +0%.
    pub pitch:        f32,
    /// 1–4.
    pub beat_in_bar:  u8,
    /// Milliseconds until the next beat, as the sender predicts it.
    pub next_beat_ms: u32,
}

impl ProDjLink {
    pub fn new(player_num: u8) -> Self {
        let mut device_name = [0u8; 20];
        let name = b"freedj-3000";
        device_name[..name.len()].copy_from_slice(name);
        Self { player_num, device_name }
    }

    /// Build a device announce / keep-alive packet (0x06).
    /// Broadcast on PORT_ANNOUNCE every 1.5 seconds.
    pub fn build_announce(&self, ip: Ipv4Addr, mac: [u8; 6]) -> Vec<u8> {
        let mut pkt = vec![0u8; announce::SIZE];
        pkt[..10].copy_from_slice(MAGIC);
        pkt[0x0a] = PKT_ANNOUNCE;
        pkt[announce::NAME..announce::NAME + 20].copy_from_slice(&self.device_name);
        pkt[0x20] = 0x01;
        pkt[0x21] = 0x02;
        pkt[announce::LEN..announce::LEN + 2].copy_from_slice(&(announce::SIZE as u16).to_be_bytes());
        pkt[announce::DEVICE] = self.player_num;
        pkt[0x25] = 0x01;                       // device type: CDJ
        pkt[announce::MAC..announce::MAC + 6].copy_from_slice(&mac);
        pkt[announce::IP..announce::IP + 4].copy_from_slice(&ip.octets());
        // Trailer per the analysis: 01 00 00 00 01 00
        pkt[0x30] = 0x01;
        pkt[0x34] = 0x01;
        pkt
    }

    /// Build a beat packet (0x28) for the beat that is happening now.
    ///
    /// `snap.bpm` is the effective tempo; `beat_in_bar` is 1–4.  The six
    /// countdown fields are derived from the beat period.
    pub fn build_beat(&self, snap: &EngineSnapshot, beat_in_bar: u8) -> Vec<u8> {
        let mut pkt = vec![0u8; beat::SIZE];
        pkt[..10].copy_from_slice(MAGIC);
        pkt[0x0a] = PKT_BEAT;
        pkt[beat::NAME..beat::NAME + 20].copy_from_slice(&self.device_name);
        pkt[0x1f] = 0x01;
        pkt[beat::DEVICE] = self.player_num;
        pkt[beat::LEN..beat::LEN + 2].copy_from_slice(&((beat::SIZE - beat::NEXT_BEAT) as u16).to_be_bytes());

        let beat_ms = 60_000.0 / snap.bpm.max(1.0);
        let bib = beat_in_bar.clamp(1, 4) as f32;
        let beats_to_next_bar = 5.0 - bib;                 // 4,3,2,1
        let counts = [1.0, 2.0, beats_to_next_bar, 4.0, beats_to_next_bar + 4.0, 8.0];
        for (i, n) in counts.iter().enumerate() {
            let ms = (n * beat_ms).round() as u32;
            let o = beat::NEXT_BEAT + i * 4;
            pkt[o..o + 4].copy_from_slice(&ms.to_be_bytes());
        }
        pkt[beat::FILL..beat::FILL + 24].fill(0xFF);

        let pitch = (snap.speed.max(0.0) * PITCH_UNITY as f32) as u32;
        pkt[beat::PITCH..beat::PITCH + 4].copy_from_slice(&pitch.to_be_bytes());
        let bpm_raw = (snap.bpm * 100.0).round() as u16;
        pkt[beat::BPM..beat::BPM + 2].copy_from_slice(&bpm_raw.to_be_bytes());
        pkt[beat::BEAT] = beat_in_bar.clamp(1, 4);
        pkt[beat::DEVICE2] = self.player_num;
        pkt
    }

    /// Build a CDJ status packet (0x0a) from the XDJ-1000MK2 template,
    /// overwriting the documented dynamic fields.  Unicast to every known
    /// device's port 50002 about five times a second.
    pub fn build_status(&self, f: &StatusFields) -> Vec<u8> {
        use crate::status_template::STATUS_TEMPLATE as T;
        let mut p = T.to_vec();
        p[0x0b..0x0b + 20].copy_from_slice(&self.device_name);
        // Byte 0x1f is 0x01 in every real device's status (verified on the wire
        // against an XDJ-1000MK2 and Beat Link's VirtualCdj); our template had
        // 0x00 here.  The XDJ tolerates it for normal status but aborts a master
        // handoff to us over it — the one structural byte where our status
        // differed from both a real XDJ and Beat Link.
        p[0x1f] = 0x01;
        p[status::DEVICE] = self.player_num;
        p[0x24] = self.player_num;
        // Track source: our own player, "USB", rekordbox-analysed — so peers
        // trust the BPM and grid we report.
        p[0x28] = if f.track_loaded { self.player_num } else { 0 };
        p[status::SLOT] = if f.track_loaded { 0x03 } else { 0 };
        p[status::TRACK_TYPE] = if f.track_loaded { 0x01 } else { 0 };
        p[status::PLAY] = match (f.track_loaded, f.playing) {
            (false, _)   => 0x00,
            (true, true) => 0x03,
            (true, false) => 0x05,
        };
        p[status::FIRMWARE..status::FIRMWARE + 4].copy_from_slice(b"0.1 ");
        p[status::FLAGS] = 0x84
            | if f.playing { 0x40 } else { 0 }
            | if f.master  { 0x20 } else { 0 }
            | if f.sync    { 0x10 } else { 0 }
            | if f.on_air  { 0x08 } else { 0 };
        p[0x8a] = if f.playing { 0xFF } else { 0x8D };
        p[0x8b] = if f.playing { 0xFA } else { 0xFE };
        let pitch = ((f.pitch.max(0.0)) * PITCH_UNITY as f32) as u32;
        for o in [status::PITCH, 0x98, 0xc0, 0xc4] {
            p[o..o + 4].copy_from_slice(&pitch.to_be_bytes());
        }
        let (mv, bpm) = match f.bpm { Some(b) => (0x8000u16, (b * 100.0).round() as u16), None => (0x7FFF, 0xFFFF) };
        p[0x90..0x92].copy_from_slice(&mv.to_be_bytes());
        p[status::BPM..status::BPM + 2].copy_from_slice(&bpm.to_be_bytes());
        p[0x9d] = if f.playing { 0x0D } else { 0x01 };   // CDJ mode / paused
        p[status::MASTER]  = if f.master { 0x01 } else { 0x00 };
        p[status::HANDOFF] = f.handoff_to.unwrap_or(0xFF);
        p[status::BEAT..status::BEAT + 4].copy_from_slice(&f.beat.unwrap_or(0xFFFF_FFFF).to_be_bytes());
        p[status::CUE_CD..status::CUE_CD + 2].copy_from_slice(&0x01FFu16.to_be_bytes());
        p[status::BEAT_BAR] = f.beat_in_bar.unwrap_or(0);
        p[status::COUNTER..status::COUNTER + 4].copy_from_slice(&f.counter.to_be_bytes());
        p[status::SYNC_N..status::SYNC_N + 4].copy_from_slice(&f.sync_counter.to_be_bytes());
        p
    }

    /// Common header for the small control packets on port 50001:
    /// magic, type, name, 01 00, our device number, remaining length.
    fn control_header(&self, kind: u8, payload_len: u16) -> Vec<u8> {
        let mut p = vec![0u8; 0x24 + payload_len as usize];
        p[..10].copy_from_slice(MAGIC);
        p[0x0a] = kind;
        p[0x0b..0x0b + 20].copy_from_slice(&self.device_name);
        p[0x1f] = 0x01;
        p[0x21] = self.player_num;
        p[0x22..0x24].copy_from_slice(&payload_len.to_be_bytes());
        p
    }

    /// Master handoff request (0x26): send to the current master's port 50001.
    pub fn build_master_request(&self) -> Vec<u8> {
        let mut p = self.control_header(PKT_MASTER_REQ, 4);
        p[0x24..0x28].copy_from_slice(&(self.player_num as u32).to_be_bytes());
        p
    }

    /// Master handoff response (0x27): send to the requester's port 50001.
    pub fn build_master_response(&self, yield_ok: bool) -> Vec<u8> {
        let mut p = self.control_header(PKT_MASTER_RSP, 8);
        p[0x24..0x28].copy_from_slice(&(self.player_num as u32).to_be_bytes());
        p[0x28..0x2c].copy_from_slice(&(yield_ok as u32).to_be_bytes());
        p
    }

    /// Sync control (0x2a): send to the target player's port 50001.
    pub fn build_sync_control(&self, target: u8, command: u32) -> Vec<u8> {
        let mut p = self.control_header(PKT_SYNC_CTRL, 8);
        p[0x24..0x28].copy_from_slice(&(target as u32).to_be_bytes());
        p[0x28..0x2c].copy_from_slice(&command.to_be_bytes());
        p
    }

    /// Parse a master handoff request: the requesting player number.
    pub fn parse_master_request(data: &[u8]) -> Option<u8> {
        (Self::packet_type(data)? == PKT_MASTER_REQ && data.len() >= 0x28)
            .then(|| data[0x27])
    }

    /// Parse a master handoff response: (responding player, yielded).
    pub fn parse_master_response(data: &[u8]) -> Option<(u8, bool)> {
        (Self::packet_type(data)? == PKT_MASTER_RSP && data.len() >= 0x2c)
            .then(|| (data[0x27], data[0x2b] == 1))
    }

    /// Parse a sync-control command: (sender, target, command).
    pub fn parse_sync_control(data: &[u8]) -> Option<(u8, u8, u32)> {
        (Self::packet_type(data)? == PKT_SYNC_CTRL && data.len() >= 0x2c).then(|| {
            (data[0x21], data[0x27], u32::from_be_bytes([data[0x28], data[0x29], data[0x2a], data[0x2b]]))
        })
    }

    /// Parse an announce / keep-alive: (player, ip).
    pub fn parse_announce(data: &[u8]) -> Option<(u8, Ipv4Addr)> {
        (Self::packet_type(data)? == PKT_ANNOUNCE && data.len() >= 0x30).then(|| {
            (data[announce::DEVICE], Ipv4Addr::new(data[0x2c], data[0x2d], data[0x2e], data[0x2f]))
        })
    }

    /// Packet type, if this is a ProDJ Link packet at all.
    pub fn packet_type(data: &[u8]) -> Option<u8> {
        if data.len() < 0x0b || &data[..10] != MAGIC {
            return None;
        }
        Some(data[0x0a])
    }

    /// Parse a beat packet.
    pub fn parse_beat(data: &[u8]) -> Option<Beat> {
        if Self::packet_type(data)? != PKT_BEAT || data.len() < beat::SIZE {
            return None;
        }
        let u16_at = |o: usize| u16::from_be_bytes([data[o], data[o + 1]]);
        let u32_at = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);

        let pitch_raw = u32_at(beat::PITCH);
        let pitch = pitch_raw as f32 / PITCH_UNITY as f32;
        // Some senders put a flag in the top byte; if the value is absurd,
        // fall back to unity rather than poisoning the tempo.
        let pitch = if (0.25..=4.0).contains(&pitch) { pitch } else { 1.0 };

        Some(Beat {
            player:       data[beat::DEVICE],
            bpm:          u16_at(beat::BPM) as f32 / 100.0,
            pitch,
            beat_in_bar:  data[beat::BEAT].clamp(1, 4),
            next_beat_ms: u32_at(beat::NEXT_BEAT),
        })
    }

    /// Parse a CDJ status packet.
    pub fn parse_status(data: &[u8]) -> Option<Status> {
        if Self::packet_type(data)? != PKT_STATUS || data.len() < status::MIN_SIZE {
            return None;
        }
        let u16_at = |o: usize| u16::from_be_bytes([data[o], data[o + 1]]);
        let u32_at = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        let f = data[status::FLAGS];
        let bpm_raw = u16_at(status::BPM);
        let beat_raw = u32_at(status::BEAT);
        let bib = data[status::BEAT_BAR];
        let handoff = data[status::HANDOFF];
        Some(Status {
            player:       data[status::DEVICE],
            play:         PlayState::from_byte(data[status::PLAY]),
            playing:      f & 0x40 != 0,
            master:       f & 0x20 != 0,
            sync:         f & 0x10 != 0,
            on_air:       f & 0x08 != 0,
            pitch:        u32_at(status::PITCH) as f32 / PITCH_UNITY as f32,
            bpm:          (bpm_raw != 0xffff).then(|| bpm_raw as f32 / 100.0),
            beat:         (beat_raw != 0xffff_ffff).then_some(beat_raw),
            beat_in_bar:  (1..=4).contains(&bib).then_some(bib),
            handoff_to:   (handoff != 0xff && handoff != 0).then_some(handoff),
            track_loaded: data[status::TRACK_TYPE] != 0,
            firmware:     [data[status::FIRMWARE], data[status::FIRMWARE + 1], data[status::FIRMWARE + 2], data[status::FIRMWARE + 3]],
            counter:      u32_at(status::COUNTER),
            sync_counter: u32_at(status::SYNC_N),
        })
    }

    /// Parse an incoming packet and return the peer's EngineSnapshot if it's
    /// a beat packet.  Status packets are decoded by `parse_status`.
    pub fn parse_packet(data: &[u8]) -> Option<(u8, EngineSnapshot)> {
        match Self::packet_type(data)? {
            PKT_BEAT => {
                let b = Self::parse_beat(data)?;
                Some((b.player, EngineSnapshot {
                    position: 0,
                    ghost_position: 0,
                    speed: b.pitch,
                    bpm: b.bpm,
                    beat_phase: 0.0,
                    bar_phase: (b.beat_in_bar - 1) as f32 / 4.0,
                    is_playing: true,
                    slip_active: false,
                    key_lock: false,
                    deck_id: b.player,
                    timestamp_ns: 0,
                }))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from prolink_virtual_cdj (device 5, "VirtualCDJ", 128 BPM)
    /// on 2026-08-25 — 96 bytes on UDP 50001.
    const CAPTURED_BEAT: [u8; 0x60] = [
        0x51, 0x73, 0x70, 0x74, 0x31, 0x57, 0x6D, 0x4A, 0x4F, 0x4C, 0x28,
        0x56, 0x69, 0x72, 0x74, 0x75, 0x61, 0x6C, 0x43, 0x44, 0x4A, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x05, 0x00, 0x3C,
        0x00, 0x00, 0x01, 0xD4,  0x00, 0x00, 0x03, 0xA8,  0x00, 0x00, 0x05, 0x7C,
        0x00, 0x00, 0x07, 0x50,  0x00, 0x00, 0x0C, 0xCF,  0x00, 0x00, 0x0E, 0xA0,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00,
        0x10, 0x10, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0x02, 0x00, 0x0D, 0x05,
    ];

    #[test]
    fn parses_captured_virtual_cdj_beat() {
        let b = ProDjLink::parse_beat(&CAPTURED_BEAT).expect("beat packet");
        assert_eq!(b.player, 5);
        assert_eq!(b.bpm, 128.0);
        assert_eq!(b.beat_in_bar, 2);
        assert_eq!(b.next_beat_ms, 468);   // 60_000 / 128 = 468.75
    }

    /// Captured from an XDJ-1000MK2 (player 1, idle, no track playing) on
    /// 2026-08-26 — 292 bytes on UDP 50002, unicast to us.
    const CAPTURED_STATUS: [u8; 292] = [
        0x51, 0x73, 0x70, 0x74, 0x31, 0x57, 0x6D, 0x4A, 0x4F, 0x4C, 0x0A, 0x58, 0x44, 0x4A, 0x2D, 0x31,
        0x30, 0x30, 0x30, 0x4D, 0x4B, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x05, 0x01, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x06, 0x04, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x31, 0x2E, 0x34, 0x34,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x84, 0x8D, 0xFE, 0x00, 0x10, 0x00, 0x00,
        0x7F, 0xFF, 0xFF, 0xFF, 0x7F, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x0C, 0x1F, 0x03, 0x00, 0x00,
        0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00,
    ];

    /// Captured from a real XDJ-1000MK2 (player 1, playing at 126.00 BPM,
    /// beat 2 of the bar) on 2026-08-26 — 96 bytes on UDP 50001.
    const CAPTURED_XDJ_BEAT: [u8; 0x60] = [
        0x51, 0x73, 0x70, 0x74, 0x31, 0x57, 0x6D, 0x4A, 0x4F, 0x4C, 0x28, 0x58, 0x44, 0x4A, 0x2D, 0x31,
        0x30, 0x30, 0x30, 0x4D, 0x4B, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x01, 0x00, 0x3C, 0x00, 0x00, 0x01, 0xDC, 0x00, 0x00, 0x03, 0xB9, 0x00, 0x00, 0x05, 0x95,
        0x00, 0x00, 0x07, 0x71, 0x00, 0x00, 0x0D, 0x06, 0x00, 0x00, 0x0E, 0xE2, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x31, 0x38, 0x02, 0x00, 0x00, 0x01,
    ];

    #[test]
    fn parses_real_xdj_beat() {
        let b = ProDjLink::parse_beat(&CAPTURED_XDJ_BEAT).expect("beat packet");
        assert_eq!(b.player, 1);
        assert_eq!(b.bpm, 126.0);
        assert_eq!(b.beat_in_bar, 2);
        assert!((b.pitch - 1.0).abs() < 1e-6);
        assert_eq!(b.next_beat_ms, 476);   // 60_000 / 126 = 476.19
    }

    #[test]
    fn parses_captured_xdj_status() {
        let st = ProDjLink::parse_status(&CAPTURED_STATUS).expect("status packet");
        assert_eq!(st.player, 1);
        assert_eq!(st.play, PlayState::NoTrack);
        assert!(!st.playing && !st.master && !st.sync);
        assert_eq!(&st.firmware, b"1.44");
        assert!((st.pitch - 1.0).abs() < 1e-6);
        assert_eq!(st.bpm, None);
        assert_eq!(st.beat, None);
        assert_eq!(st.beat_in_bar, None);
        assert_eq!(st.handoff_to, None);
        assert!(!st.track_loaded);
        assert_eq!(st.counter, 268);
        assert_eq!(st.sync_counter, 1);
    }

    #[test]
    fn status_round_trips_through_the_parser() {
        let link = ProDjLink::new(2);
        let f = StatusFields {
            playing: true, track_loaded: true, master: true, sync: false, on_air: false,
            pitch: 1.02, bpm: Some(134.7), beat: Some(77), beat_in_bar: Some(3),
            handoff_to: None, counter: 9, sync_counter: 7,
        };
        let pkt = link.build_status(&f);
        assert_eq!(pkt.len(), 0x124);
        let st = ProDjLink::parse_status(&pkt).unwrap();
        assert_eq!(st.player, 2);
        assert_eq!(st.play, PlayState::Playing);
        assert!(st.playing && st.master && !st.sync);
        assert!((st.pitch - 1.02).abs() < 1e-4);
        assert_eq!(st.bpm, Some(134.7));
        assert_eq!(st.beat, Some(77));
        assert_eq!(st.beat_in_bar, Some(3));
        assert_eq!(st.handoff_to, None);
        assert!(st.track_loaded);
        assert_eq!(&st.firmware, b"0.1 ");
        assert_eq!(st.counter, 9);
        assert_eq!(st.sync_counter, 7);
    }

    #[test]
    fn handoff_and_sync_control_round_trip() {
        let me = ProDjLink::new(3);
        assert_eq!(ProDjLink::parse_master_request(&me.build_master_request()), Some(3));
        assert_eq!(ProDjLink::parse_master_response(&me.build_master_response(true)), Some((3, true)));
        assert_eq!(ProDjLink::parse_sync_control(&me.build_sync_control(1, SYNC_ON)), Some((3, 1, SYNC_ON)));
        let ann = me.build_announce(Ipv4Addr::new(10, 0, 0, 7), [1, 2, 3, 4, 5, 6]);
        assert_eq!(ProDjLink::parse_announce(&ann), Some((3, Ipv4Addr::new(10, 0, 0, 7))));
    }

    #[test]
    fn rejects_the_old_private_format() {
        // What send_beat.py used to emit: 4-byte magic, type at offset 5.
        let mut old = [0u8; 0x30];
        old[..4].copy_from_slice(b"Qspt");
        old[4] = 0x10;
        old[5] = PKT_BEAT;
        assert!(ProDjLink::parse_packet(&old).is_none());
    }

    #[test]
    fn beat_round_trips() {
        let link = ProDjLink::new(3);
        let snap = EngineSnapshot {
            position: 0, ghost_position: 0, speed: 1.02, bpm: 130.0,
            beat_phase: 0.0, bar_phase: 0.0, is_playing: true,
            slip_active: false, key_lock: false, deck_id: 3, timestamp_ns: 0,
        };
        let pkt = link.build_beat(&snap, 3);
        assert_eq!(pkt.len(), 0x60);
        let b = ProDjLink::parse_beat(&pkt).unwrap();
        assert_eq!(b.player, 3);
        assert_eq!(b.bpm, 130.0);
        assert_eq!(b.beat_in_bar, 3);
        assert!((b.pitch - 1.02).abs() < 1e-4);
        assert_eq!(b.next_beat_ms, 462);   // 60_000 / 130
    }

    #[test]
    fn announce_has_spec_layout() {
        let pkt = ProDjLink::new(2).build_announce(Ipv4Addr::new(192, 168, 68, 64), [2, 0xfd, 0, 0, 0, 2]);
        assert_eq!(pkt.len(), 0x36);
        assert_eq!(&pkt[..10], MAGIC);
        assert_eq!(pkt[0x0a], PKT_ANNOUNCE);
        assert_eq!(pkt[0x24], 2);
        assert_eq!(&pkt[0x2c..0x30], &[192, 168, 68, 64]);
    }
}

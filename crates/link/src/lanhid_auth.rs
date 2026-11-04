//! Session handshake for the CDJ HID-over-LINK transport (stages 0x11–0x15).
//!
//! A CDJ in CONTROL MODE will not emit input reports until the host passes a
//! mutual challenge-response. This is an interoperability implementation of
//! that exchange, written from the CDJ-3000 / DJM-A9 integration brief and the
//! public CDJ HID analysis — the same stance as freedj's Pro DJ Link sender,
//! written from the Deep Symmetry protocol analysis.
//!
//! The challenge combines a per-session seed with a shared secret and hashes
//! it with FNV-1a (a public, non-cryptographic hash). The secret is a
//! per-manufacturer constant XORed with the CDJ's seed, and the host also
//! presents a per-model device ID. Those two values are the identity of the DJ
//! software the host poses as, and they are **not** compiled in: an
//! [`Identity`] is supplied by the caller, loaded at runtime from a file kept
//! out of version control (the same handling as the App Store signing key).
//! See `cdj_hid_identity.conf.example` for the format. A shipping product
//! supplies its own vendor-approved identity.

use std::io;
use std::path::Path;

/// Big-endian 32-bit Fowler–Noll–Vo (FNV-1a). Offset basis 0x811C9DC5, prime
/// 0x01000193.
pub fn fnv1a32(buf: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for &b in buf {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h
}

/// Split each byte 0xAB into 0x0A, 0x0B — the sysex-safe "spread" used for the
/// binary argument TLVs (seed, proof, device ID).
fn nibble_expand(v: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(v.len() * 2);
    for &b in v {
        o.push(b >> 4);
        o.push(b & 0x0F);
    }
    o
}

/// Inverse of [`nibble_expand`].
fn nibble_compact(v: &[u8]) -> Vec<u8> {
    v.chunks(2)
        .map(|c| (c[0] << 4) | (c.get(1).copied().unwrap_or(0) & 0x0F))
        .collect()
}

/// The identity the host presents to the CDJ. The secret and device ID belong
/// to the DJ software being posed as; they are loaded at runtime, never
/// compiled in — see the module note.
#[derive(Debug, Clone)]
pub struct Identity {
    pub manufacturer: String,
    pub product: String,
    /// XORed with the CDJ's seed E to form the shared secret (4 bytes).
    pub secret_const: [u8; 4],
    /// Per-model vendor-specific device ID (10 bytes).
    pub device_id: [u8; 10],
}

fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

impl Identity {
    /// Build from parts (the app wires these from its own secure source).
    pub fn new(
        manufacturer: impl Into<String>,
        product: impl Into<String>,
        secret_const: [u8; 4],
        device_id: [u8; 10],
    ) -> Self {
        Self {
            manufacturer: manufacturer.into(),
            product: product.into(),
            secret_const,
            device_id,
        }
    }

    /// Load from a `key = value` file (see `cdj_hid_identity.conf.example`):
    /// `manufacturer`, `product`, `secret_const` (8 hex chars), `device_id`
    /// (20 hex chars). Lines beginning with `#` are ignored. The file must be
    /// kept out of version control.
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut manufacturer = None;
        let mut product = None;
        let mut secret = None;
        let mut device = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "manufacturer" => manufacturer = Some(v.to_string()),
                "product" => product = Some(v.to_string()),
                "secret_const" => secret = hex_to_bytes(v),
                "device_id" => device = hex_to_bytes(v),
                _ => {}
            }
        }
        let err = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
        let secret = secret.ok_or_else(|| err("secret_const missing or not hex"))?;
        let device = device.ok_or_else(|| err("device_id missing or not hex"))?;
        Ok(Self {
            manufacturer: manufacturer.ok_or_else(|| err("manufacturer missing"))?,
            product: product.ok_or_else(|| err("product missing"))?,
            secret_const: secret
                .try_into()
                .map_err(|_| err("secret_const must be 4 bytes"))?,
            device_id: device.try_into().map_err(|_| err("device_id must be 10 bytes"))?,
        })
    }
}

// ── Canonical frame + TLV codec ──────────────────────────────────────────

/// Build one TLV: `[type, 2 + value.len(), value...]` (length counts the
/// 2-byte header). ASCII values go verbatim; binary values are pre-expanded by
/// the caller.
fn tlv(ty: u8, value: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + value.len());
    v.push(ty);
    v.push((2 + value.len()) as u8);
    v.extend_from_slice(value);
    v
}

/// Assemble a canonical 64-byte handshake frame for `stage`, carrying `tlvs`.
/// Header `00 F0 01 00 01 00`, then stage, then LEN = 2 + tlv bytes.
fn frame(stage: u8, tlvs: &[u8]) -> [u8; 64] {
    let mut r = [0u8; 64];
    r[1] = 0xF0;
    r[2] = 0x01; // fragment index 1 (LE16)
    r[4] = 0x01; // fragment count 1 (LE16)
    r[6] = stage;
    r[7] = (2 + tlvs.len()) as u8;
    r[8..8 + tlvs.len()].copy_from_slice(tlvs);
    r
}

/// A parsed nested TLV, value still in its on-wire (expanded, for binary) form.
struct Tlv<'a> {
    ty: u8,
    value: &'a [u8],
}

/// Parse a canonical frame's stage and nested TLVs. Returns `None` on a
/// malformed length. An all-zero frame parses as stage 0 with no TLVs (the
/// CDJ's reset packet).
fn parse(fr: &[u8]) -> Option<(u8, Vec<Tlv<'_>>)> {
    if fr.len() < 8 {
        return None;
    }
    let stage = fr[6];
    let len = fr[7] as usize;
    if stage == 0 && len == 0 {
        return Some((0, Vec::new()));
    }
    if len < 2 || 8 + (len - 2) > fr.len() {
        return None;
    }
    let body = &fr[8..8 + (len - 2)];
    let mut tlvs = Vec::new();
    let mut i = 0;
    while i + 2 <= body.len() {
        let ty = body[i];
        let l = body[i + 1] as usize;
        if l < 2 || i + l > body.len() {
            return None;
        }
        tlvs.push(Tlv { ty, value: &body[i + 2..i + l] });
        i += l;
    }
    Some((stage, tlvs))
}

fn find<'a>(tlvs: &'a [Tlv<'a>], ty: u8) -> Option<&'a [u8]> {
    tlvs.iter().find(|t| t.ty == ty).map(|t| t.value)
}

// ── State machine ────────────────────────────────────────────────────────

/// What the caller should do with the frame it just fed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Send this canonical frame back to the CDJ (framed for the transport).
    Send(Vec<u8>),
    /// Handshake complete; the CDJ now emits authenticated 0x20 input reports.
    Established,
    /// Nothing to do for this frame (e.g. a reset; wait for the next 0x11).
    Waiting,
    /// The exchange failed; `&str` says where. The CDJ will reset and restart.
    Failed(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    WaitHello,
    WaitProof,
    WaitAck,
    Established,
}

/// Drives one CDJ's handshake. One instance per player.
pub struct Handshake {
    id: Identity,
    seed_a: [u8; 8],
    state: State,
}

impl Handshake {
    /// `seed_a` is the host's 8-byte challenge. In production supply 8 random
    /// bytes; tests pass a fixed value.
    pub fn new(id: Identity, seed_a: [u8; 8]) -> Self {
        Self { id, seed_a, state: State::WaitHello }
    }

    pub fn established(&self) -> bool {
        self.state == State::Established
    }

    fn mfr_product_tlvs(&self) -> Vec<u8> {
        let mut t = tlv(0x01, self.id.manufacturer.as_bytes());
        t.extend(tlv(0x02, self.id.product.as_bytes()));
        t
    }

    /// Feed one received canonical frame; returns the next [`Step`].
    pub fn on_frame(&mut self, fr: &[u8]) -> Step {
        let Some((stage, tlvs)) = parse(fr) else {
            return Step::Failed("malformed frame");
        };

        // A reset (all-zero) at any point restarts the exchange.
        if stage == 0 {
            self.state = State::WaitHello;
            return Step::Waiting;
        }

        match (self.state, stage) {
            // Once in, ignore everything (the deck keeps announcing 0x11).
            (State::Established, _) => Step::Waiting,
            // Stage 1: CDJ announced itself. Answer with our challenge. The
            // deck re-announces 0x11 even while we wait for its 0x13 proof, so
            // a 0x11 in ANY (pre-Established) state re-arms and re-sends 0x12
            // rather than being treated as an out-of-order failure.
            (_, 0x11) => {
                let mut t = self.mfr_product_tlvs();
                t.extend(tlv(0x03, &nibble_expand(&self.seed_a)));
                self.state = State::WaitProof;
                Step::Send(frame(0x12, &t).to_vec())
            }
            // Stage 3: CDJ returned its proof and seed. Verify, then respond.
            (State::WaitProof, 0x13) => {
                let (Some(seed_e_x), Some(proof_x)) = (find(&tlvs, 0x03), find(&tlvs, 0x04))
                else {
                    return self.fail("stage 0x13 missing seed or proof");
                };
                let seed_e = nibble_compact(seed_e_x);
                let proof = nibble_compact(proof_x);
                if seed_e.len() != 4 || proof.len() != 4 {
                    return self.fail("stage 0x13 bad field length");
                }
                let k: Vec<u8> = seed_e
                    .iter()
                    .zip(self.id.secret_const.iter())
                    .map(|(e, c)| e ^ c)
                    .collect();

                // Verify the CDJ: FNV1a32(seedA || K) big-endian == proof.
                let mut buf = self.seed_a.to_vec();
                buf.extend_from_slice(&k);
                if fnv1a32(&buf).to_be_bytes() != proof[..] {
                    return self.fail("CDJ proof mismatch");
                }

                // Our response: FNV1a32(seedE || K) big-endian, plus device ID.
                let mut buf = seed_e.clone();
                buf.extend_from_slice(&k);
                let hash_e = fnv1a32(&buf).to_be_bytes();
                let mut t = self.mfr_product_tlvs();
                t.extend(tlv(0x04, &nibble_expand(&hash_e)));
                t.extend(tlv(0x05, &nibble_expand(&self.id.device_id)));
                self.state = State::WaitAck;
                Step::Send(frame(0x14, &t).to_vec())
            }
            // Stage 5: CDJ acknowledged. We are in.
            (State::WaitAck, 0x15) => {
                self.state = State::Established;
                Step::Established
            }
            _ => self.fail("unexpected stage for state"),
        }
    }

    fn fail(&mut self, why: &'static str) -> Step {
        self.state = State::WaitHello;
        Step::Failed(why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fabricated identity, so no real vendor secret appears in the tests.
    /// The handshake is identity-agnostic; the self-consistency test below uses
    /// the same fake values on both sides.
    fn fake_identity() -> Identity {
        Identity::new(
            "TestCo",
            "testapp",
            [0x11, 0x22, 0x33, 0x44],
            [0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9],
        )
    }

    #[test]
    fn fnv_known_answer() {
        assert_eq!(fnv1a32(b""), 0x811C_9DC5);
        assert_eq!(fnv1a32(b"hello"), 0x4F9F_2CAB);
    }

    #[test]
    fn nibble_round_trip() {
        let v = [0x1B, 0x1F, 0x37, 0x61];
        assert_eq!(nibble_compact(&nibble_expand(&v)), v);
        assert_eq!(nibble_expand(&[0xAB]), [0x0A, 0x0B]);
    }

    #[test]
    fn identity_loads_from_conf() {
        let dir = std::env::temp_dir();
        let path = dir.join("cdj_hid_identity_test.conf");
        std::fs::write(
            &path,
            "# test\nmanufacturer = TestCo\nproduct = testapp\nsecret_const = 11223344\ndevice_id = A0A1A2A3A4A5A6A7A8A9\n",
        )
        .unwrap();
        let id = Identity::load(&path).unwrap();
        assert_eq!(id.manufacturer, "TestCo");
        assert_eq!(id.secret_const, [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(id.device_id[0], 0xA0);
        let _ = std::fs::remove_file(&path);
    }

    /// A minimal CDJ side: given the host's stage-2 frame, produce stage 3.
    fn cdj_reply_0x13(id: &Identity, host_0x12: &[u8], seed_e: [u8; 4]) -> Vec<u8> {
        let (stage, tlvs) = parse(host_0x12).unwrap();
        assert_eq!(stage, 0x12);
        let seed_a = nibble_compact(find(&tlvs, 0x03).unwrap());
        let k: Vec<u8> =
            seed_e.iter().zip(id.secret_const.iter()).map(|(e, c)| e ^ c).collect();
        let mut buf = seed_a.clone();
        buf.extend_from_slice(&k);
        let hash_a = fnv1a32(&buf).to_be_bytes();
        let mut t = tlv(0x01, id.manufacturer.as_bytes());
        t.extend(tlv(0x02, id.product.as_bytes()));
        t.extend(tlv(0x03, &nibble_expand(&seed_e)));
        t.extend(tlv(0x04, &nibble_expand(&hash_a)));
        frame(0x13, &t).to_vec()
    }

    #[test]
    fn full_exchange_succeeds() {
        let id = fake_identity();
        let mut hs = Handshake::new(id.clone(), [1, 2, 3, 4, 5, 6, 7, 8]);

        let hello = frame(0x11, &tlv(0x01, &[0x03, 0x16]));
        let Step::Send(s12) = hs.on_frame(&hello) else { panic!("expected 0x12") };
        assert_eq!(s12[6], 0x12);

        let s13 = cdj_reply_0x13(&id, &s12, [0xDE, 0xAD, 0xBE, 0xEF]);
        let Step::Send(s14) = hs.on_frame(&s13) else { panic!("expected 0x14") };
        assert_eq!(s14[6], 0x14);

        let (_, tlvs) = parse(&s14).unwrap();
        assert_eq!(nibble_compact(find(&tlvs, 0x05).unwrap()), id.device_id);

        let ack = frame(0x15, &[]);
        assert_eq!(hs.on_frame(&ack), Step::Established);
        assert!(hs.established());
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let real = fake_identity();
        let mut wrong = real.clone();
        wrong.secret_const = [0, 0, 0, 0];
        let mut hs = Handshake::new(wrong, [1, 2, 3, 4, 5, 6, 7, 8]);
        let Step::Send(s12) = hs.on_frame(&frame(0x11, &tlv(0x01, &[3, 22]))) else {
            panic!()
        };
        let s13 = cdj_reply_0x13(&real, &s12, [1, 2, 3, 4]);
        assert_eq!(hs.on_frame(&s13), Step::Failed("CDJ proof mismatch"));
    }

    #[test]
    fn repeated_0x11_rearms_not_fails() {
        // The deck re-announces 0x11 while we wait for its 0x13; a stray 0x11
        // in WaitProof must re-send 0x12, not fail, and the handshake must
        // still complete when the 0x13 finally arrives.
        let id = fake_identity();
        let mut hs = Handshake::new(id.clone(), [1, 2, 3, 4, 5, 6, 7, 8]);
        let hello = frame(0x11, &tlv(0x01, &[3, 22]));
        assert!(matches!(hs.on_frame(&hello), Step::Send(_))); // -> WaitProof
        // stray re-announce: re-arm, re-send 0x12 (was: Failed).
        let Step::Send(s12) = hs.on_frame(&hello) else {
            panic!("repeated 0x11 should re-send 0x12")
        };
        assert_eq!(s12[6], 0x12);
        let s13 = cdj_reply_0x13(&id, &s12, [0xDE, 0xAD, 0xBE, 0xEF]);
        let Step::Send(s14) = hs.on_frame(&s13) else { panic!("expected 0x14") };
        assert_eq!(s14[6], 0x14);
        assert_eq!(hs.on_frame(&frame(0x15, &[])), Step::Established);
    }

    #[test]
    fn reset_restarts() {
        let mut hs = Handshake::new(fake_identity(), [0; 8]);
        hs.on_frame(&frame(0x11, &tlv(0x01, &[3, 22])));
        assert_eq!(hs.on_frame(&[0u8; 64]), Step::Waiting);
        assert!(matches!(hs.on_frame(&frame(0x11, &tlv(0x01, &[3, 22]))), Step::Send(_)));
    }
}

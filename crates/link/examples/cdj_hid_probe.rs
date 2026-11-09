//! Live probe: run the lanhid handshake against the cdj3k emulator's USB-B
//! PC Link socket, then print the authenticated input reports.
//!
//! The emulator carries the deck's raw HID over a Unix socket with its own
//! framing: `type(1) | len(2, BE) | payload`, type 1 HID, 3 HELLO, 4 IDENTITY.
//! The payload is the bare 64-byte report, which is exactly what
//! `lanhid_auth::Handshake` consumes — so this exercises the real firmware's
//! handshake responder with no LAN tunnel in the way.
//!
//! The vendor identity (secret + device ID) is NOT in this file; it is read
//! from a conf kept out of git (see crates/link/cdj_hid_identity.conf.example).
//!
//!   cargo run -p opendeck-link --example cdj_hid_probe -- \
//!       --id /path/to/cdj_hid_identity.conf \
//!       [--sock /tmp/cdj3k-emu-1000/instance-1/usb-link.sock] [--secs 15] [--raw]
//!
//! `--raw` also prints every input-report byte that changed, mapped or not.
//!
//! Start the emulator's bridge first (Developer Mode on):
//!   cdj3k-emu ctl --slot 1 sh 'systemctl start cdj3k-pc-link-bridge'

#[cfg(unix)]
fn main() -> std::io::Result<()> {
    use opendeck_link::lanhid::InputDecoder;
    use opendeck_link::lanhid_auth::{Handshake, Identity, Step};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    // ── args ──
    let mut sock = "/tmp/cdj3k-emu-1000/instance-1/usb-link.sock".to_string();
    let mut id_path: Option<PathBuf> = None;
    let mut secs = 15u64;
    let mut raw = false;
    let mut seed = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let mut a = std::env::args().skip(1);
    while let Some(arg) = a.next() {
        match arg.as_str() {
            "--sock" => sock = a.next().expect("--sock needs a path"),
            "--id" => id_path = a.next().map(PathBuf::from),
            "--raw" => raw = true,
            "--secs" => secs = a.next().and_then(|s| s.parse().ok()).unwrap_or(15),
            "--seed" => {
                // 16 hex chars = 8 seed bytes, optional
                if let Some(h) = a.next() {
                    for (i, c) in h.as_bytes().chunks(2).take(8).enumerate() {
                        let s = std::str::from_utf8(c).unwrap();
                        seed[i] = u8::from_str_radix(s, 16).unwrap_or(seed[i]);
                    }
                }
            }
            other => eprintln!("ignoring unknown arg {other:?}"),
        }
    }
    let id_path = id_path.expect("pass --id <cdj_hid_identity.conf>");
    let identity = Identity::load(&id_path)?;
    eprintln!(
        "identity: {} / {} (device id {} bytes)",
        identity.manufacturer,
        identity.product,
        identity.device_id.len()
    );

    let mut stream = UnixStream::connect(&sock)?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;

    // Framing helpers for the emulator socket.
    let send = |s: &mut UnixStream, ty: u8, payload: &[u8]| -> std::io::Result<()> {
        let n = payload.len() as u16;
        let mut f = vec![ty, (n >> 8) as u8, n as u8];
        f.extend_from_slice(payload);
        s.write_all(&f)
    };

    let mut hs = Handshake::new(identity, seed);
    let mut dec = InputDecoder::new();
    let mut established = false;
    let mut reached_11 = false;
    let mut highest_stage = 0u8;
    let mut input_reports = 0u64;
    let mut last_report = [0u8; 64];

    // First 16 bytes of a frame as hex, for logging the deck's stage frames.
    fn hex8(p: &[u8]) -> String {
        p.iter().take(16).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
    }
    let mut last_keepalive = Instant::now();
    let mut next_hello = Instant::now();

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(secs);

    while Instant::now() < deadline {
        // Nudge the deck with an empty HELLO until it starts talking.
        if !reached_11 && Instant::now() >= next_hello {
            send(&mut stream, 3, &[])?;
            next_hello = Instant::now() + Duration::from_millis(300);
        }
        // Keepalive after we are authenticated (empty 64-byte report).
        if established && last_keepalive.elapsed() >= Duration::from_millis(400) {
            let mut k = [0u8; 64];
            k[1] = 0xF0;
            send(&mut stream, 1, &k)?;
            last_keepalive = Instant::now();
        }

        match stream.read(&mut tmp) {
            Ok(0) => {
                eprintln!("socket EOF");
                break;
            }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e),
        }

        // Drain whole frames.
        while buf.len() >= 3 {
            let len = ((buf[1] as usize) << 8) | buf[2] as usize;
            if buf.len() < 3 + len {
                break;
            }
            let ty = buf[0];
            let payload = buf[3..3 + len].to_vec();
            buf.drain(..3 + len);

            match ty {
                4 => eprintln!(
                    "IDENTITY: {}",
                    String::from_utf8_lossy(&payload).replace('\n', " | ")
                ),
                1 => {
                    // A handshake stage? byte 1 == 0xF0.
                    if !established && payload.len() >= 8 && payload[1] == 0xF0 {
                        let stage = payload[6];
                        if stage == 0x11 {
                            reached_11 = true;
                        }
                        // Record which stages the deck actually emits — a 0x13
                        // (or higher) proves the firmware runs the real
                        // handshake responder, not just a 0x11 announce loop.
                        if stage > highest_stage {
                            highest_stage = stage;
                            eprintln!(
                                "deck stage 0x{stage:02x} (first): {}",
                                hex8(&payload)
                            );
                        }
                        match hs.on_frame(&payload) {
                            Step::Send(frame) => {
                                eprintln!("  -> got stage 0x{stage:02x}, replying 0x{:02x}", frame[6]);
                                send(&mut stream, 1, &frame)?;
                            }
                            Step::Established => {
                                eprintln!("handshake ESTABLISHED");
                                established = true;
                                last_keepalive = Instant::now();
                            }
                            Step::Waiting => {}
                            Step::Failed(why) => {
                                eprintln!("  -> stage 0x{stage:02x} rejected: {why}")
                            }
                        }
                    } else if payload.len() >= 2 && payload[1] == 0x20 {
                        input_reports += 1;
                        // --raw: print every byte that changed since the last
                        // report, so unmapped bits show up too.
                        if raw {
                            let diffs: Vec<String> = (0..payload.len().min(64))
                                .filter(|&i| payload[i] != last_report[i])
                                .map(|i| format!("[{i:02x}] {:02x}->{:02x}", last_report[i], payload[i]))
                                .collect();
                            if !diffs.is_empty() && input_reports > 1 {
                                println!("raw {}", diffs.join(" "));
                            }
                            last_report[..payload.len().min(64)].copy_from_slice(&payload[..payload.len().min(64)]);
                        }
                        for ev in dec.decode(&payload) {
                            println!("{ev:?}");
                        }
                    }
                }
                _ => {}
            }
        }
    }

    eprintln!("---");
    eprintln!("saw stage 0x11:        {reached_11}");
    eprintln!("highest deck stage:    0x{highest_stage:02x}");
    eprintln!("handshake established: {established}");
    eprintln!("0x20 input reports:    {input_reports}");
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("cdj_hid_probe needs a Unix socket; not supported on this platform.");
}

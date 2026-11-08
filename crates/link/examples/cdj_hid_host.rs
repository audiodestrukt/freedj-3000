//! Live host driver: establish the lanhid handshake against the cdj3k
//! emulator's USB-B socket, offer the 0x2A deck list, send the title/artist
//! text, and stream a real 0x21 status at ~60 Hz with the playhead advancing.
//! Per the brief, the deck only shows host content once the user presses
//! BROWSE on the CDJ and selects a deck row; the deck's input is logged so the
//! selection can be watched.
//!
//!   cargo run -p opendeck-link --example cdj_hid_host -- \
//!       --id ~/private_keys/cdj_hid_identity.conf [--secs 20] \
//!       [--track amplificate.mp3]
//!
//! With `--track`, the file is decoded and analysed the way the deck does
//! (`WaveformBuilder`), its real duration drives the status and its waveform
//! is sent as the 0x2C RGB overview.
//!
//! Start the emulator's bridge first (Developer Mode on):
//!   printf 'sh systemctl start cdj3k-pc-link-bridge\n' | nc 127.0.0.1 4401

#[cfg(unix)]
fn main() -> std::io::Result<()> {
    use opendeck_link::lanhid::InputDecoder;
    use opendeck_link::lanhid_auth::{Handshake, Identity, Step};
    use opendeck_link::lanhid_display::{self as disp, DeckRow};
    use opendeck_link::lanhid_frag::fragment;
    use opendeck_link::lanhid_status::{encode, Status, TempoRange};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    let mut sock = "/tmp/cdj3k-emu-1000/instance-1/usb-link.sock".to_string();
    let mut id_path: Option<PathBuf> = None;
    let mut secs = 20u64;
    let mut title = "Amplificate".to_string();
    let mut artist = "Audiodestrukt".to_string();
    let mut track: Option<PathBuf> = None;
    let seed = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let mut a = std::env::args().skip(1);
    while let Some(arg) = a.next() {
        match arg.as_str() {
            "--sock" => sock = a.next().expect("--sock needs a path"),
            "--id" => id_path = a.next().map(PathBuf::from),
            "--secs" => secs = a.next().and_then(|s| s.parse().ok()).unwrap_or(20),
            "--title" => title = a.next().expect("--title needs text"),
            "--artist" => artist = a.next().expect("--artist needs text"),
            "--track" => track = a.next().map(PathBuf::from),
            other => eprintln!("ignoring unknown arg {other:?}"),
        }
    }
    let identity = Identity::load(&id_path.expect("pass --id <conf>"))?;
    eprintln!("identity: {} / {}", identity.manufacturer, identity.product);

    let mut stream = UnixStream::connect(&sock)?;
    stream.set_read_timeout(Some(Duration::from_millis(50)))?;

    let send = |s: &mut UnixStream, ty: u8, payload: &[u8]| -> std::io::Result<()> {
        let n = payload.len() as u16;
        let mut f = vec![ty, (n >> 8) as u8, n as u8];
        f.extend_from_slice(payload);
        s.write_all(&f)
    };

    let mut hs = Handshake::new(identity, seed);
    let mut dec = InputDecoder::new();
    // Re-offered every second: the deck may only read it while Browse is open.
    let mut last_assets = Instant::now() - Duration::from_secs(10);
    let mut established = false;
    let mut reached_11 = false;

    // The host track: a real file's waveform + length, or a 3:00 stand-in.
    let (columns, duration_ms) = match &track {
        Some(p) => {
            let (cols, ms) = analyse(p)?;
            eprintln!("track: {} — {} columns, {} ms", p.display(), cols.len(), ms);
            (cols, ms)
        }
        None => (Vec::new(), 180_000u32),
    };
    let overview = fragment(disp::CMD_OVERVIEW, &disp::overview(&columns));
    let bpm_x10 = 1280u16;
    let play_start = Instant::now();

    let mut last_status = Instant::now();
    let mut next_hello = Instant::now();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(secs);

    while Instant::now() < deadline {
        if !reached_11 && Instant::now() >= next_hello {
            send(&mut stream, 3, &[])?;
            next_hello = Instant::now() + Duration::from_millis(300);
        }
        // ~60 Hz real status once authenticated.
        if established && last_status.elapsed() >= Duration::from_millis(16) {
            let current_ms = (play_start.elapsed().as_millis() as u32) % duration_ms;
            let st = Status {
                playing: true,
                track_loaded: true,
                tempo_range: TempoRange::Ten,
                rate: 1.0,
                current_ms,
                duration_ms,
                bpm_x10,
                original_bpm_x10: bpm_x10,
                ..Default::default()
            };
            let mut rep = encode(&st);
            rep[0] = 0x00; // emulator socket: no LAN route byte
            send(&mut stream, 1, &rep)?;
            last_status = Instant::now();
        }

        if established && last_assets.elapsed() >= Duration::from_secs(1) {
            let rows = [
                DeckRow::label("OPENDECK"),
                DeckRow { bpm_x10, text: "Deck 1".into(), ..Default::default() },
            ];
            let mut frames = fragment(disp::CMD_DECK_ROWS, &disp::deck_rows(&rows));
            frames.extend(fragment(disp::CMD_TITLE, &disp::text(&title)));
            frames.extend(fragment(disp::CMD_ARTIST, &disp::text(&artist)));
            frames.extend(overview.iter().copied());
            for f in &frames {
                send(&mut stream, 1, f)?;
            }
            last_assets = Instant::now();
        }

        match stream.read(&mut tmp) {
            Ok(0) => { eprintln!("socket EOF"); break; }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e),
        }

        while buf.len() >= 3 {
            let len = ((buf[1] as usize) << 8) | buf[2] as usize;
            if buf.len() < 3 + len { break; }
            let ty = buf[0];
            let payload = buf[3..3 + len].to_vec();
            buf.drain(..3 + len);
            if ty == 1 && !established && payload.len() >= 8 && payload[1] == 0xF0 {
                if payload[6] == 0x11 { reached_11 = true; }
                match hs.on_frame(&payload) {
                    Step::Send(frame) => send(&mut stream, 1, &frame)?,
                    Step::Established => {
                        eprintln!("handshake ESTABLISHED; streaming 0x21 status");
                        established = true;
                        last_status = Instant::now();
                    }
                    Step::Waiting => {}
                    Step::Failed(why) => eprintln!("rejected: {why}"),
                }
            } else if ty == 1 && payload.len() >= 2 && payload[1] == 0x20 {
                for ev in dec.decode(&payload) {
                    // Jog noise drowns the interesting edges.
                    if !matches!(ev, opendeck_link::lanhid::CdjControl::Jog { .. }) {
                        eprintln!("deck: {ev:?}");
                    }
                }
            } else if ty == 1 && payload.len() >= 2 && payload[1] != 0xF0 {
                eprintln!("deck other report 0x{:02x}: {:02x?}", payload[1], &payload[..16.min(payload.len())]);
            }
        }
    }
    eprintln!("--- established: {established}");
    Ok(())
}

/// Decode `path` and build its waveform columns exactly as a deck load does.
#[cfg(unix)]
fn analyse(path: &std::path::Path) -> std::io::Result<(Vec<[u8; 4]>, u32)> {
    let io = |e: opendeck_types::DecodeError| std::io::Error::other(format!("{e:?}"));
    let mut dec = opendeck_decode::open_decoder(path).map_err(io)?;
    let sr = dec.sample_rate();
    let ch = dec.channels().max(1) as usize;
    let mut wb = opendeck_analysis::waveform::WaveformBuilder::new(sr);
    let mut buf = vec![0f32; 4096 * ch];
    let mut stereo = Vec::with_capacity(4096 * 2);
    let mut frames = 0u64;
    loop {
        let n = dec.decode(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        frames += n as u64;
        stereo.clear();
        for f in buf[..n * ch].chunks(ch) {
            stereo.push(f[0]);
            stereo.push(f[1.min(ch - 1)]);
        }
        wb.push(&stereo);
    }
    let ms = (frames * 1000 / sr as u64) as u32;
    Ok((wb.finish().columns, ms))
}

#[cfg(not(unix))]
fn main() {
    eprintln!("cdj_hid_host needs a Unix socket; not supported on this platform.");
}

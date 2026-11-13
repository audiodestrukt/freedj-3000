# Audio formats

What OpenDeck plays, how it treats the audio, and what it does not do yet.
Everything here is measured or covered by a test; the numbers say how.

## File types

The deck's BROWSE lists files ending in `.mp3`, `.m4a`, `.aac`, `.flac`,
`.wav`, `.aiff`, `.aif` and `.ogg`.

| Format | Bit depth | Notes |
|---|---|---|
| **WAV** | 16, 24, 32-bit integer; 32 and 64-bit float | Float keeps its headroom above full scale |
| **AIFF** | 16, 24, 32-bit integer; 32-bit float (AIFF-C) | |
| **FLAC** | 16, 24 and 32-bit | 32-bit is the FLAC 1.4 format; decoded bit for bit as the reference `flac` tool does |
| **ALAC** (Apple Lossless, `.m4a`) | 16 and 24-bit | 32-bit ALAC is untested |
| **MP3** | — | CBR and VBR |
| **AAC** (`.m4a`, `.aac`) | — | Not DRM-protected iTunes purchases (`.m4p`) |
| **Ogg Vorbis** | — | |

**Sample rates:** any. 44.1, 48, 88.2, 96, 176.4 and 192 kHz are tested.

**Channels:** mono plays on both sides. Stereo plays as is. Files with more
channels (5.1 and so on) play their front left and right.

**Not supported:** Opus, WMA, APE (Monkey's Audio), WavPack, DSD (`.dsf`,
`.dff`), DRM-protected files, and streaming services.

## How the deck treats the audio

- **Sample rate.** A file is converted once, when it loads, to the rate of
  the sound output (usually 48 kHz). The converter is band-limited and keeps
  length, pitch and beat timing exact: a beat at 0.5 s is still at 0.5 s
  after the conversion. A 96 kHz file therefore plays at the output's rate,
  not at 96 kHz, unless the sound card is set to 96 kHz.
- **Beat grid.** A track gets the same BPM and grid whatever its sample rate
  or bit depth. The same track at 48, 96 and 192 kHz gets the same first
  beat to the hundredth of a millisecond.
- **MP3 and rekordbox.** OpenDeck keeps an MP3's encoder delay (the silence
  an encoder adds at the start), as rekordbox and Pioneer players do. A
  rekordbox beat grid and cues therefore land on the same beats in OpenDeck.
  Trimming the delay would put every grid about 24 ms late.
- **Level.** MP3 and Ogg Vorbis are held to full scale when decoded. A loud
  master can decode past it. Other formats are passed through as they are.

## Tempo and key lock

- **Tempo:** ±6, ±10, ±16 % and WIDE. In a test synced to a CDJ-3000
  playing at 123.357 BPM, OpenDeck played at 123.358 BPM.
- **MASTER TEMPO (key lock)** keeps the key when the tempo changes. A 440 Hz
  tone stays at 440 Hz from −16 % to +16 %:

  | Engine | Worst pitch error | Where it is used |
  |---|---|---|
  | Rubber Band R3 | 0.01 cent | Default |
  | Bungee | 0.11 cent | Option, about a sixth of R3's processor time |
  | WSOLA | 0.34 cent | Low-power boards such as the Raspberry Pi Zero |

  The ear starts to hear about 5 cents.
- **Not yet:** key lock off, where the pitch follows the tempo like a record.
  Today the key always stays fixed.

## Tags and artwork

| Format | Title, artist, album, genre, year | Key | BPM | Cover art |
|---|---|---|---|---|
| MP3 (ID3) | ✅ | ✅ | ✅ (decimals kept) | ✅ |
| FLAC | ✅ | ✅ | ✅ | ✅ |
| M4A (AAC, ALAC) | ✅ | untested | ✅ (whole numbers, as the format stores them) | ✅ |
| Ogg Vorbis | ✅ | ✅ | ✅ | untested |
| AIFF (ID3 chunk) | ✅ | ✅ | ✅ | ✅ |
| WAV | Not read yet | | | |

A tagged key or BPM is shown in BROWSE as tagged, and a track with no key
tag has its key detected.  The beat grid the deck plays to comes from the
track's rekordbox analysis when it has one, otherwise from OpenDeck's own
analysis; a BPM tag does not set it.

## rekordbox USB sticks and other players

A track on a rekordbox stick, or loaded from another player over LINK, plays
whatever its file format is in the list above. rekordbox's beat grid, cues
and waveforms are used when they are there.

## How this is checked

- `cargo test -p opendeck-decode`: 32-bit integer and float WAV at 96 kHz,
  and a 32-bit FLAC built so that its stereo needs every bit.
- `cargo test -p opendeck-app resampl`: conversions 96→48, 96→44.1,
  88.2→44.1 and 44.1→48 kHz keep length, pitch and timing.
- `cargo test -p opendeck-analysis a_96k`: the same grid at 48 and 96 kHz.
- `cargo test --release -p opendeck-timestretch key_lock -- --nocapture`:
  the pitch table above.
- `cargo run --release -p opendeck-decode --example decode_dump -- out/ files…`
  decodes files and hashes the samples, to compare two builds. It was used to
  check the move to Symphonia 0.6 on 23 files across every format above.

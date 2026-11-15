# Serving split stems over Pro DJ Link

Status: **proposal** (2026-11-15). Nothing here is built yet; it is the
plan for when we get to it.

## Goal

OpenDeck on a desktop (or an iPad) has the music and the separation model.
A player that has neither, such as another OpenDeck or an ESP32 deck from
freedj-esp, should play those tracks **with stems**, split by the desktop.

## No new protocol

Pro DJ Link already carries everything, and both halves exist:

- **Serving** (built): OpenDeck's media server answers dbserver (browse,
  metadata, grids, waveforms) and NFS (the files) like a CDJ with a stick
  in it.
- **Reading stems** (built 2026-11-15, `stems::load_link`): a player that
  loads a track over LINK looks it up by its track ID in the source's
  `CDJMODS/index-v2` and reads the stem bank page by page over NFS, from
  the playhead first. Tested against the emulated CDJ-3000 serving Dan's
  OverCue stick: a 2:27 bank in 6.5 s, first stems in 0.4 s.

So "the desktop splits, the player plays" means the desktop writes banks
in the open `CDJMODS` format into the folder it serves, and the player
reads them as it reads any player's stick.

We started a custom streaming protocol (OSS) and dropped it. It added only
two things:

1. **Stems while a split is still running.** The `CDJMODS` format can't
   carry a bank before it's complete, because every page's hash and the
   whole-file hashes sit in the index. Splitting the served library in the
   background beforehand mostly removes the need: tracks are ready before
   anyone browses to them.
2. **A mix mode for chips without PSRAM** (the C6 in freedj-nano). Such a
   player can play the plain full mix over LINK without stems.

## What OverCue does (for reference)

Tested 2026-11-15 (`docs/design/stems-overcue.md`, last section): OverCue
reads a source's index-v2 over plain NFS for its "S" marks, but loads no
bank from a source that doesn't run OverCue. So stems served this way
reach OpenDecks and other open players, not OverCue CDJs, unless OverCue
chooses to accept them. OverCue's phone streaming is almost certainly its
own protocol between its app and its firmware; we don't look into it.

## A. OpenDeck: serve the stems it splits (phase 5, ~2–3 days)

1. **Background split of the served folder.** One track at a time, on the
   media server's analysis thread or one beside it, after the analysis.
   A track a player has just browsed to or loaded goes to the front. A
   desktop with the ONNX model only; GPU when there is one (the RTX 4070
   SUPER on Dan's desktop would make it fast; CUDA is parked).
2. **Write the bank.** The same `CDJMODS/stems/<bundle>/` layout OverCue
   Desktop writes, in one of the profiles our reader already handles:
   - `flac-96k`, the CDJ-3000 profile, for OpenDecks;
   - `wav-44k1` (plain PCM WAV roles, `cdj2000-44k1/1`) for small players
     that shouldn't need a FLAC decoder.
   Four roles (vocal, drums, harmonics, full mix), `three_part: 2`, all
   page and whole-file hashes, a fresh bundle directory per bank.
3. **Publish the index** under the track IDs OpenDeck's dbserver hands
   out: `opendeck_stems::writer::write_index` (built 2026-11-15) writes
   `index.json` and a new index-v2 generation and swaps `current` last.
4. **Let NFS see new files.** The NFS server scans its folder once at
   start (`opendeck_nfs::server::Tree::scan`); it needs to add the new
   bank directories and the new index generation while running.
5. **Cache.** Banks are big (a 3-minute track is roughly 130 MB as FLAC at
   96 kHz). Keep them in the app's data folder, not the user's music
   folder, served at `CDJMODS/` as if they were on the stick; evict the
   oldest when over a size limit.
6. **Tests:** loopback NFS (as `stems_load_over_link_from_a_players_media`):
   a served folder with no stems gets a split bank; another deck's
   `load_link` finds it by our ID and plays it.

## B. ESP32 player: a Link client in C (freedj-esp, larger)

For the S3 deck (ESP32-S3R8, 8 MB PSRAM, I2S at the track's rate):

1. **Pro DJ Link over Wi-Fi:** announce as a player (or just a browsing
   client), find media sources. freedj-esp's TARGETS notes say Link over
   Wi-Fi is plausible; it needs measuring.
2. **dbserver client:** browse and get a track's ID, path, grid.
3. **NFSv2 client:** LOOKUP and READ over UDP (lwIP), with several reads
   in flight as our Rust client does.
4. **Stems:** index-v2 lookup by track ID; then the bank's page table, and
   pages around the playhead into a window in PSRAM. At 44.1 kHz three
   stereo 16-bit stems are 529 KB/s, so about a 10-second window fits
   comfortably. With `wav-44k1` banks a page is raw PCM; no decoder.
5. **Mix:** the stems with their gains where pages have arrived, the full
   mix elsewhere, crossfading at the edges, as OpenDeck's mixer does.

## Open questions

- Which profile by default for served banks: write both, or pick by what
  the browsing player is?
- Background splitting on battery (iPad, laptop): only when charging?
- Whether a player could ask the source to split a track **now** (it
  already does by loading it, if loads bump the queue).

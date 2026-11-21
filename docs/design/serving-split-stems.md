# StemStream: serving split stems over Pro DJ Link

Status: **live stems built (2026-11-21)**, tested over loopback only (see
"Live stems as built" below).  Writing finished banks into the served
`CDJMODS` index (A.2, A.3) and the ESP32 client (B) are still plans.

**StemStream** is the name for the whole feature: OpenDeck splits the
music it serves and players play those stems over Pro DJ Link. It is a
feature name, not a wire protocol; the wire is Link (dbserver + NFS +
`CDJMODS`).

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
   anyone browses to them.  (2026-11-21: solved without a protocol, as
   plain files beside the spec's: see "Live stems as built".)
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

## Live stems as built (2026-11-21)

Dan, after OverCue Desktop 2.2.58 shipped "Live Stems" (a computer splits
a player's track on request and streams it; their protocol is private and
we don't look into it): do the same between OpenDecks, with the iPad's fast
Core ML split as a source and small players such as an ESP32 as clients.

**What the spec gives and what it doesn't.**  The spec pages a *finished*
bank: 131,072-byte pages (32,768 stereo frames), each page's SHA-256 in
the role's page table, "a real-time reader can verify the table up front
and pages as it fetches them" (our reader does, from the playhead).  But a
writer must "finish and verify every referenced file" before publishing
the index, so a bank still being split has no place in it.

**The extension: `CDJMODS/live/<track id>/`** (`opendeck_stems::live`), plain
files beside the spec's, which OverCue readers never open:

- `stems-sidecar-<role>.s16le.wav` for vocal, drums, harmonics and the full
  mix: the spec's canonical `wav-44k1` role files, full length from the
  start, pages written in place.  A finished role is a spec `wav-44k1` role
  byte for byte.
- `pages`: a 32-byte header (`ODLIVE01`, frames, page bytes, page count,
  state splitting / complete / failed, headroom dB) and one byte per page,
  set once the page is in every role file and its hashes are written.  The
  reader polls it (about 300 bytes for a 4-minute track).
- `pages.sha`: four SHA-256 per page, one per role.  A page is used only
  once it checks out.

Everything is 16-bit stereo at 44.1 kHz (the model's rate) on the source's
own decode timeline, the full mix included, so a client without an MP3
decoder can play the whole track from these files.  The PCM is stored 6 dB
down (the device split's headroom), as the header says.

**Asking is a lookup.**  A deck whose STEMS panel opens on a track loaded
from another OpenDeck, with no prepared stems, looks up
`/CDJMODS/live/<id>`.  On the source the lookup misses and the NFS server's
miss hook (`Tree::on_miss`) queues the split (`livestems.rs`: newest
request first, one at a time).  The bank appears within the decode time;
the reader publishes an empty bank on the deck at once and fills it a few
seconds at a time from the playhead as pages arrive, converted to the
deck's rate.  A source without `/CDJMODS/live` (a CDJ, rekordbox) answers
the first lookup, and the deck splits the track itself.  The NFS server's
tree is now live (new files, growing files), and the source's banks live
in the app's cache, served as `/CDJMODS` or, inside a stick's own
`CDJMODS`, as `/CDJMODS/live`, so prepared banks stay served.  A cached
bank is kept only while its track ID still names the same file (IDs follow
the folder's scan order).

**Where the player is.**  The player tells the source its position the
same way, by a lookup that always misses: `/CDJMODS/live/<id>@<page>`
(live page numbers, at 44.1 kHz).  It does so when it first asks and
again whenever its playhead lands on a page not split yet; the source's
split starts at that page and follows (the device split's own
playhead-following, `separate::Job::playhead`).

**Cache.**  The banks stay for the next load (a finished bank is served
at once), up to 2 GB on iOS and Android, 4 GB elsewhere
(`OPENDECK_LIVE_STEMS_MB`); the oldest finished ones go first.

**The deck comes first.**  A device's own deck always has the model
before a player's request: a split for a player waits while the deck
splits, and one under way stops at its next chunk when the deck starts
one (the player keeps what came and plays the original after).  A player
whose source gave up before any stems is told so at once and splits the
track itself; a source that never starts a bank costs it 10 s at most.
Serving live stems needs a separation model and is **off by default on a
Raspberry Pi** (aarch64 Linux; `OPENDECK_LIVE_STEMS=1` turns it on): the
Pi 5 splits its own tracks at about real time only with every core, so
it stays a player.  `OPENDECK_LIVE_STEMS=0` turns it off anywhere.

**Tests** (loopback, a stand-in model): a player reads the first pages
while the split is still running, then the rest, each as split; a deck
plays them at 44.1 and 48 kHz, the start before the split is done; a
source with no live stems costs no wait; damaged pages and maps are
refused; a renumbered track's bank is dropped; the split starts at the
player's page and follows a jump back; a full cache drops the oldest; a
split for a player waits for and gives way to the deck's own, and the
player then splits it itself with no empty bank left on its deck.

**Measured** (2026-11-21, `real_model_live_stems_over_loopback`, desktop,
`separate-native` HTDemucs, a 3:00 MP3, a 48 kHz deck over loopback NFS):
the first 4 s of stems playable **5.05 s** after the deck asked, the whole
track in 58 s (3.1x real time), the stems summing to the track within
31 dB.  Before `Job::at_playhead` the split began at the reported page
plus the deck split's look-ahead (~4 s, for a deck moving on while a chunk
computes), so the first 4 s came last, after 60 s.

**Publishing finished splits (experimental, `OPENDECK_LIVE_STEMS_PUBLISH=1`).**
A finished split is also written as a standard bank in the spec's CDJ-3000
profile (96 kHz, FLAC pages stored verbatim, full level, four roles) and
indexed under our dbserver IDs (`index.json` + index-v2), for readers that
only know the spec, such as OverCue players.  Off by default: a verbatim
bank is ~276 MB for 3 minutes and the cache cap doesn't count it yet, and
OverCue loaded no bank from a non-OverCue source on 2026-11-15.  Skipped
when the served folder has its own `CDJMODS` (its index stays the
stick's).  Tested: our own reader finds it by ID over LINK and plays it.
On the emulated CDJ-3000 with OverCue 0.14.6 (2026-11-21): OverCue reads
our index-v2 and plays the track, but loads no stems (STEMS grey, no
reads of the bank): see `stems-overcue.md`, last section.
`source_timing_policy` is `opendeck-decoded/1` (our decoder's timeline,
no MP3 padding restored), so a CDJ's own decode may be offset from it.

**Not done yet:** a run between two real devices (iPad source, desktop
deck); writing the finished bank into the served index under our IDs
(A.2, A.3), which only OverCue-format readers would need now (an OpenDeck
reads the finished live bank at once).

**For an ESP32 client** (B): look up `/CDJMODS/live/<id>`, poll `pages`,
read 128 KiB pages of raw PCM near the playhead into PSRAM, check each
against `pages.sha` (or not, on a chip without the time), and mix the three
stems with the full mix as fallback.  No FLAC, no index parsing.

## Open questions

- Which profile by default for served banks: write both, or pick by what
  the browsing player is?
- Background splitting on battery (iPad, laptop): only when charging?
- Whether a player could ask the source to split a track **now** (it
  already does by loading it, if loads bump the queue).

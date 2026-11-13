# Pro DJ Link media: browsing and loading tracks between decks

Status: **built and proven both directions** (2026-09-17). OpenDeck loads
tracks from a real XDJ-1000MK2's USB and from a rekordbox 7 laptop, and it
*serves* its own music folder so that another OpenDeck (and, pending the test
at home, a real XDJ) can browse and load from it. This page is the reference
for how it works, what is on the wire, and how to test it away from the
hardware. The original feasibility study is `docs/design/prodj-link-library.md`;
the tracking issues were #27 (client), #30 (rekordbox), #31 (windowed reads)
and #44 (server), all closed into this page; #32 (browse UI fidelity) stays
open for the source-selector decision.

## The two channels

"Loading over the link" is not a push. A player with media runs two services,
and the player that wants the track pulls from both:

| channel | transport | used for | our crate |
|---|---|---|---|
| **dbserver** ("remote database") | TCP; port query on 12523, database port dynamic (players usually 1051; OpenDeck takes 1051 when free, since some tools skip the query) | browsing menus, metadata, the track's **file path**, beat grid, waveforms, cues, artwork | `crates/dbserver` (client + server) |
| **NFSv2** | UDP; portmap 111 (players) or **50111** (rekordbox), then mountd + nfsd | the audio bytes | `crates/nfs` (client + server) |

Discovery rides the Link packets we already speak: announces on UDP 50000
tell everyone who is on the network, status packets on 50002 carry the
media-slot flags, and a **media query / media response** pair (kinds `0x05` /
`0x06` on 50002) tells a browsing deck what is in a slot (name, track count,
playlist count, capacity).

```
 browsing deck                                   media source
 ─────────────                                   ────────────
 announce 50000  ◀──────────────────────────▶  announce 50000
 status 50002 (USB slot: loaded)  ◀────────────  status 50002
 media query 0x05  ──────────────────────────▶
                   ◀──────────────────────────  media response 0x06 (name, counts)
 TCP 12523 "RemoteDBServer"  ─────────────────▶
                   ◀──────────────────────────  2-byte db port
 dbserver: setup, menus, render, 0x2102 path ─▶ ◀─ rows
 portmap GETPORT (111 | 50111)  ──────────────▶ ◀─ mountd / nfsd ports
 MNT "/C/" (player) | "/" (rekordbox)  ───────▶ ◀─ root file handle
 LOOKUP path, READ ×N (32 in flight)  ────────▶ ◀─ 8 KB chunks
```

### Who serves what

| source | portmap | export | names on the wire | notes |
|---|---|---|---|---|
| CDJ / XDJ | UDP 111 | `/C/` (USB), `/B/` (SD) | UTF-16LE | needs a privileged source port for portmap on Linux; see `crates/nfs` |
| rekordbox 6/7 | **UDP 50111** | `/` | UTF-16LE | export ACL'd to rekordbox's own subnet; `EACCES` from anywhere else (Tailscale) |
| OpenDeck | UDP 111 (app) or `--portmap` | `/C/` | UTF-16LE | server in `crates/nfs/src/server.rs` |

## dbserver in one page

Framing (Deep Symmetry `dysentery` → `track_metadata.adoc`): a stream of
tagged fields, `0x0f` u8, `0x10` u16, `0x11` u32, `0x14` blob, `0x26`
UTF-16BE string (length in units incl. the trailing NUL). A message is magic
`0x872349ae`, transaction id, message type (u16), argument count, a 12-byte
blob of argument type tags, then the arguments. The first thing on a fresh
connection is a lone `U32(1)` greeting echoed back; then `0x0000` setup with
our device number, answered by `0x4000`.

Every request carries a **DMST** argument, four bytes packed into a u32:
requesting **d**evice, **m**enu (1 = main browse, 8 = analysis/waveforms),
**s**lot (1 CD, 2 SD, 3 USB, 5 rekordbox collection), **t**rack type (1 =
rekordbox).

Menus are two-step: the request (say `0x1004` all tracks) is answered with
`0x4000` carrying the row count; a `0x3000` render with offset + limit then
streams `0x4001` header, `0x4101` items, `0x4201` footer. Item rows carry
parent id, id, two labels, an item type and an artwork id. The item type is
what tells the client what a row *is*; the one that matters most:

- **`0x2102` track info → row type `0x0000`, label = absolute file path.** That
  path is what the client then LOOKUPs over NFS. Everything else on the
  track (`0x2002` metadata: title, artist, album, duration, tempo×100…) is
  for display.

Blobs: `0x2204` beat grid → `0x4602` (20-byte header, 16-byte entries:
beat-in-bar, tempo×100 LE, ms LE), `0x2004` waveform preview → `0x4402`,
`0x2904` waveform detail → `0x4a02`, `0x2c04` ANLZ tag → `0x4f02`, `0x2003`
artwork → `0x4002`. `0x4003` means unavailable. The full type table is
`crates/dbserver/src/lib.rs` `kind`.

**Device numbers.** rekordbox reports itself as device 41 and accepts
requests from devices 1–4. Players accept 1–4 as well. We connect as our own
player number when it is 1–4, else as 1.

## Client side (LINK in the browser)

`crates/app/src/browser.rs`. LINK is a folder row at the top of the file
browser; inside it, one row per media slot a Link peer has answered a
**media query** for, labelled the way a player's own LINK list is:

- `3 USB: OPENDECK` / `3 SD: MYCARD` for a player numbered 1–16 (number,
  slot, volume name from the media response);
- `Player N   <ip>` while a player has not answered yet (tried as USB);
- `rekordbox   <ip>` for device ≥ 17 or a name containing "rekordbox".

The sender thread queries every player's USB and SD slot every 5 s
(`build_media_query`); responses land in `LinkState::peer_media` and expire
after 20 s, so a pulled stick drops off the list.

Entering a slot opens a dbserver session (`connect_db`) and shows the
source's **category menu** (`0x1000`), as a player does: PLAYLIST, ARTIST,
ALBUM, TRACK, FILENAME are walkable (playlist tree `0x1105`, artists
`0x1002` → tracks `0x1202`, albums `0x1003` → tracks `0x1103`, all tracks
`0x1004`, by file name `0x1013`); any other category the source lists is
shown but inert. A source with no root menu falls back to ALL TRACKS + the
playlist tree; a player with no dbserver at all falls back to mounting the
export and parsing `export.pdb` with rekordcrate. Entering rekordbox uses the
collection slot. Track rows carry title / artist from the menu.

Loading: `0x2102` for the path, then `read_file` over NFS. The whole load
(fetch, decode, resample, waveform, grid, auto cue) runs on a **loader
thread** (`start_fetch` / `Prep::finish` in `lib.rs`); the UI thread only
swaps the prepared track in, a few milliseconds. The beat grid comes from `0x2204` when the source has one (rekordbox,
players); otherwise from the ANLZ file over NFS, otherwise our own analysis.

`read_file` keeps **32 READs in flight** (`crates/nfs/src/lib.rs`, `WINDOW`),
matched by xid, resent after 400 ms. This is what made loads over a phone
hotspot go from about two minutes to a few seconds: NFSv2 caps a read at 8 KB,
so a serial client pays one round trip per 8 KB.

## Server side (OpenDeck as a media source)

The app starts serving its browse root at launch (`start_media_server` in
`crates/app/src/lib.rs`), unless `OPENDECK_SERVE=0`. The same code runs
stand-alone as `opendeck-serve`. What "serving" means:

1. **Announce + status.** Announces as usual; status packets flag the USB slot
   as loaded (`0x6f = 0`, SD `0x73 = 4`, link media `0x75 = 1`), which is
   what makes another player list us under LINK.
2. **Media query → media response** (`crates/link/src/prodj.rs`,
   `build_media_response`): name "OPENDECK", track and playlist counts,
   capacity. The reply goes to the address inside the query *and*, when that
   differs, to the address the query came from (port 50002 either way):
   across a routed link the inner address is the peer's LAN address.
3. **dbserver** (`crates/dbserver/src/server.rs`): port query, setup, root
   menu (`0x1000`: PLAYLIST / ARTIST / ALBUM / TRACK / FILENAME), all tracks,
   artists and albums with their drill-downs (`0x1002`, `0x1003`, `0x1102`,
   `0x1103`, `0x1202`), file names (`0x1013`), playlist folder (empty for
   now), metadata (16 rows incl. duration, tempo, key), track info (7 rows,
   path in row type 0), beat grid (`0x4602`), waveform preview (`0x4402`),
   waveform detail (`0x4a02`), artwork (`0x4002`), paged render. Cue lists
   and raw ANLZ tags answer `0x4003`; so does anything unknown, logged, so an
   unexpected request from a real player shows up in the log.
4. **NFSv2** (`crates/nfs/src/server.rs`): portmap (NULL / GETPORT / DUMP),
   mountd (MNT accepts any name and returns the root handle, EXPORT lists one
   export), nfsd (NULL / GETATTR / LOOKUP / READ ≤ 8 KB / READDIR / STATFS).
   The tree is scanned once at start; handles are the node index.

**The library** (`crates/mediaserver/src/lib.rs`) starts as file names the
moment the folder is scanned, so the services are up at launch. A background
thread (`media-analysis`) then decodes one track at a time and fills in tags
(title / artist / album / key / comment / cover art), duration, tempo and beat
grid (our detector over the leading two minutes) and the two waveforms, and
writes the result to a cache (`app data/linkcache/<hash>.v1`, keyed by path +
size + mtime) so only the first launch pays. Rows update in place behind an
`RwLock`; artist and album ids are hashes of the name so a menu stays valid
while rows are still being renamed. Artwork is read from the file on request
rather than kept in memory. The deck's own loads read and write the same
cache (`docs/design/track-loading.md`), so a track is analysed once whether
the deck or the library got to it first.

Waveform encoding follows what Beat Link decodes from a player, since no
capture from a real one exists yet: preview = 400 × (height 0–31, whiteness
0–7) byte pairs; detail = 19 lead bytes then one byte per 1/150 s, height in
the low five bits, whiteness in the top three. Heights are scaled so the
track's loudest column is full height. **Unverified against an XDJ**; rekordbox
7 on the Mac could not confirm it because its copy of the test track is
unanalysed and it answers `0x4003` for both.

**Ports.** 111 and 2049 are below 1024. On Linux the workstation has
`net.ipv4.ip_unprivileged_port_start=80`; on iOS Apple DTS says low ports
were never restricted (bind `0.0.0.0`, never a specific address). If a
platform refuses, `--portmap 50111` makes us look like a rekordbox source,
which players already know how to reach.

## rekordbox specifics

Learned against rekordbox 7 on a Mac (2026-09-17):

- The **LINK button appears only after rekordbox has seen a Link device** on
  one of its LAN interfaces. Running the desktop OpenDeck on the same Mac was
  enough.
- rekordbox **ignores announces that arrive over Tailscale** (or any routed
  link): a 15 s capture showed nothing. A dbserver connection from anywhere
  still works once LINK is on.
- On the same machine rekordbox **holds UDP 50000–50002 exclusively**, so an
  OpenDeck on that machine cannot hear announces and its LINK folder stays
  empty. Quit rekordbox to test OpenDeck as a client on that Mac.
- rekordbox **never browses** a player's media. It only fetches the loaded
  track's metadata and art, hence its "please permit it on the CDJ/XDJ to show
  icons" message.
- Its NFS export is ACL'd to its own Wi-Fi subnet, so loads work from a device
  on the same Wi-Fi and fail with `EACCES` from Tailscale.

## Testing without the XDJ

All of this was developed over Tailscale between the workstation and a Mac.
Broadcast does not cross a routed link, unicast does, and peers must be
addressed by the address their packet **came from**, not the address inside
the packet.

```bash
# be a media source; unicast announces to peers across Tailscale
cargo run -p opendeck-mediaserver -- ~/Music --player 3 --peer 100.108.2.10 --ip 100.97.166.122

# the app as client: hear a peer across a routed link
OPENDECK_LINK_UNICAST=100.97.166.122 cargo run -p opendeck-app

# browse anything that speaks dbserver (player, rekordbox, opendeck-serve)
cargo run -p opendeck-dbserver --example dbserver_browse -- 100.108.2.10 3 collection
cargo run -p opendeck-dbserver --example dbserver_probe  -- <ip> <dev> <slot> <id> 2102

# NFS: list exports, mount, READDIR, timed read of one file
cargo run -p opendeck-nfs --example nfs_probe -- 192.168.68.50 111 /Contents/track.mp3
cargo run -p opendeck-nfs --example nfs_probe -- 100.108.2.10 50111

python3 tools/portmap_probe.py <host>            # GETPORT on 111 and 50111
python3 tools/link_keepalive.py <target> <our-ip> # unicast announces, no app

# end-to-end: browse + load through the app's Browser against opendeck-serve
OPENDECK_TEST_LINK=127.0.0.1 cargo test -p opendeck-app link_source
```

Result on 2026-09-17: OpenDeck on the Mac browsed the workstation's
`opendeck-serve` under LINK → Player 3 → ALL TRACKS and loaded a 12.5 MB MP3
over the hotspot, with the file's ID3 title on the deck.

## How this differs from a real XDJ's LINK

Mechanically identical: the XDJ asks the same dbserver menus and reads audio
over the same NFS. The presentation differs, and #32 tracks closing the gap:

- On the XDJ, LINK is a **source button** beside USB / SD / rekordbox, not a
  folder in a list.
- Labels (player number + slot + media name) and the category menu on entry
  now match the XDJ (2026-09-18). Categories we do not walk yet (BPM, KEY,
  SEARCH, FOLDER on a real player) are listed but inert; rows are plain text
  without the XDJ's colour labels, art or detail pane.

## Open items

- The XDJ acceptance test: LINK → OpenDeck → load, watching the server log for
  any `0x4003` / "unhandled" we send, and whether the served waveforms draw
  (see the encoding note above).
- Confirm port binding on the iPad (TestFlight 0.1.14).
- Cue lists are served (hot and memory cues; layouts below).  Still
  answered "no data", and a CDJ-3000 loads and plays without them:
  - `0x2c04` PQT2.EXT, the extended grid.  From a CDJ-3000 (track of 463
    beats, 126 BPM): header 0x38; at 0x10 `01000002`; two 8-byte entries,
    first and last beat (beat-in-bar u16, BPM×100 u16, ms u32); the beat
    count; an unknown u32; then one u16 per beat whose values always fall
    on twelfths of 1,000 (0, 83, 166, 250, …).  Their meaning is not known
    (pyrekordbox: "not yet known"), so nothing is served rather than guesses.
  - `0x2504`: a 1,604-byte blob, all zero but 4 bytes at 0x640
    (`80 7d a1 00` for that track).  Unknown.

### Cue lists as a CDJ-3000 answers them

Captured 2026-11-12 (memory cue, hot cues A and B, loop hot cue C):

- `0x2104` → `0x4702`: args `[req, 0, len, blob, 0x24, hot count, memory
  count, pairs length, pairs]`.  Blob: memory cues then hot cues, 36 bytes
  each — `[loop?, 1, hot#, 0×5, ff×4, start u32 LE, loop end u32 LE or ff×4,
  ff×16]`, positions in half-frames (1/150 s).  Pairs: `(ms, loop end ms or
  ffffffff)` u32 LE per entry, same order.
- `0x2b04` → `0x4e02`: args `[req, 0, len, blob, count]`.  Hot cues then
  memory cues; entry size u32 LE at 0 (84 hot, 76 memory), hot# u16 at 4,
  kind at 6 (1 cue, 2 loop), `e8 03` at 0x0a, start / loop end ms at
  0x0c / 0x10, `ff`×20 at 0x20, 0x1a / 0x12 at 0x34; a hot cue ends with
  its colour at 0x4e (code, R, G, B: green, a loop orange).  No cues:
  status 0x32 ("no data").

### CDJ-3000 status and position

- Status to other players is 1,152 bytes (type 0x08 at 0x20, length 0x045c).
  Active loop: `01 ff 8c` at 0x1b0, start / end µs (u32 BE) at 0x1b8 /
  0x1c0, beats u16 at 0x1c8 (Beat Link reads 0x1b6 / 0x1be ×65536/1000).
  The region also keeps earlier loops in repeated blocks.
- Absolute Position (0x0b, 60 bytes) goes unicast to each device that has
  announced, ~33 a second, never broadcast; a CDJ alone on the network
  sends only announces and beats.
- Client: BPM / KEY / SEARCH categories, colour labels, art in the detail pane
  (#32's remaining scope).

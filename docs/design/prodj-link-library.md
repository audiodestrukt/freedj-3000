# Design: track browse / load over ProDJ Link (rekordbox-on-the-network)

Status: **feasibility + plan** (2026-08-26). Not built. Answers "can freedj do
the file transfer of songs over the CDJ link like the XDJ does?" — short answer:
**yes for nexus/nexus2-era gear (incl. XDJ-1000MK2), with real caveats.**

> Protocol details here are recalled from the reverse-engineering community and
> should be verified against **Deep Symmetry's** current docs before coding
> (`dysentery`, `beat-link`, `crate-digger`) and the Rust **`rekordcrate`** crate.

## What "file transfer over the link" actually is

It is **not** a push/copy. When one player loads a track that lives on another
linked player's USB/SD, the mechanism is **NFS**:

- Each player with media runs an **RPC/NFS server** (portmapper :111, mountd,
  nfsd — historically NFSv2 over UDP).
- The other player **mounts the media and reads files on demand**: first the
  rekordbox database **`export.pdb`** (under `PIONEER/rekordbox/`), then the
  per-track **`ANLZ` analysis** files (`.DAT`/`.EXT` — beat grid, waveform, cues)
  and finally the **audio file** itself, streamed over NFS as it plays.
- Discovery/who-has-what rides the ProDJ Link packets we already speak (announce
  :50000, status :50002). Metadata can also come from the **`dbserver` query
  protocol** (TCP :1051) as an alternative to reading `export.pdb` directly.

So "load from the XDJ's stick" = *mount the XDJ's media over NFS, read its
rekordbox DB to browse, read the audio to play*. The DB already contains the beat
grid, cues, and waveform, so a linked load also inherits Pioneer's analysis.

## Two directions (very different difficulty)

### 1. freedj as CONSUMER — load from a linked XDJ's media (tractable, high value)

freedj browses and plays tracks sitting on a real XDJ/CDJ's USB:

- Implement (or wrap) an **NFS/RPC client**: portmap → mount → NFS read. Could be
  a hand-rolled minimal NFSv2/UDP client (crate-digger is the reference), or lean
  on an existing NFS client where the environment allows.
- Parse **`export.pdb`** with `rekordcrate` to build the browse tree (playlists,
  folders, tracks) — slots straight into our existing `Browser` model behind the
  same navigation interface.
- Read the audio file over NFS → feed our existing decode-to-RAM path. Optionally
  parse `ANLZ` to import Pioneer's **beat grid + cues** instead of re-analysing.
- This is the natural first target for "work alongside a regular XDJ": freedj
  plays from the same source the XDJ is playing from.

### 2. freedj as PROVIDER — an XDJ loads freedj's tracks (much harder)

For a real XDJ to browse+load *our* library over the link, freedj must:

- Run an **NFS server** exposing a media tree in the exact rekordbox layout.
- **Generate a valid `export.pdb`** (+ `ANLZ` files) the XDJ will accept —
  `rekordcrate` *reads* these formats; *writing* a byte-accurate DB the hardware
  trusts is the hard, under-explored part.
- Announce as a device advertising a populated **media slot** (extends our
  announce/status packets), and answer `dbserver`/NFS requests correctly.

Doable in principle, but it's a large, fragile surface. Defer behind direction 1.

## Caveats (important)

- **Firmware / model matters.** Nexus/nexus2-era players (CDJ-2000nexus,
  **XDJ-1000MK2**, XDJ-700) use the NFS scheme above and are reverse-engineered.
  The **CDJ-3000 encrypted** file access in newer firmware, breaking open-source
  file fetch — so this approach targets the XDJ-1000MK2 the project already
  references, not a CDJ-3000.
- **Reverse-engineered ⇒ fragile.** No official spec; firmware updates can break
  it. Treat it as best-effort interop, not a guarantee.
- **Networking on the appliance.** NFS/RPC over the wired link; the Pi has gigabit
  ethernet (we already use it for Link). Mounting user-space vs kernel NFS is a
  choice; a hand-rolled UDP client avoids needing mount privileges.
- **Legal/scope.** Interop with our own playback only; we parse Pioneer's formats,
  we don't redistribute their software.

## Connections to what exists

- ProDJ Link announce/beat/status already implemented (`crates/link/src/prodj.rs`,
  `crates/app/src/prodj.rs`) — the discovery layer this builds on.
- The file **`Browser`** (`crates/app/src/browser.rs`) is designed so a
  library/DB source slots in behind the same navigation — a network source is
  just another backend.
- Decode-to-RAM load path is source-agnostic; an NFS byte stream feeds it like a
  local file.
- Library work (WORKSTREAMS F1/F2) and `rekordcrate` overlap: parsing rekordbox
  DBs is useful for reading local rekordbox USBs too, not only over the network.

## Suggested increments

1. **Read a rekordbox USB locally** with `rekordcrate` — parse `export.pdb` into
   the Browser, play the referenced audio, optionally import beat grid + cues
   from `ANLZ`. No networking; proves the format layer and is independently
   useful.
2. **NFS client** — mount/read a linked player's media; reuse (1) to browse+play
   it over the link. This is the "load from the XDJ" feature.
3. **(Later) provider** — NFS server + `export.pdb` generation so an XDJ can load
   from freedj. Only if there's appetite for the large surface.

## References to verify against

- Deep Symmetry: `dysentery` (protocol analysis), `beat-link` (status/metadata),
  `crate-digger` (NFS fetch + `export.pdb`/`ANLZ` parsing) — the canonical
  reverse-engineering.
- `rekordcrate` (Rust) — parse rekordbox `pdb`/`anlz`.
- Confirm the CDJ-3000 encryption status and current NFS behaviour before
  committing engineering time.

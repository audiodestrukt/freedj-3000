# Vendored: rekordcrate

This is a vendored copy of **rekordcrate** (MPL-2.0), used to parse Pioneer
rekordbox device exports (`export.pdb`, `ANLZ`). It is wired in via
`[patch.crates-io]` in the workspace root `Cargo.toml`.

## Why vendored (not a plain crates.io dep)

rekordcrate keeps its `pdb` `Track`/`Artist`/`PlaylistEntry` fields **private**
(no accessors, no serde) even on `main`, so the metadata we need (title, tempo,
artist name…) is unreachable through the public API. We patch field visibility.

## Provenance

- **Upstream:** https://github.com/Holzhaus/rekordcrate
- **Vendored version:** `0.3.0` (from crates.io)
- **License:** MPL-2.0 (GPL-2.0-compatible — see `COPYING`). Keep the license
  headers on all files.

## Our patch (keep minimal, so re-syncing is easy)

1. **`src/pdb/mod.rs`** — struct fields made `pub` so we can read track/artist/
   playlist metadata. Applied mechanically:
   `sed -E 's/^(    )([a-z_][a-zA-Z0-9_]*: )/\1pub \2/'` on the struct fields.
   (`src/anlz.rs` and `src/pdb/string.rs` are left **pristine** — anlz fields we
   need are already public, and patching anlz breaks its modular-bitfield structs.)
2. **`src/lib.rs`** — added `#![allow(warnings)]` at the top and **removed the
   crate-level lint attributes** (`#![warn(missing_docs)]`,
   `#![deny(rust_2018_idioms)]`, and especially
   `#![cfg_attr(not(debug_assertions), deny(warnings))]`). As a `[patch]` path
   dependency this crate is NOT cap-lints-allowed, so those denies turned our
   pub-field edits into hard errors in **release** builds.
3. Removed `target/`, `Cargo.lock`, `Cargo.toml.orig`, `.cargo_vcs_info.json`.

## Re-syncing to a newer upstream

1. `cargo` fetch the new version, copy its `src/` over this tree.
2. Re-apply patch (1) to `src/pdb/mod.rs` and patch (2) to `src/lib.rs`.
3. `cargo build --release` (must be release — that's where the lint denies bite).

## The real fix: upstream it

The right long-term move is to **upstream the field-visibility change** (a PR to
Holzhaus/rekordcrate making the `pdb` row fields `pub`, or adding accessors).
Once merged + released we can drop this vendor and depend on crates.io again.
Until then, keep the patch tiny and documented here.

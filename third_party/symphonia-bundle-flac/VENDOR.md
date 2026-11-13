# Vendored: symphonia-bundle-flac

A vendored copy of **symphonia-bundle-flac** (MPL-2.0), Symphonia's FLAC
demuxer and decoder, wired in via `[patch.crates-io]` in the workspace root
`Cargo.toml`.

## Why vendored

Symphonia 0.6.1 opens 32-bit FLAC (FLAC 1.4+) but decodes stereo wrongly: in
Left/Side, Mid/Side and Right/Side frames the Side channel needs 33 bits and is
truncated to an `i32`.  On a 96 kHz 32-bit test file about two thirds of the
samples came out wrong.  Upstream issue
https://github.com/pdeljanov/Symphonia/issues/585, fixed by the open pull
request https://github.com/pdeljanov/Symphonia/pull/587.

## Provenance

- **Upstream:** https://github.com/pdeljanov/Symphonia
- **Vendored version:** `0.6.1` from crates.io, with `src/decoder.rs` replaced
  by PR #587's at commit `3158f50a88ae4a53478a4b393054bd944aed785b`
  (https://github.com/baylesj/Symphonia, branch `flac-33bit-side`).  No other
  file changed.
- **License:** MPL-2.0 (see `LICENSE`).

## Checked

Decoded output is bit-identical to the reference `flac` 1.5.0 decoder for
32-bit / 96 kHz, 24-bit / 96 kHz and 16-bit / 48 kHz stereo files (2026-11-13).

## Drop this when

PR #587 is merged and released: delete this folder and its `[patch.crates-io]`
line, and bump `symphonia` to that release.

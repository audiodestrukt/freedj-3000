# Vendored Bungee (open-source "Basic" edition)

Time stretcher used by `opendeck-timestretch` as the alternative to Rubber
Band (`OPENDECK_STRETCH=bungee`).  Copied, not a submodule, like
`../rubberband`.

| Component | Source | Commit | License |
|---|---|---|---|
| bungee (`bungee/`, `src/`) | https://github.com/kupix/bungee | 052451eff35165dae16221092447be0ce521dd24 (2026-09-27) | MPL-2.0 (`LICENSE`) |
| pffft (`pffft/`) | https://bitbucket.org/jpommier/pffft | 02fe7715a5bf8bfd914681c53429600f94e0f536 | BSD-like (`pffft/README.md`) |
| Eigen headers (`eigen/Eigen`) | https://gitlab.com/libeigen/eigen | c29c800126982c561e8d0b9255dc65474cd98de3 | MPL-2.0 (`eigen/COPYING.MPL2`) |

Eigen is pruned to what `<Eigen/Core>` needs (`Core`, `src/Core`,
`src/misc` without the LAPACKE headers, `src/plugins`).  Bungee's command-line
tool and its cxxopts dependency are not vendored.  No source changes.

Built by `crates/timestretch/build.rs` with the `cc` crate: pffft as C,
Bungee as C++20, plus our thin C shim `crates/timestretch/csrc/bungee_shim.cpp`.

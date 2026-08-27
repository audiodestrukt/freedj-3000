# freedj on iPad — Xcode scaffold

A minimal iPad app that links the `opendeck-app` Rust **staticlib** and calls its
`freedj_ios_main()` entry (`crates/app/src/lib.rs`). The design + rationale live
in [`docs/design/mobile-tablet-port.md`](../docs/design/mobile-tablet-port.md).

> **Status: hand-authored scaffold, NOT built or tested** (no iOS toolchain on
> the dev machine). Expect to adjust build settings on the Mac. If the project
> fights you, regenerate it with `cargo-mobile2` (below) — the Rust side, plist,
> entitlements and build script are the reliable parts; the `.xcodeproj` is the
> guess.

## What's here

| File | What it is |
|---|---|
| `freedj.xcodeproj/` | the Xcode project (one iPad app target) |
| `freedj/main.m` | ObjC entry → calls the Rust `freedj_ios_main()` |
| `freedj/Info.plist` | portrait, full-screen, `NSLocalNetworkUsageDescription` |
| `freedj/freedj.entitlements` | `com.apple.developer.networking.multicast` (for Link) |
| `build-rust.sh` | Xcode "Build Rust" phase: `cargo build --lib` for the target arch |

The Rust groundwork is already on `main`: `main()` is split into a reusable
`run(Config)`, and the crate builds as a staticlib (`[lib] crate-type = [...,
"staticlib"]`). Verified on Linux: `libopendeck_app.a` builds and the desktop bin
still runs.

## Build it (on a Mac)

1. **Toolchain:**
   ```sh
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
   ```
2. Open `ios/freedj.xcodeproj` in Xcode.
3. In the target's **Signing & Capabilities**: set your **Team** and a unique
   **Bundle Identifier** (the placeholder is `com.example.freedj`).
4. Build/run on your iPad (a paid developer account is needed for on-device +
   the multicast entitlement; a free account gives 7-day installs but **not** the
   multicast entitlement, so Link won't work under a free account).

## Two things to finish before it actually runs

1. **Bundle the assets.** The app loads a track and the faceplate image from
   paths relative to the working directory (`techno.mp3`,
   `reference/photos/XDJ1000Mk2-faceplate.jpg`). On iOS those must resolve to the
   **app bundle**. Simplest: add both files to the target's **Resources** (Copy
   Bundle Resources) and make `freedj_ios_main` resolve paths against the bundle
   dir (via `NSBundle`/`CFBundle`), or `include_bytes!` them and write to a temp
   dir on launch. This is a small, deliberate TODO — the scaffold ships with the
   desktop paths.
2. **Request the multicast entitlement** from Apple for your App ID (a form;
   paid account; usually granted in a few days), then make sure the provisioning
   profile includes it. Without it the app won't provision with
   `freedj.entitlements`.

## The first test that actually matters

Before polishing anything, **verify ProDJ Link broadcast works on the device**
(with the entitlement, over a USB-C ethernet dongle to the XDJ's network). Watch
the log for `ProDJ status: player N ...` — that means freedj is receiving the
XDJ's broadcast. If broadcast is silently dropped despite the entitlement, fall
back to unicast tempo-follow (see the design doc). This gates the whole demo, so
test it first.

## Fallback: regenerate with cargo-mobile2

If the hand-authored project doesn't build, this is the reliable path:
```sh
cargo install cargo-mobile2
cargo mobile init      # generates gen/apple/*.xcodeproj wired to the crate
```
Then port over `freedj/Info.plist` and `freedj/freedj.entitlements` (the
freedj-specific keys) into the generated project.

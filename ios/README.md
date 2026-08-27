# freedj on iPad — Xcode project

A minimal iPad app that links the `opendeck-app` Rust **staticlib** and calls its
`freedj_ios_main()` entry (`crates/app/src/lib.rs`). The design + rationale live
in [`docs/design/mobile-tablet-port.md`](../docs/design/mobile-tablet-port.md).

> **Status: builds and runs.** Verified on Xcode 16.4 / iOS 18.5 SDK: builds for
> both `iphonesimulator` (arm64) and `iphoneos` (arm64), installs in an iPad Pro
> 11" simulator, and comes up with the deck UI rendering over Metal, the Rubber
> Band R3 audio pipeline running, MiniBPM detecting the grid, and the Pro DJ Link
> listener receiving status from a real XDJ-1000MK2 on the LAN.
>
> Not yet verified **on device**. The build signs cleanly ("Apple Development:
> Dan Newcome" / "iOS Team Provisioning Profile: \*") and produces a complete
> `.app` with the track bundled, but installing was blocked by Xcode 16.4 being
> older than the target iPad — see *Your Xcode must be newer than the device*.
> The on-device Link broadcast probe remains the test that gates the demo.

## What's here

| File | What it is |
|---|---|
| `freedj.xcodeproj/` | the Xcode project (one iPad app target) |
| `freedj/main.m` | ObjC entry → calls the Rust `freedj_ios_main()` |
| `freedj/Info.plist` | portrait, full-screen, `NSLocalNetworkUsageDescription` |
| `freedj/freedj.entitlements` | `com.apple.developer.networking.multicast` (for Link) |
| `build-rust.sh` | Xcode "Build Rust" phase: `cargo build --lib` for the target arch |

## Build it

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios

make ios-sim                          # build + install + launch in a simulator
make ios-sim TRACK=~/music/x.mp3      # ...with a specific track bundled
make ios                              # signed device build
make ios-device TRACK=~/music/x.mp3   # ...plus install + launch on a connected
                                      #    iPad, streaming its log to the terminal
```

Signing is already wired up in the project: team `W527ZN3X52`, bundle id
`com.audiodestruct.opendeck`, automatic signing. `make ios` passes
`-allowProvisioningUpdates` so the profile is created/refreshed as needed.

A paid developer account is needed for on-device installs plus the multicast
entitlement. A free account gives 7-day installs but **not** the multicast
entitlement, so Link won't work under a free account.

### Your Xcode must be newer than the device

This is the one that will waste your afternoon. Installing to a device needs a
Developer Disk Image with a **variant matching that device's chip**, and the DDI
ships inside Xcode. An Xcode older than the device can't produce one, and every
downstream step fails with a different-looking error:

```
Failed to find image for variant/identity:
  (variant: DeveloperDiskImage | boardID: 20 | chipID: 33074 | securityDomain: 1)
```

Symptoms of this one root cause, in the order you'll hit them: `xcodebuild`
reports the device `is not available because the Developer Disk Image is not
mounted`; the device never gets auto-registered, so it's missing from the
provisioning profile's device list; and `devicectl device install` fails to
mount the DDI. Another tell is Xcode naming the device model wrongly (it falls
back to the nearest model it knows) — a reliable sign it predates the hardware.

There is no workaround inside the old Xcode. Install a newer one.

### First-time device setup

1. Connect over **USB-C** and unlock the iPad.
2. Pair: `xcrun devicectl manage pair --device <identifier>`, then accept the
   **Trust This Computer** prompt on the iPad. Check with
   `xcrun devicectl list devices` — you want `connected`, not
   `available (pairing)` or `connected (no DDI)`.
3. Enable **Developer Mode** on the iPad: Settings → Privacy & Security →
   Developer Mode → on → restart → confirm after reboot. The row only appears
   once a Mac has attempted a development install, so try one first if you don't
   see it. Skipping this gives
   `The operation failed because Developer Mode is disabled`.
4. Accept any "additional components" prompt Xcode shows on connect — that's the
   device support it needs.

### If `cargo` isn't found during the build

Xcode does not source your login shell, so `cargo` is almost never on `PATH`.
`build-rust.sh` covers the two normal installs — rustup.rs (`~/.cargo/bin`) and
Homebrew's `rustup` formula (`/opt/homebrew/opt/rustup/bin`, which puts *nothing*
in `~/.cargo/bin`). If yours lives somewhere else, add it to the `PATH` line in
that script.

## How the track gets in

The app plays **the first audio file it finds in the app bundle** (`.mp3`, `.m4a`,
`.aac`, `.wav`, `.aiff`, `.flac`, sorted for a stable pick).

Tracks are gitignored (`*.mp3`), so they can't be file references in the
`.pbxproj` — and on a device the track has to be inside the bundle *before* code
signing seals it. So the `Bundle Track` build phase (`bundle-track.sh`) copies one
in: from `FREEDJ_TRACK` if set, else the first audio file in the repo root. It
clears any previously bundled track first, so switching tracks doesn't leave the
old one to win on sort order. `make ios TRACK=...` sets `FREEDJ_TRACK` for you.

No track is not a build error — you still get a signed, installable bundle, and
the app logs an actionable message on launch.

`freedj_ios_main` chdirs to the bundle directory first, so every other relative
resource path the desktop build uses resolves against the bundle too. With no
audio file present, the app logs an actionable error rather than failing inside
the decoder.

The **faceplate photo** (`reference/photos/XDJ1000Mk2-faceplate.jpg`) is
deliberately not in the repo (trade dress). Without it the app logs
`faceplate image: cannot read ... — screen-only` and falls back to the
screen-only layout, which is landscape — and therefore overflows the
portrait-locked iPad window. Bundle a faceplate photo to get the intended
portrait layout, or unset `OPENDECK_FACEPLATE` in `freedj_ios_main` and allow
landscape in `Info.plist`.

## Rubber Band is vendored, not a system library

`crates/timestretch` links Rubber Band, which has no system package on iOS (or
macOS). `third_party/rubberband` holds the upstream 4.0.0 source, and
`crates/timestretch/build.rs` compiles its `single/RubberBandSingle.cpp` — one
dependency-free translation unit — with the `cc` crate whenever pkg-config
doesn't turn up a system library. Linux/Pi builds still prefer the system
`librubberband-dev` exactly as before.

On Apple platforms that single-file build uses the vDSP FFT, so the link needs
`-framework Accelerate`. Cargo's link directives do **not** propagate out of a
staticlib, so Accelerate is also listed in the Xcode target's `OTHER_LDFLAGS`
alongside the other frameworks. Anything else the Rust side starts linking must
be added there too.

## Before the demo: prove Link broadcast works on the device

1. **Request the multicast entitlement** from Apple for your App ID (a form; paid
   account; usually granted in a few days), then make sure the provisioning
   profile includes it.

   Because an ungranted entitlement makes the build fail to provision, the
   entitlements file is **off by default**: `CODE_SIGN_ENTITLEMENTS` reads
   `$(FREEDJ_ENTITLEMENTS)`, which is empty. Opt in once Apple approves:

   ```sh
   make ios-device MULTICAST=1
   ```
2. **Verify Pro DJ Link broadcast on the iPad itself** (over a USB-C ethernet
   dongle to the XDJ's network). Watch the log for `ProDJ status: player N ...`
   — that means freedj is receiving the XDJ's broadcast. If broadcast is silently
   dropped despite the entitlement, fall back to unicast tempo-follow (see the
   design doc). This gates the whole demo, so test it before polishing anything.

   Note the simulator shares the Mac's network stack, so Link working there says
   nothing about the device sandbox — only the on-device test counts.

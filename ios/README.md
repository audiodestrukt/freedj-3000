# freedj on iPad — Xcode project

A minimal iPad app that links the `opendeck-app` Rust **staticlib** and calls its
`freedj_ios_main()` entry (`crates/app/src/lib.rs`). The design + rationale live
in [`docs/design/mobile-tablet-port.md`](../docs/design/mobile-tablet-port.md).

> **Status: runs on device, and Pro DJ Link works against real hardware.**
> Verified 2026-08-27 on an iPad Air 13" (M4), iPadOS 26.5, built with Xcode 26.6
> / iOS 26.5 SDK:
>
> - Deck UI renders over Metal on the M4 GPU, full screen, ~55 fps sustained with
>   no frame spikes.
> - Rubber Band R3 timestretch pipeline runs clean — **zero audio underruns or
>   overruns** across every device run.
> - MiniBPM detects the grid; playback of a normal MP3 sounds correct.
> - **Pro DJ Link, both directions, against a real XDJ-1000MK2:** receives its
>   announces and status, takes tempo master from it, sends pitch, and the XDJ
>   follows freedj's tempo. Both broadcast *and* receive worked **without** the
>   multicast entitlement, over wired ethernet with Wi-Fi off — which the design
>   doc did not assume.
>
> Also builds and runs on the `iphonesimulator` (arm64) — but note the simulator
> shares the Mac's network stack, so Link working there says nothing about the
> device sandbox, and simulator audio glitches regardless. Only the device
> results above count.
>
> Confirmed (2026-08-27): plain UDP broadcast over wired ethernet is permitted
> on device with no multicast entitlement, so standalone sync needs neither the
> entitlement nor a Mac. Remaining Link work is interface re-bind robustness (#36).

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

A paid developer account is needed for on-device installs. The multicast
entitlement turned out **not** to be required: Pro DJ Link sync works on device
over a wired USB-C ethernet dongle (Wi-Fi off) **without** it (verified
2026-08-27). A free 7-day sideload should therefore be enough for wired-ethernet
Link; the entitlement only matters if you later need Wi-Fi multicast.

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

There is no workaround inside the old Xcode. Install a newer one — this port cost
an afternoon to Xcode 16.4 against an iPadOS 26.5 device before that was clear.
Xcode 26 also splits platform support out of the base install, so after
installing it: `sudo xcodebuild -license accept`, `sudo xcodebuild -runFirstLaunch`,
then `xcodebuild -downloadPlatform iOS` (~8.5 GB) — without that last one the
build fails with `iOS 26.5 must be installed to run the scheme`.

Automatic signing also needs the Apple ID signed in under Xcode → Settings →
Accounts before it can register a new device; `-allowProvisioningUpdates` alone
will not do it, and reports the device as not registered.

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

## Link broadcast on the device — verified working

Pro DJ Link sync is confirmed on device (2026-08-27) over a USB-C ethernet
dongle with **Wi-Fi off**, with **no multicast entitlement** and no Mac relay:
freedj announces, receives the XDJ's broadcast beat clock, and the XDJ syncs.

1. **Launch with the dongle already connected.** freedj binds its Link interface
   at startup, then re-selects onto a discovered player's subnet once it hears
   one; if you plug the dongle in *after* launch and it hasn't been addressed
   yet, relaunch. Making this fully automatic on hotplug is issue #36.
2. **Confirm it's up:** watch the log for `ProDJ status: player N ...` (freedj is
   receiving the XDJ's broadcast) and `ProDJ Link: sending as player N from <IP>
   (<iface>) to <bcast>` (it chose the right interface).

The multicast entitlement is **not** needed for wired ethernet. If you ever do
want it (Wi-Fi multicast), the entitlements file is off by default —
`CODE_SIGN_ENTITLEMENTS` reads `$(FREEDJ_ENTITLEMENTS)`, empty unless you opt in
with `make ios-device MULTICAST=1` once Apple grants it for your App ID.

Note the simulator shares the Mac's network stack, so Link working there says
nothing about the device sandbox — only the on-device test counts.

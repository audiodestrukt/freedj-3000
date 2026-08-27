# Design: porting freedj to tablets (iPad / Android)

Status: **iPad spike WORKING end-to-end, standalone** (2026-08-27). What it takes
to run freedj — screen-only or as the `--faceplate` touch deck — on an iPad or an
Android tablet. The touch faceplate is the natural mobile form, so this pairs
with [`faceplate-and-hardware.md`](faceplate-and-hardware.md).

## Spike result (2026-08-27): runs on iPad; sync still gated

freedj runs on a 13" iPad — full photo faceplate, our LCD, clean audio. Over a
USB-C ethernet dongle to the XDJ's network it **loads tracks from the XDJ's
library**. What we learned, including a correction:

- **Standalone Link broadcast WORKS — no entitlement, no Mac (resolved 2026-08-27).**
  On the iPad alone, over a wired USB-C ethernet dongle with **Wi-Fi off**, Pro DJ
  Link **sync** works: freedj broadcasts its announce and receives the XDJ's beat
  clock, and the XDJ syncs. No `com.apple.developer.networking.multicast`
  entitlement and no Mac relay were needed. Track loading (unicast NFS / dbserver
  to the XDJ's IP) also works standalone with just the local-network permission.
  - **What the earlier failures actually were:** a **startup interface-binding**
    issue, not an iOS broadcast block. freedj latches its Link interface once at
    launch (`link_interface()` = first non-loopback IPv4 with a broadcast addr);
    if the dongle wasn't up/addressed yet, it bound nothing usable and broadcast
    went nowhere. **Restarting the app with ethernet already present** made it
    bind the dongle and sync came up. The Mac-tether "relay" was a red herring —
    plain wired ethernet is sufficient.
  - This kills the entitlement-vs-interface question entirely: it was neither the
    entitlement nor Wi-Fi-vs-dongle *selection* — it was launch **timing**.
- **Remaining (robustness only, issue #36):** re-evaluate/re-bind the Link
  interface on network change (dongle hotplug, address assigned after launch) so
  no restart is ever needed. Nice-to-have for a true out-of-the-box experience,
  not a blocker — the demo works today by launching with ethernet connected.
- **cpal does not configure the `AVAudioSession` on iOS** → glitchy/underrunning
  audio until you set it up. Fix (in the app's ObjC entry, before audio starts):
  category `Playback`, preferred sample rate 48 kHz, IO buffer ~10–20 ms, then
  activate. Link `-framework AVFoundation`.
- The `run(Config)` split + `[lib]` staticlib target were all the Rust structure
  the port needed; the iOS entry is one `#[no_mangle] extern "C"` fn.

Remaining: interface re-bind robustness so no restart is needed (#36); bundle the
default track/faceplate as app resources; scale the
GUI up for the 13" screen (#33); XDJ-style jog center spinner (#34) and touch
feedback (#35). iOS build scaffold + guide live under `ios/` (and the `ios`
branch).

## TL;DR

The **app's libraries already target both platforms** — nothing in the stack is a
hard blocker. The cost is almost entirely **platform toolchain + distribution +
a few OS-specific APIs**, not freedj's code:

- **iPad:** every library freedj uses targets iOS, *including MIDI*. The tax is
  Apple's: a Mac to build, Xcode wrapper, code signing, an Apple Developer
  account, App Store/TestFlight (or limited sideload), plus iOS local-network
  permission for ProDJ Link.
- **Android:** the render/audio stack targets Android, but **MIDI needs a JNI
  bridge** (midir has no Android backend) and ProDJ Link needs a multicast lock.
  Upside: you can sideload an APK freely, no signing gatekeeper.

For MIDI specifically, **iPad is the better tablet target** — the opposite of
what you'd guess.

## What's shared (both platforms)

The whole render/UI/audio stack is portable:

| Layer | Crate | iOS | Android |
|---|---|---|---|
| Window / lifecycle | `winit` | ✓ (UIKit) | ✓ (`android-activity`) |
| GPU | `wgpu` | ✓ (Metal) | ✓ (Vulkan/GLES) |
| UI | `egui` | ✓ | ✓ |
| Audio out | `cpal` | ✓ (CoreAudio) | ✓ (AAudio/oboe) |

Two more things already in our favor:

- **Touch is native.** egui folds touch into one pointer, and the faceplate's
  targets (jog, fader, buttons) are built for fingers — the on-screen deck is a
  *better* fit on a tablet than on the desktop.
- **The input bus.** Physical/MIDI/touch all emit the same `ControlEvent`
  ([`INPUT_PLAN.md`](../INPUT_PLAN.md)), so a platform MIDI shim is one more
  adapter, not an engine change.

What always needs per-platform work regardless of target: the **entry point**
(`winit` wants `android_main` / a UIKit `@main`, not `fn main`), **window
sizing/orientation** (fractional layout scales, but full-screen + safe-area
insets need handling), and **file access** to tracks (scoped storage / document
pickers instead of a path).

## iPad (iOS / iPadOS)

**Ports cleanly:** winit(UIKit) + wgpu(Metal) + egui + cpal(CoreAudio). And —
corrected from an earlier assumption — **MIDI works**: midir 0.10.3 has an
explicit iOS CoreMIDI backend:

```rust
// midir/src/backend/mod.rs
#[cfg(all(target_os = "ios", not(feature = "jack")))]
mod coremidi;
```

So a USB-C MIDI controller (the DJ2Go via a USB-C adapter), BLE MIDI, or network
MIDI all reach freedj through midir's iOS backend — no shim needed.

**The tax (all Apple, none of it code):**
- Cross-compile to `aarch64-apple-ios`; wrap in an **Xcode** project.
- **Code signing** + an **Apple Developer account** ($99/yr) for on-device.
- **A Mac to build** (no way around this for iOS).
- **Distribution:** TestFlight/App Store, or limited sideload (7-day free cert /
  AltStore-style). No free "drop a file on it."

**iOS-specific gotchas to verify:**
- **cpal RT latency** on iOS AudioUnit differs; re-validate the timestretch
  budget (`docs/design/rt-audio-isolation.md`, `PERFORMANCE.md`).
- **Trade dress.** The Pioneer-derived faceplate (even redacted) is a review/
  legal risk for App Store distribution; fine for your own device / TestFlight.

### ProDJ Link on iPad (the one Apple-gated feature)

Link makes the demo compelling (real-XDJ sync), so it's worth the gate. freedj's
Link *code* should port — it's std/`socket2` UDP + `getifaddrs` interface
detection, all of which work on iOS, and a USB-C **ethernet dongle** gets
enumerated like any interface. The physical layer (dongle → XDJ's network) is
fine. The gate is entirely iOS's network policy, applied at the socket layer
regardless of Wi-Fi vs ethernet:

1. **`NSLocalNetworkUsageDescription`** (Info.plist) + a user permission prompt —
   needed for any local-network traffic. Easy.
2. **`com.apple.developer.networking.multicast` entitlement** — turned out **not
   required** for our case (see below). It gates custom multicast/broadcast and
   needs a paid account + Apple request form, but ProDJ Link over **wired
   ethernet, Wi-Fi off** works without it.

**RESOLVED on device (2026-08-27): UDP broadcast passes on iOS over wired
ethernet with no multicast entitlement.** freedj announces and receives the XDJ's
broadcast beat clock over a USB-C ethernet dongle (Wi-Fi off), and the XDJ syncs.
The earlier "does iOS even pass broadcast?" unknown is answered — yes. The initial
failures were a **startup interface-binding** bug (freedj binds its Link interface
once at launch; if the dongle isn't addressed yet it binds nothing), fixed by
launching/relaunching with ethernet already connected. So the unicast tempo-follow
fallback below is **not needed** for the demo; it's kept only as historical context.

Historical fallback (unused): if broadcast had been blocked, freedj would read
tempo from the XDJ's **unicast** status packets (50002 carries BPM) — tempo-follow
without tight phase, a lesser demo.

Remaining code work (issue #36, robustness only): freedj picks the interface once
in `link_interface()`; re-evaluate/re-bind it on network change (dongle hotplug,
late address assignment) so no restart is needed. Not a blocker — launch with
ethernet connected and it works today.

## Android

**Ports cleanly:** winit(`android-activity`) + wgpu(Vulkan) + egui +
cpal(AAudio/oboe). Sideloading an APK is free — no signing gatekeeper.

**Needs real work:**
- **MIDI has NO midir backend.** midir's targets are windows/macos/ios/linux/
  jack/wasm32 — **no `android`**. USB-MIDI on Android goes through the Java
  `android.media.midi` API, so the controller path needs a **JNI bridge** (or a
  small Kotlin shim) feeding the input bus. A **touch-only** build skips this.
- **ProDJ Link receive needs a `WifiManager.MulticastLock`** — Android drops
  broadcast/multicast to apps otherwise (sending is fine without it).
- **Build/packaging:** `cargo-ndk` for `aarch64-linux-android`, `android-activity`
  entry point, wrapped in a Gradle/APK shell.
- **cpal RT** on AAudio: same latency re-validation as iOS.

## Comparison

| Concern | Linux (today) | iPad | Android |
|---|---|---|---|
| Render/UI/audio stack | ✓ | ✓ | ✓ |
| Touch | mouse-as-touch | native ✓ | native ✓ |
| **MIDI controllers** | ✓ (ALSA) | ✓ (CoreMIDI, in midir) | ✗ midir → **JNI shim** |
| ProDJ Link | ✓ | local-network perm (broadcast over **wired ethernet** works w/o multicast entitlement) | **MulticastLock** |
| Build | `cargo`/`make` | Xcode + **Mac** + cross-compile | `cargo-ndk` + Gradle |
| Signing | none | **Apple cert required** | self-sign, free |
| Distribution | run it | App Store / TestFlight / limited sideload | **free sideload** APK |
| Cost gate | — | Mac + $99/yr dev acct | ~none |

## Code footprint

Small, and concentrated in bootstrap + platform glue — the app logic doesn't
change, because the stack is portable. Nothing in `screen.rs`, the faceplate,
`engine`, `timestretch`, or the ProDJ protocol needs touching; it compiles as-is.

**Core spike (screen + audio, one platform): ~3 Rust files**
- `crates/app/src/main.rs` — split `fn main()` into a reusable `run()` that the
  platform entry point calls, plus mobile window/orientation handling. *(the one
  substantive edit)*
- `crates/app/Cargo.toml` — platform-gated deps + `crate-type` (`cdylib` on
  Android).
- one new small **platform module** (an Android `lib.rs` with `android_main`, or
  the iOS entry shim).

**Full port adds a handful of new adapters: ~5–8 Rust files total**
- `browser.rs` + a file-picker shim — track loading via a document picker, not a
  path (~2).
- Android only: `midi.rs` + a new JNI `midi_android` module (~2). iPad needs
  none — midir's iOS backend just works.
- `prodj.rs` — a multicast-lock / local-network-permission hook (~1).

**Not Rust, but new:** a per-platform **build-scaffold directory** — Android
Gradle (manifest + `build.gradle`, ~4–6 files) *or* an iOS Xcode project
(`Info.plist`, entitlements, `project.pbxproj`, ~3–5 files). Boilerplate/config
in its own dir, not edits to existing files.

The `main.rs` entry-point restructure is the only real code change; everything
else is additive. This small footprint is the direct payoff of the portable
stack and the single `ControlEvent` input bus.

## Recommended sequence (iPad demo, Link required)

The demo target is an **iPad running the full faceplate, synced to a real XDJ over
Link** (a USB-C ethernet dongle to the XDJ's network). Because Link is the gated,
uncertain part, front-load it:

1. **Groundwork (done / Linux):** `main()` split into `run(Config)` (commit
   `cba1dba`) so the iOS entry just calls `run`. No Mac needed.
2. **Apple prerequisites (parallel, no code):** paid dev account for on-device
   install. The multicast entitlement turned out **not** to be needed (broadcast
   works over wired ethernet).
3. **DONE — minimal iOS build + broadcast proven:** freedj launches on the iPad
   (faceplate + audio + a bundled track), and ProDJ Link broadcast send/receive
   works standalone over the dongle (Wi-Fi off), so the full-sync demo is live.
4. **Polish the demo:** track file picking, faceplate touch already works, and
   (free on iPad) MIDI if a controller is wanted.

Remaining Link work is robustness only: interface re-bind on network change so no
restart is needed (#36).

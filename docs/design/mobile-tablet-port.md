# Design: porting freedj to tablets (iPad / Android)

Status: **research + plan, not started** (2026-08-27). What it would take to run
freedj — screen-only or as the `--faceplate` touch deck — on an iPad or an
Android tablet. The touch faceplate is the natural mobile form, so this pairs
with [`faceplate-and-hardware.md`](faceplate-and-hardware.md).

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
- **ProDJ Link ↔ local-network privacy.** iOS 14+ requires
  `NSLocalNetworkUsageDescription` (Info.plist) + a user permission prompt for
  *any* local-network traffic. Broadcast/multicast discovery may additionally
  need the `com.apple.developer.networking.multicast` **entitlement, which Apple
  must approve** — a real risk for the Link feature; verify early.
- **cpal RT latency** on iOS AudioUnit differs; re-validate the timestretch
  budget (`docs/design/rt-audio-isolation.md`, `PERFORMANCE.md`).
- **Trade dress.** The Pioneer-derived faceplate (even redacted) is a review/
  legal risk for App Store distribution; fine for your own device / TestFlight.

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
| ProDJ Link | ✓ | local-network perm + maybe multicast **entitlement** | **MulticastLock** |
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

## Recommended first spike

**Screen-only, touch, one platform, no Link, no MIDI** — the smallest thing that
proves the toolchain:

1. Pick **Android** if the goal is "a deck on a tablet I own, cheaply" (free
   sideload, no Mac); pick **iPad** if MIDI-controller support or the nicer
   touch/audio stack matters more (and you have a Mac).
2. Get freedj **building and launching** with the platform entry point +
   `cargo-ndk`/Xcode — just the screen rendering and audio out.
3. Then add, in order of payoff: **touch faceplate** (already built), **track
   file picking**, **MIDI** (free on iPad; JNI shim on Android), **ProDJ Link**
   (the permission/entitlement work).

Nothing here is committed — this is the reference for when a tablet build moves
up the list.

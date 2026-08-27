# Design: porting freedj to tablets (iPad / Android)

Status: **iPad spike WORKING end-to-end, standalone** (2026-08-27). What it takes
to run freedj — screen-only or as the `--faceplate` touch deck — on an iPad or an
Android tablet. The touch faceplate is the natural mobile form, so this pairs
with [`faceplate-and-hardware.md`](faceplate-and-hardware.md).

## Spike result (2026-08-27): runs on iPad; sync still gated

freedj runs on a 13" iPad — full photo faceplate, our LCD, clean audio. Over a
USB-C ethernet dongle to the XDJ's network it **loads tracks from the XDJ's
library**. What we learned, including a correction:

- **Unicast works standalone; broadcast does not (no entitlement).** Track
  loading is **unicast** (NFS / dbserver straight to the XDJ's IP) and works on
  the iPad alone with just the local-network permission. Pro DJ Link **sync** is
  **broadcast** (the XDJ's beat clock + freedj's announce) — and it worked *only
  while the iPad was USB-tethered to the Mac*, which was relaying broadcast. With
  the Mac disconnected, track loads still work but **sync stops**. So the
  `com.apple.developer.networking.multicast` **entitlement gate is real for
  broadcast**, even over wired — the Mac was the enabler, not the dongle. (Earlier
  in this spike we briefly thought wired sidestepped it; the sync-stops-when-
  untethered result disproved that.)
- **Implication for a self-contained demo:** standalone **sync** needs either the
  Apple multicast entitlement (paid account + request), or a **unicast
  tempo-follow** path — freedj unicasts its announce to the XDJ's known IP and
  reads BPM from the XDJ's unicast status packets (tempo only, no tight phase; and
  it must not depend on receiving broadcast beats). Track loading needs neither.
- **cpal does not configure the `AVAudioSession` on iOS** → glitchy/underrunning
  audio until you set it up. Fix (in the app's ObjC entry, before audio starts):
  category `Playback`, preferred sample rate 48 kHz, IO buffer ~10–20 ms, then
  activate. Link `-framework AVFoundation`.
- The `run(Config)` split + `[lib]` staticlib target were all the Rust structure
  the port needed; the iOS entry is one `#[no_mangle] extern "C"` fn.

Remaining: confirm the entitlement path (or the unicast tempo-follow fallback) for
standalone sync; bundle the default track/faceplate as app resources; scale the
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
2. **`com.apple.developer.networking.multicast` entitlement** — required to send
   or receive custom multicast **and broadcast**. It needs a **paid** developer
   account and an Apple **request form** (usually granted in days). The free
   7-day sideload can't hold this entitlement.

**The one genuine unknown — verify FIRST, before building the full app:** does
iOS actually pass UDP **broadcast** (send + receive) with the multicast
entitlement? ProDJ Link discovery + beat clock are broadcast; the whole iOS
regime was built around multicast/Wi-Fi, and broadcast-over-ethernet is untested.
The very first on-device test should be a minimal build that just tries to
announce and receive the XDJ's broadcast beats. Two outcomes:
- **Broadcast works** → full Link, compelling demo. Done.
- **Broadcast blocked** → fall back to reading tempo from the XDJ's **unicast**
  status packets (50002 carries BPM), which gives tempo-follow but not tight
  phase — a lesser demo. Knowing which world we're in gates the rest, so front-
  load this test; don't discover it at the end.

Possible code tweak: on iOS the broadcast socket may need explicit binding to the
dongle's interface address (freedj already picks the interface in
`link_interface()`; verify it selects the ethernet one and that `SO_BROADCAST` +
subnet-directed sends egress it).

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

## Recommended sequence (iPad demo, Link required)

The demo target is an **iPad running the full faceplate, synced to a real XDJ over
Link** (a USB-C ethernet dongle to the XDJ's network). Because Link is the gated,
uncertain part, front-load it:

1. **Groundwork (done / Linux):** `main()` split into `run(Config)` (commit
   `cba1dba`) so the iOS entry just calls `run`. No Mac needed.
2. **Apple prerequisites (parallel, no code):** paid dev account; request the
   `com.apple.developer.networking.multicast` entitlement.
3. **Minimal iOS build + the broadcast probe:** get freedj launching on the iPad
   (faceplate + audio + a bundled track) and — as the *first* on-device test —
   confirm ProDJ Link broadcast send/receive works with the entitlement over the
   dongle. This decides whether the compelling (full sync) demo is possible or we
   fall back to unicast tempo-follow.
4. **Polish the demo:** track file picking, faceplate touch already works, and
   (free on iPad) MIDI if a controller is wanted.

If Link broadcast turns out blocked, reassess: unicast tempo-follow, or pivot the
demo framing. Nothing past step 1 is committed.

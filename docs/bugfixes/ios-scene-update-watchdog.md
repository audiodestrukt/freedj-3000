# iOS crash: `0x8BADF00D` scene-update watchdog after backgrounding

**Status:** Fixed in `caa3ce5` (shipped in `ios-v0.1.1`).
**Affected:** iOS/iPadOS TestFlight build `0.1.0` (build `1788281347`).
**Reported from:** a real iPad (`iPad16,10`, iPadOS 26.5.2), crash report
`freedj-2026-09-01-161036.ips`.

---

## Symptom

The app "randomly crashed" on device. There was no visible error, no beachball
we set — it just vanished. It had been open and in the foreground for hours
beforehand.

## What the crash report actually said

This was **not** a Rust panic, a null dereference, or an out-of-bounds — nothing
in *our* code faulted. The decisive fields in the `.ips`:

```
exception:   EXC_CRASH (SIGKILL)
termination: FRONTBOARD  code: 0x8BADF00D
             "scene-update watchdog transgression:
              app ... exhausted real (wall clock) time allowance of 10.00 seconds"
WatchdogEvent:      scene-update
WatchdogVisibility: Foreground
```

`0x8BADF00D` ("ate bad food") is the iOS **watchdog**: the OS killed us because
the **main thread failed to complete a scene-update transaction within 10
seconds**. A watchdog kill means a *hang*, not a code crash.

Two more facts framed it:

- **`procLaunch` 10:07:28 → `captureTime` 16:10:35** — the process had been
  foreground for ~6 hours before dying. So this fired on a *transition*, not at
  startup or under load.
- The crashing thread (`com.apple.main-thread`) was snapshotted inside
  `__CFRunLoopDoTimer` with **no freedj frames above it**. The watchdog snapshot
  is taken asynchronously to whatever the main thread is doing, so the stack does
  **not** name the culprit — a classic watchdog-report trap. The `termination`
  field is the signal; the stack is noise here.

### The red herring

The thread list showed the Pro DJ Link receive threads contending on a mutex —
`prodj-rx-50000` mid-`pthread_mutex_unlock` (slow path, waking waiters) while
`prodj-rx-50001`/`50002` sat in `__psynch_mutexwait`. That looks like a
deadlock, and it was our first suspect. It is **not** the cause: that mutex is
`LinkState::peers` (a `HashMap<u8, Ipv4Addr>` of at most ~6 players), locked only
briefly to insert a discovered peer. What the snapshot caught is a normal
insert-and-release, not a stall. Crucially, the main thread was **not** blocked
in `__psynch_mutexwait` — if the UI had deadlocked on that lock, the main-thread
stack would show the wait. It didn't.

## Root cause

winit's iOS backend emits `Suspended` when the app is backgrounded and `Resumed`
when it returns to the foreground. On iOS the window's native surface — the
`CAMetalLayer` that the wgpu `Surface` is built from — is **invalidated while the
app is suspended**. winit's documented lifecycle is therefore: build the graphics
context in `resumed()`, and **destroy it in `suspended()`**, rebuilding on the
next resume.

We did the first half and skipped the second:

- There was **no `suspended()` handler**, so on background we kept holding the
  window and the now-doomed wgpu surface.
- `resumed()` began with `if self.window.is_some() { return; }`. After the very
  first launch that guard is always true, so on every subsequent foreground
  `resumed()` **early-returned and never rebuilt the surface**.

The resulting sequence:

1. App is backgrounded (set down) after ~6 h foreground.
2. iOS invalidates the `CAMetalLayer`; our wgpu `Surface` is now dead.
3. App is foregrounded. `resumed()` bails (`window.is_some()`), so nothing is
   rebuilt.
4. `render_frame()` calls `surface.get_current_texture()`, which now fails every
   time; the handler logs and returns without presenting. The deck never
   repaints.
5. The main thread can't drive a valid frame, the foreground **scene-update
   transaction never completes**, and at 10 s iOS fires the watchdog and SIGKILLs
   us.

This also explains the report's CPU line — `~28 s application CPU, 37%` over the
window — that is the redraw path churning against the dead surface, not real
work. Thermal stayed `nominal` because the burst was brief.

## The fix

Add the missing lifecycle half. `crates/app/src/lib.rs`, in
`impl ApplicationHandler for DeckApp`:

```rust
fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
    log::info!("suspended: releasing window + GPU surface until resume");
    self.egui_state = None;
    self.renderer   = None;   // drops the wgpu Surface
    self.window     = None;
}
```

That's the whole change. It works because:

- `resumed()` already rebuilds the window, renderer, and egui state whenever
  `self.window` is `None` — which is now exactly the state `suspended()` leaves
  behind — so the recreate path is the one we already trust for first launch and
  Android resume.
- All **playback and sync state survives**: the audio ring, decoder position,
  Pro DJ Link `LinkState`, and the CPU-side waveform live on `self` (and on
  background threads), not in the window/renderer/egui objects we drop. On
  resume, `Renderer::new` re-uploads the waveform from the retained CPU copy, so
  the loaded track and its display come back intact.
- While suspended, nothing spins: `render_frame()` early-returns on
  `window == None`, and both the `RedrawRequested` re-arm and the hybrid pacer in
  `about_to_wait()` are guarded by `self.window.is_some()`.
- It is **cross-platform-safe**: desktop compositors (Wayland/X11) don't emit
  `Suspended`, so on the desktop build this method is simply never called.

## How to verify

This bug is invisible to the desktop build and to the screenshot harness — it
only manifests on a real background→foreground cycle on device. To confirm the
fix on the TestFlight build:

1. Launch the app, load/see the deck rendering.
2. Background it — press Home / swipe up, or lock the screen — and leave it a
   little while.
3. Foreground it again. It should repaint and keep running instead of being
   killed after ~10 s. Repeat a few times, including a longer background.

The device log line `suspended: releasing window + GPU surface until resume`
(followed by the `window …x… px` line from `resumed()`) confirms the lifecycle
is now firing on each cycle.

## Lessons for the next iOS crash

- **Read `termination` before the stack.** `0x8BADF00D` = watchdog/hang
  (lifecycle or a genuinely blocked/slow main thread), a different class of
  problem from an `EXC_BAD_ACCESS` code crash. The crashing-thread stack in a
  watchdog report is usually not where the time went.
- **Rust staticlib frames won't symbolicate** — release builds strip the
  staticlib, so `opendeck` frames show as raw offsets. Reason from the thread
  *names* (`audio-proc`, `prodj-rx-50001`, `AURemoteIO::IOThread`) and the
  system frames instead.
- **A surface-lifecycle bug looks like everything else** — a frozen UI, then a
  watchdog kill, with a plausible-looking mutex in the thread dump. On a
  mobile wgpu/Metal app, suspect the surface lifecycle first.

See also: `ios/RELEASING.md` (cutting a TestFlight build) and
`docs/design/mobile-tablet-port.md`.

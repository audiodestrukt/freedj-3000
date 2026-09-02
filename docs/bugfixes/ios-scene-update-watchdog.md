# iOS crash: `0x8BADF00D` scene-update watchdog after backgrounding

**Status:** First attempt `caa3ce5` (`ios-v0.1.1`/`0.1.2`) was wrong and
reverted; **fixed properly** in the follow-up `Occluded` rework (see below).
**Affected:** iOS/iPadOS TestFlight build `0.1.0` (build `1788281347`).
**Reported from:** a real iPad (`iPad16,10`, iPadOS 26.5.2), two crash reports:
`freedj-2026-09-01-161036.ips` (foreground, 10 s allowance) and
`freedj-2026-09-01-214312.ips` (**background**, 30 s allowance, **86 s of app
CPU** — the redraw loop spinning against the dead surface in the background).

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

### First attempt (reverted): tear down the window in `suspended()`

The initial fix (commit `caa3ce5`) added a `suspended()` that dropped
`egui_state`/`renderer`/`window`, letting `resumed()` rebuild them. It shipped in
`0.1.1`/`0.1.2` but was **the wrong lever**: on winit's iOS backend `Suspended`
is wired to `applicationWillResignActive` (see
`winit-0.30/src/platform_impl/ios/event_loop.rs`), which fires on *every*
interruption — a Control Center pull, a notification banner, an alert — not just
a real background. Tearing the whole `UIWindow` down and rebuilding it on each of
those is both wasteful and fragile, and recreating the window mid-run can leave
the app frozen on a stale frame ("all I see is a red line" — the waveform-shader
pass survives, the egui overlay is gone).

### Final fix: pause on `Occluded`, reconfigure the surface

winit wires `WindowEvent::Occluded` to
`applicationDidEnterBackground`/`applicationWillEnterForeground` — the *true*,
stable background/foreground signal. Hang the whole thing off that instead:

```rust
// window_event:
WindowEvent::Occluded(occluded) => {
    self.occluded = occluded;
    if !occluded {
        // returning to foreground: the CAMetalLayer's drawables were
        // invalidated in the background — reconfigure before drawing again
        if let (Some(r), Some(w)) = (&mut self.renderer, &self.window) {
            let sz = w.inner_size();
            r.resize(sz.width, sz.height);   // reconfigures the surface
            w.request_redraw();
        }
    }
}
```

- `Occluded(true)` (background): set `self.occluded`; `render_frame()`
  early-returns, `RedrawRequested` stops re-arming, and `about_to_wait()` idles on
  `ControlFlow::Wait`. The main thread quiesces, so there's **no redraw spin** and
  iOS can suspend the app cleanly — this is what removes the background
  scene-update watchdog (the 0.1.0 crash that burned 86 s of CPU in the
  background).
- `Occluded(false)` (foreground): reconfigure the surface (re-establishing the
  drawables the background invalidated), then kick a redraw to resume.
- **Safety net**: the surface-acquire path now reconfigures on
  `SurfaceError::Lost`/`Outdated` instead of just logging and returning, so a
  stale surface self-heals even if `Occluded` didn't fire first.

The window is never torn down, so there's no `UIWindow` churn and no stale-frame
freeze. Playback/Link/waveform state is untouched throughout. Desktop compositors
emit neither `Occluded` nor `Suspended`, so this is all iOS-only.

## How to verify

This bug is invisible to the desktop build and to the screenshot harness — it
only manifests on a real background→foreground cycle on device. To confirm the
fix on the TestFlight build:

1. Launch the app, load/see the deck rendering.
2. Background it — press Home / swipe up, or lock the screen — and leave it a
   little while. Also try quick interruptions (pull down Control Center, then
   dismiss) — these must NOT tear anything down now.
3. Foreground it again. It should repaint and keep running instead of being
   killed. Repeat a few times, including a longer background.

The device log lines `occluded (background): pausing render` and `un-occluded
(foreground): reconfiguring surface, resuming` confirm the lifecycle is firing on
each real background cycle (and staying quiet on mere interruptions).

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

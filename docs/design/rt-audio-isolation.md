# Design: real-time audio isolation

WORKSTREAMS A3. How to make freedj's audio glitch-free under load — the thing
that will bite on a Pi 5 running a compositor, a busy render thread, network
Link, and (eventually) two decks, all competing for four cores.

Nothing here is implemented yet. This is the plan, cheapest and most valuable
first.

## The two audio threads, and what each needs

freedj's audio path is a lock-free producer/consumer across one `rtrb` ring
buffer (`crates/app/src/audio.rs`):

- **Consumer — the cpal callback** (`build_output_stream`, `audio.rs:197`).
  This is the *hard* real-time thread: it must hand the device a full buffer
  every period or the output glitches. It only pops from the ring
  (`consumer.pop().unwrap_or(0.0)`, `:209`) — no allocation, no locks, good.
  **But cpal owns this thread**, so we do not spawn it and cannot trivially set
  its priority from our code. On PipeWire (the Pi default) it is already given
  RT priority via rtkit; on raw ALSA it inherits our scheduling.

- **Producer — `audio-proc`** (`audio.rs:175`). Runs Rubber Band R3 and fills
  the ring. This is *soft* real-time: it has ~93 ms of ring buffer as slack, so
  it does not need per-period determinism — it needs to **never be starved long
  enough to drain the ring**. Today it paces with `thread::sleep` and runs at
  normal priority; a compositor or GC-like stall on its core empties the ring
  and the consumer plays silence.

The failure mode is: producer gets preempted → ring drains → consumer's
`unwrap_or(0.0)` fills silence → click. Right now that click is **invisible** —
nothing counts it.

## Layers of defense, cheapest and most valuable first

### 1. Make glitches visible — xrun counter (do this first)

You cannot tune what you cannot see. The consumer already detects underrun
implicitly: every `consumer.pop()` that returns `Err` while playing is a
starved sample. Count them into an `AtomicU64`, expose it in the frame log and
(later) on screen. Also count the producer's "ring full, dropped frames"
warning that already exists.

This is a few lines, needs no privilege, and turns "it glitched sometimes" into
a number that goes up. Every layer below is judged by whether it drives that
number to zero. **This is the single highest-value step and should land before
any of the OS-level work.**

### 2. Lock memory resident — `mlockall` (cheap, portable)

A page fault in the audio path (memory paged out, then faulted back in) is a
multi-millisecond stall — a guaranteed glitch. `mlockall(MCL_CURRENT|MCL_FUTURE)`
at startup pins the process resident so the kernel never pages it out. On the Pi
this also locks the ~127 MB decoded track resident, which is what we want.

Cost: needs `RLIMIT_MEMLOCK` headroom (unlimited for the audio user, or a
`memlock` entry in `limits.conf`). One `libc::mlockall` call behind a
`#[cfg(unix)]`. No downside on a dedicated deck.

### 3. Real-time priority on the producer — `SCHED_FIFO`

Give `audio-proc` a real-time scheduling class so the compositor and render
thread cannot preempt it for long. Modest priority — **below** the audio
callback's (rtkit uses ~88; use ~10–20 for the producer) so the true hard-RT
consumer always wins. Set it from inside the spawned thread with
`pthread_setschedparam`, or via the `thread-priority` crate.

Caveat — **priorities need a bounded work item**: an RT-FIFO thread that never
yields starves everything at lower priority. `audio-proc` already yields (it
sleeps when the ring is full); keep that. And audit the RT section for
unbounded blocking (see the allocation note below) so a high-priority thread
never blocks on a page fault or a mutex.

Needs privilege: `RLIMIT_RTPRIO` (via `limits.conf` `@audio ... rtprio`), or
`setcap cap_sys_nice+ep` on the binary, or joining PipeWire's rtkit. For an
appliance, the systemd unit sets `LimitRTPRIO`.

### 4. Dedicate a core — CPU isolation (the appliance step)

On a shipping deck, give the audio path a core that nothing else runs on:

- Kernel cmdline `isolcpus=3 nohz_full=3 rcu_nocbs=3` (Pi: `cmdline.txt`) walls
  off core 3 from the general scheduler.
- Pin `audio-proc` (and, where reachable, the cpal callback) to that core with
  `pthread_setaffinity_np` / a `cpuset`.
- Everything else — render, egui, wgpu, Link, decode — runs on cores 0–2.

This is what turns "usually fine" into "deterministic". It maps cleanly onto
the Pi 5's four cores, and even more cleanly once there are two decks: core 3
for both DSP producers (or one core each if we drop the compositor), cores 0–2
for UI + network. It is boot-config + a few syscalls, no code architecture
change.

### 5. PREEMPT_RT kernel (note only)

For the hardest determinism, a `PREEMPT_RT` kernel bounds worst-case scheduler
latency to tens of microseconds. Overkill until the layers above are exhausted;
recorded so the option is known. Raspberry Pi OS can run an RT kernel.

## freedj-specific work the layers assume

- **Allocation audit of the RT path.** The hard-RT consumer already allocates
  nothing. The producer allocates in two spots that matter once it is
  RT-scheduled: the end-of-track `silence` vec (`audio.rs:257,336`) and any
  RubberBand internal allocation. Rubber Band R3 in real-time mode is designed
  not to allocate after `setMaxProcessSize`; confirm we configure it that way.
  Pre-allocate the silence buffer once. `ts_out` is already reused via
  `clear()`. The goal: no allocation, no lock, no syscall in the producer's
  steady state, so RT priority is safe.
- **cpal owns the callback thread.** We influence its priority only indirectly.
  On PipeWire this is handled (rtkit). If we ever move to a raw-ALSA or JACK
  backend for lower latency, we own the callback and must set its priority and
  affinity ourselves — a reason the backend choice (below) matters.
- **Backend / device selection is a prerequisite (WORKSTREAMS B3).** Today
  `cpal::default_host()` + default device grabs whatever ALSA hands back — on
  the Pi it opened a device that did **not** route through PipeWire and
  produced no sound. RT isolation is moot if we cannot even choose the output.
  `--device` selection, and deciding PipeWire-vs-JACK-vs-ALSA-direct, comes
  first for a real deck (a USB interface with a low, fixed period is the target
  anyway, not HDMI).

## Recommended increments

1. **xrun counter** — visible glitches. No privilege. Do now.
2. **`mlockall`** — no paging. Trivial, safe.
3. **Device selection (B3)** — be able to open the right output (USB interface)
   with a known period. Prerequisite for anything audio-serious.
4. **`SCHED_FIFO` on `audio-proc`** + the allocation audit — no preemption
   stalls. Gated by rtprio permission (limits.conf / setcap / systemd).
5. **`isolcpus` + affinity** — the appliance step, deterministic. Boot config.
6. PREEMPT_RT only if 1–5 leave residual xruns.

Steps 1–2 are worth doing regardless of platform. Steps 3–5 are where the Pi
(and the eventual RP2350 appliance) earns glitch-free audio under a full UI +
two-deck load.

## How to know it worked

The xrun counter from step 1 is the measure. Baseline it on the Pi under a
realistic load — playing, pitch-bending (which loads Rubber Band), the
compositor doing 4K, Link active — and drive it to zero as the layers go in.
Pair it with the frame instrument (PERFORMANCE.md): audio isolation should not
cost frame smoothness, and a rising xrun count under load is the signal that
the next layer is needed.

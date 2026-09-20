//! Process and per-thread CPU meter.
//!
//! A phone has no profiler attached, so the app measures itself: every
//! [`EVERY`] a line says what fraction of one core the process used and which
//! threads it went to.  It goes to the log and, when a file is given (iOS:
//! `Documents/opendeck-perf.log`, readable from the Files app), to that file.
//! The process figure is also kept in [`CPU_PCT`] for the INFO screen.
//!
//! Thread times come from `/proc/self/task/*/stat` on Linux and from Mach
//! `thread_info` on Apple platforms; the process total from `getrusage`.
//! The meter itself wakes once per interval.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Sampling interval.
pub const EVERY: Duration = Duration::from_secs(10);

/// Latest process CPU use as a percentage of one core (f32 bits).
pub static CPU_PCT: AtomicU32 = AtomicU32::new(0);

/// Latest process CPU use, percent of one core (0 until the first sample).
pub fn process_pct() -> f32 { f32::from_bits(CPU_PCT.load(Ordering::Relaxed)) }

/// Whether report lines also go to the file given to [`start`].  Off by
/// default; the MENU's PERF LOG setting drives it.  While off, the file is
/// removed, so a production install never carries a log in the user's
/// Documents.
pub static FILE_LOG: AtomicBool = AtomicBool::new(false);
pub fn set_file_log(on: bool) { FILE_LOG.store(on, Ordering::Relaxed); }

/// CPU seconds consumed so far: by the process, and per thread (name, seconds).
#[derive(Debug, Default, Clone)]
pub struct Sample {
    pub process: f64,
    pub threads: Vec<(String, f64)>,
}

/// Start the meter thread.  `file`: where report lines also go while
/// [`FILE_LOG`] is on (fresh per run; removed while off).
pub fn start(file: Option<PathBuf>) {
    let r = std::thread::Builder::new().name("cpu-meter".into()).spawn(move || {
        let mut prev = (Instant::now(), sample());
        let mut file_open = false;   // header written this run
        sync_file(file.as_deref(), &mut file_open);
        loop {
            std::thread::sleep(EVERY);
            let now = Instant::now();
            let cur = sample();
            let line = report(&prev.1, &cur, now.duration_since(prev.0));
            log::info!("{line}");
            sync_file(file.as_deref(), &mut file_open);
            if file_open {
                if let Some(f) = &file {
                    use std::io::Write;
                    if let Ok(mut fh) = std::fs::OpenOptions::new().append(true).open(f) {
                        let _ = writeln!(fh, "{line}");
                    }
                }
            }
            prev = (now, cur);
        }
    });
    if let Err(e) = r { log::warn!("cpu meter: {e}"); }
}

/// Bring the file into line with [`FILE_LOG`]: create it with a header when
/// the setting turns on (fresh per run — what matters is this session, and
/// it must not grow forever in a folder the user sees), remove it when off.
fn sync_file(file: Option<&std::path::Path>, open: &mut bool) {
    let Some(f) = file else { return };
    let want = FILE_LOG.load(Ordering::Relaxed);
    if want && !*open {
        let head = format!("# OpenDeck CPU meter — % of one core, sampled every {} s\n", EVERY.as_secs());
        match std::fs::write(f, head) {
            Ok(())  => *open = true,
            Err(e)  => log::warn!("cpu meter: cannot write {}: {e}", f.display()),
        }
    } else if !want {
        if f.exists() {
            match std::fs::remove_file(f) {
                Ok(())  => log::info!("cpu meter: removed {}", f.display()),
                Err(e)  => log::warn!("cpu meter: cannot remove {}: {e}", f.display()),
            }
        }
        *open = false;
    }
}

/// One report line for the interval between two samples; also updates
/// [`CPU_PCT`].  Threads are merged by name, sorted by cost, and listed
/// while they matter (≥ 0.5 %).
pub fn report(prev: &Sample, cur: &Sample, dt: Duration) -> String {
    let secs = dt.as_secs_f64().max(1e-3);
    let pct  = ((cur.process - prev.process) / secs * 100.0).max(0.0);
    CPU_PCT.store((pct as f32).to_bits(), Ordering::Relaxed);

    // Per-thread deltas.  A thread that was not in the previous sample is
    // new; count all of its time (it started inside the interval).
    let mut by_name: Vec<(String, f64)> = Vec::new();
    for (name, t) in &cur.threads {
        let before = prev.threads.iter().find(|(n, _)| n == name).map_or(0.0, |(_, t0)| *t0);
        let d = ((t - before) / secs * 100.0).max(0.0);
        match by_name.iter_mut().find(|(n, _)| n == name) {
            Some(e) => e.1 += d,
            None    => by_name.push((name.clone(), d)),
        }
    }
    by_name.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let parts: Vec<String> = by_name.iter().filter(|(_, p)| *p >= 0.5)
        .map(|(n, p)| format!("{n} {p:.1}")).collect();
    format!("cpu: {pct:.1}% of one core — {}", if parts.is_empty() { "(all threads idle)".into() } else { parts.join("  ") })
}

// ── Linux: /proc ──────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub fn sample() -> Sample {
    let tick = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let tick = if tick > 0.0 { tick } else { 100.0 };
    let mut s = Sample::default();
    let Ok(rd) = std::fs::read_dir("/proc/self/task") else { return s };
    for e in rd.flatten() {
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        // "tid (comm) S ppid ... utime stime ..." — comm may contain spaces.
        let (Some(a), Some(b)) = (stat.find('('), stat.rfind(')')) else { continue };
        let name = stat[a + 1..b].to_string();
        let f: Vec<&str> = stat[b + 1..].split_whitespace().collect();
        // After ')': state=0 ppid=1 … utime=11 stime=12.
        let (Some(u), Some(st)) = (f.get(11).and_then(|v| v.parse::<f64>().ok()), f.get(12).and_then(|v| v.parse::<f64>().ok())) else { continue };
        let secs = (u + st) / tick;
        s.process += secs;
        s.threads.push((name, secs));
    }
    s
}

// ── Apple: Mach thread_info + getrusage ───────────────────────────────────────

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(deprecated)]   // libc::mach_task_self — fine for a task port, no mach2 dependency needed
pub fn sample() -> Sample {
    use libc::{c_char, c_int, mach_port_t};
    extern "C" {
        // Not in the libc crate; libSystem exports it.
        fn mach_port_deallocate(task: mach_port_t, name: mach_port_t) -> c_int;
    }

    #[repr(C)]
    struct TimeValue { seconds: c_int, microseconds: c_int }
    /// `thread_basic_info` (mach/thread_info.h); 10 ints.
    #[repr(C)]
    struct ThreadBasicInfo {
        user_time:     TimeValue,
        system_time:   TimeValue,
        cpu_usage:     c_int,
        policy:        c_int,
        run_state:     c_int,
        flags:         c_int,
        suspend_count: c_int,
        sleep_time:    c_int,
    }
    const THREAD_BASIC_INFO: u32 = 3;
    const INFO_COUNT: u32 = (std::mem::size_of::<ThreadBasicInfo>() / std::mem::size_of::<c_int>()) as u32;

    let mut s = Sample::default();
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut ru) == 0 {
            s.process = ru.ru_utime.tv_sec as f64 + ru.ru_utime.tv_usec as f64 * 1e-6
                      + ru.ru_stime.tv_sec as f64 + ru.ru_stime.tv_usec as f64 * 1e-6;
        }
        let task = libc::mach_task_self();
        let mut list: *mut mach_port_t = std::ptr::null_mut();
        let mut n: u32 = 0;
        if libc::task_threads(task, &mut list, &mut n) != 0 { return s; }
        for i in 0..n as usize {
            let th = *list.add(i);
            let mut info: ThreadBasicInfo = std::mem::zeroed();
            let mut cnt = INFO_COUNT;
            if libc::thread_info(th, THREAD_BASIC_INFO, &mut info as *mut ThreadBasicInfo as *mut c_int, &mut cnt) == 0 {
                let secs = info.user_time.seconds as f64 + info.user_time.microseconds as f64 * 1e-6
                         + info.system_time.seconds as f64 + info.system_time.microseconds as f64 * 1e-6;
                let mut buf = [0 as c_char; 64];
                let pt = libc::pthread_from_mach_thread_np(th);
                let mut name = String::new();
                if pt != 0 && libc::pthread_getname_np(pt, buf.as_mut_ptr(), buf.len()) == 0 {
                    name = std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
                }
                if name.is_empty() { name = if i == 0 { "main".into() } else { "thread".into() }; }
                s.threads.push((name, secs));
            }
            mach_port_deallocate(task, th);
        }
        libc::vm_deallocate(task, list as usize, n as usize * std::mem::size_of::<mach_port_t>());
    }
    s
}

#[cfg(not(any(target_os = "linux", target_os = "ios", target_os = "macos")))]
pub fn sample() -> Sample { Sample::default() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_merges_and_ranks_threads() {
        let prev = Sample { process: 10.0, threads: vec![("main".into(), 5.0), ("audio".into(), 2.0), ("w".into(), 1.0), ("w".into(), 1.0)] };
        let cur  = Sample { process: 12.0, threads: vec![("main".into(), 5.5), ("audio".into(), 3.0), ("w".into(), 1.2), ("w".into(), 1.3), ("new".into(), 0.001)] };
        let line = report(&prev, &cur, Duration::from_secs(10));
        // 2 s over 10 s = 20 %; audio 10 %, main 5 %, the two "w" merged to 5 %, "new" below the floor.
        assert!(line.starts_with("cpu: 20.0% of one core — audio 10.0  main 5.0  w 5.0"), "{line}");
        assert!(!line.contains("new"), "{line}");
        assert!((process_pct() - 20.0).abs() < 0.01);
    }

    #[test]
    fn sample_sees_this_thread() {
        // Burn a few clock ticks of CPU (Linux counts in 10 ms ticks) so the
        // numbers are nonzero, then sample.
        let t0 = Instant::now();
        let mut x = 0u64;
        while t0.elapsed() < Duration::from_millis(60) {
            for i in 0..100_000u64 { x = x.wrapping_mul(31).wrapping_add(i); }
        }
        assert!(x != 1);
        let s = sample();
        if cfg!(any(target_os = "linux", target_os = "ios", target_os = "macos")) {
            assert!(!s.threads.is_empty());
            assert!(s.process > 0.0);
        }
    }
}

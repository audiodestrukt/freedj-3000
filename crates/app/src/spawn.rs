//! Background jobs: where work the deck hands off runs.
//!
//! The deck never waits on a track load, a network fetch or a model
//! release: it hands the work to a [`Spawn`] and takes the result from a
//! channel on a later frame.  Natively each job gets a thread of its own
//! ([`Threads`]); a browser build would run them on Web Workers; the tests
//! can hold them ([`Held`]) to show the frame never waited.
//!
//! This is for jobs that finish.  Services that run for the life of the
//! deck (the audio processor, Link, the surfaces) start their own threads.

use std::sync::Arc;

/// Runs background jobs.
pub trait Spawn: Send + Sync {
    /// Start `job` running in the background, named `name` for logs and
    /// profilers.  Returns at once: the job's result comes back however
    /// the job sends it (a channel, an `ArcSwap`).
    fn spawn(&self, name: &str, job: Box<dyn FnOnce() + Send>) -> std::io::Result<()>;
}

impl dyn Spawn {
    /// [`Spawn::spawn`] without boxing at the call site.
    pub fn run(&self, name: &str, job: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
        self.spawn(name, Box::new(job))
    }
}

/// How the deck holds its job runner.
pub type Spawner = Arc<dyn Spawn>;

/// A thread per job (the native platforms).
pub struct Threads;

impl Spawn for Threads {
    fn spawn(&self, name: &str, job: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
        std::thread::Builder::new().name(name.into()).spawn(job).map(drop)
    }
}

/// The native job runner.
pub fn threads() -> Spawner { Arc::new(Threads) }

/// Tests: jobs are queued, not run, until the test says so.  A frame that
/// still returns while a job is held did not wait for it.
#[cfg(test)]
#[derive(Default)]
pub struct Held {
    jobs: std::sync::Mutex<Vec<(String, Box<dyn FnOnce() + Send>)>>,
}

#[cfg(test)]
impl Held {
    /// The names of the jobs waiting, oldest first.
    pub fn names(&self) -> Vec<String> { self.jobs.lock().unwrap().iter().map(|(n, _)| n.clone()).collect() }

    /// Run every waiting job on a thread of its own, as natively.
    pub fn release(&self) {
        for (name, job) in std::mem::take(&mut *self.jobs.lock().unwrap()) {
            Threads.spawn(&name, job).unwrap();
        }
    }
}

#[cfg(test)]
impl Spawn for Held {
    fn spawn(&self, name: &str, job: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
        self.jobs.lock().unwrap().push((name.into(), job));
        Ok(())
    }
}

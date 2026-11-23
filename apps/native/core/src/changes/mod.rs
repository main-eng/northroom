//! Changes: what the terminals changed on disk in a folder while they worked there, credited
//! to the terminal that made each change, and shown in a Changes tab until the person keeps it;
//! beside it, what git says is uncommitted or differs from the default branch (CONTEXT.md,
//! ADR-0002).
//!
//! A turn runs from a prompt to the agent's stop, read from Northroom's hooks, or for other
//! programs from Return to the activity settling. Each turn has a snapshot of its folder at its
//! start and end (`snapshot`), whose contents are kept once each in the app's data (`store`).
//! The engine (`turns`) holds the turns, snapshots, what was kept and what git said; the view
//! (`view`) lists and diffs them (`diff`) in the scope shown and keeps them, and `frames` places
//! its parts; `actions` undoes on disk; `service` runs snapshots, git and the folder watcher off
//! the UI thread, and keeps each root's `record`, what its list needs across restarts. A root is
//! a repository's top folder, else the drive (`git`), so terminals anywhere in it share one tab.

pub mod ffi;

pub mod actions;
pub mod diff;
pub mod frames;
pub mod git;
pub mod ignore;
pub mod record;
pub mod scan;
pub mod service;
pub mod snapshot;
pub mod store;
pub mod turns;
pub mod view;

use std::time::SystemTime;

/// Milliseconds since the Unix epoch, for every time the changes keep.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// The computer's clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        millis_of(SystemTime::now())
    }
}

/// `time` in milliseconds since the Unix epoch; 0 for a time before it.
pub fn millis_of(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
pub(crate) mod testing {
    use super::Clock;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A clock that says what it was set to.
    pub struct FixedClock(AtomicU64);

    impl FixedClock {
        pub fn at(now_ms: u64) -> Self {
            Self(AtomicU64::new(now_ms))
        }
    }

    impl Clock for FixedClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// A clock that moves on by a step each time it is read.
    pub struct SteppingClock {
        now_ms: AtomicU64,
        step_ms: u64,
    }

    impl SteppingClock {
        pub fn new(start_ms: u64, step_ms: u64) -> Self {
            Self {
                now_ms: AtomicU64::new(start_ms),
                step_ms,
            }
        }
    }

    impl Clock for SteppingClock {
        fn now_ms(&self) -> u64 {
            self.now_ms.fetch_add(self.step_ms, Ordering::SeqCst)
        }
    }
}

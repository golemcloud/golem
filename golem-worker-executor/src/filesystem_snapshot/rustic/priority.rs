// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Saves and prunes run at a low CPU priority, so the work of the agents comes first.
//!
//! A thread without privilege cannot raise its priority again after it lowers it. So the work runs
//! on a new thread, with a rayon pool of its own, and both end with the work.

use rayon::{ThreadPool, ThreadPoolBuildError, ThreadPoolBuilder};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, PoisonError};
use tracing::warn;

/// The nice value of the threads of a save or a prune.
pub(super) const LOW_PRIORITY: i32 = 19;

/// How the store runs work at a low priority: the thread count of the rayon pool of the work, and
/// the two steps that a test can replace.
#[derive(Clone, Copy, Debug)]
pub(super) struct LowPriority {
    /// The threads of the rayon pool of the work. `None` is the default count of rayon.
    pub(super) threads: Option<NonZeroUsize>,
    /// Gives the calling thread the low priority.
    pub(super) lower: fn() -> std::io::Result<()>,
    /// Builds the rayon pool of the work, with the name and the thread count.
    pub(super) build_pool:
        fn(&'static str, Option<NonZeroUsize>) -> Result<ThreadPool, ThreadPoolBuildError>,
}

impl LowPriority {
    /// Gives the steps of the platform, with a rayon pool of `threads` threads.
    pub(super) fn new(threads: Option<NonZeroUsize>) -> Self {
        Self {
            threads,
            lower: lower_own_priority,
            build_pool,
        }
    }

    /// Runs the work at nice 19 on a new thread with the name, inside a new rayon pool, and waits
    /// for it. The threads that the work starts get the same nice value. On a platform other than
    /// Linux the work runs as it is.
    pub(super) fn run<T: Send + 'static>(
        self,
        name: &'static str,
        work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        if cfg!(target_os = "linux") {
            self.on_own_thread(name, work)
        } else {
            work()
        }
    }

    /// Runs the work on a new thread that first lowers its priority and builds the pool. A failure
    /// of a step gives a warning, and the work runs without that step.
    fn on_own_thread<T: Send + 'static>(
        self,
        name: &'static str,
        work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        // The work waits in a slot, so the calling thread can still run it when no thread starts.
        let slot = Arc::new(Mutex::new(Some(work)));
        let spawned = std::thread::Builder::new().name(name.to_string()).spawn({
            let slot = slot.clone();
            move || {
                if let Err(error) = (self.lower)() {
                    warn!(
                        error = %error,
                        "Failed to lower the CPU priority of filesystem snapshot work, so it runs at the normal priority"
                    );
                }
                match (self.build_pool)(name, self.threads) {
                    Ok(pool) => pool.install(|| run_taken(&slot)),
                    Err(error) => {
                        warn!(
                            error = %error,
                            "Failed to build the thread pool of filesystem snapshot work, so its parallel parts use the global pool"
                        );
                        run_taken(&slot)
                    }
                }
            }
        });
        match spawned {
            Ok(thread) => thread.join().map_err(|_| {
                anyhow::anyhow!("the thread of the filesystem snapshot work panicked")
            })?,
            Err(error) => {
                warn!(
                    error = %error,
                    "Failed to start a thread for filesystem snapshot work, so it runs at the normal priority"
                );
                run_taken(&slot)
            }
        }
    }
}

/// Takes the work out of the slot and runs it.
fn run_taken<T, W: FnOnce() -> anyhow::Result<T>>(slot: &Mutex<Option<W>>) -> anyhow::Result<T> {
    let work = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
    work.map_or_else(
        || Err(anyhow::anyhow!("the work already ran")),
        |work| work(),
    )
}

/// Builds a rayon pool whose threads get the nice value of the calling thread.
fn build_pool(
    name: &'static str,
    threads: Option<NonZeroUsize>,
) -> Result<ThreadPool, ThreadPoolBuildError> {
    ThreadPoolBuilder::new()
        .num_threads(threads.map_or(0, NonZeroUsize::get))
        .thread_name(move |index| format!("{name}-{index}"))
        .build()
}

/// Gives the calling thread the nice value 19.
#[cfg(target_os = "linux")]
fn lower_own_priority() -> std::io::Result<()> {
    // SAFETY: `gettid` has no preconditions.
    let thread = unsafe { libc::gettid() };
    let thread = libc::id_t::try_from(thread).map_err(std::io::Error::other)?;
    // SAFETY: `setpriority` only reads its arguments.
    let result = unsafe { libc::setpriority(libc::PRIO_PROCESS, thread, LOW_PRIORITY) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn lower_own_priority() -> std::io::Result<()> {
    Ok(())
}

/// Gives the nice value of the calling thread.
#[cfg(all(test, target_os = "linux"))]
pub(super) fn own_nice() -> i32 {
    // SAFETY: `gettid` has no preconditions.
    let thread = libc::id_t::try_from(unsafe { libc::gettid() }).unwrap();
    // SAFETY: `getpriority` only reads its arguments.
    unsafe { libc::getpriority(libc::PRIO_PROCESS, thread) }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

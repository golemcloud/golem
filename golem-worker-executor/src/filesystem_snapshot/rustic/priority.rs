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
//! on a new thread that ends with the work, and no thread of a pool gets the low priority.

use std::sync::{Arc, Mutex, PoisonError};
use tracing::warn;

/// The nice value of the threads of a save or a prune.
#[cfg(target_os = "linux")]
pub(super) const LOW_PRIORITY: i32 = 19;

/// Runs the work at nice 19 on a new thread with the name, and waits for it. The threads that the
/// work starts get the same nice value. On a platform other than Linux the work runs as it is.
pub(super) fn at_low_priority<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    #[cfg(target_os = "linux")]
    {
        // The global rayon pool of rustic starts at its first use. It starts here, so its threads
        // keep the normal priority and a restore that uses them does not run at nice 19.
        let _ = rayon::current_num_threads();
        on_own_thread(name, work, lower_own_priority)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = name;
        work()
    }
}

/// Runs the work on a new thread that first calls `lower`, and waits for it. A failed `lower` or a
/// failed start of the thread gives a warning, and the work runs at the normal priority.
fn on_own_thread<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    lower: fn() -> std::io::Result<()>,
) -> anyhow::Result<T> {
    // The work waits in a slot, so the calling thread can still run it when no thread starts.
    let slot = Arc::new(Mutex::new(Some(work)));
    let spawned = std::thread::Builder::new().name(name.to_string()).spawn({
        let slot = slot.clone();
        move || {
            if let Err(error) = lower() {
                warn!(
                    error = %error,
                    "Failed to lower the CPU priority of filesystem snapshot work, so it runs at the normal priority"
                );
            }
            take(&slot).map_or_else(|| Err(anyhow::anyhow!("the work was taken")), |work| work())
        }
    });
    match spawned {
        Ok(thread) => thread
            .join()
            .map_err(|_| anyhow::anyhow!("the thread of the filesystem snapshot work panicked"))?,
        Err(error) => {
            warn!(
                error = %error,
                "Failed to start a thread for filesystem snapshot work, so it runs at the normal priority"
            );
            take(&slot).map_or_else(|| Err(anyhow::anyhow!("the work was taken")), |work| work())
        }
    }
}

fn take<W>(slot: &Mutex<Option<W>>) -> Option<W> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
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

/// Gives the nice value of the calling thread.
#[cfg(all(test, target_os = "linux"))]
pub(super) fn own_nice() -> i32 {
    // SAFETY: `gettid` has no preconditions, and `getpriority` only reads its arguments.
    unsafe { libc::getpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t) }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

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

//! The work of a store for each incarnation of an agent in this process, which a delete of all
//! snapshots of the incarnation waits for.
//!
//! Each operation of a store holds an [`OperationWork`] of its incarnation for its whole life,
//! and each task that the operation starts holds a clone of it. So the tracker of an
//! [`AgentWork`] counts all the work of the incarnation that began before a drain, also the work
//! that runs on after its caller stopped waiting. The store keeps only a weak entry of each
//! incarnation, so the map holds the incarnations that have work alive.

use crate::filesystem_snapshot::AgentSnapshots;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

/// The map of the weak entries of the work of each incarnation.
type Entries = HashMap<AgentSnapshots, Weak<AgentWork>>;

/// The weak entries of the work of each incarnation.
pub(super) type AgentWorks = Arc<Mutex<Entries>>;

/// The work of one incarnation that began since its last drain.
#[derive(Debug)]
pub(super) struct AgentWork {
    tracker: TaskTracker,
    works: Weak<Mutex<Entries>>,
    agent: AgentSnapshots,
    /// The share in the work that a drain put in the place of this work. This work holds it for
    /// as long as it lives, so a drain of the newer work waits for this work, also when the drain
    /// that replaced this work was dropped.
    successor: OnceLock<OperationWork>,
}

impl Drop for AgentWork {
    /// Removes the entry of the incarnation when [`ends_entry`] says so.
    fn drop(&mut self) {
        if let Some(works) = self.works.upgrade() {
            let mut works = works.lock().unwrap_or_else(PoisonError::into_inner);
            if ends_entry(works.get(&self.agent), self) {
                works.remove(&self.agent);
            }
        }
    }
}

/// Whether the end of the work `this` removes the entry `entry` of its incarnation: only when the
/// entry is still this work. So the end of an old work never removes a newer entry.
fn ends_entry(entry: Option<&Weak<AgentWork>>, this: *const AgentWork) -> bool {
    entry.is_some_and(|entry| std::ptr::eq(entry.as_ptr(), this))
}

/// The share of one operation, or of one task of it, in the work of its incarnation.
#[derive(Debug)]
pub(super) struct OperationWork {
    work: Arc<AgentWork>,
    _token: TaskTrackerToken,
}

impl Clone for OperationWork {
    fn clone(&self) -> Self {
        Self::of(Arc::clone(&self.work))
    }
}

impl OperationWork {
    fn of(work: Arc<AgentWork>) -> Self {
        Self {
            _token: work.tracker.token(),
            work,
        }
    }
}

/// Puts a new work of `agent` in the entries, and gives its share.
fn new_work(works: &AgentWorks, entries: &mut Entries, agent: &AgentSnapshots) -> OperationWork {
    let work = Arc::new(AgentWork {
        tracker: TaskTracker::new(),
        works: Arc::downgrade(works),
        agent: agent.clone(),
        successor: OnceLock::new(),
    });
    entries.insert(agent.clone(), Arc::downgrade(&work));
    OperationWork::of(work)
}

/// Gives the share of a new operation of `agent` in the work of its incarnation: the live work of
/// the entry, or a new work when the entry is gone. The share leaves the lock before it can drop,
/// so the `Drop` of a work never runs under the lock.
pub(super) fn begin_operation(works: &AgentWorks, agent: &AgentSnapshots) -> OperationWork {
    let mut entries = works.lock().unwrap_or_else(PoisonError::into_inner);
    match entries.get(agent).and_then(Weak::upgrade) {
        Some(work) => OperationWork::of(work),
        None => new_work(works, &mut entries, agent),
    }
}

/// Waits until each work of the incarnation `agent` that began before the drain ends, and gives
/// the share of the drain in a new work of the incarnation. In one locked step, the drain takes
/// the old entry out, puts the new work in its place, and gives the old work a share in the new
/// work, which the old work holds until it ends. So a later drain waits for the old work, also
/// when the caller of this drain stops waiting. Work that begins after that step joins the new
/// work, which this drain does not wait for. The drain waits outside the lock.
pub(super) async fn drain_agent(works: &AgentWorks, agent: &AgentSnapshots) -> OperationWork {
    let (drained, own) = {
        let mut entries = works.lock().unwrap_or_else(PoisonError::into_inner);
        let drained = entries.remove(agent).and_then(|entry| entry.upgrade());
        let own = new_work(works, &mut entries, agent);
        if let Some(work) = &drained {
            // Only the drain that took the entry out sets the share, so the cell is empty. A
            // refused share is a share of the new work, which `own` keeps alive, so its drop
            // under the lock never drops a work.
            let _ = work.successor.set(own.clone());
        }
        (drained, own)
    };
    if let Some(work) = drained {
        work.tracker.close();
        work.tracker.wait().await;
    }
    own
}

#[cfg(test)]
mod tests;

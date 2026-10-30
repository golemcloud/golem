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

//! Operation registration and the drain cutoff of a delete of all snapshots of an incarnation.
//!
//! Each operation holds an [`OperationLease`] for its whole life, and each retained task holds a
//! clone. An [`OperationCohort`] counts the operations and tasks registered between drain cutoffs,
//! including tasks whose caller stopped waiting. The registry keeps weak entries, so it retains
//! only incarnations with live cohorts.

use crate::filesystem_snapshot::agent_snapshots::AgentSnapshots;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

/// The current cohort of each incarnation, held weakly.
type Entries = HashMap<AgentSnapshots, Weak<OperationCohort>>;

/// Registers operations by incarnation and establishes delete-all drain cutoffs.
#[derive(Clone, Default)]
pub(super) struct OperationDrain {
    entries: Arc<Mutex<Entries>>,
}

/// Operations registered for one incarnation between two drain cutoffs.
#[derive(Debug)]
struct OperationCohort {
    tracker: TaskTracker,
    registry: Weak<Mutex<Entries>>,
    agent: AgentSnapshots,
    /// Retains a registration in the successor cohort until this cohort ends. A later drain then
    /// waits for this cohort even if the caller that established the earlier cutoff dropped.
    successor: OnceLock<OperationLease>,
}

impl Drop for OperationCohort {
    /// Removes the entry of the incarnation when [`ends_entry`] says so.
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            let mut registry = registry.lock().unwrap_or_else(PoisonError::into_inner);
            if ends_entry(registry.get(&self.agent), self) {
                registry.remove(&self.agent);
            }
        }
    }
}

/// Whether the end of the cohort `this` removes the entry `entry` of its incarnation: only when the
/// entry is still this cohort. So the end of an old cohort never removes a newer entry.
fn ends_entry(entry: Option<&Weak<OperationCohort>>, this: *const OperationCohort) -> bool {
    entry.is_some_and(|entry| std::ptr::eq(entry.as_ptr(), this))
}

/// The registration of an operation or retained task in its incarnation's cohort.
#[derive(Debug)]
pub(super) struct OperationLease {
    cohort: Arc<OperationCohort>,
    _token: TaskTrackerToken,
}

impl Clone for OperationLease {
    fn clone(&self) -> Self {
        Self::of(Arc::clone(&self.cohort))
    }
}

impl OperationLease {
    fn of(cohort: Arc<OperationCohort>) -> Self {
        Self {
            _token: cohort.tracker.token(),
            cohort,
        }
    }
}

/// Installs a new cohort for `agent` and returns its first lease.
fn new_cohort(
    registry: &Arc<Mutex<Entries>>,
    entries: &mut Entries,
    agent: &AgentSnapshots,
) -> OperationLease {
    let cohort = Arc::new(OperationCohort {
        tracker: TaskTracker::new(),
        registry: Arc::downgrade(registry),
        agent: agent.clone(),
        successor: OnceLock::new(),
    });
    entries.insert(agent.clone(), Arc::downgrade(&cohort));
    OperationLease::of(cohort)
}

impl OperationDrain {
    #[cfg(test)]
    pub(super) fn live_incarnations(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Registers an operation in the current cohort, or creates a cohort when the entry is gone.
    /// The lease leaves the lock before it can drop, so no cohort drops under the map lock.
    pub(super) fn begin(&self, agent: &AgentSnapshots) -> OperationLease {
        let registry = &self.entries;
        let mut entries = registry.lock().unwrap_or_else(PoisonError::into_inner);
        match entries.get(agent).and_then(Weak::upgrade) {
            Some(cohort) => OperationLease::of(cohort),
            None => new_cohort(registry, &mut entries, agent),
        }
    }

    /// Establishes a cutoff and waits for earlier registrations outside the map lock. The locked
    /// step replaces the current cohort and gives it a lease on its successor. A later drain still
    /// waits for that cohort if this caller drops. Operations registered after the cutoff join the
    /// successor and do not delay this drain. The returned lease registers the drain there too.
    pub(super) async fn drain_before(&self, agent: &AgentSnapshots) -> OperationLease {
        let registry = &self.entries;
        let (drained, own) = {
            let mut entries = registry.lock().unwrap_or_else(PoisonError::into_inner);
            let drained = entries.remove(agent).and_then(|entry| entry.upgrade());
            let own = new_cohort(registry, &mut entries, agent);
            if let Some(cohort) = &drained {
                // Only the drain that removed the entry sets this cell, so it is empty. If a lease
                // is refused, `own` still keeps its cohort alive while that lease drops under the lock.
                let _ = cohort.successor.set(own.clone());
            }
            (drained, own)
        };
        if let Some(cohort) = drained {
            cohort.tracker.close();
            cohort.tracker.wait().await;
        }
        own
    }
}

#[cfg(test)]
mod tests;

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

use super::{
    BTreeMap, BodyAdmissionState, DeferredAdmission, DeferredAdmissionCohort,
    DeferredAdmissionReadiness, DeferredAdmissionTable, HashMap, HashSet, Mutex, Notify,
    OplogIndex, OwnerInvocationId, OwnerLane, OwnerLaneWait, OwnerToolOperation,
    OwnerToolOperations, WorkerExecutorError,
};

impl Default for DeferredAdmissionTable {
    fn default() -> Self {
        Self {
            parents: Mutex::new(HashMap::new()),
            closed_parents: Mutex::new(HashSet::new()),
            changed: Notify::new(),
        }
    }
}

impl DeferredAdmissionTable {
    pub(crate) fn begin_generation(&self) -> Result<(), WorkerExecutorError> {
        let mut closed_parents = self.closed_parents.lock().unwrap();
        let parents = self.parents.lock().unwrap();
        if !parents.is_empty() {
            let staged_admissions = parents.values().map(BTreeMap::len).sum::<usize>();
            return Err(WorkerExecutorError::runtime(format!(
                "cannot begin an owner generation with {staged_admissions} deferred tool \
                     admissions across {} parents",
                parents.len()
            )));
        }
        closed_parents.clear();
        Ok(())
    }

    pub(crate) fn insert(&self, parent: OwnerInvocationId, start: OplogIndex) -> bool {
        let closed_parents = self.closed_parents.lock().unwrap();
        if closed_parents.contains(&parent) {
            return false;
        }
        self.parents
            .lock()
            .unwrap()
            .entry(parent)
            .or_default()
            .insert(
                start,
                DeferredAdmission {
                    readiness: DeferredAdmissionReadiness::Staging,
                    cohort: None,
                },
            )
            .is_none()
    }

    pub(crate) fn close_parent_and_snapshot(
        &self,
        parent: &OwnerInvocationId,
    ) -> Option<Vec<OplogIndex>> {
        let mut closed_parents = self.closed_parents.lock().unwrap();
        if !closed_parents.insert(parent.clone()) {
            return None;
        }
        let mut parents = self.parents.lock().unwrap();
        let starts = parents
            .get_mut(parent)
            .map(|entries| {
                for entry in entries.values_mut() {
                    entry.cohort = Some(DeferredAdmissionCohort::ParentEnd);
                }
                entries.keys().copied().collect()
            })
            .unwrap_or_default();
        Some(starts)
    }

    pub(crate) fn clear_closed_parent(&self, parent: &OwnerInvocationId) -> bool {
        let mut closed_parents = self.closed_parents.lock().unwrap();
        let parents = self.parents.lock().unwrap();
        if parents.contains_key(parent) {
            return false;
        }
        closed_parents.remove(parent)
    }

    pub(crate) fn settle_staging(
        &self,
        parent: &OwnerInvocationId,
        start: OplogIndex,
        readiness: DeferredAdmissionReadiness,
    ) -> bool {
        if readiness == DeferredAdmissionReadiness::Staging {
            return false;
        }
        let mut parents = self.parents.lock().unwrap();
        let Some(entry) = parents
            .get_mut(parent)
            .and_then(|entries| entries.get_mut(&start))
        else {
            return false;
        };
        if entry.readiness != DeferredAdmissionReadiness::Staging {
            return false;
        }
        entry.readiness = readiness;
        drop(parents);
        self.changed.notify_waiters();
        true
    }

    pub(crate) fn remove_settled_without_body(
        &self,
        parent: &OwnerInvocationId,
        start: OplogIndex,
    ) -> bool {
        let mut parents = self.parents.lock().unwrap();
        let removed = parents.get_mut(parent).is_some_and(|entries| {
            entries.get(&start).is_some_and(|entry| {
                entry.readiness == DeferredAdmissionReadiness::SettledWithoutBody
            }) && entries.remove(&start).is_some()
        });
        if parents.get(parent).is_some_and(BTreeMap::is_empty) {
            parents.remove(parent);
        }
        drop(parents);
        if removed {
            self.changed.notify_waiters();
        }
        removed
    }

    pub(crate) fn settle_operation_without_body(
        &self,
        parent: &OwnerInvocationId,
        start: OplogIndex,
        expected_readiness: DeferredAdmissionReadiness,
        operation: &OwnerToolOperation,
        expected_admission: BodyAdmissionState,
    ) -> bool {
        let _closed_parents = self.closed_parents.lock().unwrap();
        let mut parents = self.parents.lock().unwrap();
        let Some(entry) = parents
            .get_mut(parent)
            .and_then(|entries| entries.get_mut(&start))
        else {
            return false;
        };
        if entry.readiness != expected_readiness
            || !operation
                .transition_admission(expected_admission, BodyAdmissionState::SettledWithoutBody)
        {
            return false;
        }
        entry.readiness = DeferredAdmissionReadiness::SettledWithoutBody;
        drop(parents);
        self.changed.notify_waiters();
        true
    }

    pub(crate) fn remove(&self, parent: &OwnerInvocationId, start: OplogIndex) -> bool {
        let mut parents = self.parents.lock().unwrap();
        let removed = parents
            .get_mut(parent)
            .is_some_and(|entries| entries.remove(&start).is_some());
        if parents.get(parent).is_some_and(BTreeMap::is_empty) {
            parents.remove(parent);
        }
        drop(parents);
        if removed {
            self.changed.notify_waiters();
        }
        removed
    }

    pub(crate) async fn wait_and_register_cohort(
        &self,
        parent: &OwnerInvocationId,
        cohort: DeferredAdmissionCohort,
        starts: &[OplogIndex],
        operations: &OwnerToolOperations,
        lane: &OwnerLane,
    ) -> Result<Option<OwnerLaneWait>, WorkerExecutorError> {
        loop {
            let changed = self.changed.notified();
            if let Some(result) =
                self.try_claim_cohort(parent, cohort, starts.iter().copied(), |ready| {
                    let result_await_parent =
                        matches!(cohort, DeferredAdmissionCohort::ResultAwait(_)).then_some(parent);
                    operations
                        .register_ready_bodies(lane, ready, result_await_parent)
                        .map(|(_, wait)| wait)
                })
            {
                return result;
            }
            changed.await;
        }
    }

    /// Marks one existing GOL-33 causal cohort eligible and runs its registration while parent
    /// closure and no-body cancellation are excluded. A cohort is claimed only when all members
    /// have settled staging and no earlier eligible Start remains staging.
    pub(super) fn try_claim_cohort<R, E>(
        &self,
        parent: &OwnerInvocationId,
        cohort: DeferredAdmissionCohort,
        starts: impl IntoIterator<Item = OplogIndex>,
        claim: impl FnOnce(&[OplogIndex]) -> Result<R, E>,
    ) -> Option<Result<R, E>>
    where
        R: Default,
    {
        let starts = starts.into_iter().collect::<Vec<_>>();
        let closed_parents = self.closed_parents.lock().unwrap();
        if cohort != DeferredAdmissionCohort::ParentEnd && closed_parents.contains(parent) {
            return Some(Ok(R::default()));
        }
        let mut parents = self.parents.lock().unwrap();
        let Some(entries) = parents.get_mut(parent) else {
            return Some(Ok(R::default()));
        };
        let starts = starts
            .into_iter()
            .filter(|start| {
                entries
                    .get(start)
                    .is_some_and(|entry| entry.cohort.is_none_or(|assigned| assigned == cohort))
            })
            .collect::<Vec<_>>();
        for start in &starts {
            let entry = entries
                .get_mut(start)
                .expect("filtered deferred admission must remain present");
            entry.cohort = Some(cohort);
        }
        let Some(earliest_requested) = starts.iter().min().copied() else {
            return Some(Ok(R::default()));
        };
        if entries.range(..earliest_requested).any(|(_, entry)| {
            entry.cohort.is_some() && entry.readiness == DeferredAdmissionReadiness::Staging
        }) || starts
            .iter()
            .any(|start| entries[start].readiness == DeferredAdmissionReadiness::Staging)
        {
            return None;
        }

        let mut released = starts;
        released.sort_unstable();
        let released = released
            .into_iter()
            .filter(|start| entries[start].readiness == DeferredAdmissionReadiness::Ready)
            .collect::<Vec<_>>();
        let claimed = match claim(&released) {
            Ok(claimed) => claimed,
            Err(error) => return Some(Err(error)),
        };
        entries.retain(|_, entry| entry.cohort != Some(cohort));
        if entries.is_empty() {
            parents.remove(parent);
        }
        drop(parents);
        drop(closed_parents);
        self.changed.notify_waiters();
        Some(Ok(claimed))
    }
}

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

//! The invocations of one agent, read from its status record, its persisted result index and its
//! oplog. Nothing here is stored besides the per-invocation entries that the status fold keeps.

use crate::worker::*;
use golem_common::model::oplog::OplogErrorKind;
use golem_common::model::{
    AgentStatus, AgentStatusRecord, InvocationEnd, InvocationId, InvocationOperation,
    InvocationRequestKey, InvocationResultPosition, OplogIndex, PendingInvocationRef, Timestamp,
};
use std::collections::HashMap;

/// The lifecycle state of an invocation, as `get` and `list` show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InvocationState {
    Pending,
    Scheduled,
    Running,
    /// The running invocation of a suspended agent.
    Waiting,
    Retrying,
    Interrupted,
    Completed,
    /// The invocation recorded its result, then the agent gave up during its background work.
    BackgroundFailed,
    Failed,
    Cancelled,
}

/// One invocation of an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationListEntry {
    pub key: InvocationId,
    /// `None` when its request entry is not known: the fold that found its result did not see
    /// the request.
    pub operation: Option<InvocationOperation>,
    pub state: InvocationState,
    /// In milliseconds, the precision the oplog stores, so a live agent and one read back from
    /// its oplog give the same entry.
    pub accepted_at: Option<Timestamp>,
    /// The timestamp of the last change of its state, in milliseconds. Entry timestamps come
    /// from the clock of the executor that wrote them, so they can go backwards when an agent
    /// moves.
    pub changed_at: Timestamp,
    /// The oplog index of the last change of its state. Inside one agent this is the order of
    /// the changes.
    pub last_change: OplogIndex,
}

impl InvocationListEntry {
    /// The entry of an invocation with a result. A retry marker of an invocation that no longer
    /// runs is the result of exhausted retries.
    fn of_result(key: InvocationId, position: InvocationResultPosition) -> Self {
        Self {
            key,
            state: match position.end {
                InvocationEnd::Completed => InvocationState::Completed,
                InvocationEnd::Failed | InvocationEnd::Retrying => InvocationState::Failed,
                InvocationEnd::Cancelled => InvocationState::Cancelled,
                InvocationEnd::BackgroundFailed { .. } => InvocationState::BackgroundFailed,
            },
            last_change: position.last_change(),
            operation: position.operation,
            accepted_at: position.accepted_at.map(Timestamp::rounded),
            changed_at: position.changed_at.rounded(),
        }
    }

    fn of_pending(key: InvocationId, pending: &PendingInvocationRef, now: Timestamp) -> Self {
        Self {
            key,
            operation: pending.operation.clone(),
            // A scheduled invocation becomes pending at its deadline without an entry, so this
            // changes neither its order nor its `changed_at`.
            state: if pending.is_due(now) {
                InvocationState::Pending
            } else {
                InvocationState::Scheduled
            },
            accepted_at: Some(pending.timestamp.rounded()),
            changed_at: pending.timestamp.rounded(),
            last_change: pending.oplog_index,
        }
    }

    /// The entry of the running invocation, with the same rules as the result lookup: a retry
    /// marker of the running invocation is not its result while the agent can still retry it.
    fn of_running(status: &AgentStatusRecord) -> Option<Self> {
        let running = status.running.as_ref()?;
        let state = match status.status {
            AgentStatus::Failed if status.last_error_kind != Some(OplogErrorKind::Recovery) => {
                InvocationState::Failed
            }
            AgentStatus::Exited => InvocationState::Failed,
            AgentStatus::Interrupted => InvocationState::Interrupted,
            AgentStatus::Suspended => InvocationState::Waiting,
            AgentStatus::Retrying => InvocationState::Retrying,
            _ => InvocationState::Running,
        };
        Some(Self {
            key: running.key,
            operation: running.request.operation.clone(),
            state,
            accepted_at: Some(running.request.accepted_at.rounded()),
            changed_at: running.last_change_at.rounded(),
            last_change: running.last_change,
        })
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// One page of the listed invocations of the agent that match `filter`, in the list order,
    /// and the position the next page starts after when more may follow.
    ///
    /// This reads the whole result history of the agent: the recent results of the status and,
    /// once they are not the complete history, the persisted result index. Each call costs
    /// O(history).
    pub async fn list_invocation_page(
        &self,
        filter: &InvocationFilter,
        after: Option<InvocationPagePosition>,
        page_size: usize,
    ) -> Result<(Vec<InvocationListEntry>, Option<InvocationPagePosition>), WorkerExecutorError>
    {
        let entries = self.merged_invocations().await?;
        Ok(page_invocations(entries, filter, after, page_size))
    }

    /// The invocations of the agent, in the list order.
    async fn merged_invocations(&self) -> Result<Vec<InvocationListEntry>, WorkerExecutorError> {
        let status = self.state_actor.try_status().await?;
        let mut results: HashMap<InvocationId, InvocationResultPosition> = HashMap::new();
        if !status.invocation_results.is_exact_complete() {
            for (key, position) in self.older_invocation_results(&status).await? {
                keep_latest(&mut results, key, position);
            }
        }
        for (key, position) in status.invocation_results.iter() {
            keep_latest(&mut results, *key, position.clone());
        }
        crate::metrics::workers::record_invocation_listing(results.len());

        Ok(merge_invocations(&status, results, Timestamp::now_utc()))
    }

    /// The listed invocation with `key`, or `None` when the agent has none.
    pub async fn get_invocation(
        &self,
        key: &InvocationId,
    ) -> Result<Option<InvocationListEntry>, WorkerExecutorError> {
        Ok(self
            .merged_invocations()
            .await?
            .into_iter()
            .find(|entry| entry.key == *key)
            .filter(InvocationListEntry::is_listed))
    }

    /// The invocations of the agent that `request_key` started, at most one per operation.
    ///
    /// The candidate operations are `operation` when it is set, otherwise the methods of the
    /// agent's current type and every operation of the entries. So a key of a method that a
    /// later revision removed is found from its own entry.
    pub async fn lookup_invocations(
        &self,
        request_key: &InvocationRequestKey,
        operation: Option<&str>,
    ) -> Result<Vec<InvocationListEntry>, WorkerExecutorError> {
        let entries = self.merged_invocations().await?;
        let mut operations: Vec<String> = match operation {
            Some(operation) => vec![operation.to_string()],
            None => {
                let mut operations = self.current_method_names().await?;
                operations.extend(
                    entries
                        .iter()
                        .filter_map(|entry| entry.operation.as_ref()?.name().map(str::to_string)),
                );
                operations
            }
        };
        operations.sort();
        operations.dedup();
        let agent_id = self.agent_id();
        let fingerprint = self.initial_worker_metadata.fingerprint;
        let keys: HashMap<InvocationId, String> = operations
            .into_iter()
            .map(|operation| {
                let key = request_key.finalize(agent_id, fingerprint, &operation).id();
                (key, operation)
            })
            .collect();
        Ok(entries
            .into_iter()
            .filter(|entry| {
                keys.get(&entry.key).is_some_and(|operation| {
                    entry.operation.as_ref().and_then(InvocationOperation::name)
                        == Some(operation.as_str())
                })
            })
            .collect())
    }

    /// The method names of the agent type at the agent's current component revision.
    async fn current_method_names(&self) -> Result<Vec<String>, WorkerExecutorError> {
        let Some(parsed) = self.parsed_agent_id.as_ref() else {
            return Ok(Vec::new());
        };
        let revision = self.state_actor.try_status().await?.component_revision;
        let component = self
            .component_service()
            .get_metadata(self.component_id(), Some(revision))
            .await?;
        Ok(component
            .metadata
            .find_agent_type_by_name_ref(&parsed.agent_type)
            .map(|agent_type| {
                agent_type
                    .methods
                    .iter()
                    .map(|method| method.name.clone())
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The results that the recent results of `status` no longer hold, from the persisted result
    /// index once catch-up has completed it. An index that is still incomplete is a transient
    /// error.
    async fn older_invocation_results(
        &self,
        status: &AgentStatusRecord,
    ) -> Result<Vec<(InvocationId, InvocationResultPosition)>, WorkerExecutorError> {
        let worker_service = self.deps.worker_service();
        worker_service
            .catch_up_invocation_result_index(
                &self.owned_agent_id,
                self.agent_mode(),
                self.initial_worker_metadata.fingerprint,
                status,
            )
            .await?;
        match worker_service
            .list_invocation_result_index(
                &self.owned_agent_id,
                self.initial_worker_metadata.fingerprint,
                status,
            )
            .await
        {
            Ok(Some(results)) => Ok(results),
            Ok(None) => Err(super::outcome::invocation_result_index_unavailable(
                &self.agent_id(),
                "the invocation result index is incomplete after catch-up".to_string(),
            )
            .into()),
            Err(error) => Err(super::outcome::invocation_result_index_unavailable(
                &self.agent_id(),
                error,
            )
            .into()),
        }
    }
}

/// The kind of an invocation, from its operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationKind {
    Method,
    Constructor,
    Tool,
}

impl InvocationListEntry {
    /// `None` when the request of the invocation is not known.
    pub fn kind(&self) -> Option<InvocationKind> {
        match self.operation.as_ref()? {
            InvocationOperation::Method(_) => Some(InvocationKind::Method),
            InvocationOperation::Constructor => Some(InvocationKind::Constructor),
            InvocationOperation::Tool(_) => Some(InvocationKind::Tool),
            InvocationOperation::Internal => None,
        }
    }

    /// Whether `list` shows the entry: internal operations are not listed. An entry whose
    /// operation is not known is listed.
    fn is_listed(&self) -> bool {
        self.operation
            .as_ref()
            .is_none_or(InvocationOperation::is_listed)
    }

    fn position(&self) -> InvocationPagePosition {
        InvocationPagePosition {
            last_change: self.last_change,
            key: self.key,
        }
    }
}

/// Which invocations `list` returns. Every set field must match.
#[derive(Clone, Debug, Default)]
pub struct InvocationFilter {
    pub operation: Option<String>,
    /// Any of these states; every state when empty.
    pub states: Vec<InvocationState>,
    pub kind: Option<InvocationKind>,
    /// Inclusive bounds on `changed_at`.
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
}

impl InvocationFilter {
    fn matches(&self, entry: &InvocationListEntry) -> bool {
        (self.states.is_empty() || self.states.contains(&entry.state))
            && self.since.is_none_or(|since| entry.changed_at >= since)
            && self.until.is_none_or(|until| entry.changed_at <= until)
            && self.operation.as_deref().is_none_or(|operation| {
                entry.operation.as_ref().and_then(InvocationOperation::name) == Some(operation)
            })
            && self.kind.is_none_or(|kind| entry.kind() == Some(kind))
    }
}

/// The largest page `list` returns.
pub const MAX_INVOCATION_PAGE_SIZE: usize = 1000;

/// The page size of a `list` request that names none.
pub const DEFAULT_INVOCATION_PAGE_SIZE: usize = 100;

/// Where a page of `list` ended: the last listed invocation in the list order. The next page
/// starts after it, so an invocation that changes between pages moves before the cursor and is
/// listed at most once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvocationPagePosition {
    pub last_change: OplogIndex,
    pub key: InvocationId,
}

impl InvocationPagePosition {
    /// Whether `entry` comes after this position in the list order.
    fn precedes(&self, entry: &InvocationListEntry) -> bool {
        (entry.last_change, entry.key.0) < (self.last_change, self.key.0)
    }
}

/// One page of the listed entries of `entries` that match `filter`, and the position the next
/// page starts after when more invocations may follow. `entries` are in the list order.
pub(crate) fn page_invocations(
    entries: Vec<InvocationListEntry>,
    filter: &InvocationFilter,
    after: Option<InvocationPagePosition>,
    page_size: usize,
) -> (Vec<InvocationListEntry>, Option<InvocationPagePosition>) {
    let page_size = match page_size {
        0 => DEFAULT_INVOCATION_PAGE_SIZE,
        page_size => page_size.min(MAX_INVOCATION_PAGE_SIZE),
    };
    let mut page = Vec::with_capacity(page_size.min(entries.len()));
    let mut more = false;
    for entry in entries {
        if after.is_some_and(|after| !after.precedes(&entry))
            || !filter.matches(&entry)
            || !entry.is_listed()
        {
            continue;
        }
        if page.len() == page_size {
            more = true;
            break;
        }
        page.push(entry);
    }
    let next = if more {
        page.last().map(InvocationListEntry::position)
    } else {
        None
    };
    (page, next)
}

/// The invocations of `status` and `results`, the latest change first with the invocation ID as
/// the tie-breaker.
///
/// A result answers for its key, as in the result lookup: a queued duplicate of it is not
/// listed, and a rejection after the start ends the running invocation. A retry marker of the
/// running invocation is the exception while the agent can still retry it.
pub(crate) fn merge_invocations(
    status: &AgentStatusRecord,
    mut results: HashMap<InvocationId, InvocationResultPosition>,
    now: Timestamp,
) -> Vec<InvocationListEntry> {
    let mut entries: Vec<InvocationListEntry> = Vec::new();
    if let Some(running) = &status.running
        && !answered(&results, &running.key)
    {
        results.remove(&running.key);
        if let Some(entry) = InvocationListEntry::of_running(status) {
            entries.push(entry);
        }
    }
    // Any result of a key that does not run answers for it, a retry marker included: that
    // marker is the result of exhausted retries, and the queued duplicate is discarded.
    for pending in &status.pending_invocations {
        if let Some(key) = pending.idempotency_key
            && !results.contains_key(&key)
            && status.current_idempotency_key() != Some(&key)
        {
            entries.push(InvocationListEntry::of_pending(key, pending, now));
        }
    }
    for (key, position) in results {
        entries.push(InvocationListEntry::of_result(key, position));
    }
    entries.sort_by(|left, right| {
        right
            .last_change
            .cmp(&left.last_change)
            .then_with(|| right.key.0.cmp(&left.key.0))
    });
    entries
}

/// Whether `key` has a result other than a retry marker.
fn answered(results: &HashMap<InvocationId, InvocationResultPosition>, key: &InvocationId) -> bool {
    results
        .get(key)
        .is_some_and(|position| position.end != InvocationEnd::Retrying)
}

/// Keeps the position of the latest change of `key`. A withdrawal never replaces a result: a key
/// with a result is withdrawn only as a queued duplicate, which keeps the existing result.
fn keep_latest(
    results: &mut HashMap<InvocationId, InvocationResultPosition>,
    key: InvocationId,
    position: InvocationResultPosition,
) {
    match results.get(&key) {
        Some(known)
            if known.last_change() >= position.last_change()
                || position.end == InvocationEnd::Cancelled => {}
        _ => {
            results.insert(key, position);
        }
    }
}

#[cfg(test)]
mod tests;

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

use crate::services::oplog::OplogOps;
use crate::worker::*;
use golem_common::model::oplog::{OplogEntry, OplogErrorKind};
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
            },
            operation: position.operation,
            accepted_at: position.accepted_at.map(Timestamp::rounded),
            changed_at: position.changed_at.rounded(),
            last_change: position.terminal,
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
    /// once they are not the complete history, the persisted result index or the oplog. Each
    /// call costs O(history). Only the entries that reach the page read their missing operation
    /// from their request entry.
    pub async fn list_invocation_page(
        &self,
        filter: &InvocationFilter,
        after: Option<InvocationPagePosition>,
        page_size: usize,
    ) -> Result<(Vec<InvocationListEntry>, Option<InvocationPagePosition>), WorkerExecutorError>
    {
        let entries = self.merged_invocations().await?;
        page_invocations(entries, filter, after, page_size, |request| {
            self.request_operation_at(request)
        })
        .await
    }

    /// The merged invocations, before their missing operations are read from their requests.
    async fn merged_invocations(
        &self,
    ) -> Result<Vec<(InvocationListEntry, Option<OplogIndex>)>, WorkerExecutorError> {
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

    /// Reads the operation of `entry` from its request entry when the entry does not know it,
    /// downloading an external payload.
    async fn fill_operation(
        &self,
        entry: &mut InvocationListEntry,
        request: Option<OplogIndex>,
    ) -> Result<(), WorkerExecutorError> {
        if entry.operation.is_none()
            && let Some(request) = request
        {
            entry.operation = self.request_operation_at(request).await?;
        }
        Ok(())
    }

    /// The listed invocation with `key`, or `None` when the agent has none.
    pub async fn get_invocation(
        &self,
        key: &InvocationId,
    ) -> Result<Option<InvocationListEntry>, WorkerExecutorError> {
        let Some((mut entry, request)) = self
            .merged_invocations()
            .await?
            .into_iter()
            .find(|(entry, _)| entry.key == *key)
        else {
            return Ok(None);
        };
        self.fill_operation(&mut entry, request).await?;
        Ok(entry.is_listed().then_some(entry))
    }

    /// The invocations of the agent that `request_key` started, at most one per operation.
    ///
    /// The candidate operations are `operation` when it is set, otherwise the methods of the
    /// agent's current type and every operation that the entries already know. So a key of a
    /// method that a later revision removed is found while one entry of that method keeps its
    /// operation; when every such entry has its operation only in an external payload, the
    /// caller names the operation. Only the entries whose ID matches a candidate read their
    /// missing operation, so a lookup downloads no payload of other invocations.
    pub async fn lookup_invocations(
        &self,
        request_key: &InvocationRequestKey,
        operation: Option<&str>,
    ) -> Result<Vec<InvocationListEntry>, WorkerExecutorError> {
        let entries = self.merged_invocations().await?;
        let mut operations: Vec<String> =
            match operation {
                Some(operation) => vec![operation.to_string()],
                None => {
                    let mut operations = self.current_method_names().await?;
                    operations.extend(entries.iter().filter_map(|(entry, _)| {
                        entry.operation.as_ref()?.name().map(str::to_string)
                    }));
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
        let mut found = Vec::new();
        for (mut entry, request) in entries {
            let Some(operation) = keys.get(&entry.key) else {
                continue;
            };
            self.fill_operation(&mut entry, request).await?;
            if entry.operation.as_ref().and_then(InvocationOperation::name)
                == Some(operation.as_str())
            {
                found.push(entry);
            }
        }
        Ok(found)
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

    /// The results that the recent results of `status` no longer hold: from the persisted result
    /// index once it is caught up and complete, otherwise from the oplog.
    async fn older_invocation_results(
        &self,
        status: &AgentStatusRecord,
    ) -> Result<Vec<(InvocationId, InvocationResultPosition)>, WorkerExecutorError> {
        let worker_service = self.deps.worker_service();
        if worker_service
            .catch_up_invocation_result_index(
                &self.owned_agent_id,
                self.agent_mode(),
                self.initial_worker_metadata.fingerprint,
                status,
            )
            .await
            .is_ok()
            && let Ok(Some(results)) = worker_service
                .list_invocation_result_index(
                    &self.owned_agent_id,
                    self.initial_worker_metadata.fingerprint,
                    status,
                )
                .await
        {
            return Ok(results);
        }

        let mut results = HashMap::new();
        let mut requests = InvocationRequests::default();
        let mut first = OplogIndex::INITIAL;
        let chunk_size = self
            .deps
            .config()
            .invocation_results
            .physical_index_catch_up_chunk_size
            .max(1);
        while first <= status.oplog_idx {
            let count = (status.oplog_idx.as_u64() - first.as_u64() + 1).min(chunk_size);
            let entries = self
                .deps
                .oplog_service()
                .read_exact(&self.owned_agent_id, self.agent_mode(), first, count)
                .await?;
            if entries.is_empty() {
                return Err(WorkerExecutorError::runtime(format!(
                    "failed to list the invocations of {}: oplog range starting at {first} was empty",
                    self.owned_agent_id
                )));
            }
            fold_invocation_result_entries(
                &mut requests,
                &status.deleted_regions,
                &entries,
                |_, entry| request_operation(entry),
                |key, position| {
                    if !(position.end == InvocationEnd::Cancelled && results.contains_key(key)) {
                        results.insert(*key, position);
                    }
                },
            );
            first = entries.keys().max().unwrap().next();
        }
        Ok(results.into_iter().collect())
    }

    /// The operation of the request at `index`, downloading an external payload.
    async fn request_operation_at(
        &self,
        index: OplogIndex,
    ) -> Result<Option<InvocationOperation>, WorkerExecutorError> {
        let payload = match self.oplog.read(index).await? {
            OplogEntry::PendingAgentInvocation { payload, .. }
            | OplogEntry::AgentInvocationStarted { payload, .. } => payload,
            _ => return Ok(None),
        };
        let payload = self
            .oplog
            .download_payload(payload)
            .await
            .map_err(WorkerExecutorError::runtime)?;
        Ok(payload.operation())
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
    /// The fields that do not need the operation of `entry`.
    fn matches_state_and_time(&self, entry: &InvocationListEntry) -> bool {
        (self.states.is_empty() || self.states.contains(&entry.state))
            && self.since.is_none_or(|since| entry.changed_at >= since)
            && self.until.is_none_or(|until| entry.changed_at <= until)
    }

    /// The fields that need the operation of `entry`.
    fn matches_operation(&self, entry: &InvocationListEntry) -> bool {
        self.operation.as_deref().is_none_or(|operation| {
            entry.operation.as_ref().and_then(InvocationOperation::name) == Some(operation)
        }) && self.kind.is_none_or(|kind| entry.kind() == Some(kind))
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
/// page starts after when more invocations may follow. `entries` are in the list order, each
/// with the index of its request entry when it is known.
///
/// `operation_at` reads the operation of the request entry at an index. It runs only for an entry
/// without an operation that passes the cursor and the filter fields that need no operation, and
/// only until the page and the one entry that says whether more follow are found. So a page
/// costs at most `page_size + 1` reads of a request entry on top of the entries that the
/// operation filter or the internal operations skip.
pub(crate) async fn page_invocations<E, F>(
    entries: Vec<(InvocationListEntry, Option<OplogIndex>)>,
    filter: &InvocationFilter,
    after: Option<InvocationPagePosition>,
    page_size: usize,
    mut operation_at: impl FnMut(OplogIndex) -> F,
) -> Result<(Vec<InvocationListEntry>, Option<InvocationPagePosition>), E>
where
    F: Future<Output = Result<Option<InvocationOperation>, E>>,
{
    let page_size = match page_size {
        0 => DEFAULT_INVOCATION_PAGE_SIZE,
        page_size => page_size.min(MAX_INVOCATION_PAGE_SIZE),
    };
    let mut page = Vec::with_capacity(page_size.min(entries.len()));
    let mut more = false;
    for (mut entry, request) in entries {
        if after.is_some_and(|after| !after.precedes(&entry))
            || !filter.matches_state_and_time(&entry)
        {
            continue;
        }
        if entry.operation.is_none()
            && let Some(request) = request
        {
            entry.operation = operation_at(request).await?;
        }
        if !entry.is_listed() || !filter.matches_operation(&entry) {
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
    Ok((page, next))
}

/// The invocations of `status` and `results`, the latest change first with the invocation ID as
/// the tie-breaker, each with the index of its request entry when it is known.
///
/// A result answers for its key, as in the result lookup: a queued duplicate of it is not
/// listed, and a rejection after the start ends the running invocation. A retry marker of the
/// running invocation is the exception while the agent can still retry it.
pub(crate) fn merge_invocations(
    status: &AgentStatusRecord,
    mut results: HashMap<InvocationId, InvocationResultPosition>,
    now: Timestamp,
) -> Vec<(InvocationListEntry, Option<OplogIndex>)> {
    let mut entries: Vec<(InvocationListEntry, Option<OplogIndex>)> = Vec::new();
    if let Some(running) = &status.running
        && !answered(&results, &running.key)
    {
        results.remove(&running.key);
        if let Some(entry) = InvocationListEntry::of_running(status) {
            entries.push((entry, Some(running.request.index)));
        }
    }
    // Any result of a key that does not run answers for it, a retry marker included: that
    // marker is the result of exhausted retries, and the queued duplicate is discarded.
    for pending in &status.pending_invocations {
        if let Some(key) = pending.idempotency_key
            && !results.contains_key(&key)
            && status.current_idempotency_key() != Some(&key)
        {
            entries.push((
                InvocationListEntry::of_pending(key, pending, now),
                Some(pending.oplog_index),
            ));
        }
    }
    for (key, position) in results {
        let request = position.request;
        entries.push((InvocationListEntry::of_result(key, position), request));
    }
    entries.sort_by(|(left, _), (right, _)| {
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
            if known.terminal >= position.terminal || position.end == InvocationEnd::Cancelled => {}
        _ => {
            results.insert(key, position);
        }
    }
}

#[cfg(test)]
mod tests;

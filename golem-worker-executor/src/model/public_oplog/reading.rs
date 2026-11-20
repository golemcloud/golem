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

use super::*;
use crate::services::golem_config::ScanBudget;
use golem_common::model::oplog::OplogCursor;
use golem_service_base::error::worker_executor::StaleCursorReason;
use std::time::Instant;

/// Collects the entries of an oplog chunk in the form that its reader gets.
trait ChunkBuilder {
    type Entry;

    /// Returns the form of `entry` that the chunk holds, or `None` if the entry does not fit in
    /// the chunk. `component_revision` is the revision that `entry` was decoded with.
    fn admit(
        &mut self,
        entry: PublicOplogEntryWithIndex,
        component_revision: ComponentRevision,
    ) -> Result<Option<Self::Entry>, String>;
}

/// The chunk of a reader in the executor process, which gets every entry in full.
struct InProcessChunk;

impl ChunkBuilder for InProcessChunk {
    type Entry = PublicOplogEntryWithIndex;

    fn admit(
        &mut self,
        entry: PublicOplogEntryWithIndex,
        _component_revision: ComponentRevision,
    ) -> Result<Option<Self::Entry>, String> {
        Ok(Some(entry))
    }
}

/// An oplog page for a remote reader, in its protobuf form.
struct RemotePage {
    limits: PublicOplogPageLimits,
    used_bytes: usize,
    admitted: usize,
}

impl RemotePage {
    fn new(limits: PublicOplogPageLimits) -> Self {
        Self {
            limits,
            used_bytes: 0,
            admitted: 0,
        }
    }
}

impl ChunkBuilder for RemotePage {
    type Entry = golem_api_grpc::proto::golem::worker::OplogPageEntry;

    /// Replaces an entry that is larger than `max_entry_bytes` with an `OmittedOplogEntry`. The
    /// first entry of a page always fits in the byte budget, so that a reader makes progress past
    /// an entry that is larger than the budget.
    fn admit(
        &mut self,
        entry: PublicOplogEntryWithIndex,
        component_revision: ComponentRevision,
    ) -> Result<Option<Self::Entry>, String> {
        let omitted = OmittedOplogEntry {
            oplog_index: entry.oplog_index,
            attribution: entry.attribution.clone(),
            timestamp: entry.entry.timestamp(),
            entry_type: entry.entry.entry_type().to_string(),
            size: 0,
            component_revision,
        };
        let full: golem_api_grpc::proto::golem::worker::OplogEntryWithIndex = entry.try_into()?;
        let full_size = prost::Message::encoded_len(&full);
        let page_entry = if full_size > self.limits.max_entry_bytes {
            PublicOplogPageEntry::Omitted(OmittedOplogEntry {
                size: full_size as u64,
                ..omitted
            })
            .try_into()?
        } else {
            golem_api_grpc::proto::golem::worker::OplogPageEntry {
                entry: Some(
                    golem_api_grpc::proto::golem::worker::oplog_page_entry::Entry::Included(full),
                ),
            }
        };

        let size = prost::Message::encoded_len(&page_entry);
        if self.admitted > 0 && self.used_bytes.saturating_add(size) > self.limits.max_bytes {
            return Ok(None);
        }
        self.used_bytes = self.used_bytes.saturating_add(size);
        self.admitted += 1;
        Ok(Some(page_entry))
    }
}

pub async fn get_public_oplog_chunk(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
) -> Result<PublicOplogChunk, String> {
    let (chunk, _) = read_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        count,
        None,
        &mut InProcessChunk,
    )
    .await?;
    Ok(chunk)
}

/// Reads an oplog page for a remote reader. The page omits entries that are larger than
/// `limits.max_entry_bytes`, and ends early when the next entry does not fit in
/// `limits.max_bytes`. The page reads no entry after `last_oplog_index`. Its `last_index` is the
/// end of the read: the last index of the oplog, or `last_oplog_index` when that is earlier.
pub async fn get_public_oplog_page(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    last_oplog_index: Option<OplogIndex>,
    limits: PublicOplogPageLimits,
) -> Result<PublicOplogChunk<golem_api_grpc::proto::golem::worker::OplogPageEntry>, String> {
    let count = limits.count.min(
        usize::try_from(entries_up_to(initial_oplog_index, last_oplog_index)).unwrap_or(usize::MAX),
    );
    let (mut chunk, _) = read_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        count,
        None,
        &mut RemotePage::new(limits),
    )
    .await?;
    chunk.last_index = read_end(chunk.last_index, last_oplog_index);
    Ok(chunk)
}

/// The count of the oplog entries from `index` to `end`, including both. It is 0 when `end` is
/// before `index`, and `u64::MAX` when there is no `end`. The count saturates, so an `end` at
/// `u64::MAX` does not overflow.
fn entries_up_to(index: OplogIndex, end: Option<OplogIndex>) -> u64 {
    end.map_or(u64::MAX, |end| {
        end.as_u64()
            .checked_sub(index.as_u64())
            .map_or(0, |distance| distance.saturating_add(1))
    })
}

/// The last index of a read that ends at `end`, in an oplog whose last index is `oplog_last`:
/// `end`, or the end of the oplog when `end` is after it or when there is no `end`.
fn read_end(oplog_last: OplogIndex, end: Option<OplogIndex>) -> OplogIndex {
    end.map_or(oplog_last, |end| end.min(oplog_last))
}

/// Reads one oplog entry in full. `component_revision` is the revision to decode the entry with,
/// as an oplog page reports it for an omitted entry; without it, the oplog is scanned up to the
/// entry to find the revision.
pub async fn get_public_oplog_entry(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    oplog_index: OplogIndex,
    component_revision: Option<ComponentRevision>,
) -> Result<PublicOplogEntryWithIndex, WorkerExecutorError> {
    let last_index = oplog_service
        .get_last_index(owned_agent_id, agent_mode)
        .await?;
    if oplog_index < OplogIndex::INITIAL || oplog_index > last_index {
        return Err(WorkerExecutorError::invalid_request(format!(
            "agent {owned_agent_id} has no oplog entry at index {oplog_index}"
        )));
    }
    let component_revision = match component_revision {
        Some(component_revision) => component_revision,
        None => {
            find_component_revision_at(
                oplog_service.clone(),
                owned_agent_id,
                agent_mode,
                oplog_index,
            )
            .await?
        }
    };
    let chunk = get_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        owner_component_id,
        owner_kind,
        agent_type_name,
        component_revision,
        oplog_index,
        1,
    )
    .await
    .map_err(WorkerExecutorError::unknown)?;
    chunk.entries.into_iter().next().ok_or_else(|| {
        WorkerExecutorError::unknown(format!(
            "agent {owned_agent_id} has no oplog entry at index {oplog_index}"
        ))
    })
}

/// Reads up to `count` oplog entries from `initial_oplog_index` and adds the entries that match
/// `filter` to the chunk, in the form that `builder` selects. The chunk ends early at the first
/// matching entry that does not fit; the cursor of the chunk then points to that entry, and the
/// returned flag is `true`.
#[allow(clippy::too_many_arguments)]
async fn read_public_oplog_chunk<B: ChunkBuilder>(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
    filter: Option<&Query>,
    builder: &mut B,
) -> Result<(PublicOplogChunk<B::Entry>, bool), String> {
    let initial_oplog_index = initial_oplog_index.max(OplogIndex::INITIAL);
    let last_index = oplog_service
        .get_last_index(owned_agent_id, agent_mode)
        .await
        .map_err(|error| error.to_string())?;
    let available = if initial_oplog_index <= last_index {
        last_index.as_u64() - initial_oplog_index.as_u64() + 1
    } else {
        0
    };
    let raw_entries = oplog_service
        .read_exact(
            owned_agent_id,
            agent_mode,
            initial_oplog_index,
            (count as u64).min(available),
        )
        .await
        .map_err(|error| error.to_string())?;

    let mut entries = Vec::new();
    let mut current_component_revision = initial_component_revision;
    let mut next_oplog_index = initial_oplog_index;
    let mut first_index_in_chunk = None;
    let mut page_full = false;

    let mut attribution_resolver =
        PublicOplogAttributionResolver::new(oplog_service.clone(), owned_agent_id, agent_mode);
    for (index, raw_entry) in &raw_entries {
        attribution_resolver.cache_entry(*index, raw_entry);
    }

    for (index, raw_entry) in raw_entries {
        let entry_component_revision = raw_entry
            .specifies_component_revision()
            .unwrap_or(current_component_revision);

        let attribution = attribution_resolver
            .attribution_for_entry(index, &raw_entry)
            .await?;
        let entry = PublicOplogEntry::from_oplog_entry(
            index,
            raw_entry,
            oplog_service.clone(),
            components.clone(),
            owned_agent_id,
            agent_mode,
            owner_component_id,
            owner_kind,
            agent_type_name,
            entry_component_revision,
        )
        .await?;
        if filter.is_none_or(|query| entry.matches(query)) {
            let entry = PublicOplogEntryWithIndex {
                oplog_index: index,
                attribution,
                entry,
            };
            match builder.admit(entry, entry_component_revision)? {
                Some(chunk_entry) => entries.push(chunk_entry),
                None => {
                    page_full = true;
                    break;
                }
            }
        }

        if first_index_in_chunk.is_none() {
            first_index_in_chunk = Some(index);
        }
        current_component_revision = entry_component_revision;
        next_oplog_index = index.next();
    }

    Ok((
        PublicOplogChunk {
            entries,
            next_oplog_index,
            current_component_revision,
            first_index_in_chunk: first_index_in_chunk.unwrap_or(initial_oplog_index),
            last_index,
        },
        page_full,
    ))
}

/// Searches the oplog for a guest. The search reads no entry after `last_oplog_index`; the guest
/// passes the `last_index` of its first result, so that its own entries, which every page adds,
/// do not move the end of the search.
pub async fn search_public_oplog(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    last_oplog_index: Option<OplogIndex>,
    count: usize,
    query: &str,
    budget: ScanBudget,
) -> Result<PublicOplogSearchResult, String> {
    search_public_oplog_with(
        component_service,
        oplog_service,
        owned_agent_id,
        agent_mode,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        last_oplog_index,
        count,
        query,
        budget,
        InProcessChunk,
    )
    .await
}

/// Searches the oplog for a remote reader. The result is an oplog page with the limits of
/// `get_public_oplog_page`; only the matching entries count against the limits. The search
/// reads no entry after `last_oplog_index`, and stops early when `budget` is spent.
pub async fn search_public_oplog_page(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    last_oplog_index: Option<OplogIndex>,
    limits: PublicOplogPageLimits,
    query: &str,
    budget: ScanBudget,
) -> Result<PublicOplogSearchResult<golem_api_grpc::proto::golem::worker::OplogPageEntry>, String> {
    search_public_oplog_with(
        component_service,
        oplog_service,
        owned_agent_id,
        agent_mode,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        last_oplog_index,
        limits.count,
        query,
        budget,
        RemotePage::new(limits),
    )
    .await
}

/// Searches the oplog from `initial_oplog_index` until the result holds `count` matches, the page
/// is full, the oplog or `last_oplog_index` ends the search, or `budget` is spent. The budget
/// counts the oplog entries that the search reads, matching or not, and the time since the call
/// started. Each chunk reads at most as many entries as the result still needs matches, so the
/// result never holds more than `count` matches. A search always reads at least one chunk, and
/// the result points to the first entry it did not read.
#[allow(clippy::too_many_arguments)]
async fn search_public_oplog_with<B: ChunkBuilder>(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    last_oplog_index: Option<OplogIndex>,
    count: usize,
    query: &str,
    budget: ScanBudget,
    mut builder: B,
) -> Result<PublicOplogSearchResult<B::Entry>, String> {
    let started = Instant::now();
    let mut scanned = 0u64;
    let mut results = Vec::new();
    let mut last_index;
    let mut current_index = initial_oplog_index;
    let mut current_component_revision = initial_component_revision;

    let query = Query::parse(query)?;

    loop {
        let chunk_size = (count.saturating_sub(results.len()) as u64)
            .min(budget.remaining_entries(scanned))
            .min(entries_up_to(current_index, last_oplog_index)) as usize;
        let (chunk, page_full) = read_public_oplog_chunk(
            component_service.clone(),
            oplog_service.clone(),
            owned_agent_id,
            agent_mode,
            owner_component_id,
            owner_kind,
            agent_type_name,
            current_component_revision,
            current_index,
            chunk_size,
            Some(&query),
            &mut builder,
        )
        .await?;

        results.extend(chunk.entries);

        scanned += chunk.next_oplog_index.as_u64() - current_index.as_u64();
        last_index = read_end(chunk.last_index, last_oplog_index);
        current_index = chunk.next_oplog_index;
        current_component_revision = chunk.current_component_revision;

        if page_full
            || current_index > last_index
            || results.len() >= count
            || budget.is_spent(scanned, started)
        {
            break;
        }
    }

    Ok(PublicOplogSearchResult {
        entries: results,
        next_oplog_index: current_index,
        current_component_revision,
        last_index,
    })
}

pub async fn find_component_revision_at(
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    start: OplogIndex,
) -> Result<ComponentRevision, WorkerExecutorError> {
    let mut initial_component_revision = ComponentRevision::INITIAL;
    let last_oplog_index = oplog_service
        .get_last_index(owned_agent_id, agent_mode)
        .await?;
    const BATCH_SIZE: u64 = 256;
    let end = start.min(last_oplog_index.next());
    let mut current = OplogIndex::INITIAL;
    while current < end {
        let count = (end.as_u64() - current.as_u64()).min(BATCH_SIZE);
        let entries = oplog_service
            .read_exact(owned_agent_id, agent_mode, current, count)
            .await?;
        for entry in entries.into_values() {
            if let Some(revision) = entry.specifies_component_revision() {
                initial_component_revision = revision;
            }
        }
        current = OplogIndex::from_u64(current.as_u64() + count);
    }

    Ok(initial_component_revision)
}

/// The cursor of an oplog page of `query` whose next unread entry is `next_oplog_index`. A page has
/// a cursor if and only if the oplog has entries after the page, also when the page itself is
/// empty because a search spent its scan budget before any match.
pub fn oplog_page_cursor(
    next_oplog_index: OplogIndex,
    current_component_revision: ComponentRevision,
    last_index: OplogIndex,
    query: Option<&str>,
) -> Option<OplogCursor> {
    (next_oplog_index <= last_index).then(|| OplogCursor {
        next_oplog_index: next_oplog_index.as_u64(),
        current_component_revision: current_component_revision.into(),
        query_hash: OplogCursor::query_hash(query),
    })
}

/// Refuses a cursor of the pages of another query as stale, because continuing it would mix the
/// results of two reads. The check reads no storage, so a read makes it before it resolves the
/// agent: a mismatched cursor then fails the same way whether or not the agent and its storage
/// can be read.
pub fn check_oplog_cursor_query(
    owned_agent_id: &OwnedAgentId,
    cursor: Option<&OplogCursor>,
    query: Option<&str>,
) -> Result<(), WorkerExecutorError> {
    match cursor {
        Some(cursor) if cursor.query_hash != OplogCursor::query_hash(query) => {
            Err(WorkerExecutorError::StaleCursor {
                agent_id: owned_agent_id.agent_id.clone(),
                next_oplog_index: OplogIndex::from_u64(cursor.next_oplog_index),
                reason: StaleCursorReason::QueryMismatch,
            })
        }
        _ => Ok(()),
    }
}

/// Where a paged oplog read starts, and the component revision to decode its first entry with:
/// the cursor of the previous page, or else `from` with the revision in effect there. The caller
/// checks the query of the cursor first, with [`check_oplog_cursor_query`].
///
/// A page hands out a cursor only while the oplog has entries after it, and the oplog only grows,
/// so a cursor that points past the last entry did not come from a page of this oplog: the agent
/// was deleted and created again since, or the cursor was made up. It is refused as stale, instead
/// of read as an empty page, which a caller would take for the end of the oplog. The revision of
/// the cursor is not checked, because that would scan the oplog up to the cursor on every page.
pub async fn oplog_page_start(
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    cursor: Option<OplogCursor>,
    from: OplogIndex,
) -> Result<(OplogIndex, ComponentRevision), WorkerExecutorError> {
    match cursor {
        Some(cursor) => {
            let last_index = oplog_service
                .get_last_index(owned_agent_id, agent_mode)
                .await?;
            if cursor.next_oplog_index > last_index.as_u64() {
                return Err(WorkerExecutorError::StaleCursor {
                    agent_id: owned_agent_id.agent_id.clone(),
                    next_oplog_index: OplogIndex::from_u64(cursor.next_oplog_index),
                    reason: StaleCursorReason::PastEnd {
                        last_oplog_index: last_index,
                    },
                });
            }
            let revision = cursor.current_component_revision.try_into().map_err(|e| {
                WorkerExecutorError::invalid_request(format!(
                    "the oplog cursor {cursor} has an invalid component revision: {e}"
                ))
            })?;
            Ok((OplogIndex::from_u64(cursor.next_oplog_index), revision))
        }
        None => {
            let revision =
                find_component_revision_at(oplog_service, owned_agent_id, agent_mode, from).await?;
            Ok((from, revision))
        }
    }
}

#[cfg(test)]
mod range_tests {
    use super::{entries_up_to, read_end};
    use golem_common::model::oplog::OplogIndex;
    use test_r::test;

    #[test]
    fn a_read_counts_the_entries_up_to_its_end_without_overflow() {
        let index = OplogIndex::from_u64;
        [
            ((1, None), u64::MAX),
            ((1, Some(1)), 1),
            ((3, Some(7)), 5),
            ((8, Some(7)), 0),
            ((9, Some(7)), 0),
            ((1, Some(u64::MAX)), u64::MAX),
            ((2, Some(u64::MAX)), u64::MAX - 1),
            ((u64::MAX, Some(u64::MAX)), 1),
        ]
        .into_iter()
        .for_each(|((from, end), expected)| {
            assert_eq!(
                entries_up_to(index(from), end.map(index)),
                expected,
                "from {from} to {end:?}"
            )
        });
    }

    #[test]
    fn a_read_ends_at_its_end_or_at_the_end_of_the_oplog() {
        let index = OplogIndex::from_u64;
        [
            ((10, None), 10),
            ((10, Some(4)), 4),
            ((10, Some(10)), 10),
            ((10, Some(11)), 10),
            ((10, Some(u64::MAX)), 10),
        ]
        .into_iter()
        .for_each(|((oplog_last, end), expected)| {
            assert_eq!(
                read_end(index(oplog_last), end.map(index)),
                index(expected),
                "oplog last {oplog_last}, end {end:?}"
            )
        });
    }
}

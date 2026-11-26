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
use golem_common::model::oplog::{
    OplogCursor, OplogRange, OplogScan, OplogScanAllowance, OplogSearchResume,
};
use golem_service_base::error::worker_executor::StaleCursorReason;
use std::time::{Duration, Instant};

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
    claims: ClaimVisibility,
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
        claims,
        &mut InProcessChunk,
    )
    .await?;
    Ok(chunk)
}

/// Reads an oplog page for a remote reader. The page omits entries that are larger than
/// `limits.max_entry_bytes`, and ends early when the next entry does not fit in `limits.max_bytes`.
/// The entries hold claims only with `ClaimVisibility::Shown`, so only claims that the reader asked
/// for count against the limits. The page reads from the first index of `range`, and reads no
/// entry after its end. Its `last_index` is the last index of the range in the oplog. That is the
/// last index of the oplog, or the end of the range when that is earlier.
pub async fn get_public_oplog_page(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    range: OplogRange,
    limits: PublicOplogPageLimits,
    claims: ClaimVisibility,
) -> Result<PublicOplogChunk<golem_api_grpc::proto::golem::worker::OplogPageEntry>, String> {
    let start = range.first_index_in(OplogIndex::INITIAL);
    let count = range.page_count(start, u64::try_from(limits.count).unwrap_or(u64::MAX));
    let (mut chunk, _) = read_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        start,
        count,
        None,
        claims,
        &mut RemotePage::new(limits),
    )
    .await?;
    chunk.last_index = range.last_index_in(chunk.last_index);
    Ok(chunk)
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
    claims: ClaimVisibility,
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
        claims,
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
    claims: ClaimVisibility,
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
            claims,
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
/// do not move the end of the search. A search from after `last_oplog_index` reads no entry and
/// gives an empty result, which tells the guest that the search is at its end.
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
    claims: ClaimVisibility,
) -> Result<PublicOplogSearchResult, String> {
    let range = match OplogRange::new(Some(initial_oplog_index), last_oplog_index) {
        Ok(range) => range,
        Err(ends_before_start) => {
            return Ok(PublicOplogSearchResult {
                entries: Vec::new(),
                next_oplog_index: initial_oplog_index,
                current_component_revision: initial_component_revision,
                last_index: ends_before_start.end(),
            });
        }
    };
    search_public_oplog_with(
        component_service,
        oplog_service,
        owned_agent_id,
        agent_mode,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        range,
        count,
        &Query::parse(query)?,
        RangeScan::within(budget),
        claims,
        InProcessChunk,
    )
    .await
}

/// An oplog search page for a remote reader.
#[derive(Debug)]
pub struct PublicOplogSearchPage {
    pub entries: Vec<golem_api_grpc::proto::golem::worker::OplogPageEntry>,
    /// The cursor of the next page; `None` when the search reached the end of its range.
    pub next: Option<OplogCursor>,
    /// The last index of the range in the oplog.
    pub last_index: OplogIndex,
}

/// Searches one page of `range` for a remote reader, from `cursor`. The page has the limits of
/// [`get_public_oplog_page`]; only the matching entries count against the limits.
///
/// `budget` covers the whole call. A search of a range with a start first scans the oplog before
/// the start for the component revision at the start. That scan counts against `budget`. When
/// the budget runs out before the scan reaches the start, the page is empty. Its cursor then
/// continues the scan, and holds the start of the range as its `scan_to`. Each call reads at least
/// one entry before it checks the budget, so each cursor is after the cursor of the request.
///
/// A range that holds no index of the oplog gives an empty page without a cursor, before any
/// scan. A cursor that points past the last entry of the oplog is stale, as for
/// [`oplog_page_start`].
#[allow(clippy::too_many_arguments)]
pub async fn search_public_oplog_remote(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    cursor: Option<OplogCursor>,
    range: OplogRange,
    limits: PublicOplogPageLimits,
    query: &str,
    budget: ScanBudget,
    claims: ClaimVisibility,
) -> Result<PublicOplogSearchPage, WorkerExecutorError> {
    let mut scan = RangeScan::within(budget);
    let parsed_query = Query::parse(query).map_err(WorkerExecutorError::unknown)?;
    let last = oplog_service
        .get_last_index(owned_agent_id, agent_mode)
        .await?;
    let (resume, revision) = match &cursor {
        None => (
            OplogSearchResume::Scan {
                next: OplogIndex::INITIAL,
                range,
            },
            ComponentRevision::INITIAL,
        ),
        Some(cursor) => {
            let resume = range
                .resume_search(cursor)
                .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
            (resume, oplog_cursor_revision(owned_agent_id, cursor, last)?)
        }
    };
    let (range, scan_from) = match resume {
        OplogSearchResume::Scan { next, range } => (range, Some(next)),
        OplogSearchResume::Read(rest) => (rest, None),
    };
    let empty = |next: Option<OplogCursor>| PublicOplogSearchPage {
        entries: Vec::new(),
        next,
        last_index: range.last_index_in(last),
    };
    if range.is_empty_in(last) {
        return Ok(empty(None));
    }
    let start = range.first_index_in(OplogIndex::INITIAL);
    let revision = match scan_from {
        None => revision,
        Some(next) => {
            let scanned = scan_component_revision_from(
                oplog_service.clone(),
                owned_agent_id,
                agent_mode,
                next,
                revision,
                start.min(last.next()),
                1,
                &mut scan,
            )
            .await?;
            match scanned {
                RevisionScan::Found(revision) => revision,
                RevisionScan::Spent { next, revision } => {
                    return Ok(empty(Some(OplogCursor {
                        next_oplog_index: next.as_u64(),
                        current_component_revision: revision.into(),
                        query_hash: OplogCursor::query_hash(Some(query)),
                        scan_to: Some(start.as_u64()),
                    })));
                }
            }
        }
    };
    let chunk = search_public_oplog_with(
        component_service,
        oplog_service,
        owned_agent_id,
        agent_mode,
        owner_component_id,
        owner_kind,
        agent_type_name,
        revision,
        range,
        limits.count,
        &parsed_query,
        scan,
        claims,
        RemotePage::new(limits),
    )
    .await
    .map_err(WorkerExecutorError::unknown)?;
    Ok(PublicOplogSearchPage {
        next: oplog_page_cursor(
            chunk.next_oplog_index,
            chunk.current_component_revision,
            chunk.last_index,
            Some(query),
        ),
        entries: chunk.entries,
        last_index: chunk.last_index,
    })
}

/// Searches the oplog from the first index of `range`. The search stops when the result holds
/// `count` matches or the page is full. It also stops at the end of the oplog, at the end of
/// `range`, or when `scan` is spent. The scan counts the oplog entries that the search reads,
/// matching or not, and the time since the scan started. The caller can charge other reads of the
/// same call to `scan` first. Each chunk reads at most as many entries as the result still needs
/// matches, so the result never holds more than `count` matches. A search always reads at least
/// one chunk, and the result points to the first entry it did not read.
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
    range: OplogRange,
    count: usize,
    query: &Query,
    mut scan: RangeScan,
    claims: ClaimVisibility,
    mut builder: B,
) -> Result<PublicOplogSearchResult<B::Entry>, String> {
    let mut results = Vec::new();
    let mut last_index;
    let mut current_index = range.first_index_in(OplogIndex::INITIAL);
    let mut current_component_revision = initial_component_revision;

    loop {
        let chunk_size = range.page_count(
            current_index,
            u64::try_from(count.saturating_sub(results.len()))
                .unwrap_or(u64::MAX)
                .min(scan.remaining_entries()),
        );
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
            Some(query),
            claims,
            &mut builder,
        )
        .await?;

        results.extend(chunk.entries);

        scan.charge(chunk.next_oplog_index.as_u64() - current_index.as_u64());
        last_index = range.last_index_in(chunk.last_index);
        current_index = chunk.next_oplog_index;
        current_component_revision = chunk.current_component_revision;

        if page_full || current_index > last_index || results.len() >= count || scan.is_spent() {
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
    let revision = scan_component_revision(
        oplog_service,
        owned_agent_id,
        agent_mode,
        start,
        &mut RangeScan::unlimited(),
    )
    .await?;
    Ok(revision.expect("an unlimited scan finds the component revision"))
}

/// Scans the oplog entries before `start` for the component revision in effect at `start`, and
/// charges the entries that it reads to `scan`. Returns `None` when `scan` is spent before the
/// scan reaches `start`. The scan checks the allowance between batches.
async fn scan_component_revision(
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    start: OplogIndex,
    scan: &mut RangeScan,
) -> Result<Option<ComponentRevision>, WorkerExecutorError> {
    let mut initial_component_revision = ComponentRevision::INITIAL;
    let last_oplog_index = oplog_service
        .get_last_index(owned_agent_id, agent_mode)
        .await?;
    const BATCH_SIZE: u64 = 256;
    let end = start.min(last_oplog_index.next());
    let mut current = OplogIndex::INITIAL;
    while current < end {
        if scan.is_spent() {
            return Ok(None);
        }
        let count = (end.as_u64() - current.as_u64())
            .min(BATCH_SIZE)
            .min(scan.remaining_entries());
        let entries = oplog_service
            .read_exact(owned_agent_id, agent_mode, current, count)
            .await?;
        for entry in entries.into_values() {
            if let Some(revision) = entry.specifies_component_revision() {
                initial_component_revision = revision;
            }
        }
        scan.charge(count);
        current = OplogIndex::from_u64(current.as_u64() + count);
    }

    Ok(Some(initial_component_revision))
}

/// Where a scan for the component revision stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevisionScan {
    /// The scan reached its end. The revision is in effect at the end.
    Found(ComponentRevision),
    /// The allowance ran out before the end. `revision` is in effect at `next`, the first entry
    /// that the scan did not read.
    Spent {
        next: OplogIndex,
        revision: ComponentRevision,
    },
}

/// A position of a scan for the component revision: `revision` is in effect at `next`, the first
/// entry that the scan did not read.
#[derive(Debug, Clone, Copy)]
struct RevisionScanAt {
    next: OplogIndex,
    revision: ComponentRevision,
}

/// Scans the oplog entries from `next` to before `end` for the component revision in effect at
/// `end`. `revision` is in effect at `next`. The scan charges the entries that it reads to `scan`.
/// It reads at least `at_least` entries before it checks the allowance, or all entries up to
/// `end` when they are fewer. After that, it checks the allowance before each batch.
#[allow(clippy::too_many_arguments)]
async fn scan_component_revision_from(
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    next: OplogIndex,
    revision: ComponentRevision,
    end: OplogIndex,
    at_least: u64,
    scan: &mut RangeScan,
) -> Result<RevisionScan, WorkerExecutorError> {
    use futures::TryStreamExt;
    const BATCH_SIZE: u64 = 256;
    let start = RevisionScanAt { next, revision };
    // Each item is the position after one batch. The batches stop at `end`, or when the allowance
    // is spent after the first `at_least` entries.
    let batches = futures::stream::try_unfold((start, scan), move |(at, scan)| {
        let oplog_service = oplog_service.clone();
        async move {
            let required = at_least.saturating_sub(at.next.as_u64() - start.next.as_u64());
            if at.next >= end || (required == 0 && scan.is_spent()) {
                return Ok(None);
            }
            let count = (end.as_u64() - at.next.as_u64())
                .min(BATCH_SIZE)
                .min(scan.remaining_entries().max(required));
            let revision = oplog_service
                .read_exact(owned_agent_id, agent_mode, at.next, count)
                .await?
                .into_values()
                .filter_map(|entry| entry.specifies_component_revision())
                .next_back()
                .unwrap_or(at.revision);
            scan.charge(count);
            let after = RevisionScanAt {
                next: OplogIndex::from_u64(at.next.as_u64() + count),
                revision,
            };
            Ok::<_, WorkerExecutorError>(Some((after, (after, scan))))
        }
    });
    let last = batches
        .try_fold(start, |_, after| std::future::ready(Ok(after)))
        .await?;
    Ok(if last.next >= end {
        RevisionScan::Found(last.revision)
    } else {
        RevisionScan::Spent {
            next: last.next,
            revision: last.revision,
        }
    })
}

/// The allowance of a scan that has no limit.
const UNLIMITED_SCAN: OplogScanAllowance = OplogScanAllowance {
    entries: u64::MAX,
    millis: u64::MAX,
};

/// The allowance of an oplog range read, and what the read charged to it so far. The time runs
/// from the creation of the scan.
struct RangeScan {
    limit: OplogScanAllowance,
    started: Instant,
    scanned: u64,
}

impl RangeScan {
    fn new(limit: OplogScanAllowance) -> Self {
        Self {
            limit,
            started: Instant::now(),
            scanned: 0,
        }
    }

    fn unlimited() -> Self {
        Self::new(UNLIMITED_SCAN)
    }

    /// The scan of a call that `budget` bounds. The scan stops at the entry limit or at the time
    /// limit of `budget`. The time limit counts whole milliseconds and drops the rest, so a time
    /// limit below 1 ms is spent after the first chunk.
    fn within(budget: ScanBudget) -> Self {
        Self::new(oplog_range_allowance(Some(UNLIMITED_SCAN), budget))
    }

    fn remaining_entries(&self) -> u64 {
        self.limit.entries.saturating_sub(self.scanned)
    }

    fn charge(&mut self, entries: u64) {
        self.scanned = self.scanned.saturating_add(entries);
    }

    fn elapsed_millis(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_micros().div_ceil(1000)).unwrap_or(u64::MAX)
    }

    fn is_spent(&self) -> bool {
        self.scanned >= self.limit.entries
            || self.started.elapsed() >= Duration::from_millis(self.limit.millis)
    }

    fn report(&self, exhausted: bool) -> OplogScan {
        OplogScan {
            limit: self.limit,
            charged_entries: self.scanned,
            charged_millis: self.elapsed_millis(),
            exhausted,
        }
    }
}

/// An oplog page of a range read, with its scan accounting.
pub struct PublicOplogRangePage {
    pub entries: Vec<golem_api_grpc::proto::golem::worker::OplogPageEntry>,
    /// The cursor of the next page of the range; `None` when the page reached the end of the
    /// range, or when the allowance ran out before the read found the component revision at the
    /// start of the range.
    pub next: Option<OplogCursor>,
    pub first_index_in_chunk: OplogIndex,
    pub last_index: OplogIndex,
    pub scan: OplogScan,
}

/// The allowance of a range read. A read without an allowance has no limit: its count and its size
/// still bound each of its pages. A read with an allowance gets `requested`, clamped to
/// `configured`. An executor never grants more than its own configured allowance. So a reader that
/// passes on what is left of its allowance gets no fresh allowance from a retry or another
/// executor.
pub fn oplog_range_allowance(
    requested: Option<OplogScanAllowance>,
    configured: ScanBudget,
) -> OplogScanAllowance {
    let configured_millis = u64::try_from(configured.time.as_millis()).unwrap_or(u64::MAX);
    match requested {
        Some(requested) => OplogScanAllowance {
            entries: requested.entries.min(configured.entries.get()),
            millis: requested.millis.min(configured_millis),
        },
        None => UNLIMITED_SCAN,
    }
}

/// Reads one oplog page of the range from `from` to `to`, inclusive, for a remote reader, within
/// `allowance`. The page has the limits of [`get_public_oplog_page`], and reads no entry after
/// `to`.
///
/// The allowance covers the entries that the read reads to find the component revision at
/// `from`, when it has no cursor, and the entries that it reads for the page, also those that the
/// byte limit of the page leaves out. The read checks it between storage batches, so a batch can
/// take it past its limit, as the scan budget of a search can.
/// When the allowance runs out before the end of the range, the page reports `exhausted`; a page
/// that reaches the end of the range is complete, also when it spent the allowance. A cursor
/// continues the range where the page stopped; it must be within the range. The `last_index` of
/// the page is the last index of the oplog, not of the range.
#[allow(clippy::too_many_arguments)]
pub async fn get_public_oplog_range_page(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    cursor: Option<OplogCursor>,
    from: OplogIndex,
    to: OplogIndex,
    limits: PublicOplogPageLimits,
    allowance: OplogScanAllowance,
    claims: ClaimVisibility,
) -> Result<PublicOplogRangePage, WorkerExecutorError> {
    let mut scan = RangeScan::new(allowance);
    let from = from.max(OplogIndex::INITIAL);
    if to < from {
        return Err(WorkerExecutorError::invalid_request(format!(
            "the oplog range ends at {to}, before its start {from}"
        )));
    }
    if let Some(cursor) = &cursor
        && (cursor.next_oplog_index < from.as_u64() || cursor.next_oplog_index > to.as_u64())
    {
        return Err(WorkerExecutorError::invalid_request(format!(
            "the oplog cursor {cursor} is outside the range from {from} to {to}"
        )));
    }
    let (start, revision) = match cursor {
        Some(cursor) => {
            let range = OplogRange::new(Some(from), Some(to))
                .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
            let (rest, revision) = oplog_page_start(
                oplog_service.clone(),
                owned_agent_id,
                agent_mode,
                Some(cursor),
                range,
            )
            .await?;
            (rest.first_index_in(OplogIndex::INITIAL), Some(revision))
        }
        None => {
            let revision = scan_component_revision(
                oplog_service.clone(),
                owned_agent_id,
                agent_mode,
                from,
                &mut scan,
            )
            .await?;
            (from, revision)
        }
    };
    let last_index = oplog_service
        .get_last_index(owned_agent_id, agent_mode)
        .await?;
    let end = to.min(last_index);
    let empty = |next: Option<OplogCursor>, scan: OplogScan| PublicOplogRangePage {
        entries: Vec::new(),
        next,
        first_index_in_chunk: start,
        last_index,
        scan,
    };
    let Some(revision) = revision else {
        return Ok(empty(None, scan.report(true)));
    };
    if start > end {
        return Ok(empty(None, scan.report(false)));
    }
    let cursor_at = |next: OplogIndex, revision: ComponentRevision| OplogCursor {
        next_oplog_index: next.as_u64(),
        current_component_revision: revision.into(),
        query_hash: OplogCursor::query_hash(None),
        scan_to: None,
    };
    if scan.is_spent() {
        return Ok(empty(Some(cursor_at(start, revision)), scan.report(true)));
    }

    let count = (limits.count as u64)
        .min(end.as_u64() - start.as_u64() + 1)
        .min(scan.remaining_entries()) as usize;
    let (chunk, _) = read_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        owner_component_id,
        owner_kind,
        agent_type_name,
        revision,
        start,
        count,
        None,
        claims,
        &mut RemotePage::new(limits),
    )
    .await
    .map_err(WorkerExecutorError::unknown)?;
    // The read fetches `count` entries from storage, also when the byte limit of the page admits
    // fewer of them.
    scan.charge(count as u64);

    let complete = chunk.next_oplog_index > end;
    Ok(PublicOplogRangePage {
        entries: chunk.entries,
        next: (!complete)
            .then(|| cursor_at(chunk.next_oplog_index, chunk.current_component_revision)),
        first_index_in_chunk: chunk.first_index_in_chunk,
        last_index,
        scan: scan.report(!complete && scan.is_spent()),
    })
}

/// The cursor of an oplog page of `query` whose next unread entry is `next_oplog_index`. A page has
/// a cursor if and only if the oplog has entries after the page within its range. This is also true
/// when the page itself is empty, because a search spent its scan budget before any match.
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
        scan_to: None,
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
                agent_id: owned_agent_id.agent_id,
                next_oplog_index: OplogIndex::from_u64(cursor.next_oplog_index),
                reason: StaleCursorReason::QueryMismatch,
            })
        }
        _ => Ok(()),
    }
}

/// The range that a paged oplog read of `range` reads, and the component revision that decodes its
/// first entry. With the cursor of the previous page, it is the rest of `range` from the cursor.
/// Without a cursor, it is `range`, with the revision in effect at its first index. The caller
/// checks the query of the cursor first, with [`check_oplog_cursor_query`]. A cursor outside
/// `range` came from a read of another range, and the function refuses it as an invalid request.
///
/// A page hands out a cursor only while the oplog has entries after the page within its range. The
/// oplog only grows. So a cursor that points past the last entry did not come from a page of this
/// oplog. The agent was deleted and created again since, or the cursor was made up. The function
/// refuses such a cursor as stale. It does not read it as an empty page, because a caller would
/// take that page for the end of the oplog. The function does not check the revision of the cursor,
/// because that check would scan the oplog up to the cursor on every page.
pub async fn oplog_page_start(
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    cursor: Option<OplogCursor>,
    range: OplogRange,
) -> Result<(OplogRange, ComponentRevision), WorkerExecutorError> {
    match cursor {
        Some(cursor) => {
            let resumed = range
                .resume(&cursor)
                .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
            let last_index = oplog_service
                .get_last_index(owned_agent_id, agent_mode)
                .await?;
            let revision = oplog_cursor_revision(owned_agent_id, &cursor, last_index)?;
            Ok((resumed, revision))
        }
        None => {
            let revision = find_component_revision_at(
                oplog_service,
                owned_agent_id,
                agent_mode,
                range.first_index_in(OplogIndex::INITIAL),
            )
            .await?;
            Ok((range, revision))
        }
    }
}

/// The component revision that `cursor` carries, in an oplog whose last index is `last_index`. A
/// cursor that points past the last entry is stale, as [`oplog_page_start`] tells.
fn oplog_cursor_revision(
    owned_agent_id: &OwnedAgentId,
    cursor: &OplogCursor,
    last_index: OplogIndex,
) -> Result<ComponentRevision, WorkerExecutorError> {
    if cursor.next_oplog_index > last_index.as_u64() {
        return Err(WorkerExecutorError::StaleCursor {
            agent_id: owned_agent_id.agent_id,
            next_oplog_index: OplogIndex::from_u64(cursor.next_oplog_index),
            reason: StaleCursorReason::PastEnd {
                last_oplog_index: last_index,
            },
        });
    }
    cursor.current_component_revision.try_into().map_err(|e| {
        WorkerExecutorError::invalid_request(format!(
            "the oplog cursor {cursor} has an invalid component revision: {e}"
        ))
    })
}

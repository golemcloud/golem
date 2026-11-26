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
use crate::services::oplog::retention::{OplogRange, OplogRetention};
use golem_common::model::oplog::{OplogCursor, OplogScan, OplogScanAllowance};
use golem_service_base::error::worker_executor::StaleCursorReason;
use std::collections::BTreeMap;
use std::num::NonZeroU64;
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

/// Reads up to `count` oplog entries from `initial_oplog_index` in full. `retention` is the
/// retention of the oplog, which the caller reads once for each page.
pub async fn get_public_oplog_chunk(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
    claims: ClaimVisibility,
) -> Result<PublicOplogChunk, WorkerExecutorError> {
    let (chunk, _) = read_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        retention,
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
/// `limits.max_entry_bytes`, and ends early when the next entry does not fit in
/// `limits.max_bytes`. The entries hold claims only with `ClaimVisibility::Shown`, so only claims
/// that the reader asked for count against the limits.
pub async fn get_public_oplog_page(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    limits: PublicOplogPageLimits,
    claims: ClaimVisibility,
) -> Result<
    PublicOplogChunk<golem_api_grpc::proto::golem::worker::OplogPageEntry>,
    WorkerExecutorError,
> {
    let (chunk, _) = read_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        retention,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        limits.count,
        None,
        claims,
        &mut RemotePage::new(limits),
    )
    .await?;
    Ok(chunk)
}

/// Reads one oplog entry in full. `component_revision` is the revision to decode the entry with,
/// as an oplog page reports it for an omitted entry; without it, the reader scans the oplog up to
/// the entry to find the revision. The read serves the `Create` entry at index 1 also when it is
/// below the retained start; any other entry below the start is expired.
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
    let retention = oplog_service.retention(owned_agent_id, agent_mode).await?;
    retention.check_agent_read(&owned_agent_id.agent_id, oplog_index, 1)?;
    let component_revision = match component_revision {
        Some(component_revision) => component_revision,
        None => {
            find_component_revision_at(
                oplog_service.clone(),
                owned_agent_id,
                agent_mode,
                retention,
                oplog_index,
            )
            .await?
        }
    };
    let (chunk, _) = read_public_oplog_range(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        retention,
        owner_component_id,
        owner_kind,
        agent_type_name,
        component_revision,
        oplog_index,
        last_index,
        OplogRange::within(oplog_index, last_index, 1),
        None,
        claims,
        &mut InProcessChunk,
    )
    .await?;
    chunk.entries.into_iter().next().ok_or_else(|| {
        WorkerExecutorError::unknown(format!(
            "agent {owned_agent_id} has no oplog entry at index {oplog_index}"
        ))
    })
}

/// Reads up to `count` oplog entries from `initial_oplog_index` and adds the entries that match
/// `filter` to the chunk, in the form that `builder` selects. The chunk ends early at the first
/// matching entry that does not fit; the cursor of the chunk then points to that entry, and the
/// returned flag is `true`. `retention` gives the entries that the read reads: see
/// [`OplogRetention::page`].
#[allow(clippy::too_many_arguments)]
async fn read_public_oplog_chunk<B: ChunkBuilder>(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
    filter: Option<&Query>,
    claims: ClaimVisibility,
    builder: &mut B,
) -> Result<(PublicOplogChunk<B::Entry>, bool), WorkerExecutorError> {
    let initial_oplog_index = initial_oplog_index.max(OplogIndex::INITIAL);
    let last_index = oplog_service
        .get_last_index(owned_agent_id, agent_mode)
        .await?;
    let page = retention.page(
        &owned_agent_id.agent_id,
        initial_oplog_index,
        last_index,
        count as u64,
    )?;
    read_public_oplog_range(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        retention,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        last_index,
        page,
        filter,
        claims,
        builder,
    )
    .await
}

/// Reads the entries of `range`, which the caller checked against the retained start, and adds
/// the entries that match `filter` to the chunk, as [`read_public_oplog_chunk`] does. A `range`
/// of `None` gives an empty chunk at `initial_oplog_index`. `last_index` is the last index of the
/// oplog.
#[allow(clippy::too_many_arguments)]
async fn read_public_oplog_range<B: ChunkBuilder>(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    last_index: OplogIndex,
    range: Option<OplogRange>,
    filter: Option<&Query>,
    claims: ClaimVisibility,
    builder: &mut B,
) -> Result<(PublicOplogChunk<B::Entry>, bool), WorkerExecutorError> {
    let raw_entries = match range {
        Some(range) => {
            oplog_service
                .read_exact(owned_agent_id, agent_mode, range.start, range.count)
                .await?
        }
        None => BTreeMap::new(),
    };

    let mut entries = Vec::new();
    let mut current_component_revision = initial_component_revision;
    let mut next_oplog_index = initial_oplog_index;
    let mut first_index_in_chunk = None;
    let mut page_full = false;

    let mut attribution_resolver = PublicOplogAttributionResolver::new(
        oplog_service.clone(),
        owned_agent_id,
        agent_mode,
        retention,
    );
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
        .await
        .map_err(WorkerExecutorError::unknown)?;
        if filter.is_none_or(|query| entry.matches(query)) {
            let entry = PublicOplogEntryWithIndex {
                oplog_index: index,
                attribution,
                entry,
            };
            match builder
                .admit(entry, entry_component_revision)
                .map_err(WorkerExecutorError::unknown)?
            {
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
    retention: OplogRetention,
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
) -> Result<PublicOplogSearchResult, WorkerExecutorError> {
    search_public_oplog_with(
        component_service,
        oplog_service,
        owned_agent_id,
        agent_mode,
        retention,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        last_oplog_index,
        count,
        query,
        budget,
        claims,
        InProcessChunk,
    )
    .await
}

/// Searches the oplog for a remote reader. The result is an oplog page with the limits of
/// `get_public_oplog_page`; only the matching entries count against the limits. The search
/// stops early when `budget` is spent.
pub async fn search_public_oplog_page(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    limits: PublicOplogPageLimits,
    query: &str,
    budget: ScanBudget,
    claims: ClaimVisibility,
) -> Result<
    PublicOplogSearchResult<golem_api_grpc::proto::golem::worker::OplogPageEntry>,
    WorkerExecutorError,
> {
    search_public_oplog_with(
        component_service,
        oplog_service,
        owned_agent_id,
        agent_mode,
        retention,
        owner_component_id,
        owner_kind,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        None,
        limits.count,
        query,
        budget,
        claims,
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
    retention: OplogRetention,
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
    mut builder: B,
) -> Result<PublicOplogSearchResult<B::Entry>, WorkerExecutorError> {
    let started = Instant::now();
    let mut scanned = 0u64;
    let mut results = Vec::new();
    let mut last_index;
    let mut current_index = initial_oplog_index;
    let mut current_component_revision = initial_component_revision;

    let query = Query::parse(query).map_err(WorkerExecutorError::unknown)?;

    loop {
        let to_last = last_oplog_index.map_or(u64::MAX, |last| {
            (last.as_u64() + 1).saturating_sub(current_index.as_u64())
        });
        let chunk_size = (count.saturating_sub(results.len()) as u64)
            .min(budget.remaining_entries(scanned))
            .min(to_last) as usize;
        let (chunk, page_full) = read_public_oplog_chunk(
            component_service.clone(),
            oplog_service.clone(),
            owned_agent_id,
            agent_mode,
            retention,
            owner_component_id,
            owner_kind,
            agent_type_name,
            current_component_revision,
            current_index,
            chunk_size,
            Some(&query),
            claims,
            &mut builder,
        )
        .await?;

        results.extend(chunk.entries);

        scanned += chunk.next_oplog_index.as_u64() - current_index.as_u64();
        last_index = last_oplog_index.map_or(chunk.last_index, |last| last.min(chunk.last_index));
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

/// The component revision in effect at `start`: the revision of the last entry before it that
/// names one, or the revision that the agent started with. The scan reads the `Create` entry and
/// the retained entries, so it does not see a revision that an entry below the retained start
/// names. `retention` is the retention of the oplog, which the caller reads once.
pub async fn find_component_revision_at(
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    start: OplogIndex,
) -> Result<ComponentRevision, WorkerExecutorError> {
    let revision = scan_component_revision(
        oplog_service,
        owned_agent_id,
        agent_mode,
        retention,
        start,
        &mut RangeScan::unlimited(),
    )
    .await?;
    Ok(revision.expect("an unlimited scan finds the component revision"))
}

/// Scans the `Create` entry and the retained entries before `start` for the component revision in
/// effect at `start`, as [`find_component_revision_at`] gives it, and charges the entries that it
/// reads to `scan`. Returns `None` when `scan` is spent before the scan reaches `start`. The scan
/// checks the allowance between batches.
async fn scan_component_revision(
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    start: OplogIndex,
    scan: &mut RangeScan,
) -> Result<Option<ComponentRevision>, WorkerExecutorError> {
    let mut initial_component_revision = ComponentRevision::INITIAL;
    let last_oplog_index = oplog_service
        .get_last_index(owned_agent_id, agent_mode)
        .await?;
    // No entry is before `start`, or the oplog is empty.
    if start <= OplogIndex::INITIAL || !last_oplog_index.is_defined() {
        return Ok(Some(initial_component_revision));
    }
    let last = start.previous().min(last_oplog_index);
    // The scan reads the retained entries before `start`. When the `Create` entry is below the
    // retained start, the scan reads it alone first: it gives the revision that the agent started
    // with.
    if let Some(create) = retention.create_read() {
        if scan.is_spent() {
            return Ok(None);
        }
        initial_component_revision = oplog_service
            .read_exact(owned_agent_id, agent_mode, create.start, create.count)
            .await?
            .values()
            .find_map(OplogEntry::specifies_component_revision)
            .unwrap_or(initial_component_revision);
        scan.charge(create.count);
    }
    const BATCH_SIZE: u64 = 256;
    let mut current = OplogIndex::INITIAL;
    while let Some(chunk) = retention.chunk(current, last, BATCH_SIZE) {
        if scan.is_spent() {
            return Ok(None);
        }
        let count = chunk.count.min(scan.remaining_entries());
        let entries = oplog_service
            .read_exact(owned_agent_id, agent_mode, chunk.start, count)
            .await?;
        for entry in entries.into_values() {
            if let Some(revision) = entry.specifies_component_revision() {
                initial_component_revision = revision;
            }
        }
        scan.charge(count);
        current = OplogIndex::from_u64(chunk.start.as_u64() + count);
    }

    Ok(Some(initial_component_revision))
}

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
        Self::new(OplogScanAllowance {
            entries: u64::MAX,
            millis: u64::MAX,
        })
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

/// The allowance of a range read: `requested`, or `configured` without one, clamped to
/// `configured`. An executor never grants more than its own configured allowance, so a reader
/// that passes on what is left of its allowance gets no fresh allowance from a retry or another
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
        None => OplogScanAllowance {
            entries: configured.entries.get(),
            millis: configured_millis,
        },
    }
}

/// Checks the request of a range page from `from` to `to` that `cursor` continues, and gives the
/// start of the range: `from`, or index 1 when `from` is 0, or the retained start without `from`.
/// A `to` or a cursor below the retained start is expired, before the checks of the range, because a range without `from` starts at the
/// retained start. Then a range that ends before its start, or a cursor outside the range, is an
/// invalid request.
fn checked_range_start(
    retention: OplogRetention,
    agent_id: &golem_common::model::AgentId,
    from: Option<OplogIndex>,
    to: OplogIndex,
    cursor: Option<&OplogCursor>,
) -> Result<OplogIndex, WorkerExecutorError> {
    let from = from.map_or(retention.start(), |from| from.max(OplogIndex::INITIAL));
    retention.check_agent_from(agent_id, to.max(OplogIndex::INITIAL))?;
    if let Some(cursor) = cursor {
        retention.check_agent_from(
            agent_id,
            OplogIndex::from_u64(cursor.next_oplog_index).max(OplogIndex::INITIAL),
        )?;
    }
    if to < from {
        return Err(WorkerExecutorError::invalid_request(format!(
            "the oplog range ends at {to}, before its start {from}"
        )));
    }
    if let Some(cursor) = cursor
        && (cursor.next_oplog_index < from.as_u64() || cursor.next_oplog_index > to.as_u64())
    {
        return Err(WorkerExecutorError::invalid_request(format!(
            "the oplog cursor {cursor} is outside the range from {from} to {to}"
        )));
    }
    Ok(from)
}

/// Reads one oplog page of the range from `from` to `to`, inclusive, for a remote reader, within
/// `allowance`. Without `from`, the range starts at the retained start. The page has the limits of [`get_public_oplog_page`], and reads no entry after
/// `to`.
///
/// The allowance covers the entries that the read reads to find the component revision at
/// `from`, when it has no cursor, and the entries that it reads for the page, also those that the
/// byte limit of the page leaves out. The read checks it between storage batches, so a batch can
/// take it past its limit, as the scan budget of a search can.
/// When the allowance runs out before the end of the range, the page reports `exhausted`; a page
/// that reaches the end of the range is complete, also when it spent the allowance. A cursor
/// continues the range where the page stopped; it must be within the range.
///
/// `retention` is the retention of the oplog, which the caller reads once. A range read is a
/// page: a `to` below the retained start is expired, also at index 1, before the read charges any
/// entry to the allowance. Without a cursor, a `from` below the start is expired. A cursor takes
/// the place of `from` as the start of the read, so with a cursor a cursor below the start is
/// expired.
#[allow(clippy::too_many_arguments)]
pub async fn get_public_oplog_range_page(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    cursor: Option<OplogCursor>,
    from: Option<OplogIndex>,
    to: OplogIndex,
    limits: PublicOplogPageLimits,
    allowance: OplogScanAllowance,
    claims: ClaimVisibility,
) -> Result<PublicOplogRangePage, WorkerExecutorError> {
    let mut scan = RangeScan::new(allowance);
    let from = checked_range_start(
        retention,
        &owned_agent_id.agent_id,
        from,
        to,
        cursor.as_ref(),
    )?;
    let (start, revision) = match cursor {
        Some(cursor) => {
            let (start, revision) = oplog_page_start(
                oplog_service.clone(),
                owned_agent_id,
                agent_mode,
                retention,
                Some(cursor),
                Some(from),
            )
            .await?;
            (start, Some(revision))
        }
        None => {
            retention.check_agent_from(&owned_agent_id.agent_id, from)?;
            let revision = scan_component_revision(
                oplog_service.clone(),
                owned_agent_id,
                agent_mode,
                retention,
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
    };
    if scan.is_spent() {
        return Ok(empty(Some(cursor_at(start, revision)), scan.report(true)));
    }

    let count = retention
        .page(
            &owned_agent_id.agent_id,
            start,
            end,
            (limits.count as u64).min(scan.remaining_entries()),
        )?
        .map_or(0, |range| range.count) as usize;
    let (chunk, _) = read_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        retention,
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
    .await?;
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

/// Where a guest search reads next, and the component revision to decode that entry with. A
/// search that read a page before continues at `next_oplog_index` with `continued`, the revision
/// that the page gave. A new search starts at `next_oplog_index` moved to the retained start, with
/// the revision that [`find_component_revision_at`] gives there.
pub async fn guest_search_start(
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    next_oplog_index: OplogIndex,
    continued: Option<ComponentRevision>,
) -> Result<(OplogIndex, ComponentRevision), WorkerExecutorError> {
    match continued {
        Some(revision) => Ok((next_oplog_index, revision)),
        None => {
            let start = retention.clamp(next_oplog_index);
            let revision = find_component_revision_at(
                oplog_service,
                owned_agent_id,
                agent_mode,
                retention,
                start,
            )
            .await?;
            Ok((start, revision))
        }
    }
}

/// What a guest search batch does after [`guest_search_start`] gave its `start`.
#[derive(Clone, Copy, Debug)]
pub enum GuestSearchBatch {
    /// The start is after the window that the grant of the batch covers. The batch moves the
    /// cursor to the start and reads no entry, so the next batch checks the grant for its own
    /// window.
    SkipTo(OplogIndex),
    /// The batch searches from `start` and scans at most `budget.entries` entries.
    Read {
        start: OplogIndex,
        budget: ScanBudget,
    },
}

/// The guest search batch from `start`, the start that [`guest_search_start`] gives. The grant of
/// the batch covers `page_size` indexes from `window_start`, the cursor of the search, not
/// `page_size` matches. A new search starts at the retained start, which can be inside the window
/// or after it. The batch reads only the indexes of the window at or after `start`: its budget is
/// `configured` with the entries cut to that part of the window.
pub fn guest_search_batch(
    window_start: OplogIndex,
    page_size: NonZeroU64,
    start: OplogIndex,
    configured: ScanBudget,
) -> GuestSearchBatch {
    let window_last =
        OplogIndex::from_u64(window_start.as_u64().saturating_add(page_size.get() - 1));
    match OplogRange::within(start, window_last, page_size.get()) {
        Some(window) => GuestSearchBatch::Read {
            start: window.start,
            budget: ScanBudget {
                entries: configured
                    .entries
                    .min(NonZeroU64::new(window.count).expect("a range holds an entry")),
                ..configured
            },
        },
        None => GuestSearchBatch::SkipTo(start),
    }
}

/// The next chunk of a guest `get-oplog` resource whose cursor is `next_oplog_index`. The read
/// reads the retention of the oplog. `continued` is the component revision of a resource that
/// already read a chunk; a new resource finds the revision at its cursor. The chunk reads from
/// the cursor, so a chunk below the retained start is expired. The error is the text that the
/// guest gets in `internal-error`.
#[allow(clippy::too_many_arguments)]
pub async fn get_guest_oplog_chunk(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    next_oplog_index: OplogIndex,
    continued: Option<ComponentRevision>,
    count: usize,
    claims: ClaimVisibility,
) -> Result<PublicOplogChunk, String> {
    let retention = oplog_service
        .retention(owned_agent_id, agent_mode)
        .await
        .map_err(|error| error.to_string())?;
    let revision = match continued {
        Some(revision) => revision,
        None => find_component_revision_at(
            oplog_service.clone(),
            owned_agent_id,
            agent_mode,
            retention,
            next_oplog_index,
        )
        .await
        .map_err(|error| error.to_string())?,
    };
    get_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        retention,
        owner_component_id,
        owner_kind,
        agent_type_name,
        revision,
        next_oplog_index,
        count,
        claims,
    )
    .await
    .map_err(guest_oplog_read_error)
}

/// The next batch of a guest `search-oplog` resource whose cursor is `next_oplog_index`. The
/// batch reads the retention of the oplog. [`guest_search_start`] gives the start of the batch
/// and the component revision to decode its first entry with, and [`guest_search_batch`] cuts
/// the batch to the window of the grant from the cursor. `continued` is the revision of a search
/// that already ran a batch. A batch whose start is after its window reads no entry and moves the
/// cursor to the start. The error is the text that the guest gets in `internal-error`.
#[allow(clippy::too_many_arguments)]
pub async fn search_guest_oplog(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    owner_component_id: ComponentId,
    owner_kind: OwnerKind,
    agent_type_name: Option<&AgentTypeName>,
    next_oplog_index: OplogIndex,
    last_oplog_index: Option<OplogIndex>,
    page_size: usize,
    continued: Option<ComponentRevision>,
    query: &str,
    configured: ScanBudget,
    claims: ClaimVisibility,
) -> Result<PublicOplogSearchResult, String> {
    let retention = oplog_service
        .retention(owned_agent_id, agent_mode)
        .await
        .map_err(|error| error.to_string())?;
    let (start, revision) = guest_search_start(
        oplog_service.clone(),
        owned_agent_id,
        agent_mode,
        retention,
        next_oplog_index,
        continued,
    )
    .await
    .map_err(|error| error.to_string())?;
    // The scanner caps each storage read at the remaining entry budget, even when a chunk has no
    // matches, so the budget of the batch keeps the scan inside the window of its grant.
    let window = NonZeroU64::new(page_size as u64).expect("search page size is nonzero");
    match guest_search_batch(next_oplog_index, window, start, configured) {
        GuestSearchBatch::SkipTo(start) => {
            let last_index = match last_oplog_index {
                Some(last_index) => last_index,
                None => oplog_service
                    .get_last_index(owned_agent_id, agent_mode)
                    .await
                    .map_err(|error| error.to_string())?,
            };
            Ok(PublicOplogSearchResult {
                entries: Vec::new(),
                next_oplog_index: start,
                current_component_revision: revision,
                last_index,
            })
        }
        GuestSearchBatch::Read { start, budget } => search_public_oplog(
            components,
            oplog_service,
            owned_agent_id,
            agent_mode,
            retention,
            owner_component_id,
            owner_kind,
            agent_type_name,
            revision,
            start,
            last_oplog_index,
            page_size,
            query,
            budget,
            claims,
        )
        .await
        .map_err(guest_oplog_read_error),
    }
}

/// The text of a failed guest oplog read. The guest gets it in `internal-error`, because the
/// guest oplog API has no case for an expired read. An entry or a query without a public form
/// gives its own text, and every other error gives its display text, so an expired read names
/// the expired cause and the retained start.
pub fn guest_oplog_read_error(error: WorkerExecutorError) -> String {
    match error {
        WorkerExecutorError::Unknown { details } => details,
        other => other.to_string(),
    }
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
                agent_id: owned_agent_id.agent_id,
                next_oplog_index: OplogIndex::from_u64(cursor.next_oplog_index),
                reason: StaleCursorReason::QueryMismatch,
            })
        }
        _ => Ok(()),
    }
}

/// Where a paged oplog read starts, and the component revision to decode its first entry with:
/// the cursor of the previous page, or else `from` with the revision that
/// [`find_component_revision_at`] gives there. Without a
/// cursor and `from`, the read starts at the retained start of the oplog. A start below the
/// retained start fails with the expired error that names the start. The caller checks the
/// query of the cursor first, with [`check_oplog_cursor_query`].
///
/// A page hands out a cursor only while the oplog has entries after it, and the oplog only grows,
/// so a cursor that points past the last entry did not come from a page of this oplog: the agent
/// was deleted and created again since, or the cursor was made up. It is refused as stale, instead
/// of read as an empty page, which a caller would take for the end of the oplog. The revision of
/// the cursor is not checked, because that would scan the oplog up to the cursor on every page.
/// A cursor at index 0 starts at index 1, as `from` does.
pub async fn oplog_page_start(
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    retention: OplogRetention,
    cursor: Option<OplogCursor>,
    from: Option<OplogIndex>,
) -> Result<(OplogIndex, ComponentRevision), WorkerExecutorError> {
    let start = cursor.as_ref().map_or_else(
        || from.map_or(retention.start(), |from| from.max(OplogIndex::INITIAL)),
        |cursor| OplogIndex::from_u64(cursor.next_oplog_index).max(OplogIndex::INITIAL),
    );
    retention.check_agent_from(&owned_agent_id.agent_id, start)?;
    match cursor {
        Some(cursor) => {
            let last_index = oplog_service
                .get_last_index(owned_agent_id, agent_mode)
                .await?;
            if cursor.next_oplog_index > last_index.as_u64() {
                return Err(WorkerExecutorError::StaleCursor {
                    agent_id: owned_agent_id.agent_id,
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
            Ok((start, revision))
        }
        None => {
            let revision = find_component_revision_at(
                oplog_service,
                owned_agent_id,
                agent_mode,
                retention,
                start,
            )
            .await?;
            Ok((start, revision))
        }
    }
}

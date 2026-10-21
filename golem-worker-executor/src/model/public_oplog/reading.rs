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
/// `limits.max_bytes`.
pub async fn get_public_oplog_page(
    components: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    limits: PublicOplogPageLimits,
) -> Result<PublicOplogChunk<golem_api_grpc::proto::golem::worker::OplogPageEntry>, String> {
    let (chunk, _) = read_public_oplog_chunk(
        components,
        oplog_service,
        owned_agent_id,
        agent_mode,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        limits.count,
        None,
        &mut RemotePage::new(limits),
    )
    .await?;
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

pub async fn search_public_oplog(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
    query: &str,
) -> Result<PublicOplogSearchResult, String> {
    search_public_oplog_with(
        component_service,
        oplog_service,
        owned_agent_id,
        agent_mode,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        count,
        query,
        InProcessChunk,
    )
    .await
}

/// Searches the oplog for a remote reader. The result is an oplog page with the limits of
/// `get_public_oplog_page`; only the matching entries count against the limits.
pub async fn search_public_oplog_page(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    limits: PublicOplogPageLimits,
    query: &str,
) -> Result<PublicOplogSearchResult<golem_api_grpc::proto::golem::worker::OplogPageEntry>, String> {
    search_public_oplog_with(
        component_service,
        oplog_service,
        owned_agent_id,
        agent_mode,
        agent_type_name,
        initial_component_revision,
        initial_oplog_index,
        limits.count,
        query,
        RemotePage::new(limits),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn search_public_oplog_with<B: ChunkBuilder>(
    component_service: Arc<dyn ComponentService>,
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    agent_type_name: Option<&AgentTypeName>,
    initial_component_revision: ComponentRevision,
    initial_oplog_index: OplogIndex,
    count: usize,
    query: &str,
    mut builder: B,
) -> Result<PublicOplogSearchResult<B::Entry>, String> {
    let mut results = Vec::new();
    let mut last_index;
    let mut current_index = initial_oplog_index;
    let mut current_component_revision = initial_component_revision;

    let query = Query::parse(query)?;

    loop {
        let (chunk, page_full) = read_public_oplog_chunk(
            component_service.clone(),
            oplog_service.clone(),
            owned_agent_id,
            agent_mode,
            agent_type_name,
            current_component_revision,
            current_index,
            count,
            Some(&query),
            &mut builder,
        )
        .await?;

        results.extend(chunk.entries);

        last_index = chunk.last_index;
        current_index = chunk.next_oplog_index;
        current_component_revision = chunk.current_component_revision;

        if page_full || current_index > last_index || results.len() >= count {
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

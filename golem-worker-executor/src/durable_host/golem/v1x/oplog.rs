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

use super::{AGENT_OPERATION_PERMISSION_DENIED, agent_operation_denial, resolve_agent_owner};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::authorization::targets::oplog_target;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::ResolvedCall;
use crate::model::public_oplog::PublicOplogEntryOps;
use crate::model::public_oplog::find_component_revision_at;
use crate::model::public_oplog::get_public_oplog_chunk;
use crate::model::public_oplog::search_public_oplog;
use crate::preview2::golem_api_1_x;
use crate::preview2::golem_api_1_x::oplog::Host as OplogHost;
use crate::preview2::golem_api_1_x::oplog::HostGetOplog;
use crate::preview2::golem_api_1_x::oplog::HostSearchOplog;
use crate::preview2::golem_api_1_x::oplog::OplogReadError;
use crate::preview2::golem_api_1_x::oplog::SearchOplog;
use crate::services::HasOplogService;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::model::AgentId;
use golem_common::model::OplogIndex;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostRequestGolemApiOplogEnrich;
use golem_common::model::oplog::HostRequestGolemApiOplogRead;
use golem_common::model::oplog::HostResponseGolemApiOplogChunk;
use golem_common::model::oplog::HostResponseGolemApiOplogEntries;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::oplog::PublicOplogEntry;
use golem_common::model::oplog::host_functions::GolemApiEnrichOplogEntries;
use golem_common::model::oplog::host_functions::GolemApiGetOplogNext;
use golem_common::model::oplog::host_functions::GolemApiSearchOplogNext;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

async fn oplog_read_denied<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    owned_agent_id: &OwnedAgentId,
    start: OplogIndex,
    page_size: usize,
) -> Result<bool, WorkerExecutorError> {
    let (owner, environment_id) = resolve_agent_owner(
        ctx,
        &owned_agent_id.component_id(),
        Some(&owned_agent_id.agent_id.agent_id),
    )
    .await?;
    if owned_agent_id.environment_id != environment_id {
        return Ok(true);
    }
    let Some(end) = (page_size as u64)
        .checked_sub(1)
        .and_then(|offset| start.as_u64().checked_add(offset))
        .map(OplogIndex::from_u64)
    else {
        return Ok(true);
    };
    let target = match oplog_target(owner, Some(start.into()), Some(end.into())) {
        Ok(target) => target,
        Err(_) => return Ok(true),
    };
    Ok(ctx.authorize_live_permission(&target).await?.is_err())
}

async fn oplog_entries_denied<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    owned_agent_id: &OwnedAgentId,
    indexes: &[OplogIndex],
) -> Result<bool, WorkerExecutorError> {
    let (owner, environment_id) = resolve_agent_owner(
        ctx,
        &owned_agent_id.component_id(),
        Some(&owned_agent_id.agent_id.agent_id),
    )
    .await?;
    if owned_agent_id.environment_id != environment_id {
        return Ok(true);
    }
    let targets = indexes
        .iter()
        .map(|index| oplog_target(owner.clone(), Some((*index).into()), Some((*index).into())))
        .collect::<Result<Vec<_>, _>>();
    let Ok(targets) = targets else {
        return Ok(true);
    };
    Ok(ctx.authorize_live_permissions(&targets).await?.is_err())
}

/// A denied oplog read does not advance the resource: its response carries the position of the
/// recorded request, which does not hold the resource's component revision before the first read.
fn is_denied_oplog_read(response: &HostResponseGolemApiOplogChunk) -> bool {
    response
        .result
        .as_ref()
        .is_err_and(|error| error == AGENT_OPERATION_PERMISSION_DENIED)
}

type SerializedOplogEntries = Vec<(u64, Vec<u8>)>;
type OplogEntryConversion = Result<Vec<(u64, OplogEntry)>, String>;

fn prepare_oplog_enrichment_entries<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entries: Vec<(u64, golem_api_1_x::oplog::OplogEntry)>,
) -> (SerializedOplogEntries, OplogEntryConversion) {
    let indexes = entries.iter().map(|(index, _)| *index).collect::<Vec<_>>();
    let mut entries =
        crate::model::public_oplog::wit::reject_quota_handles_in_oplog_entries(entries, ctx)
            .and_then(|entries| {
                entries
                    .into_iter()
                    .map(|(index, entry)| OplogEntry::try_from(entry).map(|entry| (index, entry)))
                    .collect::<Result<Vec<_>, _>>()
            });
    let request_entries = match &entries {
        Ok(entries) => entries
            .iter()
            .map(|(index, entry)| {
                golem_common::serialization::serialize(entry).map(|entry| (*index, entry))
            })
            .collect::<Result<Vec<_>, _>>(),
        Err(error) => Err(error.clone()),
    };
    match request_entries {
        Ok(request_entries) => (request_entries, entries),
        Err(error) => {
            entries = Err(error);
            (
                indexes
                    .into_iter()
                    .map(|index| (index, Vec::new()))
                    .collect(),
                entries,
            )
        }
    }
}

async fn get_oplog_chunk<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    entry: &GetOplogEntry,
) -> Result<crate::model::public_oplog::PublicOplogChunk, String> {
    let identity = ctx
        .state
        .worker_service
        .resolve_agent_identity(&entry.owned_agent_id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("agent {} does not exist", entry.owned_agent_id))?;
    let agent_mode = identity.agent_mode;
    let current_component_revision = if entry.initialized {
        entry.current_component_revision
    } else {
        find_component_revision_at(
            ctx.state.oplog_service(),
            &entry.owned_agent_id,
            agent_mode,
            entry.next_oplog_index,
        )
        .await
        .map_err(|error| error.to_string())?
    };
    let agent_type =
        ParsedAgentId::parse_agent_type_name(&entry.owned_agent_id.agent_id.agent_id).ok();
    get_public_oplog_chunk(
        ctx.state.component_service.clone(),
        ctx.state.oplog_service(),
        &entry.owned_agent_id,
        agent_mode,
        agent_type.as_ref(),
        current_component_revision,
        entry.next_oplog_index,
        entry.page_size,
    )
    .await
}

async fn get_search_oplog_chunk<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    entry: &SearchOplogEntry,
) -> Result<crate::model::public_oplog::PublicOplogSearchResult, String> {
    let identity = ctx
        .state
        .worker_service
        .resolve_agent_identity(&entry.owned_agent_id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("agent {} does not exist", entry.owned_agent_id))?;
    let agent_mode = identity.agent_mode;
    let current_component_revision = if entry.initialized {
        entry.current_component_revision
    } else {
        find_component_revision_at(
            ctx.state.oplog_service(),
            &entry.owned_agent_id,
            agent_mode,
            entry.next_oplog_index,
        )
        .await
        .map_err(|error| error.to_string())?
    };
    let agent_type =
        ParsedAgentId::parse_agent_type_name(&entry.owned_agent_id.agent_id.agent_id).ok();
    search_public_oplog(
        ctx.state.component_service.clone(),
        ctx.state.oplog_service(),
        &entry.owned_agent_id,
        agent_mode,
        agent_type.as_ref(),
        current_component_revision,
        entry.next_oplog_index,
        entry.page_size,
        &entry.query,
    )
    .await
}

impl<Ctx: WorkerCtx> HostGetOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        agent_id: golem_api_1_x::oplog::AgentId,
        start: golem_api_1_x::oplog::OplogIndex,
    ) -> anyhow::Result<Resource<GetOplogEntry>> {
        self.observe_function_call("golem::api::get-oplog", "new");

        let agent_id: AgentId = agent_id.into();
        let owned_agent_id = OwnedAgentId::new(self.owned_agent_id.environment_id(), &agent_id);

        let start = OplogIndex::from_u64(start);
        let entry = GetOplogEntry::new(
            owned_agent_id,
            start,
            self.component_metadata().revision,
            100,
        );
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetOplogEntry>,
    ) -> anyhow::Result<Result<Option<Vec<golem_api_1_x::oplog::PublicOplogEntry>>, OplogReadError>>
    {
        self.observe_function_call("golem::api::get-oplog", "get-next");

        let begun = DurableCallSession::<GolemApiGetOplogNext, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let response = 'response: {
            let (handle, entry) = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let entry = self.as_wasi_view().table().get(&self_)?.clone();
                    let denied = oplog_read_denied(
                        self,
                        &entry.owned_agent_id,
                        entry.next_oplog_index,
                        entry.page_size,
                    )
                    .await?;
                    let request = HostRequestGolemApiOplogRead {
                        agent_id: entry.owned_agent_id.agent_id(),
                        next_oplog_index: entry.next_oplog_index,
                        query: None,
                        page_size: entry.page_size,
                        current_component_revision: entry
                            .initialized
                            .then_some(entry.current_component_revision),
                    };
                    if let Some(denial) = agent_operation_denial(denied) {
                        break 'response begun.start_live_denied(self, request, denial).await?;
                    }
                    (begun.start_live(self, request).await?, entry)
                }
                ResolvedCall::Replay(mut handle) => {
                    match handle.replay(self).await? {
                        CallReplayOutcome::Replayed(response) => break 'response response,
                        CallReplayOutcome::Incomplete(live) => {
                            handle = live;
                        }
                    }
                    let entry = self.as_wasi_view().table().get(&self_)?.clone();
                    (handle, entry)
                }
            };

            let result = get_oplog_chunk(self, &entry).await;
            let response = match result {
                Ok(chunk) if chunk.next_oplog_index != entry.next_oplog_index => {
                    let entries = chunk
                        .entries
                        .into_iter()
                        .map(|entry| entry.entry)
                        .collect::<Vec<_>>();
                    HostResponseGolemApiOplogChunk {
                        result: serde_json::to_vec(&entries)
                            .map(Some)
                            .map_err(|error| error.to_string()),
                        next_oplog_index: chunk.next_oplog_index,
                        current_component_revision: chunk.current_component_revision,
                    }
                }
                Ok(chunk) => HostResponseGolemApiOplogChunk {
                    result: Ok(None),
                    next_oplog_index: chunk.next_oplog_index,
                    current_component_revision: chunk.current_component_revision,
                },
                Err(error) => HostResponseGolemApiOplogChunk {
                    result: Err(error),
                    next_oplog_index: entry.next_oplog_index,
                    current_component_revision: entry.current_component_revision,
                },
            };
            handle.complete(self, response).await?
        };

        let denied = is_denied_oplog_read(&response);
        self.as_wasi_view().table().get_mut(&self_).map(|entry| {
            if denied {
                entry.update(entry.next_oplog_index, entry.current_component_revision);
            } else {
                entry.update(
                    response.next_oplog_index,
                    response.current_component_revision,
                );
            }
        })?;
        match response.result {
            Ok(Some(entries)) => {
                let entries: Vec<PublicOplogEntry> = serde_json::from_slice(&entries)
                    .map_err(|error| anyhow!("invalid persisted public oplog chunk: {error}"))?;
                let entries = entries
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<Vec<_>, String>>()
                    .map_err(|error| {
                        anyhow!("cannot project oplog entry to golem:api/oplog@1.5.0: {error}")
                    })?;
                Ok(Ok(Some(entries)))
            }
            Ok(None) => Ok(Ok(None)),
            Err(error) if error == AGENT_OPERATION_PERMISSION_DENIED => {
                Ok(Err(OplogReadError::PermissionDenied))
            }
            Err(error) => Ok(Err(OplogReadError::InternalError(error))),
        }
    }

    async fn drop(&mut self, rep: Resource<GetOplogEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::get-oplog", "drop");
        self.as_wasi_view().table().delete(rep)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct GetOplogEntry {
    pub owned_agent_id: OwnedAgentId,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub page_size: usize,
    pub initialized: bool,
}

impl GetOplogEntry {
    pub fn new(
        owned_agent_id: OwnedAgentId,
        initial_oplog_index: OplogIndex,
        initial_component_revision: ComponentRevision,
        page_size: usize,
    ) -> Self {
        Self {
            owned_agent_id,
            next_oplog_index: initial_oplog_index,
            current_component_revision: initial_component_revision,
            page_size,
            initialized: false,
        }
    }

    pub fn update(
        &mut self,
        next_oplog_index: OplogIndex,
        current_component_revision: ComponentRevision,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_revision = current_component_revision;
        self.initialized = true;
    }
}

impl<Ctx: WorkerCtx> HostSearchOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        agent_id: golem_api_1_x::oplog::AgentId,
        text: String,
    ) -> anyhow::Result<Resource<SearchOplog>> {
        self.observe_function_call("golem::api::search-oplog", "new");

        let agent_id: AgentId = agent_id.into();
        let owned_agent_id = OwnedAgentId::new(self.owned_agent_id.environment_id(), &agent_id);

        let start = OplogIndex::INITIAL;
        let entry = SearchOplogEntry::new(
            owned_agent_id,
            start,
            self.component_metadata().revision,
            100,
            text,
        );
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<SearchOplog>,
    ) -> anyhow::Result<
        Result<
            Option<
                Vec<(
                    golem_api_1_x::oplog::OplogIndex,
                    golem_api_1_x::oplog::PublicOplogEntry,
                )>,
            >,
            OplogReadError,
        >,
    > {
        self.observe_function_call("golem::api::search-oplog", "get-next");

        let begun = DurableCallSession::<GolemApiSearchOplogNext, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let response = 'response: {
            let (handle, entry) = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let entry = self.as_wasi_view().table().get(&self_)?.clone();
                    let denied = oplog_read_denied(
                        self,
                        &entry.owned_agent_id,
                        entry.next_oplog_index,
                        entry.page_size,
                    )
                    .await?;
                    let request = HostRequestGolemApiOplogRead {
                        agent_id: entry.owned_agent_id.agent_id(),
                        next_oplog_index: entry.next_oplog_index,
                        query: Some(entry.query.clone()),
                        page_size: entry.page_size,
                        current_component_revision: entry
                            .initialized
                            .then_some(entry.current_component_revision),
                    };
                    if let Some(denial) = agent_operation_denial(denied) {
                        break 'response begun.start_live_denied(self, request, denial).await?;
                    }
                    (begun.start_live(self, request).await?, entry)
                }
                ResolvedCall::Replay(mut handle) => {
                    match handle.replay(self).await? {
                        CallReplayOutcome::Replayed(response) => break 'response response,
                        CallReplayOutcome::Incomplete(live) => {
                            handle = live;
                        }
                    }
                    let entry = self.as_wasi_view().table().get(&self_)?.clone();
                    (handle, entry)
                }
            };

            let result = get_search_oplog_chunk(self, &entry).await;
            let response = match result {
                Ok(chunk) if chunk.next_oplog_index != entry.next_oplog_index => {
                    let entries = chunk
                        .entries
                        .into_iter()
                        .map(|entry| (entry.oplog_index, entry.entry))
                        .collect::<Vec<_>>();
                    HostResponseGolemApiOplogChunk {
                        result: serde_json::to_vec(&entries)
                            .map(Some)
                            .map_err(|error| error.to_string()),
                        next_oplog_index: chunk.next_oplog_index,
                        current_component_revision: chunk.current_component_revision,
                    }
                }
                Ok(chunk) => HostResponseGolemApiOplogChunk {
                    result: Ok(None),
                    next_oplog_index: chunk.next_oplog_index,
                    current_component_revision: chunk.current_component_revision,
                },
                Err(error) => HostResponseGolemApiOplogChunk {
                    result: Err(error),
                    next_oplog_index: entry.next_oplog_index,
                    current_component_revision: entry.current_component_revision,
                },
            };
            handle.complete(self, response).await?
        };

        let denied = is_denied_oplog_read(&response);
        self.as_wasi_view().table().get_mut(&self_).map(|entry| {
            if denied {
                entry.update(entry.next_oplog_index, entry.current_component_revision);
            } else {
                entry.update(
                    response.next_oplog_index,
                    response.current_component_revision,
                );
            }
        })?;
        match response.result {
            Ok(Some(entries)) => {
                let entries: Vec<(OplogIndex, PublicOplogEntry)> = serde_json::from_slice(&entries)
                    .map_err(|error| {
                        anyhow!("invalid persisted public oplog search chunk: {error}")
                    })?;
                let entries = entries
                    .into_iter()
                    .map(|(idx, entry)| {
                        let idx: golem_api_1_x::oplog::OplogIndex = idx.into();
                        let entry: golem_api_1_x::oplog::PublicOplogEntry = entry.try_into()?;
                        Ok((idx, entry))
                    })
                    .collect::<Result<Vec<_>, String>>()
                    .map_err(|error| {
                        anyhow!("cannot project oplog entry to golem:api/oplog@1.5.0: {error}")
                    })?;
                Ok(Ok(Some(entries)))
            }
            Ok(None) => Ok(Ok(None)),
            Err(error) if error == AGENT_OPERATION_PERMISSION_DENIED => {
                Ok(Err(OplogReadError::PermissionDenied))
            }
            Err(error) => Ok(Err(OplogReadError::InternalError(error))),
        }
    }

    async fn drop(&mut self, rep: Resource<SearchOplog>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::search-oplog", "drop");
        self.as_wasi_view().table().delete(rep)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SearchOplogEntry {
    pub owned_agent_id: OwnedAgentId,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub page_size: usize,
    pub query: String,
    pub initialized: bool,
}

impl SearchOplogEntry {
    pub fn new(
        owned_agent_id: OwnedAgentId,
        initial_oplog_index: OplogIndex,
        initial_component_revision: ComponentRevision,
        page_size: usize,
        query: String,
    ) -> Self {
        Self {
            owned_agent_id,
            next_oplog_index: initial_oplog_index,
            current_component_revision: initial_component_revision,
            page_size,
            query,
            initialized: false,
        }
    }

    pub fn update(
        &mut self,
        next_oplog_index: OplogIndex,
        current_component_revision: ComponentRevision,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_revision = current_component_revision;
        self.initialized = true;
    }
}

impl<Ctx: WorkerCtx> OplogHost for DurableWorkerCtx<Ctx> {
    async fn enrich_oplog_entries(
        &mut self,
        environment_id: golem_api_1_x::host::EnvironmentId,
        agent_id: golem_api_1_x::oplog::AgentId,
        entries: Vec<(u64, golem_api_1_x::oplog::OplogEntry)>,
        component_revision: u64,
    ) -> anyhow::Result<Result<Vec<golem_api_1_x::oplog::PublicOplogEntry>, String>> {
        self.observe_function_call("golem::api::oplog", "enrich-oplog-entries");

        let begun = DurableCallSession::<GolemApiEnrichOplogEntries, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let response = 'response: {
            let mut raw_entries = Some(entries);
            let (handle, owned_agent_id, prepared_entries) = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let environment_id =
                        golem_common::model::environment::EnvironmentId::from(Uuid::from_u64_pair(
                            environment_id.uuid.high_bits,
                            environment_id.uuid.low_bits,
                        ));
                    let agent_id: AgentId = agent_id.into();
                    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
                    let entry_indexes = raw_entries
                        .as_ref()
                        .expect("raw oplog entries must be present")
                        .iter()
                        .map(|(index, _)| OplogIndex::from_u64(*index))
                        .collect::<Vec<_>>();
                    let denied =
                        oplog_entries_denied(self, &owned_agent_id, &entry_indexes).await?;
                    if let Some(denial) = agent_operation_denial(denied) {
                        let _ =
                            crate::model::public_oplog::wit::reject_quota_handles_in_oplog_entries(
                                raw_entries
                                    .take()
                                    .expect("raw oplog entries must be present"),
                                self,
                            );
                        let request = HostRequestGolemApiOplogEnrich {
                            environment_id,
                            agent_id,
                            entries: entry_indexes
                                .iter()
                                .map(|index| (index.as_u64(), Vec::new()))
                                .collect(),
                            component_revision,
                        };
                        break 'response begun.start_live_denied(self, request, denial).await?;
                    }
                    let (request_entries, prepared_entries) = prepare_oplog_enrichment_entries(
                        self,
                        raw_entries
                            .take()
                            .expect("raw oplog entries must be present"),
                    );
                    let request = HostRequestGolemApiOplogEnrich {
                        environment_id,
                        agent_id,
                        entries: request_entries,
                        component_revision,
                    };
                    (
                        begun.start_live(self, request).await?,
                        owned_agent_id,
                        prepared_entries,
                    )
                }
                ResolvedCall::Replay(mut handle) => {
                    match handle.replay(self).await? {
                        CallReplayOutcome::Replayed(response) => {
                            let _ =
                            crate::model::public_oplog::wit::reject_quota_handles_in_oplog_entries(
                                raw_entries
                                    .take()
                                    .expect("raw oplog entries must be present"),
                                self,
                            );
                            break 'response response;
                        }
                        CallReplayOutcome::Incomplete(live) => {
                            handle = live;
                        }
                    }
                    let environment_id =
                        golem_common::model::environment::EnvironmentId::from(Uuid::from_u64_pair(
                            environment_id.uuid.high_bits,
                            environment_id.uuid.low_bits,
                        ));
                    let agent_id: AgentId = agent_id.into();
                    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
                    let (_, prepared_entries) = prepare_oplog_enrichment_entries(
                        self,
                        raw_entries
                            .take()
                            .expect("raw oplog entries must be present"),
                    );
                    (handle, owned_agent_id, prepared_entries)
                }
            };

            let entries = match prepared_entries {
                Ok(entries) => entries,
                Err(error) => {
                    break 'response handle
                        .complete(
                            self,
                            HostResponseGolemApiOplogEntries { result: Err(error) },
                        )
                        .await?;
                }
            };

            let result = async {
                let component_service = self.state.component_service.clone();
                let oplog_service = self.state.oplog_service();
                let agent_type =
                    ParsedAgentId::parse_agent_type_name(&owned_agent_id.agent_id.agent_id).ok();
                let mut current_revision = ComponentRevision::try_from(component_revision)?;
                let identity = self
                    .state
                    .worker_service
                    .resolve_agent_identity(&owned_agent_id)
                    .await
                    .map_err(|err| err.to_string())?
                    .ok_or_else(|| format!("agent {owned_agent_id} does not exist"))?;
                let agent_mode = identity.agent_mode;

                let mut result = Vec::with_capacity(entries.len());
                for (index, entry) in entries {
                    if let Some(revision) = entry.specifies_component_revision() {
                        current_revision = revision;
                    }
                    result.push(
                        PublicOplogEntry::from_oplog_entry(
                            OplogIndex::from_u64(index),
                            entry,
                            oplog_service.clone(),
                            component_service.clone(),
                            &owned_agent_id,
                            agent_mode,
                            agent_type.as_ref(),
                            current_revision,
                        )
                        .await?,
                    );
                }
                serde_json::to_vec(&result).map_err(|error| error.to_string())
            }
            .await;

            handle
                .complete(self, HostResponseGolemApiOplogEntries { result })
                .await?
        };

        match response.result {
            Ok(entries) => {
                let entries: Vec<PublicOplogEntry> =
                    serde_json::from_slice(&entries).map_err(|error| {
                        anyhow!("invalid persisted enriched public oplog entries: {error}")
                    })?;
                entries
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<Vec<_>, String>>()
                    .map(Ok)
                    .map_err(|error| {
                        anyhow!("cannot project oplog entry to golem:api/oplog@1.5.0: {error}")
                    })
            }
            Err(error) => Ok(Err(error)),
        }
    }
}

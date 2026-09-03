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

pub mod wit;

#[cfg(test)]
mod tests;

use crate::services::component::ComponentService;
use crate::services::oplog::OplogService;
use crate::services::oplog::OplogServiceOps;
use async_trait::async_trait;
use golem_common::model::agent::{AgentMode, AgentTypeName, ParsedAgentId};
use golem_common::model::component::{ComponentRevision, InstalledPlugin};
use golem_common::model::entity::{
    AgentEntity, EntityCallMode, EntityInvocationDescriptor, EntityInvocationId,
    EntityInvocationRequest,
};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::lucene::Query;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginAtomicRegionParams, BeginRemoteTransactionParams, CancelPendingInvocationParams,
    CancelledParams, CardDerivedParams, CardEventQueuedParams, CardExpiredParams,
    CardInstallFailedParams, CardInstalledParams, CardRevokedCascadeParams, CardRevokedParams,
    CardTransferConfirmedParams, CardTransferStartedParams, CardTransferredParams,
    CommittedRemoteTransactionParams, CompletionDeliveredParams, CompletionDiscardedParams,
    CreateParams, CreateResourceParams, DeactivatePluginParams, DropResourceParams,
    EndAtomicRegionParams, EndParams, ErrorParams, ExitedParams, FailedUpdateParams,
    FinishSpanParams, GrowMemoryParams, HostStreamFrameParams, InterruptedParams, JumpParams,
    LogParams, NoOpParams, OplogProcessorCheckpointParams, PendingAgentInvocationParams,
    PendingUpdateParams, PreCommitRemoteTransactionParams, PreRollbackRemoteTransactionParams,
    RemoveRetryPolicyParams, RestartParams, RevertParams, RolledBackRemoteTransactionParams,
    SetRetryPolicyParams, SetSpanAttributeParams, SnapshotParams, StartParams, StartSpanParams,
    StreamCancelParams, StreamEndParams, StreamItemsParams, StreamRegisteredParams,
    StreamSessionParams, SuccessfulUpdateParams, SuspendParams,
};
use golem_common::model::oplog::types::encode_span_data;
use golem_common::model::oplog::{
    AgentInitializationParameters, AgentInvocationOutputParameters,
    AgentMethodInvocationParameters, FallibleResultParameters, HostRequest,
    HostRequestGolemRpcInvoke, HostRequestGolemRpcScheduledInvocation, HostResponse,
    HostResponseEntityInvocation, JsonSnapshotData, LoadSnapshotParameters, ManualUpdateParameters,
    MultipartPartData, MultipartSnapshotData, MultipartSnapshotPart, OplogEntry, OplogIndex,
    OplogScopeProjection, PluginInstallationDescription, ProcessOplogEntriesParameters,
    ProcessOplogEntriesResultParameters, PublicAgentEntity, PublicAgentEntityKind,
    PublicAgentInvocation, PublicAgentInvocationResult, PublicAttribute, PublicEntityCallMode,
    PublicEntityInvocation, PublicEntityInvocationContext, PublicEntityInvocationOperation,
    PublicOplogEntry, PublicOplogEntryAttribution, PublicOplogEntryWithIndex, PublicSnapshotData,
    PublicToolInvocationOperation, PublicTypedAgentConfigEntry, PublicUpdateDescription,
    RawSnapshotData, SaveSnapshotResultParameters, SnapshotBasedUpdateParameters,
    UpdateDescription,
};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationPayload, AgentInvocationResult, Empty, OwnedAgentId,
};
use golem_common::schema::agent::FieldSource;
use golem_common::schema::{
    InputSchema, IntoTypedSchemaValue, NamedFieldType, OutputSchema, SchemaGraph, SchemaType,
    SchemaValue, TypedSchemaValue,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub struct PublicOplogChunk {
    pub entries: Vec<PublicOplogEntryWithIndex>,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub first_index_in_chunk: OplogIndex,
    pub last_index: OplogIndex,
}

#[derive(Clone)]
struct OplogStartAttributionSource {
    parent_start_index: Option<OplogIndex>,
    observational_owner: Option<OplogIndex>,
    function_name: HostFunctionName,
    request: Option<golem_common::model::oplog::OplogPayload<HostRequest>>,
}

impl OplogStartAttributionSource {
    fn from_entry(entry: &OplogEntry) -> Option<Self> {
        match entry {
            OplogEntry::Start {
                parent_start_index,
                observational_owner,
                function_name,
                request,
                ..
            } => Some(Self {
                parent_start_index: *parent_start_index,
                observational_owner: *observational_owner,
                function_name: function_name.clone(),
                request: request.clone(),
            }),
            _ => None,
        }
    }

    fn parent(&self) -> Option<OplogIndex> {
        self.observational_owner.or(self.parent_start_index)
    }
}

enum AttributionPathNode {
    Inherit(OplogIndex),
    Entity(OplogIndex, PublicEntityInvocation),
    Agent(OplogIndex),
}

struct PublicOplogAttributionResolver<'a> {
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &'a OwnedAgentId,
    agent_mode: AgentMode,
    starts: HashMap<OplogIndex, Option<OplogStartAttributionSource>>,
    resolved: HashMap<OplogIndex, Option<PublicEntityInvocationContext>>,
}

impl<'a> PublicOplogAttributionResolver<'a> {
    fn new(
        oplog_service: Arc<dyn OplogService>,
        owned_agent_id: &'a OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Self {
        Self {
            oplog_service,
            owned_agent_id,
            agent_mode,
            starts: HashMap::new(),
            resolved: HashMap::new(),
        }
    }

    fn cache_entry(&mut self, index: OplogIndex, entry: &OplogEntry) {
        self.starts
            .insert(index, OplogStartAttributionSource::from_entry(entry));
    }

    async fn attribution_for_entry(
        &mut self,
        index: OplogIndex,
        entry: &OplogEntry,
    ) -> Result<PublicOplogEntryAttribution, String> {
        if let Some(start_index) = entry.entity_parent_start_index() {
            if start_index >= index {
                return Err(format!(
                    "oplog entry {index} has non-causal entity parent Start index {start_index}"
                ));
            }
            let source = self.load_start(start_index).await?.ok_or_else(|| {
                format!(
                    "oplog entry {index} entity parent index {start_index} does not reference a Start"
                )
            })?;
            if source.function_name != HostFunctionName::GolemEntityInvoke {
                return Err(format!(
                    "oplog entry {index} entity parent Start {start_index} is not an entity invocation"
                ));
            }
            return self
                .entity_context_for_start(start_index)
                .await?
                .map(PublicOplogEntryAttribution::entity)
                .ok_or_else(|| {
                    format!(
                        "oplog entry {index} entity parent Start {start_index} has no entity attribution"
                    )
                });
        }

        let owner_start_index = match entry {
            OplogEntry::Start { .. } => Some(index),
            OplogEntry::End { start_index, .. }
            | OplogEntry::Cancelled { start_index, .. }
            | OplogEntry::CompletionDiscarded { start_index, .. }
            | OplogEntry::CompletionDelivered { start_index, .. } => Some(*start_index),
            OplogEntry::HostStreamFrame {
                parent_start_index, ..
            }
            | OplogEntry::Log {
                parent_start_index: Some(parent_start_index),
                ..
            }
            | OplogEntry::StartSpan {
                parent_start_index: Some(parent_start_index),
                ..
            }
            | OplogEntry::FinishSpan {
                parent_start_index: Some(parent_start_index),
                ..
            }
            | OplogEntry::SetSpanAttribute {
                parent_start_index: Some(parent_start_index),
                ..
            } => Some(*parent_start_index),
            OplogEntry::Error {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::NoOp {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::Jump {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::BeginAtomicRegion {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::EndAtomicRegion {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CreateResource {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::DropResource {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::SetRetryPolicy {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::RemoveRetryPolicy {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardEventQueued {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardInstalled {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardInstallFailed {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardRevoked {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardExpired {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardDerived {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardTransferStarted {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardTransferred {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardRevokedCascade {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardTransferConfirmed {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamRegistered {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamItems {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamEnd {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamCancel {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamSession {
                entity_parent_start_index,
                ..
            } => *entity_parent_start_index,
            OplogEntry::BeginRemoteTransaction {
                original_begin_index: Some(begin_index),
                ..
            } => Some(*begin_index),
            OplogEntry::BeginRemoteTransaction {
                original_begin_index: None,
                ..
            } => Some(index.previous()),
            OplogEntry::PreCommitRemoteTransaction { begin_index, .. }
            | OplogEntry::PreRollbackRemoteTransaction { begin_index, .. }
            | OplogEntry::CommittedRemoteTransaction { begin_index, .. }
            | OplogEntry::RolledBackRemoteTransaction { begin_index, .. } => Some(*begin_index),
            OplogEntry::Create { .. }
            | OplogEntry::AgentInvocationStarted { .. }
            | OplogEntry::AgentInvocationFinished { .. }
            | OplogEntry::Suspend { .. }
            | OplogEntry::Interrupted { .. }
            | OplogEntry::Exited { .. }
            | OplogEntry::PendingAgentInvocation { .. }
            | OplogEntry::PendingUpdate { .. }
            | OplogEntry::SuccessfulUpdate { .. }
            | OplogEntry::FailedUpdate { .. }
            | OplogEntry::GrowMemory { .. }
            | OplogEntry::Log {
                parent_start_index: None,
                ..
            }
            | OplogEntry::Restart { .. }
            | OplogEntry::ActivatePlugin { .. }
            | OplogEntry::DeactivatePlugin { .. }
            | OplogEntry::Revert { .. }
            | OplogEntry::CancelPendingInvocation { .. }
            | OplogEntry::StartSpan {
                parent_start_index: None,
                ..
            }
            | OplogEntry::FinishSpan {
                parent_start_index: None,
                ..
            }
            | OplogEntry::SetSpanAttribute {
                parent_start_index: None,
                ..
            }
            | OplogEntry::Snapshot { .. }
            | OplogEntry::OplogProcessorCheckpoint { .. } => None,
        };

        match owner_start_index {
            Some(start_index) => self
                .entity_context_for_start(start_index)
                .await
                .map(|context| {
                    context.map_or_else(
                        PublicOplogEntryAttribution::agent,
                        PublicOplogEntryAttribution::entity,
                    )
                }),
            None => Ok(PublicOplogEntryAttribution::agent()),
        }
    }

    async fn entity_context_for_start(
        &mut self,
        start_index: OplogIndex,
    ) -> Result<Option<PublicEntityInvocationContext>, String> {
        let mut path = Vec::new();
        let mut visited = HashSet::new();
        let mut current = start_index;
        let mut context = loop {
            if let Some(resolved) = self.resolved.get(&current) {
                break resolved.clone();
            }
            if !visited.insert(current) {
                return Err(format!("cyclic oplog Start attribution at index {current}"));
            }

            let Some(source) = self.load_start(current).await? else {
                break None;
            };
            if source.function_name == HostFunctionName::GolemToolInvocationRejected {
                path.push(AttributionPathNode::Agent(current));
                break None;
            }
            if source.function_name == HostFunctionName::GolemEntityInvoke {
                let invocation = self.load_public_entity_invocation(current, &source).await?;
                let parent = source.parent();
                path.push(AttributionPathNode::Entity(current, invocation));
                match parent {
                    Some(parent) => current = parent,
                    None => break None,
                }
            } else {
                let parent = source.parent();
                path.push(AttributionPathNode::Inherit(current));
                match parent {
                    Some(parent) => current = parent,
                    None => break None,
                }
            }
        };

        for node in path.into_iter().rev() {
            let index = match node {
                AttributionPathNode::Inherit(index) => index,
                AttributionPathNode::Agent(index) => {
                    context = None;
                    index
                }
                AttributionPathNode::Entity(index, invocation) => {
                    let ancestors = context
                        .as_ref()
                        .map(|parent| {
                            let mut ancestors = parent.ancestors.clone();
                            ancestors.push(parent.invocation.clone());
                            ancestors
                        })
                        .unwrap_or_default();
                    context = Some(PublicEntityInvocationContext {
                        invocation,
                        ancestors,
                    });
                    index
                }
            };
            self.resolved.insert(index, context.clone());
        }

        Ok(context)
    }

    async fn load_start(
        &mut self,
        index: OplogIndex,
    ) -> Result<Option<OplogStartAttributionSource>, String> {
        if let Some(start) = self.starts.get(&index) {
            return Ok(start.clone());
        }

        let entry = self
            .oplog_service
            .read_exact(self.owned_agent_id, self.agent_mode, index, 1)
            .await
            .remove(&index);
        let start = entry
            .as_ref()
            .and_then(OplogStartAttributionSource::from_entry);
        self.starts.insert(index, start.clone());
        Ok(start)
    }

    async fn load_public_entity_invocation(
        &self,
        start_index: OplogIndex,
        source: &OplogStartAttributionSource,
    ) -> Result<PublicEntityInvocation, String> {
        let request_payload = source
            .request
            .clone()
            .ok_or_else(|| format!("entity invocation Start {start_index} has no request"))?;
        let request: HostRequest = self
            .oplog_service
            .download_payload(self.owned_agent_id, self.agent_mode, request_payload)
            .await?;
        let request = match request {
            HostRequest::EntityInvocation(request) => request,
            actual => {
                return Err(format!(
                    "entity invocation Start {start_index} has unexpected request {actual:?}"
                ));
            }
        };
        let metadata = desert_rust::deserialize::<EntityInvocationRequest>(&request.metadata)
            .map_err(|error| {
                format!("failed to decode entity invocation Start {start_index}: {error}")
            })?;

        Ok(public_entity_invocation(start_index, metadata))
    }
}

fn public_entity_invocation(
    start_index: OplogIndex,
    request: EntityInvocationRequest,
) -> PublicEntityInvocation {
    let entity = PublicAgentEntity {
        kind: match &request.entity {
            AgentEntity::Tool(_) => PublicAgentEntityKind::Tool,
            AgentEntity::ToolMiddleware(_) => PublicAgentEntityKind::ToolMiddleware,
        },
        name: request.entity.name().to_string(),
    };
    let call_mode = match request.call_mode {
        EntityCallMode::Synchronous => PublicEntityCallMode::Synchronous,
        EntityCallMode::Asynchronous => PublicEntityCallMode::Asynchronous,
        EntityCallMode::FireAndForget => PublicEntityCallMode::FireAndForget,
    };
    let operation = request.operation.map(|operation| match operation {
        EntityInvocationDescriptor::Tool(tool) => {
            PublicEntityInvocationOperation::Tool(PublicToolInvocationOperation {
                command_path: tool.command_path,
                has_stdin: tool.has_stdin,
                has_stdout: tool.has_stdout,
                declares_stdout: tool.declares_stdout,
            })
        }
    });
    PublicEntityInvocation {
        entity,
        start_index,
        call_mode,
        operation,
    }
}

/// Projects one entity invocation's transitive durable-call tree from its owner's raw oplog.
/// Entity histories remain owner records; this is a filtered view, not a child oplog or status.
pub fn project_entity_oplog_entries(
    invocation_id: &EntityInvocationId,
    entries: impl IntoIterator<Item = (OplogIndex, OplogEntry)>,
) -> Vec<(OplogIndex, OplogEntry)> {
    let mut projection = OplogScopeProjection::new(invocation_id.start_index());
    entries
        .into_iter()
        .filter(|(index, entry)| projection.includes(*index, entry))
        .collect()
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
    let initial_oplog_index = initial_oplog_index.max(OplogIndex::INITIAL);
    let last_index = oplog_service
        .get_last_index(owned_agent_id, agent_mode)
        .await;
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
        .await;

    let mut entries = Vec::new();
    let mut current_component_revision = initial_component_revision;
    let mut next_oplog_index = initial_oplog_index;
    let mut first_index_in_chunk = None;

    let mut attribution_resolver =
        PublicOplogAttributionResolver::new(oplog_service.clone(), owned_agent_id, agent_mode);
    for (index, raw_entry) in &raw_entries {
        attribution_resolver.cache_entry(*index, raw_entry);
    }

    for (index, raw_entry) in raw_entries {
        if first_index_in_chunk.is_none() {
            first_index_in_chunk = Some(index);
        }
        if let Some(revision) = raw_entry.specifies_component_revision() {
            current_component_revision = revision;
        }

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
            current_component_revision,
        )
        .await?;
        entries.push(PublicOplogEntryWithIndex {
            oplog_index: index,
            attribution,
            entry,
        });
        next_oplog_index = index.next();
    }

    Ok(PublicOplogChunk {
        entries,
        next_oplog_index,
        current_component_revision,
        first_index_in_chunk: first_index_in_chunk.unwrap_or(initial_oplog_index),
        last_index,
    })
}

pub struct PublicOplogSearchResult {
    pub entries: Vec<PublicOplogEntryWithIndex>,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub last_index: OplogIndex,
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
    let mut results = Vec::new();
    let mut last_index;
    let mut current_index = initial_oplog_index;
    let mut current_component_revision = initial_component_revision;

    let query = Query::parse(query)?;

    loop {
        let chunk = get_public_oplog_chunk(
            component_service.clone(),
            oplog_service.clone(),
            owned_agent_id,
            agent_mode,
            agent_type_name,
            current_component_revision,
            current_index,
            count,
        )
        .await?;

        for entry in chunk.entries {
            if entry.entry.matches(&query) {
                results.push(entry);
            }
        }

        last_index = chunk.last_index;
        current_index = chunk.next_oplog_index;
        current_component_revision = chunk.current_component_revision;

        if current_index > last_index || results.len() >= count {
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
        .await;
    let mut current = OplogIndex::INITIAL;
    while current < start && current <= last_oplog_index {
        // NOTE: could be reading in pages for optimization
        let entry = oplog_service
            .read_exact(owned_agent_id, agent_mode, current, 1)
            .await
            .remove(&current);

        if let Some(revision) = entry.and_then(|entry| entry.specifies_component_revision()) {
            initial_component_revision = revision;
        }

        current = current.next();
    }

    Ok(initial_component_revision)
}

#[async_trait]
pub trait PublicOplogEntryOps: Sized {
    async fn from_oplog_entry(
        oplog_index: OplogIndex,
        value: OplogEntry,
        oplog_service: Arc<dyn OplogService>,
        components: Arc<dyn ComponentService>,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        agent_type: Option<&AgentTypeName>,
        component_revision: ComponentRevision,
    ) -> Result<Self, String>;
}

fn host_response_to_public_value(response: HostResponse) -> Result<TypedSchemaValue, String> {
    match response {
        HostResponse::EntityInvocation(HostResponseEntityInvocation { result: Ok(value) }) => {
            Ok(value)
        }
        response => response
            .into_typed_schema_value()
            .map_err(|error| error.to_string()),
    }
}

#[async_trait]
impl PublicOplogEntryOps for PublicOplogEntry {
    async fn from_oplog_entry(
        _oplog_index: OplogIndex,
        value: OplogEntry,
        oplog_service: Arc<dyn OplogService>,
        components: Arc<dyn ComponentService>,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        agent_type_name: Option<&AgentTypeName>,
        component_revision: ComponentRevision,
    ) -> Result<Self, String> {
        match value {
            OplogEntry::Create {
                timestamp,
                agent_id,
                agent_mode,
                component_revision,
                env,
                environment_id,
                created_by,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
                local_agent_config,
                original_phantom_id,
                instance_id,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_agent_id.agent_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let initial_plugins = agent_type_name
                    .and_then(|t| metadata.metadata.agent_type_plugins(t))
                    .unwrap_or_default()
                    .iter()
                    .filter(|&p| initial_active_plugins.contains(&p.environment_plugin_grant_id))
                    .cloned()
                    .map(make_plugin_installation_description)
                    .collect();

                let local_agent_config = local_agent_config
                    .into_iter()
                    .map(|lac| {
                        let typed = lac.enrich_with_type(&metadata.metadata, agent_type_name)?;
                        Ok::<_, String>(PublicTypedAgentConfigEntry {
                            path: typed.path,
                            value: typed.value,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(PublicOplogEntry::Create(CreateParams {
                    timestamp,
                    agent_id,
                    agent_mode,
                    component_revision,
                    env: env.into_iter().collect(),
                    environment_id,
                    created_by,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins: initial_plugins,
                    local_agent_config,
                    original_phantom_id,
                    instance_id,
                }))
            }
            OplogEntry::Start {
                timestamp,
                parent_start_index,
                function_name,
                invocation_id,
                observational_owner,
                request,
                durable_function_type,
            } => {
                let request_value = if let Some(request_payload) = request {
                    let host_request: HostRequest = oplog_service
                        .download_payload(owned_agent_id, agent_mode, request_payload)
                        .await?;

                    let request_value = match host_request {
                        HostRequest::EntityInvocation(request) => request.input,
                        HostRequest::GolemRpcInvoke(inner) => HostRequest::GolemRpcInvoke(
                            enrich_golem_rpc_invoke(components, inner).await,
                        )
                        .into_typed_schema_value()
                        .map_err(|error| error.to_string())?,
                        HostRequest::GolemRpcScheduledInvocation(inner) => {
                            HostRequest::GolemRpcScheduledInvocation(
                                enrich_golem_rpc_scheduled_invocation(components, inner).await,
                            )
                            .into_typed_schema_value()
                            .map_err(|error| error.to_string())?
                        }
                        other => other
                            .into_typed_schema_value()
                            .map_err(|error| error.to_string())?,
                    };
                    Some(request_value)
                } else {
                    None
                };

                Ok(PublicOplogEntry::Start(StartParams {
                    timestamp,
                    parent_start_index,
                    function_name: function_name.to_string(),
                    invocation_id,
                    observational_owner,
                    request: request_value,
                    durable_function_type: durable_function_type.into(),
                }))
            }
            OplogEntry::End {
                timestamp,
                start_index,
                response,
                forced_commit,
            } => {
                let response_value = if let Some(response_payload) = response {
                    let host_response: HostResponse = oplog_service
                        .download_payload(owned_agent_id, agent_mode, response_payload)
                        .await?;
                    Some(host_response_to_public_value(host_response)?)
                } else {
                    None
                };

                Ok(PublicOplogEntry::End(EndParams {
                    timestamp,
                    start_index,
                    response: response_value,
                    forced_commit,
                }))
            }
            OplogEntry::Cancelled {
                timestamp,
                start_index,
                partial,
            } => {
                let partial_value = if let Some(partial_payload) = partial {
                    let host_response: HostResponse = oplog_service
                        .download_payload(owned_agent_id, agent_mode, partial_payload)
                        .await?;
                    Some(host_response_to_public_value(host_response)?)
                } else {
                    None
                };

                Ok(PublicOplogEntry::Cancelled(CancelledParams {
                    timestamp,
                    start_index,
                    partial: partial_value,
                }))
            }
            OplogEntry::CompletionDiscarded {
                timestamp,
                start_index,
            } => Ok(PublicOplogEntry::CompletionDiscarded(
                CompletionDiscardedParams {
                    timestamp,
                    start_index,
                },
            )),
            OplogEntry::CompletionDelivered {
                timestamp,
                start_index,
            } => Ok(PublicOplogEntry::CompletionDelivered(
                CompletionDeliveredParams {
                    timestamp,
                    start_index,
                },
            )),
            OplogEntry::AgentInvocationStarted {
                timestamp,
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
                wallet_pin,
            } => {
                let invocation_payload: AgentInvocationPayload = oplog_service
                    .download_payload(owned_agent_id, agent_mode, payload)
                    .await?;

                let invocation_context_stack = InvocationContextStack::from_oplog_data(
                    trace_id,
                    trace_states,
                    invocation_context,
                );
                let invocation = AgentInvocation::from_parts(
                    idempotency_key,
                    invocation_payload,
                    invocation_context_stack,
                );
                let public_invocation = agent_invocation_to_public(
                    components.clone(),
                    owned_agent_id,
                    component_revision,
                    invocation,
                )
                .await?;

                Ok(PublicOplogEntry::AgentInvocationStarted(
                    AgentInvocationStartedParams {
                        timestamp,
                        invocation: public_invocation,
                        wallet_pin: wallet_pin.map(|pin| {
                            golem_common::model::card::PublicInvocationWalletPin {
                                wallet_token: pin.wallet_token,
                                scope_card_id: pin.scope_card_id,
                            }
                        }),
                    },
                ))
            }
            OplogEntry::AgentInvocationFinished {
                timestamp,
                result,
                method_name,
                consumed_fuel,
                component_revision: entry_component_revision,
            } => {
                let invocation_result: AgentInvocationResult = oplog_service
                    .download_payload(owned_agent_id, agent_mode, result)
                    .await?;

                let public_result = agent_invocation_result_to_public(
                    components.clone(),
                    owned_agent_id,
                    component_revision,
                    method_name.clone(),
                    invocation_result,
                )
                .await?;

                Ok(PublicOplogEntry::AgentInvocationFinished(
                    AgentInvocationFinishedParams {
                        timestamp,
                        result: public_result,
                        method_name,
                        consumed_fuel,
                        component_revision: entry_component_revision,
                    },
                ))
            }
            OplogEntry::Suspend { timestamp } => {
                Ok(PublicOplogEntry::Suspend(SuspendParams { timestamp }))
            }
            OplogEntry::Error {
                timestamp,
                error,
                retry_from,
                inside_atomic_region,
                retry_policy_state,
                ..
            } => Ok(PublicOplogEntry::Error(ErrorParams {
                timestamp,
                error: error.to_string(""),
                retry_from,
                inside_atomic_region,
                retry_policy_state: retry_policy_state.map(Into::into),
            })),
            OplogEntry::NoOp { timestamp, .. } => {
                Ok(PublicOplogEntry::NoOp(NoOpParams { timestamp }))
            }
            OplogEntry::Jump {
                timestamp, jump, ..
            } => Ok(PublicOplogEntry::Jump(JumpParams { timestamp, jump })),
            OplogEntry::Interrupted { timestamp } => {
                Ok(PublicOplogEntry::Interrupted(InterruptedParams {
                    timestamp,
                }))
            }
            OplogEntry::Exited { timestamp } => {
                Ok(PublicOplogEntry::Exited(ExitedParams { timestamp }))
            }
            OplogEntry::BeginAtomicRegion { timestamp, .. } => Ok(
                PublicOplogEntry::BeginAtomicRegion(BeginAtomicRegionParams { timestamp }),
            ),
            OplogEntry::EndAtomicRegion {
                timestamp,
                begin_index,
                ..
            } => Ok(PublicOplogEntry::EndAtomicRegion(EndAtomicRegionParams {
                timestamp,
                begin_index,
            })),
            OplogEntry::PendingAgentInvocation {
                timestamp,
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
            } => {
                let invocation_payload: AgentInvocationPayload = oplog_service
                    .download_payload(owned_agent_id, agent_mode, payload)
                    .await?;

                let invocation_context_stack = InvocationContextStack::from_oplog_data(
                    trace_id,
                    trace_states,
                    invocation_context,
                );
                let invocation = AgentInvocation::from_parts(
                    idempotency_key,
                    invocation_payload,
                    invocation_context_stack,
                );
                let public_invocation = agent_invocation_to_public(
                    components.clone(),
                    owned_agent_id,
                    component_revision,
                    invocation,
                )
                .await?;

                Ok(PublicOplogEntry::PendingAgentInvocation(
                    PendingAgentInvocationParams {
                        timestamp,
                        invocation: public_invocation,
                    },
                ))
            }
            OplogEntry::PendingUpdate {
                timestamp,
                description,
            } => {
                let target_revision = *description.target_revision();
                let public_description = match description {
                    UpdateDescription::Automatic { .. } => {
                        PublicUpdateDescription::Automatic(Empty {})
                    }
                    UpdateDescription::SnapshotBased {
                        payload, mime_type, ..
                    } => {
                        let bytes = oplog_service
                            .download_payload(owned_agent_id, agent_mode, payload)
                            .await?;
                        PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                            payload: bytes,
                            mime_type,
                        })
                    }
                };
                Ok(PublicOplogEntry::PendingUpdate(PendingUpdateParams {
                    timestamp,
                    target_revision,
                    description: public_description,
                }))
            }
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_revision,
                new_component_size,
                new_total_linear_memory_size: _,
                new_active_plugins,
            } => {
                let metadata = components
                    .get_metadata(owned_agent_id.agent_id.component_id, Some(target_revision))
                    .await
                    .map_err(|err| err.to_string())?;

                let new_plugins = agent_type_name
                    .and_then(|t| metadata.metadata.agent_type_plugins(t))
                    .unwrap_or_default()
                    .iter()
                    .filter(|&p| new_active_plugins.contains(&p.environment_plugin_grant_id))
                    .cloned()
                    .map(make_plugin_installation_description)
                    .collect();

                Ok(PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
                    timestamp,
                    target_revision,
                    new_component_size,
                    new_active_plugins: new_plugins,
                }))
            }
            OplogEntry::FailedUpdate {
                timestamp,
                target_revision,
                details,
            } => Ok(PublicOplogEntry::FailedUpdate(FailedUpdateParams {
                timestamp,
                target_revision,
                details,
            })),
            OplogEntry::GrowMemory { timestamp, delta } => {
                Ok(PublicOplogEntry::GrowMemory(GrowMemoryParams {
                    timestamp,
                    delta,
                }))
            }
            OplogEntry::CreateResource {
                timestamp,
                id,
                resource_type_id,
                ..
            } => Ok(PublicOplogEntry::CreateResource(CreateResourceParams {
                timestamp,
                id,
                name: resource_type_id.name,
                owner: resource_type_id.owner,
            })),
            OplogEntry::DropResource {
                timestamp,
                id,
                resource_type_id,
                ..
            } => Ok(PublicOplogEntry::DropResource(DropResourceParams {
                timestamp,
                id,
                name: resource_type_id.name,
                owner: resource_type_id.owner,
            })),

            OplogEntry::Log {
                timestamp,
                level,
                context,
                message,
                ..
            } => Ok(PublicOplogEntry::Log(LogParams {
                timestamp,
                level,
                context,
                message,
            })),
            OplogEntry::Restart { timestamp } => {
                Ok(PublicOplogEntry::Restart(RestartParams { timestamp }))
            }
            OplogEntry::ActivatePlugin {
                timestamp,
                plugin_grant_id,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_agent_id.agent_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let plugin_installation = agent_type_name
                    .and_then(|t| metadata.metadata.agent_type_plugins(t))
                    .and_then(|plugins| {
                        plugins
                            .iter()
                            .find(|p| p.environment_plugin_grant_id == plugin_grant_id)
                    })
                    .cloned()
                    .ok_or("plugin not found in metadata".to_string())?;

                let desc = make_plugin_installation_description(plugin_installation);
                Ok(PublicOplogEntry::ActivatePlugin(ActivatePluginParams {
                    timestamp,
                    plugin: desc,
                }))
            }
            OplogEntry::DeactivatePlugin {
                timestamp,
                plugin_grant_id,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_agent_id.agent_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let plugin_installation = agent_type_name
                    .and_then(|t| metadata.metadata.agent_type_plugins(t))
                    .and_then(|plugins| {
                        plugins
                            .iter()
                            .find(|p| p.environment_plugin_grant_id == plugin_grant_id)
                    })
                    .cloned()
                    .ok_or("plugin not found in metadata".to_string())?;

                let desc = make_plugin_installation_description(plugin_installation);
                Ok(PublicOplogEntry::DeactivatePlugin(DeactivatePluginParams {
                    timestamp,
                    plugin: desc,
                }))
            }
            OplogEntry::Revert {
                timestamp,
                dropped_region,
            } => Ok(PublicOplogEntry::Revert(RevertParams {
                timestamp,
                dropped_region,
            })),
            OplogEntry::CancelPendingInvocation {
                timestamp,
                idempotency_key,
            } => Ok(PublicOplogEntry::CancelPendingInvocation(
                CancelPendingInvocationParams {
                    timestamp,
                    idempotency_key,
                },
            )),
            OplogEntry::StartSpan {
                timestamp,
                span_id,
                parent: parent_id,
                linked_context_id,
                attributes,
                ..
            } => Ok(PublicOplogEntry::StartSpan(StartSpanParams {
                timestamp,
                span_id,
                parent_id,
                linked_context: linked_context_id,
                attributes: attributes
                    .0
                    .into_iter()
                    .map(|(k, v)| PublicAttribute {
                        key: k,
                        value: v.into(),
                    })
                    .collect(),
            })),
            OplogEntry::FinishSpan {
                timestamp, span_id, ..
            } => Ok(PublicOplogEntry::FinishSpan(FinishSpanParams {
                timestamp,
                span_id,
            })),
            OplogEntry::SetSpanAttribute {
                timestamp,
                span_id,
                key,
                value,
                ..
            } => Ok(PublicOplogEntry::SetSpanAttribute(SetSpanAttributeParams {
                timestamp,
                span_id,
                key,
                value: value.into(),
            })),
            OplogEntry::BeginRemoteTransaction {
                timestamp,
                transaction_id,
                ..
            } => Ok(PublicOplogEntry::BeginRemoteTransaction(
                BeginRemoteTransactionParams {
                    timestamp,
                    transaction_id,
                },
            )),
            OplogEntry::PreCommitRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::PreCommitRemoteTransaction(
                PreCommitRemoteTransactionParams {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::PreRollbackRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::PreRollbackRemoteTransaction(
                PreRollbackRemoteTransactionParams {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::CommittedRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::CommittedRemoteTransaction(
                CommittedRemoteTransactionParams {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::RolledBackRemoteTransaction {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::RolledBackRemoteTransaction(
                RolledBackRemoteTransactionParams {
                    timestamp,
                    begin_index,
                },
            )),
            OplogEntry::Snapshot {
                timestamp,
                data,
                mime_type,
                ..
            } => {
                let bytes: Vec<u8> = oplog_service
                    .download_payload(owned_agent_id, agent_mode, data)
                    .await?;

                let snapshot_data = raw_snapshot_to_public(RawSnapshotData {
                    data: bytes,
                    mime_type,
                });

                Ok(PublicOplogEntry::Snapshot(SnapshotParams {
                    timestamp,
                    data: snapshot_data,
                }))
            }
            OplogEntry::OplogProcessorCheckpoint {
                timestamp,
                plugin_grant_id,
                target_agent_id,
                confirmed_up_to,
                sending_up_to,
                last_batch_start,
            } => {
                let metadata = components
                    .get_metadata(
                        owned_agent_id.agent_id.component_id,
                        Some(component_revision),
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                let plugin_installation = agent_type_name
                    .and_then(|t| metadata.metadata.agent_type_plugins(t))
                    .and_then(|plugins| {
                        plugins
                            .iter()
                            .find(|p| p.environment_plugin_grant_id == plugin_grant_id)
                    })
                    .cloned()
                    .ok_or("plugin not found in metadata".to_string())?;

                let desc = make_plugin_installation_description(plugin_installation);
                Ok(PublicOplogEntry::OplogProcessorCheckpoint(
                    OplogProcessorCheckpointParams {
                        timestamp,
                        plugin: desc,
                        target_agent_id,
                        confirmed_up_to,
                        sending_up_to,
                        last_batch_start,
                    },
                ))
            }
            OplogEntry::SetRetryPolicy {
                timestamp, policy, ..
            } => Ok(PublicOplogEntry::SetRetryPolicy(SetRetryPolicyParams {
                timestamp,
                policy: policy.into(),
            })),
            OplogEntry::RemoveRetryPolicy {
                timestamp, name, ..
            } => Ok(PublicOplogEntry::RemoveRetryPolicy(
                RemoveRetryPolicyParams { timestamp, name },
            )),
            OplogEntry::CardRevoked {
                timestamp,
                queued_event_index,
                card_id,
                wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardRevoked(CardRevokedParams {
                timestamp,
                queued_event_index,
                card_id,
                wallet_generation,
            })),
            OplogEntry::CardExpired {
                timestamp,
                card_id,
                wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardExpired(CardExpiredParams {
                timestamp,
                card_id,
                wallet_generation,
            })),
            OplogEntry::HostStreamFrame {
                timestamp,
                parent_start_index,
                kind,
                payload,
            } => {
                let host_request: HostRequest = oplog_service
                    .download_payload(owned_agent_id, agent_mode, payload)
                    .await?;

                Ok(PublicOplogEntry::HostStreamFrame(HostStreamFrameParams {
                    timestamp,
                    parent_start_index,
                    kind,
                    payload: host_request
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::StreamRegistered {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamRegistered(StreamRegisteredParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::StreamItems {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamItems(StreamItemsParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::StreamEnd {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamEnd(StreamEndParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::StreamCancel {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamCancel(StreamCancelParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::StreamSession {
                timestamp, record, ..
            } => {
                let record = oplog_service
                    .download_payload(owned_agent_id, agent_mode, record)
                    .await?;
                Ok(PublicOplogEntry::StreamSession(StreamSessionParams {
                    timestamp,
                    record: record
                        .into_typed_schema_value()
                        .map_err(|e| e.to_string())?,
                }))
            }
            OplogEntry::CardEventQueued {
                timestamp, event, ..
            } => Ok(PublicOplogEntry::CardEventQueued(CardEventQueuedParams {
                timestamp,
                event: event.into(),
            })),
            OplogEntry::CardInstalled {
                timestamp,
                queued_event_index,
                card,
                wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardInstalled(CardInstalledParams {
                timestamp,
                queued_event_index,
                card_id: card.card_id(),
                wallet_generation,
            })),
            OplogEntry::CardInstallFailed {
                timestamp,
                queued_event_index,
                card_id,
                reason,
                ..
            } => Ok(PublicOplogEntry::CardInstallFailed(
                CardInstallFailedParams {
                    timestamp,
                    queued_event_index,
                    card_id,
                    reason,
                },
            )),
            OplogEntry::CardDerived {
                timestamp,
                card,
                wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardDerived(CardDerivedParams {
                timestamp,
                card_id: card.card_id(),
                parent_ids: card.parent_ids().to_vec(),
                wallet_generation,
            })),
            OplogEntry::CardTransferStarted {
                timestamp,
                transfer_id,
                card_id,
                target_holder,
                source_wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardTransferStarted(
                CardTransferStartedParams {
                    timestamp,
                    transfer_id,
                    card_id,
                    target_holder,
                    source_wallet_generation,
                },
            )),
            OplogEntry::CardTransferred {
                timestamp,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                target_wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardTransferred(CardTransferredParams {
                timestamp,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                target_wallet_generation,
            })),
            OplogEntry::CardRevokedCascade {
                timestamp,
                revoked_card_ids,
                local_wallet_generation,
                ..
            } => Ok(PublicOplogEntry::CardRevokedCascade(
                CardRevokedCascadeParams {
                    timestamp,
                    revoked_card_ids,
                    local_wallet_generation,
                },
            )),
            OplogEntry::CardTransferConfirmed {
                timestamp,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                ..
            } => Ok(PublicOplogEntry::CardTransferConfirmed(
                CardTransferConfirmedParams {
                    timestamp,
                    transfer_id,
                    source_card_id,
                    installed_card_id,
                    target_holder,
                },
            )),
        }
    }
}

fn raw_snapshot_to_public(snapshot: RawSnapshotData) -> PublicSnapshotData {
    if snapshot.mime_type == "application/json" {
        match serde_json::from_slice(&snapshot.data) {
            Ok(json_value) => PublicSnapshotData::Json(JsonSnapshotData { data: json_value }),
            Err(_) => PublicSnapshotData::Raw(snapshot),
        }
    } else if snapshot.mime_type.starts_with("multipart/mixed") {
        parse_multipart_snapshot(snapshot)
    } else {
        PublicSnapshotData::Raw(snapshot)
    }
}

fn parse_multipart_snapshot(snapshot: RawSnapshotData) -> PublicSnapshotData {
    use golem_common::base_model::oplog::multipart::{extract_boundary, parse_multipart_mixed};

    let boundary = match extract_boundary(&snapshot.mime_type) {
        Some(b) => b.to_string(),
        None => return PublicSnapshotData::Raw(snapshot),
    };

    let parsed = match parse_multipart_mixed(&boundary, &snapshot.data) {
        Some(parts) => parts,
        None => return PublicSnapshotData::Raw(snapshot),
    };

    let parts = parsed
        .iter()
        .map(|p| {
            let name = p.name.clone().unwrap_or_default();
            let content_type = p.content_type.clone().unwrap_or_default();
            let data = if content_type == "application/json" {
                match serde_json::from_slice(p.body) {
                    Ok(json_value) => {
                        MultipartPartData::Json(JsonSnapshotData { data: json_value })
                    }
                    Err(_) => MultipartPartData::Raw(RawSnapshotData {
                        data: p.body.to_vec(),
                        mime_type: content_type.clone(),
                    }),
                }
            } else {
                MultipartPartData::Raw(RawSnapshotData {
                    data: p.body.to_vec(),
                    mime_type: content_type.clone(),
                })
            };
            MultipartSnapshotPart {
                name,
                content_type,
                data,
            }
        })
        .collect();

    PublicSnapshotData::Multipart(MultipartSnapshotData {
        mime_type: snapshot.mime_type,
        parts,
    })
}

async fn try_resolve_agent_id(
    component_service: Arc<dyn ComponentService>,
    agent_id: &AgentId,
) -> Option<ParsedAgentId> {
    if let Ok(component) = component_service
        .get_metadata(agent_id.component_id, None)
        .await
    {
        ParsedAgentId::parse(&agent_id.agent_id, &component.metadata).ok()
    } else {
        None
    }
}

async fn enrich_golem_rpc_invoke(
    components: Arc<dyn ComponentService>,
    mut payload: HostRequestGolemRpcInvoke,
) -> HostRequestGolemRpcInvoke {
    let agent_id = try_resolve_agent_id(components, &payload.remote_agent_id).await;
    payload.remote_agent_type = agent_id
        .as_ref()
        .map(|agent_id| agent_id.agent_type.clone());
    payload.remote_agent_parameters = agent_id.map(|agent_id| agent_id.parameters);
    payload
}

async fn enrich_golem_rpc_scheduled_invocation(
    components: Arc<dyn ComponentService>,
    mut payload: HostRequestGolemRpcScheduledInvocation,
) -> HostRequestGolemRpcScheduledInvocation {
    let agent_id = try_resolve_agent_id(components, &payload.remote_agent_id).await;
    payload.remote_agent_type = agent_id
        .as_ref()
        .map(|agent_id| agent_id.agent_type.clone());
    payload.remote_agent_parameters = agent_id.map(|agent_id| agent_id.parameters);
    payload
}

fn resolve_agent_type_from_worker_name<'a>(
    metadata: &'a golem_common::model::component_metadata::ComponentMetadata,
    worker_name: &str,
) -> Option<&'a golem_common::schema::agent::AgentTypeSchema> {
    ParsedAgentId::parse_agent_type_name(worker_name)
        .ok()
        .and_then(|type_name| metadata.find_agent_type_by_name_ref(&type_name))
}

/// A schema-native empty value (`()`), used as the best-effort fallback when
/// the driving schema cannot be resolved from the component metadata. Mirrors
/// the previous `DataValue::Tuple(empty)` fallback in the legacy renderer.
fn empty_typed_schema_value() -> TypedSchemaValue {
    TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
        SchemaValue::Tuple {
            elements: Vec::new(),
        },
    )
}

/// Pair a schema-native invocation **input** value (a parameter record, see
/// [`crate::worker::invocation::lower_invocation`]) with the record schema
/// derived from the agent's declared [`InputSchema`].
///
/// The recorded value is caller-only: auto-injected fields (e.g. the principal)
/// travel out of band and are not part of it, so they are excluded from the
/// derived record schema to keep the schema and value arities aligned.
fn input_value_to_typed_schema_value(
    input_schema: &InputSchema,
    value: SchemaValue,
) -> Result<TypedSchemaValue, String> {
    let fields = input_schema
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .map(|field| NamedFieldType {
            name: field.name.clone(),
            body: field.schema.clone(),
            metadata: field.metadata.clone(),
        })
        .collect();
    Ok(TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(fields)),
        value,
    ))
}

/// Pair a schema-native invocation **output** value with the schema derived
/// from the agent method's declared [`OutputSchema`]. A `unit` output is
/// represented by the canonical empty tuple (see
/// [`crate::worker::invocation::decode_invoke_output`]).
fn output_value_to_typed_schema_value(
    output_schema: &OutputSchema,
    value: SchemaValue,
) -> Result<TypedSchemaValue, String> {
    let root = match output_schema.schema() {
        Some(ty) => ty.clone(),
        None => SchemaType::tuple(Vec::new()),
    };
    Ok(TypedSchemaValue::new(SchemaGraph::anonymous(root), value))
}

async fn agent_invocation_to_public(
    components: Arc<dyn ComponentService>,
    owned_agent_id: &OwnedAgentId,
    component_revision: ComponentRevision,
    invocation: AgentInvocation,
) -> Result<PublicAgentInvocation, String> {
    match invocation {
        AgentInvocation::AgentInitialization {
            idempotency_key,
            input,
            invocation_context,
            ..
        } => {
            let metadata = components
                .get_metadata(
                    owned_agent_id.agent_id.component_id,
                    Some(component_revision),
                )
                .await
                .map_err(|err| err.to_string())?;

            let agent_type = resolve_agent_type_from_worker_name(
                &metadata.metadata,
                &owned_agent_id.agent_id.agent_id,
            );

            let constructor_schema = agent_type.map(|at| at.constructor.input_schema.clone());

            let constructor_parameters = match constructor_schema {
                Some(schema) => input_value_to_typed_schema_value(&schema, input)
                    .unwrap_or_else(|_| empty_typed_schema_value()),
                None => empty_typed_schema_value(),
            };

            let span_data = invocation_context.to_oplog_data();

            Ok(PublicAgentInvocation::AgentInitialization(
                AgentInitializationParameters {
                    idempotency_key,
                    constructor_parameters,
                    trace_id: invocation_context.trace_id.clone(),
                    trace_states: invocation_context.trace_states.clone(),
                    invocation_context: encode_span_data(&span_data),
                },
            ))
        }
        AgentInvocation::AgentMethod {
            idempotency_key,
            method_name,
            input,
            invocation_context,
            ..
        } => {
            let metadata = components
                .get_metadata(
                    owned_agent_id.agent_id.component_id,
                    Some(component_revision),
                )
                .await
                .map_err(|err| err.to_string())?;

            let agent_type = resolve_agent_type_from_worker_name(
                &metadata.metadata,
                &owned_agent_id.agent_id.agent_id,
            );

            let method_schema = agent_type
                .and_then(|at| at.methods.iter().find(|m| m.name == method_name).cloned())
                .map(|m| m.input_schema.clone());

            let function_input = match method_schema {
                Some(schema) => input_value_to_typed_schema_value(&schema, input)
                    .unwrap_or_else(|_| empty_typed_schema_value()),
                None => empty_typed_schema_value(),
            };

            let span_data = invocation_context.to_oplog_data();

            Ok(PublicAgentInvocation::AgentMethodInvocation(
                AgentMethodInvocationParameters {
                    idempotency_key,
                    method_name,
                    function_input,
                    trace_id: invocation_context.trace_id.clone(),
                    trace_states: invocation_context.trace_states.clone(),
                    invocation_context: encode_span_data(&span_data),
                },
            ))
        }
        AgentInvocation::ManualUpdate { target_revision } => Ok(
            PublicAgentInvocation::ManualUpdate(ManualUpdateParameters { target_revision }),
        ),
        AgentInvocation::SaveSnapshot { .. } => Ok(PublicAgentInvocation::SaveSnapshot(Empty {})),
        AgentInvocation::LoadSnapshot { snapshot, .. } => Ok(PublicAgentInvocation::LoadSnapshot(
            LoadSnapshotParameters {
                snapshot: raw_snapshot_to_public(snapshot),
            },
        )),
        AgentInvocation::ProcessOplogEntries {
            idempotency_key, ..
        } => Ok(PublicAgentInvocation::ProcessOplogEntries(
            ProcessOplogEntriesParameters { idempotency_key },
        )),
    }
}

async fn agent_invocation_result_to_public(
    components: Arc<dyn ComponentService>,
    owned_agent_id: &OwnedAgentId,
    component_revision: ComponentRevision,
    method_name: Option<String>,
    result: AgentInvocationResult,
) -> Result<PublicAgentInvocationResult, String> {
    match result {
        AgentInvocationResult::AgentInitialization => {
            let _ = components;
            let _ = owned_agent_id;
            let _ = component_revision;

            Ok(PublicAgentInvocationResult::AgentInitialization(
                AgentInvocationOutputParameters {
                    output: empty_typed_schema_value(),
                },
            ))
        }
        AgentInvocationResult::AgentMethod { output } => {
            // The persisted `method_name` lets us resolve the method's declared
            // output schema and pair it with the schema-native output value.
            let output_schema = match &method_name {
                Some(method_name) => {
                    let metadata = components
                        .get_metadata(
                            owned_agent_id.agent_id.component_id,
                            Some(component_revision),
                        )
                        .await
                        .map_err(|err| err.to_string())?;

                    resolve_agent_type_from_worker_name(
                        &metadata.metadata,
                        &owned_agent_id.agent_id.agent_id,
                    )
                    .and_then(|at| at.methods.iter().find(|m| m.name == *method_name).cloned())
                    .map(|m| m.output_schema.clone())
                }
                None => None,
            };

            let output = match output_schema {
                Some(schema) => output_value_to_typed_schema_value(&schema, output)
                    .unwrap_or_else(|_| empty_typed_schema_value()),
                None => empty_typed_schema_value(),
            };

            Ok(PublicAgentInvocationResult::AgentMethod(
                AgentInvocationOutputParameters { output },
            ))
        }
        AgentInvocationResult::ManualUpdate => {
            Ok(PublicAgentInvocationResult::ManualUpdate(Empty {}))
        }
        AgentInvocationResult::LoadSnapshot { error } => Ok(
            PublicAgentInvocationResult::LoadSnapshot(FallibleResultParameters { error }),
        ),
        AgentInvocationResult::SaveSnapshot { snapshot } => Ok(
            PublicAgentInvocationResult::SaveSnapshot(SaveSnapshotResultParameters {
                snapshot: raw_snapshot_to_public(snapshot),
            }),
        ),
        AgentInvocationResult::ProcessOplogEntries { error } => {
            Ok(PublicAgentInvocationResult::ProcessOplogEntries(
                ProcessOplogEntriesResultParameters { error },
            ))
        }
    }
}

fn make_plugin_installation_description(
    installation: InstalledPlugin,
) -> PluginInstallationDescription {
    PluginInstallationDescription {
        environment_plugin_grant_id: installation.environment_plugin_grant_id,
        plugin_priority: installation.priority,
        plugin_name: installation.plugin_name,
        plugin_version: installation.plugin_version,
        parameters: installation.parameters,
    }
}

#[cfg(test)]
mod conversion_tests {
    use super::*;
    use golem_common::schema::agent::{AutoInjectedKind, NamedField};
    use test_r::test;

    /// An agent method (or constructor) input schema that mixes user-supplied
    /// fields with an auto-injected `principal` field. The value recorded in
    /// the oplog is caller-only (the principal travels out of band), so the
    /// typed value paired for the public oplog must describe exactly the
    /// user-supplied fields — its root record arity must match the value's.
    #[test]
    fn input_value_to_typed_schema_value_excludes_auto_injected_fields() {
        let input_schema = InputSchema::parameters([
            NamedField::user_supplied("count", SchemaType::u32()),
            NamedField::user_supplied("label", SchemaType::string()),
            NamedField::auto_injected(
                "principal",
                AutoInjectedKind::Principal,
                SchemaType::string(),
            ),
        ]);
        // Caller-only record as stored in the oplog (two user-supplied values).
        let value = SchemaValue::Record {
            fields: vec![SchemaValue::U32(7), SchemaValue::String("hi".to_string())],
        };

        let typed = input_value_to_typed_schema_value(&input_schema, value)
            .expect("pairing caller-only value with method schema must succeed");

        let SchemaType::Record { fields, .. } = typed.root_type() else {
            panic!("expected record root, got {:?}", typed.root_type());
        };
        let SchemaValue::Record { fields: values } = typed.value() else {
            panic!("expected record value, got {:?}", typed.value());
        };
        assert_eq!(
            fields.len(),
            values.len(),
            "root record schema arity must match the caller-only value arity"
        );
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["count", "label"]);
    }
}

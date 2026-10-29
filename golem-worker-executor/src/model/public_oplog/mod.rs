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

mod attribution;
mod conversion;
mod invocation;
mod reading;
mod snapshots;

use attribution::PublicOplogAttributionResolver;
#[cfg(test)]
use conversion::host_response_to_public_value;
use invocation::{
    agent_invocation_result_to_public, agent_invocation_to_public, enrich_golem_rpc_invoke,
    enrich_golem_rpc_scheduled_invocation,
};
pub use reading::{
    find_component_revision_at, get_public_oplog_chunk, get_public_oplog_entry,
    get_public_oplog_page, search_public_oplog, search_public_oplog_page,
};
use snapshots::raw_snapshot_to_public;

#[cfg(test)]
mod tests;

use crate::services::component::ComponentService;
use crate::services::oplog::OplogService;
use crate::services::oplog::OplogServiceOps;
use async_trait::async_trait;
use golem_common::model::agent::{AgentMode, AgentTypeName, OwnerKind, ParsedAgentId};
use golem_common::model::component::{ComponentRevision, InstalledPlugin};
use golem_common::model::entity::{
    AgentEntity, EntityCallMode, EntityInvocationDescriptor, EntityInvocationRequest,
};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::lucene::Query;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginRemoteTransactionParams, CancelPendingInvocationParams, CancelledParams,
    CardDerivedParams, CardEventQueuedParams, CardExpiredParams, CardInstallFailedParams,
    CardInstalledParams, CardRevokedCascadeParams, CardRevokedParams, CardTransferConfirmedParams,
    CardTransferStartedParams, CardTransferredParams, CommittedRemoteTransactionParams,
    CompletionDeliveredParams, CompletionDiscardedParams, CreateParams, CreateResourceParams,
    DeactivatePluginParams, DropResourceParams, EndParams, ErrorParams, ExitedParams,
    FailedUpdateParams, GrowMemoryParams, HostStreamFrameParams, InterruptedParams, JumpParams,
    LogParams, OplogProcessorCheckpointParams, PendingAgentInvocationParams, PendingUpdateParams,
    PreCommitRemoteTransactionParams, PreRollbackRemoteTransactionParams, RecoverySucceededParams,
    RemoveRetryPolicyParams, RestartParams, ResumedParams, RevertParams,
    RolledBackRemoteTransactionParams, SetRetryPolicyParams, SnapshotConfirmedParams,
    SnapshotParams, SpanClosedParams, StartParams, StreamCancelParams, StreamEndParams,
    StreamItemsParams, StreamRegisteredParams, StreamSessionParams, SuccessfulUpdateParams,
    SuspendParams,
};
use golem_common::model::oplog::public_oplog_entry::{
    PublicSpanAttributes, PublicSpanFinished, PublicSpanKind, PublicSpanLink, PublicSpanOutcome,
    PublicSpanStarted,
};
use golem_common::model::oplog::types::encode_span_data;
use golem_common::model::oplog::{
    AgentInitializationParameters, AgentInvocationOutputParameters,
    AgentMethodInvocationParameters, ExternalToolInvocationParameters,
    ExternalToolResultParameters, FallibleResultParameters, HostRequest, HostRequestGolemRpcInvoke,
    HostRequestGolemRpcScheduledInvocation, HostResponse, HostResponseEntityInvocation,
    JsonSnapshotData, LoadSnapshotParameters, ManualUpdateParameters, MultipartPartData,
    MultipartSnapshotData, MultipartSnapshotPart, OmittedOplogEntry, OplogEntry, OplogIndex,
    PluginInstallationDescription, ProcessOplogEntriesParameters,
    ProcessOplogEntriesResultParameters, PublicAgentEntity, PublicAgentEntityKind,
    PublicAgentInvocation, PublicAgentInvocationResult, PublicAttribute, PublicEntityCallMode,
    PublicEntityInvocation, PublicEntityInvocationContext, PublicEntityInvocationOperation,
    PublicExternalToolResult, PublicFailedSnapshotAssistedUpdateDetails, PublicOplogEntry,
    PublicOplogEntryAttribution, PublicOplogEntryWithIndex, PublicOplogPageEntry,
    PublicSnapshotAssistedUpdateDetails, PublicSnapshotData, PublicToolInvocationOperation,
    PublicTypedAgentConfigEntry, PublicUpdateDescription, RawSnapshotData,
    SaveSnapshotResultParameters, SnapshotAssistedAutomaticUpdateParameters,
    SnapshotBasedUpdateParameters, UpdateDescription,
};
use golem_common::model::oplog::{
    SpanAttributes, SpanFinished, SpanKind, SpanOutcome, SpanStarted,
};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationPayload, AgentInvocationResult, Empty, OwnedAgentId,
};
use golem_common::schema::agent::FieldSource;
use golem_common::schema::graph::reachable_defs;
use golem_common::schema::{
    InputSchema, IntoTypedSchemaValue, NamedFieldType, OutputSchema, SchemaGraph, SchemaType,
    SchemaValue, TypedSchemaValue,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
pub struct PublicOplogChunk<E = PublicOplogEntryWithIndex> {
    pub entries: Vec<E>,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub first_index_in_chunk: OplogIndex,
    pub last_index: OplogIndex,
}

/// The limits of an oplog page that the executor returns to a remote reader.
#[derive(Clone, Copy, Debug)]
pub struct PublicOplogPageLimits {
    /// The maximum number of oplog entries that the page reads.
    pub count: usize,
    /// The maximum protobuf-encoded size of the entries of the page. A page always holds at
    /// least one entry.
    pub max_bytes: usize,
    /// The page replaces an entry whose protobuf-encoded size is larger than this with an
    /// `OmittedOplogEntry`.
    pub max_entry_bytes: usize,
}

pub struct PublicOplogSearchResult<E = PublicOplogEntryWithIndex> {
    pub entries: Vec<E>,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub last_index: OplogIndex,
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

fn public_attributes(attributes: golem_common::model::oplog::AttributeMap) -> Vec<PublicAttribute> {
    attributes
        .0
        .into_iter()
        .map(|(key, value)| PublicAttribute {
            key,
            value: value.into(),
        })
        .collect()
}

fn public_span_started(span: SpanStarted) -> PublicSpanStarted {
    PublicSpanStarted {
        span_id: span.span_id,
        trace_id: span.trace_id,
        trace_states: span.trace_states,
        parent_span_id: span.parent_span_id,
        links: span
            .links
            .into_iter()
            .map(|link| PublicSpanLink {
                trace_id: link.trace_id,
                span_id: link.span_id,
                trace_states: link.trace_states,
            })
            .collect(),
        started_at: span.started_at,
        attributes: public_attributes(span.attributes),
        kind: match span.kind {
            SpanKind::Internal => PublicSpanKind::Internal,
            SpanKind::Client => PublicSpanKind::Client,
            SpanKind::Server => PublicSpanKind::Server,
        },
    }
}

fn public_span_finished(span: SpanFinished) -> PublicSpanFinished {
    PublicSpanFinished {
        span_id: span.span_id,
        finished_at: span.finished_at,
        outcome: match span.outcome {
            SpanOutcome::Completed => PublicSpanOutcome::Completed,
            SpanOutcome::Failed => PublicSpanOutcome::Failed,
            SpanOutcome::Cancelled => PublicSpanOutcome::Cancelled,
            SpanOutcome::Abandoned => PublicSpanOutcome::Abandoned,
            SpanOutcome::Denied => PublicSpanOutcome::Denied,
        },
    }
}

fn public_span_attributes(span: SpanAttributes) -> PublicSpanAttributes {
    PublicSpanAttributes {
        span_id: span.span_id,
        attributes: public_attributes(span.attributes),
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
    let operation = Some(match request.operation {
        EntityInvocationDescriptor::Tool(tool) => {
            PublicEntityInvocationOperation::Tool(PublicToolInvocationOperation {
                command_path: tool.command_path,
                has_stdin: tool.has_stdin,
                has_stdout: tool.has_stdout,
                declares_stdout: tool.declares_stdout,
                has_stderr: tool.has_stderr,
                declares_stderr: tool.declares_stderr,
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

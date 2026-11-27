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

use crate::durable_host::durable_session::StreamSession;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::rpc::RpcDemand;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use golem_common::model::AgentFingerprint;
use golem_common::model::NamedRetryPolicy;
use golem_common::model::OplogIndex;
use golem_common::model::OwnedAgentId;
use golem_common::model::OwnedInvocationId;
use golem_common::model::RetryProperties;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::type_id::TypeId;
use golem_common::schema::agent::AgentTypeSchema;
use golem_common::schema::schema_value::SchemaValue;
use golem_schema::schema::SchemaGraph;
use golem_schema::schema::SchemaType;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;
use std::time::Duration;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;

/// Host-side resource table entry backing the `golem:agent/host.wasm-rpc` resource.
pub struct WasmRpcEntry {
    pub payload: Box<WasmRpcEntryPayload>,
}

/// Host-side resource table entry backing the `golem:agent/host.invocation` resource: the
/// handle of an accepted invocation, addressed by its ID only.
pub struct InvocationEntry {
    pub id: OwnedInvocationId,
}

/// The agent type that a client of an ephemeral agent type invokes. Each invocation runs on a
/// new one-shot instance, whose ID derives from the invocation's owner key.
#[derive(Clone, Debug)]
pub struct EphemeralRpcTarget {
    pub type_id: TypeId,
    pub component_id: ComponentId,
    pub constructor: ParsedAgentKey,
}

#[derive(Clone)]
struct PreparedRpcInvocation {
    logical_remote_agent_id: OwnedAgentId,
    ephemeral: Option<EphemeralRpcTarget>,
    connection_span_id: SpanId,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    method_name: String,
    input_value: SchemaValue,
    streaming: bool,
    remote_agent_type: Arc<AgentTypeSchema>,
    /// The component that implements the remote agent type.
    remote_component_id: ComponentId,
}

type RpcTaskResult = Result<Result<SchemaValue, InternalRpcError>, Error>;

struct RpcTask {
    task: AbortOnDropJoinHandle<RpcTaskResult>,
    revoker: Option<crate::worker::suspension::RpcActivityRevoker>,
}

struct TaskRetryParams<Ctx: WorkerCtx> {
    environment_state_service: Arc<dyn EnvironmentStateService>,
    environment_id: EnvironmentId,
    default_retry_policy: NamedRetryPolicy,
    agent_config_retry_policies: Vec<NamedRetryPolicy>,
    runtime_retry_policy_mutations: std::collections::BTreeMap<String, Option<NamedRetryPolicy>>,
    retry_properties: RetryProperties,
    max_in_function_retry_delay: Duration,
    worker: Arc<crate::worker::Worker<Ctx>>,
    retry_point: OplogIndex,
    /// The `Start` of the RPC call that retries.
    retrying_start_index: OplogIndex,
    entity_parent_start_index: Option<OplogIndex>,
    invocation_id: Option<golem_common::model::InvocationId>,
    execution_status: Arc<std::sync::RwLock<crate::model::ExecutionStatus>>,
}

enum RpcTaskError {
    Rpc(InternalRpcError),
    Host(Error),
}

#[derive(Clone)]
struct DurableStreamingTaskParams {
    streams: StreamSession,
    input: golem_schema::proto::golem::schema::SchemaValue,
    input_mappings: Vec<golem_api_grpc::proto::golem::worker::DurableStreamMapping>,
    expected_callee_fingerprint: AgentFingerprint,
    attempt_id: uuid::Uuid,
    origin_invocation: golem_api_grpc::proto::golem::worker::StreamInvocationIdentity,
    output_graph: Arc<SchemaGraph>,
    output_root: SchemaType,
}

pub struct WasmRpcEntryPayload {
    pub remote_agent_id: OwnedAgentId,
    /// Set for a client of an ephemeral agent type. `remote_agent_id` is then only the logical
    /// ID of the client, which addresses no agent: each invocation runs at its own ID.
    pub ephemeral: Option<EphemeralRpcTarget>,
    pub span_id: SpanId,
    /// Absent for snapshot-created resources whose span opening was not persisted.
    pub creation_index: Option<OplogIndex>,
    pub target_activation: WasmRpcTargetActivation,
    /// Cached remote agent type, used to resolve per-method input/output
    /// schemas for the in-process [`SchemaValue`] / [`golem_common::schema::TypedSchemaValue`]
    /// flow. Sourced from the durable `get_agent_type` lookup performed in
    /// [`HostWasmRpc::new`](crate::preview2::golem::agent::host::HostWasmRpc::new), so it is consistent across live execution and
    /// replay.
    pub remote_agent_type: Arc<AgentTypeSchema>,
    /// The component that implements the remote agent type.
    pub remote_component_id: ComponentId,
    pub remote_method_streams: Arc<BTreeMap<String, bool>>,
    pub remote_owner: AgentOwnerPattern,
}

pub enum WasmRpcTargetActivation {
    DeferredEphemeral {
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
    },
    Activated {
        #[allow(dead_code)]
        demand: Box<dyn RpcDemand>,
        target_fingerprint: AgentFingerprint,
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
    },
    ReplayPending {
        target_fingerprint: AgentFingerprint,
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
    },
}

#[derive(Clone)]
struct RpcTargetActivation {
    target_fingerprint: AgentFingerprint,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
}

impl Debug for WasmRpcEntryPayload {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmRpcEntryPayload")
            .field("remote_agent_id", &self.remote_agent_id)
            .finish()
    }
}

mod call;
mod create;
mod denial;
mod ephemeral;
mod execution;
mod host;
mod invocation;
mod invoke_and_await;
mod remote_call;
mod schedule;
mod spans;
mod target;
mod trigger;
mod values;

use call::*;
use ephemeral::*;
use execution::*;
use remote_call::*;
use spans::*;
use target::*;
use values::*;

pub use target::construct_wasm_rpc_resource;
#[cfg(test)]
mod tests;

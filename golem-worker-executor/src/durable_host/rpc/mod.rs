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

use crate::durable_host::concurrent::Cancellable;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::durable_session::StreamSession;
use crate::preview2::golem::agent::host::InvocationMetadata;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::rpc::RpcDemand;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use golem_common::model::AgentFingerprint;
use golem_common::model::IdempotencyKey;
use golem_common::model::NamedRetryPolicy;
use golem_common::model::OplogIndex;
use golem_common::model::OwnedAgentId;
use golem_common::model::RetryProperties;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::card::PermissionTarget;
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::HostRequestGolemRpcInvoke;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvokeAndAwaitResult;
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
    pub payload: Box<dyn std::any::Any + Send + Sync>,
}

/// Type-erased payload of a [`FutureInvokeResultEntry`] that can be polled for readiness.
#[async_trait::async_trait]
pub trait SubscribeAny: std::any::Any {
    async fn ready(&mut self);
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Host-side resource table entry backing the `golem:agent/host.future-invoke-result` resource.
pub struct FutureInvokeResultEntry {
    pub payload: Box<dyn SubscribeAny + Send + Sync>,
    /// Tracks child Pollable rep indices created by `subscribe()`.
    /// Used to defer parent deletion until all children are dropped,
    /// because JS GC does not guarantee LIFO drop order.
    pub child_pollables: Vec<u32>,
    /// Set to `true` when the guest drops the parent while children still exist.
    /// The parent entry stays alive until the last child pollable is dropped.
    pub drop_pending: bool,
}

/// Host-side resource table entry backing the `golem:agent/host.cancellation-token` resource.
pub struct CancellationTokenEntry {
    pub schedule_id: Vec<u8>, // ScheduleId is defined locally in the worker-executor, so store a serialized version here
    pub metadata: InvocationMetadata,
    pub owner: OwnedAgentId,
    pub recipient: OwnedAgentId,
}

struct OutboundRpcDenial {
    error: RpcError,
    activation_decision: Option<IdempotencyKey>,
}

enum RpcTargetAdmission {
    Recorded,
    LiveOnly(Box<PermissionTarget>),
}

#[derive(Clone)]
struct PreparedRpcInvocation {
    logical_remote_agent_id: OwnedAgentId,
    ephemeral_logical_agent_id: Option<ParsedAgentId>,
    connection_span_id: SpanId,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    method_name: String,
    input_value: SchemaValue,
    streaming: bool,
    remote_agent_type: Arc<AgentTypeSchema>,
}

type FutureInvokeTaskResult = Result<Result<SchemaValue, InternalRpcError>, Error>;
type FutureInvokeTaskHandle = Arc<tokio::sync::Mutex<RpcTask>>;

struct RpcTask {
    task: AbortOnDropJoinHandle<FutureInvokeTaskResult>,
    revoker: Option<crate::worker::suspension::RpcActivityRevoker>,
}

type FutureInvokeGetResult = Result<Result<SchemaValue, RpcError>, Error>;
type FutureInvokeCallHandle = DurableCallSession<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>;

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
    entity_parent_start_index: Option<OplogIndex>,
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
    pub ephemeral_logical_agent_id: Option<ParsedAgentId>,
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
    pub remote_method_streams: Arc<BTreeMap<String, bool>>,
    pub remote_component_revision: ComponentRevision,
    pub remote_owner: AgentOwnerPattern,
}

pub enum WasmRpcTargetActivation {
    DeferredEphemeral {
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
    },
    DeferredDurable {
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
    target_fingerprint: Option<AgentFingerprint>,
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

#[allow(clippy::large_enum_variant)]
enum FutureInvokeResultState {
    /// The eager host-call `Start` has been written (live) or claimed from the oplog (replay). The
    /// single [`FutureInvokeCallHandle`] owns both halves of the durable call: `get` finishes it
    /// with an `End`, `cancel` / `drop` with a `Cancelled`. It is always `take`n out of the state
    /// before a terminal runs, so a live handle is never dropped while still owned here (which would
    /// spuriously enqueue a `Cancelled`).
    ///
    /// `task` is `Some` only on the live path — the background RPC task whose result feeds the `End`.
    /// On the replay path it is `None`: `get` replays the recorded `End`. If replay instead finds an
    /// incomplete `Start` (the worker crashed after the eager `Start` but before its terminal), `get`
    /// re-dispatches the read-only RPC — using `remote_agent_id`, `request`, and `env` — and
    /// completes the existing `Start`.
    ///
    /// `cancel_token` is the side-channel for cancelling an *in-flight* `get`: `get` takes the
    /// handle (and task) out of this state before awaiting, so a concurrent `cancel` finds
    /// `handle: None` and cannot record the `Cancelled` itself. Instead it triggers this token;
    /// the in-flight `get` races its await against it, and on cancellation aborts the RPC task,
    /// best-effort cancels the remote invocation, and records the `Cancelled` (with a partial
    /// "Invocation cancelled" response so replay deterministically delivers the same result).
    Active {
        handle: Option<FutureInvokeCallHandle>,
        task: Option<FutureInvokeTaskHandle>,
        streaming: Option<DurableStreamingTaskParams>,
        request: HostRequestGolemRpcInvoke,
        remote_agent_id: OwnedAgentId,
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
        deferred_activation: Option<RpcTargetActivation>,
        span_id: SpanId,
        cancel_token: tokio_util::sync::CancellationToken,
    },
    /// A deterministic validation failure or an already recorded local denial.
    /// `span_id` retains an opening only when its owner has not already closed it.
    Baked { result: FutureInvokeTaskResult },
    /// The future was cancelled: its host call recorded a `Cancelled` and its span was finished.
    /// `get` returns a cancellation error without touching the oplog.
    Cancelled,
    /// `get` already produced the result and finished the span; a second `get` traps.
    Consumed,
}

impl Debug for FutureInvokeResultState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active { .. } => write!(f, "Active"),
            Self::Baked { .. } => write!(f, "Baked"),
            Self::Cancelled => write!(f, "Cancelled"),
            Self::Consumed => write!(f, "Consumed"),
        }
    }
}

mod denial;
mod execution;
mod future;
mod host;
mod schedule;
mod spans;
mod target;
mod values;

use denial::*;
use execution::*;
use spans::*;
use target::*;
use values::*;

pub use target::construct_wasm_rpc_resource;
#[cfg(test)]
mod tests;

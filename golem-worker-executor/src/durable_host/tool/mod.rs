// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

//! Host implementation of `golem:tool/host@0.1.0`.
//!
//! The interface is wired into the linker for every agent component. Tool
//! discovery is implemented as a durable environment-state read. Tool
//! invocation authorization is enforced here for every admission, including
//! middleware underlying calls, before the invocation backend, which is
//! implemented separately on top of sidecar instances.

mod accepted;
mod call;
mod command;
mod discovery;
mod endpoints;
mod execution;
mod host;
mod native;
mod response;
mod sidecar;

#[cfg(test)]
use accepted::{publish_no_body_terminals, recorded_tool_body_is_skipped};
#[cfg(test)]
use call::{caller_tool_owner, describe_tool_command};
#[cfg(test)]
use command::option_args;
#[cfg(test)]
use discovery::{merge_discovered_tools, terminal_tool_discovery_error};
#[cfg(test)]
use endpoints::cleanup_tool_endpoints;
#[cfg(test)]
use execution::capable_result_await_cohort;
#[cfg(test)]
use native::{
    await_native_entity_body, native_output_runtime_teardown_probe, select_native_body_result,
};
#[cfg(test)]
use response::output_limit_error;

use accepted::execute_accepted_tool_call;
use accepted::rollback_live_attachment_admission_rejection;
use call::dispatch_tool_attempt;
use call::dispatch_tool_call;
pub(crate) use call::invoke_external_tool;
use call::invoke_tool_terminal;
use call::read_tool_attempt;
use call::register_tool_admission;
use call::tool_admission_activity;
use call::tool_rpc_for_current_owner;
use call::tool_rpc_resource;
use command::resolve_tool_command;
use command::validate_stream_attachments;
use discovery::classify_tool_discovery_error;
use endpoints::admit_tool_response_secret_holds;
use endpoints::cleanup_failed_tool_dispatch;
use endpoints::close_stdin;
use endpoints::create_native_underlying_stdin;
use endpoints::create_output_attachment;
use endpoints::create_underlying_stdin;
use endpoints::materialize_output_writer;
use endpoints::native_output_handles;
use endpoints::reject_output;
use execution::await_tool_invoke_plans;
pub use execution::fence_tool_operations_for_jump;
use execution::get_tool_invoke_results;
pub(crate) use execution::prepare_tool_parent_end;
pub(crate) use execution::prepare_tool_parent_end_owned;
pub(crate) use execution::settle_tool_children;
pub(crate) use execution::settle_tool_children_owned;
use execution::spawn_tool_execution;
use native::guest_body_accepts_admission_rejection;
use native::guest_trap_output_failure;
use native::invoke_native_tool;
use native::resource_exhausted_after_body;
use native::resource_exhausted_terminal;
use native::resource_exhausted_without_body;
use response::decode_guest_tool_error;
use response::decode_tool_terminal;
use response::decode_typed_tool_value;
use response::encode_tool_operation_terminal;
use response::encode_typed_tool_value;
use response::first_output_limit_error;
use response::project_tool_response;
use response::project_tool_unit;
use response::publish_output_completions;
use response::terminal_from_response;
use response::validate_declared_tool_error;
use response::validate_declared_tool_result;
use response::validate_native_tool_output;

pub(crate) mod attachment;
pub(crate) mod boundary;
pub(crate) mod durability;
pub(crate) mod input;
mod mcp;
pub(crate) mod operation;
mod streams;

pub use attachment::{
    ToolAttachmentMetadata, ToolAttachmentModeMetadata, ToolAttachmentTerminalMetadata,
};
pub use operation::{
    ToolBodyAdmissionMetadata, ToolOperationLaneMetadata, ToolOperationMetadata,
    ToolOperationSetMetadata, ToolOperationWinnerMetadata, ToolOwnerFailureMetadata,
};

use crate::durable_host::authorization::targets::tool_target;
use crate::durable_host::concurrent::{
    CallReplayOutcome, Cancellable, DurableCallSession, NotCancellable,
    authorize_live_permissions_at_serialized_access, reserve_invocation_id_before_await,
};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
use crate::durable_host::durable_session::{
    DurableByteInputProducer, DurableInputEndpoint, DurableInputProducer, DurableToolStdinProducer,
    StreamSession,
};
use crate::durable_host::durable_stream::SessionError;
use crate::durable_host::entity::{
    EntityInvocationDurability, EntityInvocationKeyContext, EntitySettlementContext,
    IncompleteLiveRepairBeforeBody, RecordedEntityTerminal, ToolInvocationReplayOutcome,
    encode_tool_terminal, record_tool_rejection_access,
};
use crate::durable_host::secrets::secret_hold_targets_for_value;
use crate::durable_host::stream_transport::{
    LiveStreamEndpoint, accounted_byte_output_stream_pair,
};
use crate::durable_host::tool::attachment::{
    AttachmentConsumer, AttachmentController, AttachmentMemory, AttachmentObserver,
    AttachmentProducer, AttachmentStreamProducer, attachment_pair, discard_producer,
};
use crate::durable_host::{
    DurabilityHost, DurableWorkerCtx, InternalRetryResult, LiveAuthorizationPermit,
};
use crate::native_tool::{NativeToolInvocation, NativeToolKey, NativeToolOutput, NativeToolStdin};
use crate::preview2::golem::tool::host::{
    ByteStreamCloseCause, ByteStreamFailure, Host, HostFutureInvokeResult,
    HostFutureInvokeResultWithStore, HostToolOutput, HostToolRpc, HostToolRpcWithStore,
    HostToolStdin, HostToolStdinClosed, HostToolStdinClosedWithStore, HostToolStdinWriter,
    HostToolStdinWriterWithStore, HostWithStore, InvocationResult,
    RegisteredTool as WitRegisteredTool, StreamWriteError, ToolRpcError, TypedSchemaValue,
};
use crate::preview2::golem::tool::streams::{
    Host as HostToolStreams, HostToolOutputWriter, HostToolOutputWriterWithStore,
};
use crate::preview2::golem::tool::underlying::{
    Host as HostUnderlying, HostUnderlyingInvokeResult, HostUnderlyingInvokeResultWithStore,
    HostUnderlyingTool, HostUnderlyingToolWithStore, UnderlyingError,
};
use crate::preview2::tool_guest::exports::golem::tool::guest as tool_guest_exports;
use crate::preview2::tool_middleware_guest::exports::golem::tool::tool_middleware_guest as tool_middleware_guest_exports;
use crate::services::environment_state::{
    ToolActivationOutcome, ToolDiscoveryError, get_accessible_tool_from_deployment,
    get_accessible_tools_from_deployment,
};
use crate::services::shutdown::join_or_park_at_runtime_shutdown;
use crate::services::{HasActiveAgents, HasNativeToolCatalog, HasWorker};
use crate::worker::entity_invocation::{EntityBodyFinalization, EntityBodyOutcome};
use crate::worker::entity_invocation::{RetainedEntityStore, RetainedNativeContext};
use crate::worker::instance::EntityInvocationBody;
use crate::worker::invocation::{
    GuestCallSettlementError, InvokeResult, finish_invocation_and_get_fuel_consumption,
    permanent_tool_guest_trap, prepare_guest_call, run_guest_call_settled,
};
use crate::workerctx::WorkerCtx;
use crate::workerctx::WorkerCtxExecutable;
use anyhow::{Context, anyhow};
use golem_common::model::OwnedAgentId;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::{Principal, ResolvedOwnerContext};
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::ToolOwnerPattern;
use golem_common::model::card::{ToolArgPattern, ToolIdentifier, ToolInvocationValue};
use golem_common::model::component::ComponentName;
use golem_common::model::durable_stream::{
    DurableStreamHandle, SessionStreamRole, StreamCancelReason,
};
use golem_common::model::entity::{
    AgentEntity, EntityCallMode, EntityInvocationDescriptor, EntityInvocationPlanReference,
    InvocationExecutionMode, NamedToolErrorSchema, ToolInputDecodeFailure,
    ToolInvocationDescriptor, ToolOutputContract,
};
use golem_common::model::environment::EnvironmentName;
use golem_common::model::oplog::host_functions::{
    GolemToolGetAllTools, GolemToolGetTool, GolemToolResponseSecretHoldAdmission,
};
use golem_common::model::oplog::payload::types::{
    SerializableCustomToolError, SerializableEntityBodyExecution,
    SerializableToolDiscoverySnapshot, SerializableToolError, SerializableToolInvocationResult,
    SerializableToolOperationTerminal, SerializableToolResultValue, SerializableToolRpcError,
    SerializableToolStructuredResult,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemToolGetTool, HostRequestGolemToolInvocationRejected,
    HostRequestGolemToolResponseSecretHoldAdmission, HostRequestNoInput,
    HostResponseEntityInvocation, HostResponseGolemToolResponseSecretHoldAdmission,
    HostResponseGolemToolTool, HostResponseGolemToolTools,
};
use golem_common::model::tool::{
    ToolActivationSnapshot, ToolBindingOwner, ToolDeploymentState, ToolInvocationInput,
    ToolInvocationOutput, ToolName,
};
use golem_common::schema::render::cli_text::value_to_cli_text_unredacted;
use golem_common::schema::tool::DiscoveredTool;
use golem_common::schema::tool::canonical::CanonicalSurfaceRef;
use golem_common::schema::tool::wit::wire::{Host as HostToolCommon, Tool as WitTool, ToolError};
use golem_common::schema::tool::{FlagShape, OptionShape, OptionSpec, Tool};
use golem_common::schema::validation::{is_equivalent_cross_graph, validate_graph, validate_value};
use golem_common::schema::wit::{
    decode_graph, decode_value_with, encode_graph, encode_value_with_streams,
};
use golem_common::schema::{
    FromSchema, SchemaType, SchemaValue, TypedSchemaValue as ModelTypedSchemaValue,
};
use golem_common::schema::{IntoTypedSchemaValue, SchemaGraph};
use golem_schema::schema::SchemaValueStream;
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::Weak;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context as TaskContext, Poll};
use tokio::sync::{Notify, mpsc, oneshot};
use wasmtime::component::{
    Accessor, AccessorTask, HasData, HasSelf, Linker, Resource, Source, StreamConsumer,
    StreamReader, StreamResult,
};
use wasmtime::{AsContextMut, Store, StoreContextMut};

struct ToolCommonHost<Ctx: WorkerCtx>(std::marker::PhantomData<fn() -> Ctx>);

impl<Ctx: WorkerCtx> HasData for ToolCommonHost<Ctx> {
    type Data<'a> = &'a mut DurableWorkerCtx<Ctx>;
}

pub fn add_common_to_linker<Ctx: WorkerCtx>(
    linker: &mut Linker<Ctx>,
    get: fn(&mut Ctx) -> &mut DurableWorkerCtx<Ctx>,
) -> wasmtime::Result<()> {
    golem_common::schema::tool::wit::wire::add_to_linker::<_, ToolCommonHost<Ctx>>(linker, get)?;
    crate::preview2::golem::tool::streams::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        linker, get,
    )?;
    crate::preview2::golem::tool::underlying::add_to_linker::<_, ToolCommonHost<Ctx>>(linker, get)
}

/// Host-side resource table entry backing the `golem:tool/host.tool-rpc`
/// resource.
#[derive(Clone)]
pub struct ToolRpcEntry {
    tool_name: ToolName,
    owner: ToolRpcOwnerContext,
}

#[derive(Clone)]
struct ToolRpcOwnerContext {
    owner_id: OwnedAgentId,
}

/// Host-side resource table entry backing the
/// `golem:tool/host.future-invoke-result` resource.
pub struct FutureInvokeResultEntry {
    state: FutureToolInvokeState,
}

/// Invocation-scoped capability for the next layer of a recorded middleware chain.
#[derive(Clone)]
pub struct UnderlyingToolEntry {
    position: crate::durable_host::entity::ResolvedEntityInvocationPosition,
    revoked: Arc<AtomicBool>,
}

pub struct UnderlyingInvokeResultEntry {
    state: FutureToolInvokeState,
    response_edge: Option<boundary::SelectedToolProjectionEdge>,
}

pub struct ToolStdinWriterEntry {
    producer: AttachmentProducer,
}

pub struct ToolStdinEntry {
    consumer: AttachmentConsumer,
}

/// The stdin that a tool call starts with.
enum ToolCallStdin {
    /// Bytes that arrive through an attachment: a guest's stream or a guest's writer.
    Attachment(Resource<ToolStdinEntry>),
    /// The durable stdin of a call that a client started. See [`ToolBodyStdin::Durable`].
    Durable(Box<DurableInputEndpoint>),
}

/// The stdin that the body of a tool call reads.
enum ToolBodyStdin {
    Attachment(ToolStdinEntry),
    /// The durable stdin of a call that a client started, read by a component body itself.
    /// What the body's guest takes is its own recorded observation, so a replay hands it over
    /// at the same place among the body's other recorded calls. There is no attachment for it:
    /// nothing buffers it ahead of the body, and no host task reads it for the body.
    Durable(Box<DurableInputEndpoint>),
}

pub struct ToolStdinClosedEntry {
    observer: AttachmentObserver,
    next_operation_ordinal: u64,
}

struct ToolStdinStreamItem {
    item: Result<Vec<u8>, ByteStreamFailure>,
    acknowledged: oneshot::Sender<()>,
}

struct ToolStdinStreamConsumer {
    items: Option<mpsc::UnboundedSender<ToolStdinStreamItem>>,
    pending_acknowledgement: Option<oneshot::Receiver<()>>,
    attachment_closed: Pin<Box<dyn Future<Output = ByteStreamCloseCause> + Send>>,
}

struct UnderlyingToolStdinStreamConsumer {
    items: Option<mpsc::UnboundedSender<ToolStdinStreamItem>>,
    pending_acknowledgement: Option<oneshot::Receiver<()>>,
    attachment_closed: Pin<Box<dyn Future<Output = ByteStreamCloseCause> + Send>>,
    max_chunk_bytes: usize,
}

struct ToolStdinStreamPumpTask<Ctx> {
    producer: AttachmentProducer,
    items: mpsc::UnboundedReceiver<ToolStdinStreamItem>,
    /// The reason of a cancel by the producer of a client's stdin, set before the source ends.
    source_cancel: Option<Arc<OnceLock<StreamCancelReason>>>,
    _ctx: std::marker::PhantomData<fn() -> Ctx>,
}

pub struct ToolOutputEntry {
    producer: Option<AttachmentProducer>,
    completion_only: bool,
}

pub struct ToolOutputWriterEntry {
    producer: AttachmentProducer,
    completion_only: bool,
}

pub(crate) type ToolInvokeResponse =
    Result<SerializableToolInvocationResult, SerializableToolRpcError>;

enum FutureToolInvokeState {
    Ready(Box<ToolInvokeResponse>),
    Active(Arc<ToolExecution>),
}

enum FutureToolInvokeGet {
    Ready(Box<ToolInvokeResponse>),
    Failed(String),
    Active(Arc<ToolExecution>),
}

struct ToolExecutionState {
    result: Option<ToolInvokeResponse>,
    failure: Option<String>,
}

struct ToolExecution {
    parent: crate::worker::owner_lane::OwnerInvocationId,
    invocation: golem_common::model::entity::EntityInvocationId,
    start: golem_common::model::oplog::OplogIndex,
    filesystem: golem_common::model::entity::FilesystemCapability,
    operation: operation::OwnerToolOperation,
    cancellable: bool,
    state: Mutex<ToolExecutionState>,
    changed: Notify,
    get_active: AtomicBool,
    cancel: tokio_util::sync::CancellationToken,
    producer: Option<std::sync::Weak<crate::worker::suspension::ExternalActivity>>,
}

struct ToolExecutionGetGuard(Arc<ToolExecution>);

struct FailedRetainedEntityResources {
    resources: crate::worker::entity_invocation::EntityInvocationResources,
    parent_end_attempted: bool,
}

struct ResolvedToolCommand {
    command_index: usize,
    args: Vec<ToolArgPattern<ToolInvocationValue>>,
    stdin_required: Option<bool>,
    stdout_required: Option<bool>,
    stderr_required: Option<bool>,
}

struct PreparedToolCall {
    stdin: Option<ToolCallStdin>,
    permit: LiveAuthorizationPermit,
    operation: operation::ProvisionalOwnerToolOperation,
    plan: EntityInvocationPlanReference,
}

enum ToolCallPreparation {
    Ready(PreparedToolCall),
    Rejected {
        request: Box<HostRequestGolemToolInvocationRejected>,
        stdin: Option<ToolCallStdin>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolAttachmentCounterparty {
    Guest,
    SessionJournal,
}

struct AcceptedToolCall {
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    stdin: Option<ToolCallStdin>,
    deferred_admission_inserted: bool,
    incapable_lane_ticket: Option<crate::worker::owner_lane::OwnerInvocationTicket>,
    attachment_counterparty: ToolAttachmentCounterparty,
}

/// Caller protocol handles, not a second guest context. Accepted work retains these before
/// returning to the guest so destruction of its Store cannot discard a child's settlement.
struct ToolCaller<Ctx: WorkerCtx> {
    settlement: EntitySettlementContext<Ctx>,
    primary: Arc<crate::worker::Worker<Ctx>>,
    execution: Arc<crate::worker::instance::OwnerExecution>,
    metadata: Arc<golem_service_base::model::component::Component>,
    revision: golem_common::model::component::ComponentRevision,
    publication_failure: durability::PublicationFailureContext<Ctx>,
    tracker: crate::durable_host::tail_work::TailWorkTracker,
    counter: Arc<std::sync::atomic::AtomicUsize>,
    mode: Option<InvocationExecutionMode>,
    local_live: Arc<AtomicBool>,
    suppressed: bool,
}

impl<Ctx: WorkerCtx> ToolCaller<Ctx> {
    fn capture<U: Send + 'static>(accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>) -> Self {
        let settlement = EntitySettlementContext::capture(accessor, accessor.getter());
        accessor.with(|mut access| {
            let ctx = access.get();
            Self {
                settlement,
                primary: ctx.public_state.worker(),
                execution: ctx.owner_execution.clone(),
                metadata: Arc::new(ctx.owner_component_metadata().clone()),
                revision: ctx.component_metadata().revision,
                publication_failure: durability::PublicationFailureContext::new(ctx),
                tracker: ctx.tail_work_tracker(),
                counter: ctx.state.live_host_call_counter(),
                mode: ctx.entity_invocation_scope().map(|scope| scope.mode()),
                local_live: ctx.state.local_live_tail(),
                suppressed: ctx.state.durability_is_suppressed(),
            }
        })
    }

    fn is_live(&self) -> bool {
        crate::durable_host::store_is_live(
            self.mode,
            self.local_live.load(Ordering::Acquire),
            self.settlement.replay.is_live_published(),
        )
    }
}

struct OwnedToolEndpoints {
    stdin: Option<ToolBodyStdin>,
    stdout: Option<ToolOutputWriterEntry>,
    stderr: Option<ToolOutputWriterEntry>,
}

enum ToolCallDispatch {
    Rejected {
        response: Box<ToolInvokeResponse>,
        stdin: Option<ToolCallStdin>,
    },
    Accepted(Box<AcceptedToolCall>),
}

struct ToolInvocationAttempt {
    target: ToolCallTarget,
    input: Result<ModelTypedSchemaValue, ToolInputDecodeFailure>,
    rejection: Option<SerializableToolRpcError>,
    key_context: EntityInvocationKeyContext,
    parent: crate::worker::owner_lane::OwnerInvocationId,
    attempt_ordinal: u64,
}

#[derive(Clone)]
enum ToolCallTarget {
    Ambient(ToolRpcEntry),
    Underlying(UnderlyingToolEntry),
}

struct ToolSidecarInvocation {
    tool_name: String,
    position: crate::durable_host::entity::ResolvedEntityInvocationPosition,
    command_path: Vec<String>,
    input: ModelTypedSchemaValue,
    stdin: Option<ToolBodyStdin>,
    stdout: Option<ToolOutputWriterEntry>,
    stderr: Option<ToolOutputWriterEntry>,
    principal: Principal,
    output_contract: ToolOutputContract,
    result_streams: crate::durable_host::durable_session::StreamSession,
}

struct ToolSidecarBody {
    invocation: ToolSidecarInvocation,
    operation: operation::OwnerToolOperation,
    cancellation: Option<tokio_util::sync::CancellationToken>,
}

struct EntityCancellationEpochTask(tokio::task::JoinHandle<()>);

struct UnderlyingToolRevocationGuard(UnderlyingToolEntry);

#[cfg(test)]
struct SkippedToolAttachmentEndpoints {
    stdin: Option<ToolStdinEntry>,
    stdout: Option<ToolOutputWriterEntry>,
    stderr: Option<ToolOutputWriterEntry>,
}

struct DeferredAdmissionCleanup {
    table: Arc<operation::DeferredAdmissionTable>,
    parent: crate::worker::owner_lane::OwnerInvocationId,
    start: Option<golem_common::model::oplog::OplogIndex>,
}

struct NativeToolTask {
    activation: Arc<ToolActivationSnapshot>,
    tool_name: ToolName,
    command_path: Vec<String>,
    input: ModelTypedSchemaValue,
    stdin: bool,
    stdout: bool,
    stderr: bool,
    principal: Principal,
    result: oneshot::Sender<ToolInvokeResponse>,
}

#[cfg(test)]
mod tests;

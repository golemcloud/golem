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
//! invocation authorization is enforced here before the invocation backend,
//! which is implemented separately on top of sidecar instances.

pub(crate) mod attachment;
pub(crate) mod boundary;
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
    authorize_live_permissions_at_serialized_access,
};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
use crate::durable_host::durable_session::{
    DurableByteInputProducer, DurableInputEndpoint, DurableInputProducer, strip_typed_streams,
};
use crate::durable_host::entity::{
    EntityInvocationDurability, EntityInvocationKeyContext, IncompleteLiveRepairBeforeBody,
    RecordedEntityTerminal, ToolInvocationReplayOutcome, encode_tool_terminal,
    record_tool_rejection_access,
};
use crate::durable_host::secrets::secret_hold_targets_for_value;
use crate::durable_host::stream_transport::{LiveStreamEndpoint, byte_output_stream_pair};
use crate::durable_host::tool::attachment::{
    AttachmentConsumer, AttachmentController, AttachmentMemory, AttachmentObserver,
    AttachmentProducer, AttachmentStreamProducer, attachment_pair, discard_producer,
};
use crate::durable_host::{
    DurabilityHost, DurableWorkerCtx, InternalRetryResult, LiveAuthorizationPermit,
};
use crate::native_tool::{NativeToolInvocation, NativeToolKey, NativeToolStdin, NativeToolStdout};
use crate::preview2::golem::tool::host::{
    ByteStreamCloseCause, ByteStreamFailure, Host, HostFutureInvokeResult,
    HostFutureInvokeResultWithStore, HostToolRpc, HostToolRpcWithStore, HostToolStdin,
    HostToolStdinClosed, HostToolStdinClosedWithStore, HostToolStdinWriter,
    HostToolStdinWriterWithStore, HostToolStdout, HostWithStore, InvocationResult,
    RegisteredTool as WitRegisteredTool, StreamWriteError, ToolRpcError, TypedSchemaValue,
};
use crate::preview2::golem::tool::streams::{
    Host as HostToolStreams, HostToolStdoutWriter, HostToolStdoutWriterWithStore,
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
use crate::services::{HasActiveAgents, HasNativeToolCatalog, HasWorker};
use crate::worker::entity_invocation::{RetainedEntityStore, RetainedNativeContext};
use crate::worker::instance::EntityInvocationBody;
use crate::worker::invocation::{
    GuestCallSettlementError, InvokeResult, finish_invocation_and_get_fuel_consumption,
    prepare_guest_call, run_guest_call_settled,
};
use crate::workerctx::WorkerCtx;
use crate::workerctx::WorkerCtxExecutable;
use anyhow::{Context, anyhow};
use golem_common::model::OwnedAgentId;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::{AgentPrincipal, Principal, ResolvedOwnerContext};
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::ToolOwnerPattern;
use golem_common::model::card::{
    ToolArgPattern, ToolIdentifier, ToolValueLiteral, ToolValuePattern,
};
use golem_common::model::component::ComponentName;
use golem_common::model::durable_stream::SessionStreamRole;
use golem_common::model::entity::{
    AgentEntity, EntityCallMode, EntityInvocationDescriptor, EntityInvocationDescriptorIdentity,
    EntityInvocationPlanReference, EntityInvocationRequestIdentity, InvocationExecutionMode,
    NamedToolErrorSchema, ToolInputDecodeFailure, ToolInvocationClaimIdentity,
    ToolInvocationDescriptor, ToolInvocationDescriptorIdentity, ToolInvocationRejectedIdentity,
    ToolOutputContract,
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
use golem_common::schema::tool::{FlagShape, OptionShape, OptionSpec, Repetition, Tool};
use golem_common::schema::validation::{is_equivalent_cross_graph, validate_value};
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
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
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

impl UnderlyingToolEntry {
    pub(crate) fn new(
        position: crate::durable_host::entity::ResolvedEntityInvocationPosition,
    ) -> Self {
        Self {
            position,
            revoked: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }

    fn next_definition(&self) -> anyhow::Result<&Tool> {
        match self.position.layer() {
            golem_common::model::entity::EntityInvocationPlanLayer::Middleware {
                next_effective_definition,
                ..
            } => Ok(next_effective_definition),
            golem_common::model::entity::EntityInvocationPlanLayer::Tool { .. } => {
                Err(anyhow!("the terminal tool layer has no underlying tool"))
            }
        }
    }

    fn admit(&self) -> anyhow::Result<()> {
        (!self.revoked.load(Ordering::Acquire))
            .then_some(())
            .ok_or_else(|| anyhow!("underlying-tool capability is no longer active"))
    }
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

impl ToolStdinEntry {
    pub(crate) fn controller(&self) -> AttachmentController {
        self.consumer.controller()
    }

    pub(crate) fn into_stream_producer(self) -> AttachmentStreamProducer {
        self.consumer.into_stream_producer()
    }

    fn into_native(self) -> NativeToolStdin {
        NativeToolStdin {
            consumer: self.consumer,
        }
    }
}

pub struct ToolStdinClosedEntry {
    observer: AttachmentObserver,
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

impl ToolStdinStreamConsumer {
    fn new(
        items: mpsc::UnboundedSender<ToolStdinStreamItem>,
        observer: AttachmentObserver,
    ) -> Self {
        Self {
            items: Some(items),
            pending_acknowledgement: None,
            attachment_closed: Box::pin(async move { observer.wait_terminal().await }),
        }
    }
}

impl<D> StreamConsumer<D> for ToolStdinStreamConsumer {
    type Item = Result<Vec<u8>, ByteStreamFailure>;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        mut store: StoreContextMut<'_, D>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.attachment_closed.as_mut().poll(cx).is_ready() {
            self.items.take();
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        if let Some(acknowledged) = &mut self.pending_acknowledgement {
            match Pin::new(acknowledged).poll(cx) {
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(())) => self.pending_acknowledgement = None,
                Poll::Ready(Err(_)) => {
                    self.items.take();
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }

        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }

        if source.remaining(store.as_context_mut()) == 0 {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let mut received = Vec::with_capacity(1);
        source.read(store.as_context_mut(), &mut received)?;
        let item = received
            .pop()
            .expect("a non-empty tool stdin source did not produce an item");
        let (acknowledged, acknowledgement) = oneshot::channel();
        let Some(items) = &self.items else {
            return Poll::Ready(Ok(StreamResult::Dropped));
        };
        if items
            .send(ToolStdinStreamItem { item, acknowledged })
            .is_err()
        {
            self.items.take();
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        self.pending_acknowledgement = Some(acknowledgement);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

struct UnderlyingToolStdinStreamConsumer {
    items: Option<mpsc::UnboundedSender<ToolStdinStreamItem>>,
    pending_acknowledgement: Option<oneshot::Receiver<()>>,
    attachment_closed: Pin<Box<dyn Future<Output = ByteStreamCloseCause> + Send>>,
    max_chunk_bytes: usize,
}

impl UnderlyingToolStdinStreamConsumer {
    fn new(
        items: mpsc::UnboundedSender<ToolStdinStreamItem>,
        observer: AttachmentObserver,
        max_chunk_bytes: usize,
    ) -> Self {
        Self {
            items: Some(items),
            pending_acknowledgement: None,
            attachment_closed: Box::pin(async move { observer.wait_terminal().await }),
            max_chunk_bytes,
        }
    }
}

impl<D> StreamConsumer<D> for UnderlyingToolStdinStreamConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        mut store: StoreContextMut<'_, D>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.attachment_closed.as_mut().poll(cx).is_ready() {
            self.items.take();
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        if let Some(acknowledged) = &mut self.pending_acknowledgement {
            match Pin::new(acknowledged).poll(cx) {
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(())) => self.pending_acknowledgement = None,
                Poll::Ready(Err(_)) => {
                    self.items.take();
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }

        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }

        let mut source = source.as_direct(store.as_context_mut());
        let remaining = source.remaining();
        if remaining.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let count = remaining.len().min(self.max_chunk_bytes.max(1));
        let received = remaining[..count].to_vec();
        source.mark_read(count);
        let (acknowledged, acknowledgement) = oneshot::channel();
        let Some(items) = &self.items else {
            return Poll::Ready(Ok(StreamResult::Dropped));
        };
        if items
            .send(ToolStdinStreamItem {
                item: Ok(received),
                acknowledged,
            })
            .is_err()
        {
            self.items.take();
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        self.pending_acknowledgement = Some(acknowledgement);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

struct ToolStdinStreamPumpTask<Ctx> {
    producer: AttachmentProducer,
    items: mpsc::UnboundedReceiver<ToolStdinStreamItem>,
    _ctx: std::marker::PhantomData<fn() -> Ctx>,
}

impl<Ctx: WorkerCtx, U: Send + 'static> AccessorTask<U, HasSelf<DurableWorkerCtx<Ctx>>>
    for ToolStdinStreamPumpTask<Ctx>
{
    async fn run(
        mut self,
        _accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    ) -> wasmtime::Result<()> {
        while let Some(ToolStdinStreamItem { item, acknowledged }) = self.items.recv().await {
            let terminal = item.is_err();
            let result = match item {
                Ok(bytes) if bytes.is_empty() => Ok(()),
                Ok(bytes) => self.producer.write(bytes).await,
                Err(reason) => self.producer.fail(reason),
            };
            if result.is_err() {
                return Ok(());
            }
            let _ = acknowledged.send(());
            if terminal {
                return Ok(());
            }
        }
        let _ = self.producer.finish();
        Ok(())
    }
}

pub struct ToolStdoutEntry {
    producer: Option<AttachmentProducer>,
    completion_only: bool,
}

impl ToolStdoutEntry {
    fn producer(&self) -> &AttachmentProducer {
        self.producer
            .as_ref()
            .expect("stdout target already consumed")
    }

    fn reject_unconfigured(&self) {
        let _ = self.producer().reject_unconfigured();
    }

    fn abandon_unconfigured(&self) {
        let _ = self.producer().abandon_unconfigured();
    }

    pub(crate) fn into_writer(mut self) -> ToolStdoutWriterEntry {
        ToolStdoutWriterEntry {
            producer: self
                .producer
                .take()
                .expect("stdout target already consumed"),
            completion_only: self.completion_only,
        }
    }
}

impl Drop for ToolStdoutEntry {
    fn drop(&mut self) {
        if let Some(producer) = &self.producer {
            let _ = producer.abandon_unconfigured();
        }
    }
}

pub struct ToolStdoutWriterEntry {
    producer: AttachmentProducer,
    completion_only: bool,
}

impl ToolStdoutWriterEntry {
    pub(crate) fn discard(memory: AttachmentMemory) -> Self {
        Self {
            producer: discard_producer(memory),
            completion_only: false,
        }
    }

    pub(crate) fn controller(&self) -> AttachmentController {
        self.producer.controller()
    }

    fn completion_only(&self) -> bool {
        self.completion_only
    }

    fn native_writer(&self) -> NativeToolStdout {
        NativeToolStdout {
            writer: self.producer.writer(),
        }
    }
}

pub(crate) type ToolInvokeResponse =
    Result<SerializableToolInvocationResult, SerializableToolRpcError>;

async fn admit_tool_response_secret_holds<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: ToolInvokeResponse,
) -> anyhow::Result<ToolInvokeResponse>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let (value, targets) = accessor.with(|mut access| {
        let ctx = access.get();
        let value = match &response {
            Ok(result) => result.result.as_deref(),
            Err(SerializableToolRpcError::RemoteToolError(error)) => match error.as_ref() {
                SerializableToolError::CustomError(value) => Some(&value.payload),
                _ => None,
            },
            _ => None,
        };
        match value {
            Some(value) => Ok::<_, WorkerExecutorError>((
                Some(value.clone()),
                secret_hold_targets_for_value(ctx, value.value())?,
            )),
            None => Ok((None, Vec::new())),
        }
    })?;
    if targets.is_empty() {
        return Ok(response);
    }

    let request = HostRequestGolemToolResponseSecretHoldAdmission {
        value: value.expect("secret hold targets require a response value"),
        targets: targets.clone(),
    };
    let admission =
        DurableCallSession::<GolemToolResponseSecretHoldAdmission, Cancellable>::invoke_access(
            accessor,
            accessor.getter(),
            request,
            DurableFunctionType::ReadLocal,
            async || {
                Ok::<_, anyhow::Error>(HostResponseGolemToolResponseSecretHoldAdmission {
                    admitted: authorize_live_permissions_at_serialized_access(
                        accessor,
                        accessor.getter(),
                        &targets,
                    )
                    .await?
                    .is_ok(),
                })
            },
        )
        .await?;
    if admission.admitted {
        Ok(response)
    } else {
        Ok(Err(SerializableToolRpcError::Denied(
            "permission denied".to_string(),
        )))
    }
}

enum FutureToolInvokeState {
    Ready(Box<ToolInvokeResponse>),
    Active(Arc<ToolExecution>),
}

enum FutureToolInvokeGet {
    Ready(Box<ToolInvokeResponse>),
    Failed(String),
    Active(Arc<ToolExecution>),
}

fn capable_result_await_cohort(
    plans: &[FutureToolInvokeGet],
    current_parent: &crate::worker::owner_lane::OwnerInvocationId,
) -> Option<Vec<golem_common::model::oplog::OplogIndex>> {
    let mut starts = Vec::new();
    for plan in plans {
        let FutureToolInvokeGet::Active(execution) = plan else {
            continue;
        };
        if execution.filesystem != golem_common::model::entity::FilesystemCapability::Capable {
            continue;
        }
        if &execution.parent != current_parent || execution.result_snapshot().is_some() {
            continue;
        }
        starts.push(execution.start);
    }
    starts.sort_unstable();
    (!starts.is_empty()).then_some(starts)
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
}

impl ToolExecution {
    fn new(
        accepted: &AcceptedToolCall,
        inherited_cancellation: Option<tokio_util::sync::CancellationToken>,
    ) -> Arc<Self> {
        Arc::new(Self {
            parent: accepted.durability.parent().clone(),
            invocation: accepted.durability.scope().invocation_id().clone(),
            start: accepted.durability.scope().invocation_id().start_index(),
            filesystem: accepted.operation.context().activation.filesystem(),
            operation: accepted.operation.clone(),
            cancellable: accepted.durability.scope().mode()
                != golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted,
            state: Mutex::new(ToolExecutionState {
                result: None,
                failure: None,
            }),
            changed: Notify::new(),
            get_active: AtomicBool::new(false),
            cancel: inherited_cancellation
                .map(|cancellation| cancellation.child_token())
                .unwrap_or_default(),
        })
    }

    fn complete(&self, result: anyhow::Result<ToolInvokeResponse>) {
        let mut state = self.state.lock().unwrap();
        if state.result.is_some() || state.failure.is_some() {
            return;
        }
        match result {
            Ok(result) => state.result = Some(result),
            Err(error) => state.failure = Some(error.to_string()),
        }
        drop(state);
        self.changed.notify_waiters();
    }

    fn result_snapshot(&self) -> Option<anyhow::Result<ToolInvokeResponse>> {
        let state = self.state.lock().unwrap();
        state
            .result
            .as_ref()
            .map(|result| Ok(result.clone()))
            .or_else(|| {
                state
                    .failure
                    .as_ref()
                    .map(|error| Err(anyhow!(error.clone())))
            })
    }

    fn get_plan(self: &Arc<Self>) -> FutureToolInvokeGet {
        FutureToolInvokeGet::Active(self.clone())
    }

    async fn result(&self) -> anyhow::Result<ToolInvokeResponse> {
        loop {
            let changed = self.changed.notified();
            if let Some(result) = self.result_snapshot() {
                return result;
            }
            changed.await;
        }
    }

    fn cancel(&self) {
        if self.cancellable && self.operation.begin_cancel() {
            self.cancel.cancel();
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancellable && self.cancel.is_cancelled()
    }

    async fn cancelled(&self) {
        if self.cancellable {
            self.cancel.cancelled().await;
        } else {
            std::future::pending().await
        }
    }
}

struct ToolExecutionGetGuard(Arc<ToolExecution>);

impl Drop for ToolExecutionGetGuard {
    fn drop(&mut self) {
        self.0.get_active.store(false, Ordering::Release);
    }
}

struct ToolExecutionTask<Ctx: WorkerCtx> {
    accepted: AcceptedToolCall,
    stdout: Option<Resource<ToolStdoutEntry>>,
    execution: Arc<ToolExecution>,
    completed_supervisor_started: Option<oneshot::Sender<()>>,
    _ctx: std::marker::PhantomData<fn() -> Ctx>,
}

struct FailedRetainedEntityResources {
    resources: crate::worker::entity_invocation::EntityInvocationResources,
    parent_end_attempted: bool,
}

impl<Ctx: WorkerCtx, U: Send + 'static> AccessorTask<U, HasSelf<DurableWorkerCtx<Ctx>>>
    for ToolExecutionTask<Ctx>
{
    fn run(
        self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    ) -> impl Future<Output = wasmtime::Result<()>> + Send {
        let execution: Pin<Box<dyn Future<Output = wasmtime::Result<()>> + Send + '_>> =
            Box::pin(async move {
                let result = execute_accepted_tool_call(
                    accessor,
                    self.accepted,
                    self.stdout,
                    Some(&self.execution),
                    self.completed_supervisor_started,
                )
                .await;
                self.execution.complete(result);
                Ok(())
            });
        execution
    }
}

fn render_tool_value(
    tool: &Tool,
    type_: &SchemaType,
    value: &SchemaValue,
) -> Result<ToolValuePattern, String> {
    if crate::durable_host::schema_value_stream::contains_stream(value) {
        // A stream's contents are unknown at admission. Preserve its argument position without
        // allowing a literal-value permission to authorize those contents.
        Ok(ToolValuePattern::Star)
    } else {
        value_to_cli_text_unredacted(&tool.schema, type_, value)
            .map(|value| ToolValuePattern::Literal(ToolValueLiteral(value)))
            .map_err(|error| error.to_string())
    }
}

fn join_tool_values(values: Vec<ToolValuePattern>, separator: &str) -> ToolValuePattern {
    let literals = values
        .into_iter()
        .map(|value| match value {
            ToolValuePattern::Literal(ToolValueLiteral(value)) => Some(value),
            _ => None,
        })
        .collect::<Option<Vec<_>>>();
    match literals {
        Some(values) => ToolValuePattern::Literal(ToolValueLiteral(values.join(separator))),
        None => ToolValuePattern::Star,
    }
}

fn option_args(
    tool: &Tool,
    option: &OptionSpec,
    value: &SchemaValue,
) -> Result<Vec<ToolArgPattern>, String> {
    let value = if !option.required && option.default.is_none() {
        match value {
            SchemaValue::Option { inner } => match inner.as_deref() {
                Some(value) => value,
                None => return Ok(Vec::new()),
            },
            _ => value,
        }
    } else {
        value
    };
    let name = ToolIdentifier::parse(&option.long)?;
    let argument = |value| ToolArgPattern::LongFlag {
        name: name.clone(),
        value: Some(value),
    };
    match &option.shape {
        OptionShape::Scalar(type_) | OptionShape::OptionalScalar(type_) => {
            let type_ = if !option.required && option.default.is_none() {
                match type_ {
                    SchemaType::Option { inner, .. } => inner,
                    _ => type_,
                }
            } else {
                type_
            };
            Ok(vec![argument(render_tool_value(tool, type_, value)?)])
        }
        OptionShape::RepeatableList(shape) => {
            let SchemaValue::List { elements } = value else {
                return Err(format!(
                    "canonical tool option '{}' must contain a list",
                    option.long
                ));
            };
            let rendered = elements
                .iter()
                .map(|value| render_tool_value(tool, &shape.item_type, value))
                .collect::<Result<Vec<_>, _>>()?;
            match shape.repetition {
                Repetition::Repeated => Ok(rendered.into_iter().map(argument).collect()),
                Repetition::Delimited(separator) | Repetition::Either(separator) => Ok((!rendered
                    .is_empty())
                .then(|| argument(join_tool_values(rendered, &separator.to_string())))
                .into_iter()
                .collect()),
            }
        }
        OptionShape::RepeatableMap(shape) => {
            let SchemaType::Map {
                key: key_type,
                value: value_type,
                ..
            } = tool
                .schema
                .resolve_ref(&shape.map_type)
                .map_err(|error| error.to_string())?
            else {
                return Err(format!(
                    "canonical tool option '{}' must declare a map type",
                    option.long
                ));
            };
            let SchemaValue::Map { entries } = value else {
                return Err(format!(
                    "canonical tool option '{}' must contain a map",
                    option.long
                ));
            };
            let rendered = entries
                .iter()
                .map(|(key, value)| {
                    Ok(join_tool_values(
                        vec![
                            render_tool_value(tool, key_type, key)?,
                            render_tool_value(tool, value_type, value)?,
                        ],
                        "=",
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            match shape.repetition {
                Repetition::Repeated => Ok(rendered.into_iter().map(argument).collect()),
                Repetition::Delimited(separator) | Repetition::Either(separator) => Ok((!rendered
                    .is_empty())
                .then(|| argument(join_tool_values(rendered, &separator.to_string())))
                .into_iter()
                .collect()),
            }
        }
    }
}

fn flag_args(
    flag: &golem_common::schema::tool::FlagSpec,
    value: &SchemaValue,
) -> Result<Vec<ToolArgPattern>, String> {
    let argument = |name: &str| {
        ToolIdentifier::parse(name).map(|name| ToolArgPattern::LongFlag { name, value: None })
    };
    match (flag.shape, value) {
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(value)) if *value == shape.default => {
            Ok(Vec::new())
        }
        (FlagShape::BoolFlag(_), SchemaValue::Bool(true)) => Ok(vec![argument(&flag.long)?]),
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(false)) if shape.negatable => {
            Ok(vec![argument(&format!("no-{}", flag.long))?])
        }
        (FlagShape::BoolFlag(_), SchemaValue::Bool(false)) => Err(format!(
            "canonical tool flag '{}' cannot represent false",
            flag.long
        )),
        (FlagShape::CountFlag(max), SchemaValue::U32(count)) => {
            if max.is_some_and(|max| *count > max) {
                return Err(format!(
                    "canonical tool flag '{}' count {} exceeds its maximum",
                    flag.long, count
                ));
            }
            Ok(vec![argument(&flag.long)?; *count as usize])
        }
        (FlagShape::BoolFlag(_), _) => Err(format!(
            "canonical tool flag '{}' must contain a boolean",
            flag.long
        )),
        (FlagShape::CountFlag(_), _) => Err(format!(
            "canonical tool flag '{}' must contain a u32 count",
            flag.long
        )),
    }
}

struct ResolvedToolCommand {
    command_index: usize,
    args: Vec<ToolArgPattern>,
    stdin_required: Option<bool>,
    stdout_required: Option<bool>,
}

fn validate_stream_attachments(
    command: &ResolvedToolCommand,
    command_path: &[String],
    has_stdin: bool,
    has_stdout: bool,
    call_mode: EntityCallMode,
) -> Result<(), SerializableToolRpcError> {
    if command.stdin_required.is_none() && has_stdin
        || command.stdin_required == Some(true) && !has_stdin
    {
        return Err(SerializableToolRpcError::ProtocolError(format!(
            "tool command {} stdin declaration does not match the supplied attachment",
            command_path.join(" ")
        )));
    }
    if call_mode != EntityCallMode::FireAndForget
        && (command.stdout_required.is_none() && has_stdout
            || command.stdout_required == Some(true) && !has_stdout)
    {
        return Err(SerializableToolRpcError::ProtocolError(format!(
            "tool command {} stdout declaration does not match the supplied attachment",
            command_path.join(" ")
        )));
    }
    Ok(())
}

fn resolve_tool_command(
    tool: &Tool,
    command_path: &[String],
    input: &ModelTypedSchemaValue,
) -> Result<ResolvedToolCommand, SerializableToolError> {
    let command_index = tool
        .command_index_by_path(command_path)
        .ok_or_else(|| SerializableToolError::InvalidCommandPath(command_path.to_vec()))?;
    let expected_input = tool
        .canonical_input_record_schema(command_index)
        .map_err(|error| SerializableToolError::InvalidInput(error.to_string()))?;
    if !is_equivalent_cross_graph(
        input.graph(),
        &input.graph().root,
        &expected_input,
        &expected_input.root,
    ) {
        return Err(SerializableToolError::InvalidInput(
            "tool input schema does not match the selected command".to_string(),
        ));
    }
    validate_value(input.graph(), &input.graph().root, input.value()).map_err(|errors| {
        SerializableToolError::InvalidInput(format!(
            "tool input value does not satisfy its schema: {}",
            errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        ))
    })?;
    let values = tool
        .decode_canonical_input_record(command_index, input.value().clone())
        .map_err(|error| SerializableToolError::InvalidInput(error.to_string()))?;
    let surfaces = tool.canonical_input_surfaces(command_index);
    let body = tool.commands.nodes[command_index]
        .body
        .as_ref()
        .expect("command_index_by_path only resolves commands with bodies");
    golem_schema::schema::tool::constraints::validate_tool_constraints(
        tool,
        command_index,
        &body.constraints,
        &surfaces,
        &values,
    )
    .map_err(SerializableToolError::ConstraintViolation)?;
    let mut args = Vec::new();

    for (surface, field) in surfaces.into_iter().zip(values) {
        match surface {
            CanonicalSurfaceRef::GlobalOption { node, index } => args.extend(
                option_args(
                    tool,
                    &tool.commands.nodes[node].globals.options[index],
                    &field.value,
                )
                .map_err(SerializableToolError::InvalidInput)?,
            ),
            CanonicalSurfaceRef::GlobalFlag { node, index } => args.extend(
                flag_args(
                    &tool.commands.nodes[node].globals.flags[index],
                    &field.value,
                )
                .map_err(SerializableToolError::InvalidInput)?,
            ),
            CanonicalSurfaceRef::BodyPositional { index } => {
                let positional = &body.positionals.fixed[index];
                let value = if !positional.required && positional.default.is_none() {
                    match &field.value {
                        SchemaValue::Option { inner } => inner.as_deref(),
                        value => Some(value),
                    }
                } else {
                    Some(&field.value)
                };
                if let Some(value) = value {
                    args.push(ToolArgPattern::Positional(
                        render_tool_value(tool, &positional.type_, value)
                            .map_err(SerializableToolError::InvalidInput)?,
                    ));
                }
            }
            CanonicalSurfaceRef::BodyTail => {
                let tail = body
                    .positionals
                    .tail
                    .as_ref()
                    .expect("BodyTail resolves an existing tail positional");
                let SchemaValue::List { elements } = &field.value else {
                    return Err(SerializableToolError::InvalidInput(format!(
                        "canonical tool tail positional '{}' must contain a list",
                        tail.name
                    )));
                };
                if !elements.is_empty()
                    && let Some(separator) = &tail.separator
                {
                    args.push(ToolArgPattern::Positional(ToolValuePattern::Literal(
                        ToolValueLiteral(separator.clone()),
                    )));
                }
                args.extend(
                    elements
                        .iter()
                        .map(|value| {
                            render_tool_value(tool, &tail.item_type, value)
                                .map(ToolArgPattern::Positional)
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(SerializableToolError::InvalidInput)?,
                );
            }
            CanonicalSurfaceRef::BodyOption { index } => args.extend(
                option_args(tool, &body.options[index], &field.value)
                    .map_err(SerializableToolError::InvalidInput)?,
            ),
            CanonicalSurfaceRef::BodyFlag { index } => args.extend(
                flag_args(&body.flags[index], &field.value)
                    .map_err(SerializableToolError::InvalidInput)?,
            ),
        }
    }

    Ok(ResolvedToolCommand {
        command_index,
        args,
        stdin_required: body.stdin.as_ref().map(|stream| stream.required),
        stdout_required: body.stdout.as_ref().map(|stream| stream.required),
    })
}

fn decode_typed_tool_value<Ctx: WorkerCtx>(
    value: TypedSchemaValue,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<ModelTypedSchemaValue, ToolInputDecodeFailure> {
    let graph =
        decode_graph(&value.graph).map_err(|_| ToolInputDecodeFailure::InvalidSchemaGraph)?;
    let decoded = decode_value_with(value.value, ctx)
        .map_err(|_| ToolInputDecodeFailure::InvalidSchemaValue)?;
    Ok(ModelTypedSchemaValue::new(graph, decoded))
}

fn encode_typed_tool_value<Ctx: WorkerCtx>(
    value: &ModelTypedSchemaValue,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<TypedSchemaValue, String> {
    Ok(TypedSchemaValue {
        graph: encode_graph(value.graph()).map_err(|error| error.to_string())?,
        value: encode_value_with_streams(value.value(), ctx).map_err(|error| error.to_string())?,
    })
}

fn project_tool_error<Ctx: WorkerCtx>(
    error: SerializableToolError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> ToolRpcError {
    let error = match error {
        SerializableToolError::InvalidToolName(value) => ToolError::InvalidToolName(value),
        SerializableToolError::InvalidCommandPath(value) => ToolError::InvalidCommandPath(value),
        SerializableToolError::InvalidInput(value) => ToolError::InvalidInput(value),
        SerializableToolError::ConstraintViolation(value) => ToolError::ConstraintViolation(value),
        SerializableToolError::InvalidResult(value) => ToolError::InvalidResult(value),
        SerializableToolError::CustomError(value) => {
            match encode_typed_tool_value(&value.payload, ctx) {
                Ok(payload) => {
                    ToolError::CustomError(golem_common::schema::wit::wire::CustomToolError {
                        name: value.name,
                        payload,
                    })
                }
                Err(error) => return ToolRpcError::ProtocolError(error),
            }
        }
    };
    ToolRpcError::RemoteToolError(error)
}

fn project_tool_rpc_error<Ctx: WorkerCtx>(
    error: SerializableToolRpcError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> ToolRpcError {
    match error {
        SerializableToolRpcError::ProtocolError(value) => ToolRpcError::ProtocolError(value),
        SerializableToolRpcError::Denied(value) => ToolRpcError::Denied(value),
        SerializableToolRpcError::NotFound(value) => ToolRpcError::NotFound(value),
        SerializableToolRpcError::RemoteInternalError(value) => {
            ToolRpcError::RemoteInternalError(value)
        }
        SerializableToolRpcError::RemoteToolError(error) => project_tool_error(*error, ctx),
        SerializableToolRpcError::Cancelled => ToolRpcError::Cancelled,
        SerializableToolRpcError::ResourceExhausted(value) => {
            ToolRpcError::ResourceExhausted(value)
        }
    }
}

fn project_tool_response_value<Ctx: WorkerCtx>(
    response: ToolInvokeResponse,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<Option<TypedSchemaValue>, ToolRpcError> {
    response
        .map_err(|error| project_tool_rpc_error(error, ctx))
        .and_then(|response| {
            response
                .result
                .as_ref()
                .map(|value| encode_typed_tool_value(value, ctx))
                .transpose()
                .map_err(ToolRpcError::ProtocolError)
        })
}

fn project_tool_response<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: ToolInvokeResponse,
) -> Result<InvocationResult, ToolRpcError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let result = project_tool_response_value(response, access.get())?;
        Ok(InvocationResult {
            result,
            stdout: None,
        })
    })
}

fn project_tool_unit<Ctx: WorkerCtx>(
    response: Result<(), SerializableToolRpcError>,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<(), ToolRpcError> {
    response.map_err(|error| project_tool_rpc_error(error, ctx))
}

fn caller_tool_owner(
    account: &AccountEmail,
    application: &ApplicationName,
    environment: &EnvironmentName,
    component: &ComponentName,
    tool_name: &ToolName,
) -> ToolOwnerPattern {
    ToolOwnerPattern::Tool {
        account: account.clone(),
        application: application.clone(),
        environment: environment.clone(),
        component: component.clone(),
        tool: tool_name.to_string(),
    }
}

struct PreparedToolCall {
    stdin: Option<Resource<ToolStdinEntry>>,
    permit: Option<LiveAuthorizationPermit>,
    operation: operation::ProvisionalOwnerToolOperation,
    plan: EntityInvocationPlanReference,
}

enum ToolCallPreparation {
    Ready(PreparedToolCall),
    Rejected {
        request: Box<HostRequestGolemToolInvocationRejected>,
        stdin: Option<Resource<ToolStdinEntry>>,
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
    stdin: Option<Resource<ToolStdinEntry>>,
    deferred_admission_inserted: bool,
    incapable_lane_ticket: Option<crate::worker::owner_lane::OwnerInvocationTicket>,
    attachment_counterparty: ToolAttachmentCounterparty,
}

enum ToolCallDispatch {
    Rejected {
        response: Box<ToolInvokeResponse>,
        stdin: Option<Resource<ToolStdinEntry>>,
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
    calling_principal: Principal,
}

#[derive(Clone)]
enum ToolCallTarget {
    Ambient(ToolRpcEntry),
    Underlying(UnderlyingToolEntry),
}

impl ToolCallTarget {
    fn tool_name(&self) -> anyhow::Result<ToolName> {
        match self {
            Self::Ambient(rpc) => Ok(rpc.tool_name.clone()),
            Self::Underlying(underlying) => match underlying
                .position
                .plan()
                .layer((underlying.position.plan().len() - 1) as u32)
                .map_err(anyhow::Error::msg)?
                .activation()
                .entity()
            {
                AgentEntity::Tool(name) => Ok(name),
                AgentEntity::ToolMiddleware(_) => {
                    Err(anyhow!("middleware plan has no terminal tool"))
                }
            },
        }
    }

    fn calling_principal<Ctx: WorkerCtx>(&self, ctx: &DurableWorkerCtx<Ctx>) -> Principal {
        let agent_id = match self {
            Self::Ambient(rpc) => rpc.owner.owner_id.agent_id.clone(),
            Self::Underlying(_) => ctx.state.owned_agent_id.agent_id.clone(),
        };
        Principal::Agent(AgentPrincipal { agent_id })
    }

    fn accepted_identity(
        &self,
    ) -> anyhow::Result<(
        AgentEntity,
        Option<golem_common::model::entity::EntityInvocationPlanPositionIdentity>,
    )> {
        match self {
            Self::Ambient(rpc) => Ok((AgentEntity::Tool(rpc.tool_name.clone()), None)),
            Self::Underlying(underlying) => {
                let position = underlying.position.position() + 1;
                let entity = underlying
                    .position
                    .plan()
                    .layer(position)
                    .map_err(anyhow::Error::msg)?
                    .activation()
                    .entity();
                Ok((
                    entity,
                    Some(
                        golem_common::model::entity::EntityInvocationPlanPositionIdentity {
                            root_start_index: underlying.position.root_start_index(),
                            position,
                        },
                    ),
                ))
            }
        }
    }
}

impl ToolInvocationAttempt {
    fn claim_identity(
        &self,
        command_path: &[String],
        has_stdin: bool,
        has_stdout: bool,
        call_mode: EntityCallMode,
    ) -> ToolInvocationClaimIdentity {
        let (entity, plan_position) = self
            .target
            .accepted_identity()
            .expect("validated tool call target");
        let calling_principal = self.calling_principal.clone();
        let input = self.input.as_ref().ok().map(strip_typed_streams);
        ToolInvocationClaimIdentity {
            accepted: input.clone().map(|input| EntityInvocationRequestIdentity {
                entity,
                calling_principal,
                call_mode,
                operation: EntityInvocationDescriptorIdentity::Tool(
                    ToolInvocationDescriptorIdentity {
                        attempt_ordinal: self.attempt_ordinal,
                        command_path: command_path.to_vec(),
                        has_stdin,
                        has_stdout,
                    },
                ),
                plan_position,
                input,
            }),
            rejected: ToolInvocationRejectedIdentity {
                attempt_ordinal: self.attempt_ordinal,
                tool_name: self.target.tool_name().expect("validated tool call target"),
                command_path: command_path.to_vec(),
                input,
                input_decode_failure: self.input.as_ref().err().copied(),
                has_stdin,
                has_stdout,
                call_mode,
            },
        }
    }
}

fn tool_rpc_for_current_owner<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    tool_name: ToolName,
) -> anyhow::Result<ToolRpcEntry> {
    Ok(ToolRpcEntry {
        tool_name,
        owner: ToolRpcOwnerContext {
            owner_id: ctx.state.owned_agent_id.clone(),
        },
    })
}

fn tool_rpc_resource<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    resource: &Resource<ToolRpcEntry>,
) -> anyhow::Result<ToolRpcEntry>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| Ok(access.get().table().get(resource)?.clone()))
}

fn read_tool_attempt<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    target: ToolCallTarget,
    input: TypedSchemaValue,
) -> anyhow::Result<ToolInvocationAttempt>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let ctx = access.get();
        if let ToolCallTarget::Ambient(rpc) = &target
            && ctx.state.owned_agent_id != rpc.owner.owner_id
        {
            return Err(anyhow!(
                "tool RPC resource belongs to a different owner runtime"
            ));
        }
        if let ToolCallTarget::Underlying(underlying) = &target {
            underlying.admit()?;
            underlying
                .position
                .next_reference()
                .ok_or_else(|| anyhow!("underlying-tool has no next chain layer"))?;
        }
        let (parent, attempt_ordinal) = next_tool_attempt_ordinal(ctx)?;
        let calling_principal = target.calling_principal(ctx);
        let key_context = EntityInvocationKeyContext::capture(ctx, attempt_ordinal)?;
        Ok(ToolInvocationAttempt {
            target,
            input: decode_typed_tool_value(input, ctx),
            rejection: None,
            key_context,
            parent,
            attempt_ordinal,
            calling_principal,
        })
    })
}

fn next_tool_attempt_ordinal<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<(crate::worker::owner_lane::OwnerInvocationId, u64)> {
    let parent = ctx.owner_invocation_id()?;
    let next = ctx
        .state
        .tool_invocation_attempt_ordinals
        .entry(parent.clone())
        .or_default();
    let ordinal = *next;
    *next = next
        .checked_add(1)
        .ok_or_else(|| anyhow!("tool invocation attempt ordinal overflow"))?;
    Ok((parent, ordinal))
}

fn describe_tool_command(
    definition: &Tool,
    input: &ModelTypedSchemaValue,
    attempt_ordinal: u64,
    command_path: Vec<String>,
    has_stdin: bool,
    has_stdout: bool,
    call_mode: EntityCallMode,
) -> Result<ToolInvocationDescriptor, SerializableToolRpcError> {
    let command = resolve_tool_command(definition, &command_path, input)
        .map_err(|error| SerializableToolRpcError::RemoteToolError(Box::new(error)))?;
    validate_stream_attachments(&command, &command_path, has_stdin, has_stdout, call_mode)?;
    let declares_stdout = command.stdout_required.is_some();
    let body = definition.commands.nodes[command.command_index]
        .body
        .as_ref()
        .expect("a resolved command has a body");
    Ok(ToolInvocationDescriptor {
        attempt_ordinal,
        command_path,
        args: command.args,
        has_stdin,
        has_stdout,
        declares_stdout,
        output_contract: ToolOutputContract {
            result: body
                .result
                .as_ref()
                .map(|result| golem_common::schema::SchemaGraph {
                    defs: definition.schema.defs.clone(),
                    root: result.type_.clone(),
                }),
            errors: body
                .errors
                .iter()
                .map(|error| NamedToolErrorSchema {
                    name: error.name.clone(),
                    payload: golem_common::schema::SchemaGraph {
                        defs: definition.schema.defs.clone(),
                        root: error
                            .payload
                            .clone()
                            .unwrap_or_else(|| SchemaType::tuple(Vec::new())),
                    },
                })
                .collect(),
        },
    })
}

fn rejected_tool_call(
    tool_name: &ToolName,
    attempt_ordinal: u64,
    command_path: &[String],
    input: Option<ModelTypedSchemaValue>,
    input_decode_failure: Option<ToolInputDecodeFailure>,
    has_stdin: bool,
    has_stdout: bool,
    call_mode: EntityCallMode,
    error: SerializableToolRpcError,
    stdin: Option<Resource<ToolStdinEntry>>,
) -> ToolCallPreparation {
    ToolCallPreparation::Rejected {
        request: Box::new(HostRequestGolemToolInvocationRejected {
            attempt_ordinal,
            tool_name: tool_name.to_string(),
            command_path: command_path.to_vec(),
            input,
            input_decode_failure,
            has_stdin,
            has_stdout,
            call_mode,
            error,
        }),
        stdin,
    }
}

async fn resolve_tool_activation<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    rpc: &ToolRpcEntry,
    binding_owner: &ToolBindingOwner,
) -> Result<ToolActivationOutcome, ToolDiscoveryError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    use crate::services::environment_state::{
        get_tool_activation_from_deployment, tool_activation_from_mcp,
    };
    use golem_common::model::mcp_import::McpImportSource;

    let (service, owner) = accessor.with(|mut access| {
        let ctx = access.get();
        (
            ctx.state.environment_state_service.clone(),
            ctx.owner_component_metadata().clone(),
        )
    });
    let deployment = service
        .get_live_tool_deployment_state(rpc.owner.owner_id.environment_id, owner.id, owner.revision)
        .await?;
    let fixed =
        get_tool_activation_from_deployment(deployment.as_deref(), binding_owner, &rpc.tool_name)?;
    if !matches!(fixed, ToolActivationOutcome::NotRegistered) {
        return Ok(fixed);
    }
    let Some(deployment) = deployment.filter(|deployment| !deployment.mcp_imports.is_empty())
    else {
        return Ok(fixed);
    };
    let auth = crate::durable_host::call_coordinator::agent_auth_ctx_at_serialized_access(
        accessor,
        accessor.getter(),
    )
    .await?;
    for index in 0..deployment.mcp_imports.len() {
        let source = McpImportSource {
            environment_id: rpc.owner.owner_id.environment_id,
            deployment_revision: deployment.deployment_revision,
            import_index: index.try_into().map_err(|_| {
                ToolDiscoveryError::InconsistentSnapshot {
                    details: "too many MCP imports".into(),
                }
            })?,
            upstream_tool_name: String::new(),
        };
        let observation = service
            .resolve_mcp_import(&source, &auth, false)
            .await
            .map_err(ToolDiscoveryError::Mcp)?;
        if let Some(tool) = observation
            .tools
            .into_iter()
            .find(|tool| tool.definition.name() == Some(rpc.tool_name.as_str()))
        {
            let binding_owner = binding_owner.clone();
            let deployment = deployment.clone();
            return tokio::task::spawn_blocking(move || {
                tool_activation_from_mcp(
                    source,
                    observation.protocol_version,
                    tool,
                    &deployment,
                    &owner,
                    &binding_owner,
                )
                .map(|activation| ToolActivationOutcome::Ready(Box::new(activation)))
            })
            .await
            .map_err(|error| {
                ToolDiscoveryError::Retrieval(WorkerExecutorError::runtime(error.to_string()))
            })?;
        }
    }
    Ok(ToolActivationOutcome::NotRegistered)
}

async fn prepare_tool_call<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    attempt: ToolInvocationAttempt,
    command_path: Vec<String>,
    stdin: Option<Resource<ToolStdinEntry>>,
    stdout_requested: bool,
    call_mode: EntityCallMode,
    pinned_activation: Option<Arc<ToolActivationSnapshot>>,
    principal: Option<Principal>,
) -> anyhow::Result<ToolCallPreparation>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let has_stdin = stdin.is_some();
    let ToolInvocationAttempt {
        target,
        input,
        rejection,
        key_context: _,
        parent,
        attempt_ordinal,
        calling_principal,
    } = attempt;
    let tool_name = target.tool_name()?;
    let input = match input {
        Ok(input) => input,
        Err(error) => {
            return Ok(rejected_tool_call(
                &tool_name,
                attempt_ordinal,
                &command_path,
                None,
                Some(error),
                has_stdin,
                stdout_requested,
                call_mode,
                SerializableToolRpcError::RemoteToolError(Box::new(
                    SerializableToolError::InvalidInput(match error {
                        ToolInputDecodeFailure::InvalidSchemaGraph => {
                            "invalid typed input schema graph".to_string()
                        }
                        ToolInputDecodeFailure::InvalidSchemaValue => {
                            "invalid typed input schema value".to_string()
                        }
                    }),
                )),
                stdin,
            ));
        }
    };

    if let Some(error) = rejection {
        return Ok(rejected_tool_call(
            &tool_name,
            attempt_ordinal,
            &command_path,
            Some(input),
            None,
            has_stdin,
            stdout_requested,
            call_mode,
            error,
            stdin,
        ));
    }

    let (effective_definition, activation, plan) = match &target {
        ToolCallTarget::Ambient(rpc) => {
            let binding_owner = accessor.with(|mut access| {
                let state = access.get();
                let component = state.owner_component_metadata();
                match state.owner_context() {
                    ResolvedOwnerContext::Agent(agent) => ToolBindingOwner::AgentType {
                        agent_type_name: agent.agent_type.clone(),
                    },
                    ResolvedOwnerContext::ComponentWorker
                    | ResolvedOwnerContext::ComponentBaseline => {
                        ToolBindingOwner::ComponentBaseline {
                            component_id: component.id,
                        }
                    }
                }
            });
            let activation_snapshot = if let Some(activation) = pinned_activation {
                (*activation).clone()
            } else {
                match resolve_tool_activation(accessor, rpc, &binding_owner).await {
                    Ok(ToolActivationOutcome::Ready(activation)) => *activation,
                    Ok(ToolActivationOutcome::NotBound) => {
                        return Ok(rejected_tool_call(
                            &tool_name,
                            attempt_ordinal,
                            &command_path,
                            Some(input),
                            None,
                            has_stdin,
                            stdout_requested,
                            call_mode,
                            SerializableToolRpcError::Denied(format!(
                                "tool '{}' is not bound to owner '{binding_owner:?}'",
                                rpc.tool_name
                            )),
                            stdin,
                        ));
                    }
                    Ok(ToolActivationOutcome::NotRegistered) => {
                        return Ok(rejected_tool_call(
                            &tool_name,
                            attempt_ordinal,
                            &command_path,
                            Some(input),
                            None,
                            has_stdin,
                            stdout_requested,
                            call_mode,
                            SerializableToolRpcError::NotFound(format!(
                                "tool '{}' is not registered",
                                rpc.tool_name
                            )),
                            stdin,
                        ));
                    }
                    Err(ToolDiscoveryError::Mcp(
                        golem_service_base::clients::registry::RegistryServiceError::LimitExceeded(
                            _,
                        ),
                    )) => {
                        return Err(
                            GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted.into()
                        );
                    }
                    Err(ToolDiscoveryError::Mcp(error))
                        if matches!(
                            classify_tool_discovery_error(&ToolDiscoveryError::Mcp(error.clone())),
                            HostFailureKind::Permanent
                        ) =>
                    {
                        return Ok(rejected_tool_call(
                            &tool_name,
                            attempt_ordinal,
                            &command_path,
                            Some(input),
                            None,
                            has_stdin,
                            stdout_requested,
                            call_mode,
                            mcp::registry_failure(&error),
                            stdin,
                        ));
                    }
                    Err(error) => {
                        return Err(anyhow::Error::new(ClassifiedHostError {
                            kind: classify_tool_discovery_error(&error),
                            message: error.to_string(),
                        }));
                    }
                }
            };
            let plan = activation_snapshot.runtime_plan().map_err(|error| {
                let error = ToolDiscoveryError::InconsistentSnapshot { details: error };
                anyhow::Error::new(ClassifiedHostError {
                    kind: classify_tool_discovery_error(&error),
                    message: error.to_string(),
                })
            })?;
            let activation = Arc::new(
                plan.layer(0)
                    .expect("runtime plan is non-empty")
                    .activation()
                    .clone(),
            );
            (
                activation_snapshot.effective_definition().clone(),
                activation,
                EntityInvocationPlanReference::Root { plan },
            )
        }
        ToolCallTarget::Underlying(underlying) => {
            let next_reference = underlying
                .position
                .next_reference()
                .ok_or_else(|| anyhow!("underlying-tool has no next chain layer"))?;
            let definition = underlying.next_definition()?.clone();
            let activation = Arc::new(
                underlying
                    .position
                    .plan()
                    .layer(underlying.position.position() + 1)
                    .map_err(anyhow::Error::msg)?
                    .activation()
                    .clone(),
            );
            (definition, activation, next_reference)
        }
    };

    let descriptor = match describe_tool_command(
        &effective_definition,
        &input,
        attempt_ordinal,
        command_path.clone(),
        has_stdin,
        stdout_requested,
        call_mode,
    ) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            return Ok(rejected_tool_call(
                &tool_name,
                attempt_ordinal,
                &command_path,
                Some(input),
                None,
                has_stdin,
                stdout_requested,
                call_mode,
                error,
                stdin,
            ));
        }
    };

    let permit = if let ToolCallTarget::Ambient(rpc) = &target {
        let permission_target = accessor.with(|mut access| {
            let component = access.get().owner_component_metadata();
            let owner = caller_tool_owner(
                &component.account_email,
                &component.application_name,
                &component.environment_name,
                &component.component_name,
                &rpc.tool_name,
            );
            let command_path = command_path.iter().map(String::as_str).collect::<Vec<_>>();
            tool_target(owner, &command_path, descriptor.args.clone())
        });
        let permission_target = match permission_target {
            Ok(target) => target,
            Err(error) => {
                return Ok(rejected_tool_call(
                    &tool_name,
                    attempt_ordinal,
                    &command_path,
                    Some(input),
                    None,
                    has_stdin,
                    stdout_requested,
                    call_mode,
                    SerializableToolRpcError::ProtocolError(error.to_string()),
                    stdin,
                ));
            }
        };
        Some(
            match authorize_live_permissions_at_serialized_access(
                accessor,
                accessor.getter(),
                &[permission_target],
            )
            .await?
            {
                Ok(permit) => permit,
                Err(error) => {
                    return Ok(rejected_tool_call(
                        &tool_name,
                        attempt_ordinal,
                        &command_path,
                        Some(input),
                        None,
                        has_stdin,
                        stdout_requested,
                        call_mode,
                        SerializableToolRpcError::Denied(error.to_string()),
                        stdin,
                    ));
                }
            },
        )
    } else {
        None
    };

    let descriptor = EntityInvocationDescriptor::Tool(descriptor);
    let operation = accessor.with(|mut access| {
        let ctx = access.get();
        let principal = principal.unwrap_or_else(|| ctx.invocation_principal());
        ctx.owner_execution
            .tool_operations()
            .create(operation::OwnerToolOperationContext {
                parent,
                call_mode,
                activation,
                calling_principal,
                principal,
                descriptor,
                input,
            })
    });
    if operation.is_rejected() {
        return Err(anyhow!(
            "owner generation was fenced before tool operation registration"
        ));
    }

    Ok(ToolCallPreparation::Ready(PreparedToolCall {
        stdin,
        permit,
        operation,
        plan,
    }))
}

fn decode_tool_terminal(
    response: golem_common::model::oplog::HostResponseEntityInvocation,
) -> Result<ToolInvokeResponse, WorkerExecutorError> {
    let value = response.result.map_err(|error| {
        WorkerExecutorError::runtime(format!("entity invocation terminal failed: {error}"))
    })?;
    let terminal =
        SerializableToolOperationTerminal::from_value(value.value()).map_err(|error| {
            WorkerExecutorError::runtime(format!("invalid durable tool terminal: {error}"))
        })?;
    match terminal.result {
        Ok(result) => {
            let result = result
                .result
                .map(SerializableToolResultValue::into_typed)
                .transpose()
                .map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "invalid durable tool result payload: {error}"
                    ))
                })?;
            Ok(Ok(SerializableToolInvocationResult {
                result: result.map(Box::new),
            }))
        }
        Err(error) => Ok(Err(error)),
    }
}

fn decode_guest_tool_error<Ctx: WorkerCtx>(
    error: ToolError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> SerializableToolRpcError {
    let error = match error {
        ToolError::InvalidToolName(value) => SerializableToolError::InvalidToolName(value),
        ToolError::InvalidCommandPath(value) => SerializableToolError::InvalidCommandPath(value),
        ToolError::InvalidInput(value) => SerializableToolError::InvalidInput(value),
        ToolError::ConstraintViolation(value) => SerializableToolError::ConstraintViolation(value),
        ToolError::InvalidResult(value) => SerializableToolError::InvalidResult(value),
        ToolError::CustomError(value) => match decode_typed_tool_value(value.payload, ctx) {
            Ok(payload) => {
                SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                    name: value.name,
                    payload,
                }))
            }
            Err(_) => {
                return SerializableToolRpcError::ProtocolError(
                    "tool guest returned an invalid custom-error payload".to_string(),
                );
            }
        },
    };
    SerializableToolRpcError::RemoteToolError(Box::new(error))
}

fn validate_declared_tool_error(
    error: SerializableToolRpcError,
    contract: &ToolOutputContract,
) -> SerializableToolRpcError {
    let SerializableToolRpcError::RemoteToolError(tool_error) = error else {
        return error;
    };
    let SerializableToolError::CustomError(custom) = tool_error.as_ref() else {
        return SerializableToolRpcError::RemoteToolError(tool_error);
    };
    let Some(declared) = contract.errors.iter().find(|case| case.name == custom.name) else {
        return SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::InvalidResult(format!(
                "tool returned undeclared custom error '{}'",
                custom.name
            )),
        ));
    };
    if !is_equivalent_cross_graph(
        custom.payload.graph(),
        &custom.payload.graph().root,
        &declared.payload,
        &declared.payload.root,
    ) || validate_value(
        custom.payload.graph(),
        &custom.payload.graph().root,
        custom.payload.value(),
    )
    .is_err()
    {
        return SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::InvalidResult(format!(
                "payload for custom error '{}' does not match its declared schema",
                custom.name
            )),
        ));
    }
    SerializableToolRpcError::RemoteToolError(tool_error)
}

fn validate_declared_tool_result(
    result: Option<ModelTypedSchemaValue>,
    contract: &ToolOutputContract,
) -> Result<Option<ModelTypedSchemaValue>, SerializableToolRpcError> {
    match (result.as_ref(), contract.result.as_ref()) {
        (None, None) => Ok(result),
        (Some(value), Some(declared))
            if is_equivalent_cross_graph(
                value.graph(),
                &value.graph().root,
                declared,
                &declared.root,
            ) && validate_value(value.graph(), &value.graph().root, value.value()).is_ok() =>
        {
            Ok(result)
        }
        _ => Err(SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::InvalidResult(
                "tool result does not match the selected command's declared result".to_string(),
            ),
        ))),
    }
}

fn validate_native_tool_output(
    result: Result<SerializableToolStructuredResult, SerializableToolRpcError>,
    contract: &ToolOutputContract,
) -> Result<SerializableToolStructuredResult, SerializableToolRpcError> {
    match result {
        Ok(result) => {
            let typed = result
                .result
                .clone()
                .map(SerializableToolResultValue::into_typed)
                .transpose()
                .map_err(|_| {
                    SerializableToolRpcError::RemoteToolError(Box::new(
                        SerializableToolError::InvalidResult(
                            "native tool returned an invalid result payload".to_string(),
                        ),
                    ))
                })?;
            validate_declared_tool_result(typed, contract)?;
            Ok(result)
        }
        Err(error) => Err(validate_declared_tool_error(error, contract)),
    }
}

async fn encode_tool_operation_terminal(
    result: Result<SerializableToolStructuredResult, SerializableToolRpcError>,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    encode_tool_terminal(
        SerializableToolOperationTerminal {
            body_execution: SerializableEntityBodyExecution::Executed,
            result,
        },
        "failed to encode durable tool operation terminal",
    )
    .await
}

fn stdout_limit_error(host_resource_exhausted: bool) -> Option<SerializableToolRpcError> {
    host_resource_exhausted.then(|| {
        SerializableToolRpcError::ResourceExhausted(
            "tool stdout exceeded the attachment byte limit".to_string(),
        )
    })
}

struct ToolSidecarInvocation {
    tool_name: String,
    position: crate::durable_host::entity::ResolvedEntityInvocationPosition,
    command_path: Vec<String>,
    input: ModelTypedSchemaValue,
    stdin: Option<ToolStdinEntry>,
    stdout: Option<ToolStdoutWriterEntry>,
    principal: Principal,
    output_contract: ToolOutputContract,
    result_streams: crate::durable_host::durable_session::StreamSession,
}

struct ToolSidecarBody {
    invocation: ToolSidecarInvocation,
    operation: operation::OwnerToolOperation,
    cancellation: Option<tokio_util::sync::CancellationToken>,
}

impl<Ctx: WorkerCtx> EntityInvocationBody<Ctx, HostResponseEntityInvocation> for ToolSidecarBody {
    fn invoke<'a>(
        self,
        instance: &'a wasmtime::component::Instance,
        store: &'a mut Store<Ctx>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<HostResponseEntityInvocation, WorkerExecutorError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(invoke_tool_sidecar(
            instance,
            store,
            self.invocation,
            self.operation,
            self.cancellation,
        ))
    }
}

struct EntityCancellationEpochTask(tokio::task::JoinHandle<()>);

impl Drop for EntityCancellationEpochTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct UnderlyingToolRevocationGuard(UnderlyingToolEntry);

impl Drop for UnderlyingToolRevocationGuard {
    fn drop(&mut self) {
        self.0.revoke();
    }
}

async fn materialize_guest_tool_response<Ctx: WorkerCtx>(
    accessor: &Accessor<Ctx>,
    result: Result<
        golem_common::schema::tool::wit::wire::InvocationResult,
        golem_common::schema::tool::wit::wire::ToolError,
    >,
    output_contract: &ToolOutputContract,
    result_streams: &crate::durable_host::durable_session::StreamSession,
) -> wasmtime::Result<HostResponseEntityInvocation> {
    let response = accessor
        .with(
            |mut access| -> Result<ToolInvokeResponse, WorkerExecutorError> {
                let ctx = access.data_mut().durable_ctx_mut();
                Ok(match result {
                    Ok(result) => {
                        let value = result
                            .result
                            .map(|value| decode_typed_tool_value(value, ctx))
                            .transpose()
                            .map_err(|_| {
                                WorkerExecutorError::runtime(
                                    "tool guest returned an invalid result payload",
                                )
                            })?;
                        validate_declared_tool_result(value, output_contract).map(|result| {
                            SerializableToolInvocationResult {
                                result: result.map(Box::new),
                            }
                        })
                    }
                    Err(error) => Err(validate_declared_tool_error(
                        decode_guest_tool_error(error, ctx),
                        output_contract,
                    )),
                })
            },
        )
        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
    let (completed, completion) = oneshot::channel();
    accessor.with(|mut access| {
        let ctx = access.data_mut().durable_ctx_mut();
        let activity = ctx.tail_work_tracker().activity();
        let revision = ctx.component_metadata().revision;
        access.spawn(streams::MaterializeResponse {
            streams: result_streams.clone(),
            revision,
            response,
            completed,
            activity,
        });
    });
    completion
        .await
        .map_err(|_| wasmtime::Error::msg("tool result materialization task stopped"))?
        .map_err(|error| wasmtime::Error::msg(error.to_string()))
}

async fn invoke_tool_sidecar<Ctx: WorkerCtx>(
    instance: &wasmtime::component::Instance,
    store: &mut Store<Ctx>,
    invocation: ToolSidecarInvocation,
    operation: operation::OwnerToolOperation,
    cancellation: Option<tokio_util::sync::CancellationToken>,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    let ToolSidecarInvocation {
        tool_name,
        position,
        command_path,
        input,
        stdin,
        stdout,
        principal,
        output_contract,
        result_streams,
    } = invocation;
    let mut store = store.as_context_mut();
    store
        .data_mut()
        .durable_ctx_mut()
        .set_entity_tool_operation(operation.clone())?;
    let cancellation_for_arbitration = cancellation.clone();
    let cancellation_epoch = if let Some(cancellation) = cancellation {
        store
            .data_mut()
            .durable_ctx_mut()
            .set_entity_cancellation(cancellation.clone())?;
        let engine = store.engine().clone();
        Some(EntityCancellationEpochTask(tokio::spawn(async move {
            cancellation.cancelled().await;
            engine.increment_epoch();
        })))
    } else {
        None
    };
    let input = encode_typed_tool_value(&input, store.data_mut().durable_ctx_mut())
        .map_err(WorkerExecutorError::runtime)?;
    let stdout_controller = stdout.as_ref().map(ToolStdoutWriterEntry::controller);
    let stdout_observer = stdout_controller
        .as_ref()
        .map(AttachmentController::observer);
    let stdout = stdout
        .map(|stdout| store.data_mut().durable_ctx_mut().table().push(stdout))
        .transpose()
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let display_name = match position.layer() {
        golem_common::model::entity::EntityInvocationPlanLayer::Middleware {
            activation, ..
        } => {
            format!(
                "golem:tool/tool-middleware-guest.invoke-tool-middleware({})",
                activation.entity().name()
            )
        }
        golem_common::model::entity::EntityInvocationPlanLayer::Tool { .. } => {
            format!("golem:tool/guest.invoke({tool_name})")
        }
    };
    prepare_guest_call(&mut store, &display_name).await;
    let stdin = stdin
        .map(|stdin| StreamReader::new(store.as_context_mut(), stdin.into_stream_producer()))
        .transpose()
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    store
        .data_mut()
        .durable_ctx_mut()
        .set_invocation_principal(Some(principal.clone()));
    let principal = principal.into();
    let replaying_completed = store
        .data()
        .durable_ctx()
        .entity_invocation_scope()
        .is_some_and(|scope| {
            scope.mode() == golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted
        });
    let parent = store.data().durable_ctx().owner_invocation_id()?;
    let instance_pre = instance.instance_pre(&store);
    let result = match position.layer() {
        golem_common::model::entity::EntityInvocationPlanLayer::Tool { .. } => {
            let indices =
                tool_guest_exports::GuestIndices::new(&instance_pre).map_err(|error| {
                    WorkerExecutorError::invalid_request(format!(
                        "tool guest export not available: {error}"
                    ))
                })?;
            let guest = indices.load(&mut store, instance).map_err(|error| {
                WorkerExecutorError::invalid_request(format!(
                    "failed to load tool guest export: {error}"
                ))
            })?;
            run_guest_call_settled(&mut store, async move |accessor| {
                let result = guest
                    .call_invoke(
                        accessor,
                        tool_name,
                        command_path,
                        input,
                        stdin,
                        stdout,
                        principal,
                    )
                    .await?;
                materialize_guest_tool_response(accessor, result, &output_contract, &result_streams)
                    .await
            })
            .await
        }
        golem_common::model::entity::EntityInvocationPlanLayer::Middleware {
            activation,
            parameters,
            next_effective_definition,
            ..
        } => {
            let middleware_name = activation.entity().name().to_string();
            let metadata = WitTool::try_from(next_effective_definition).map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to encode middleware tool metadata: {error}"
                ))
            })?;
            let parameters =
                encode_typed_tool_value(parameters, store.data_mut().durable_ctx_mut())
                    .map_err(WorkerExecutorError::runtime)?;
            let underlying = UnderlyingToolEntry::new(position.clone());
            let revoke = underlying.clone();
            let wrapped = store
                .data_mut()
                .durable_ctx_mut()
                .table()
                .push(underlying)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            let _revoke_on_exit = UnderlyingToolRevocationGuard(revoke.clone());
            let indices = tool_middleware_guest_exports::GuestIndices::new(&instance_pre).map_err(
                |error| {
                    WorkerExecutorError::invalid_request(format!(
                        "tool middleware guest export not available: {error}"
                    ))
                },
            )?;
            let guest = indices.load(&mut store, instance).map_err(|error| {
                WorkerExecutorError::invalid_request(format!(
                    "failed to load tool middleware guest export: {error}"
                ))
            })?;
            run_guest_call_settled(&mut store, async move |accessor| {
                let result = guest
                    .call_invoke_tool_middleware(
                        accessor,
                        middleware_name,
                        tool_name,
                        metadata,
                        parameters,
                        command_path,
                        input,
                        stdin,
                        stdout,
                        principal,
                        wrapped,
                    )
                    .await;
                revoke.revoke();
                materialize_guest_tool_response(
                    accessor,
                    result?,
                    &output_contract,
                    &result_streams,
                )
                .await
            })
            .await
        }
    };
    store
        .data_mut()
        .durable_ctx_mut()
        .set_invocation_principal(None);
    let finish = finish_invocation_and_get_fuel_consumption(&mut store, &display_name).await;
    let parent_end = prepare_tool_parent_end(&mut store, parent).await;
    drop(cancellation_epoch);
    if operation.has_pending_live_admission_rejection().await {
        return Err(crate::durable_host::tool_attachment_live_admission_rejected_error());
    }
    finish?;
    if let Err(error) = parent_end
        && matches!(&result, Ok(Ok(_)))
    {
        return Err(error);
    }
    let local_cancellation_selected = cancellation_for_arbitration
        .is_some_and(|cancellation| cancellation.is_cancelled())
        && operation.claim_local_cancellation_interruption();

    match result {
        Ok(Ok(result)) => {
            let host_resource_exhausted = stdout_observer
                .as_ref()
                .and_then(AttachmentObserver::terminal_snapshot)
                .is_some_and(|terminal| terminal.host_resource_exhausted);
            if let Some(error) = stdout_limit_error(host_resource_exhausted) {
                return encode_tool_operation_terminal(Err(error)).await;
            }
            Ok(result)
        }
        Ok(Err(error))
        | Err(GuestCallSettlementError::Interrupted(error))
        | Err(GuestCallSettlementError::Trap(error)) => {
            let error: anyhow::Error = error.into();
            let trap = InvokeResult::from_error::<Ctx>(
                0,
                &error,
                store.data().get_current_retry_point().await,
                store.data().current_in_atomic_region(),
                store.data().current_atomic_region_had_side_effects(),
                store.data().agent_mode(),
            )
            .as_trap_type::<Ctx>()
            .expect("a failed tool guest call must classify as a trap");
            let stdout_failure = guest_trap_stdout_failure(
                &operation,
                trap.clone(),
                replaying_completed,
                local_cancellation_selected,
            )
            .await;
            if let Some(stdout) = stdout_controller {
                let _ = stdout.host_fail(stdout_failure);
            }
            Err(trap
                .as_golem_error("")
                .unwrap_or_else(|| WorkerExecutorError::runtime("tool sidecar was interrupted")))
        }
        Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
    }
}

async fn invoke_native_tool<Ctx: WorkerCtx>(
    worker: Arc<crate::worker::Worker<Ctx>>,
    owner_component_metadata: Arc<golem_service_base::model::component::Component>,
    scope: golem_common::model::entity::EntityInvocationScope,
    registration: &crate::worker::entity_slot::EntitySlotRegistration,
    invocation: ToolSidecarInvocation,
    operation: operation::OwnerToolOperation,
    cancellation: Option<tokio_util::sync::CancellationToken>,
    runner_abort: tokio_util::sync::CancellationToken,
) -> (
    Result<HostResponseEntityInvocation, WorkerExecutorError>,
    Option<Box<dyn RetainedEntityStore>>,
) {
    let source = scope.activation().source();
    let output_contract = invocation.output_contract.clone();
    let golem_common::model::entity::EntityActivationSource::Host {
        host_tool_id,
        implementation_version,
    } = source
    else {
        return (
            Err(WorkerExecutorError::runtime(
                "native runner requires a host activation",
            )),
            None,
        );
    };
    let key = NativeToolKey {
        host_tool_id: host_tool_id.clone(),
        implementation_version: implementation_version.clone(),
    };
    let golem_common::model::entity::EntityActivationPolicy::Tool {
        binding,
        mcp_import,
        ..
    } = scope.activation().policy()
    else {
        return (
            Err(WorkerExecutorError::runtime(
                "native tool activation does not contain a tool binding",
            )),
            None,
        );
    };
    let handler = if mcp_import.is_some() {
        None
    } else {
        let Some(registration) = worker.native_tool_catalog().get(&key) else {
            return (
                Err(WorkerExecutorError::runtime(format!(
                    "native tool implementation '{:?}@{}' is not installed",
                    key.host_tool_id, key.implementation_version
                ))),
                None,
            );
        };
        if let Err(error) = registration.validate_dispatch(binding) {
            return (Err(WorkerExecutorError::runtime(error)), None);
        }
        Some(registration.handler)
    };
    let executable = WorkerCtxExecutable::Native {
        host_tool_id: host_tool_id.clone(),
        implementation_version: implementation_version.clone(),
    };
    let mut ctx = match await_native_entity_body(
        &runner_abort,
        worker.create_entity_context(
            golem_common::model::entity::OwnerRuntime::Entity(scope.activation().entity()),
            scope.mode(),
            scope.activation().filesystem(),
            executable,
            scope.activation().clone(),
            owner_component_metadata,
        ),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(error) => return (Err(error), None),
    };
    if let Err(error) = registration
        .attach_linear_memory(ctx.durable_ctx().linear_memory_tracker())
        .and_then(|_| ctx.set_entity_invocation_scope(Some(scope.clone())))
        .and_then(|_| {
            ctx.durable_ctx_mut()
                .set_entity_tool_operation(operation.clone())
        })
    {
        return (Err(error), None);
    }
    if let Some(cancellation) = cancellation.clone()
        && let Err(error) = ctx
            .durable_ctx_mut()
            .set_entity_cancellation(cancellation.clone())
    {
        return (Err(error), None);
    }
    ctx.durable_ctx_mut()
        .set_invocation_principal(Some(invocation.principal.clone()));
    let mut retained = RetainedNativeContext::new(
        ctx,
        worker.owner_execution(),
        crate::worker::owner_lane::OwnerInvocationId::Entity(scope.invocation_id().clone()),
    );
    let stdout_controller = invocation
        .stdout
        .as_ref()
        .map(ToolStdoutWriterEntry::controller);
    let stdout_observer = stdout_controller
        .as_ref()
        .map(AttachmentController::observer);
    let native_invocation = NativeToolInvocation {
        command_path: invocation.command_path,
        input: invocation.input,
        principal: invocation.principal,
        cancellation,
        stdin: invocation.stdin.map(ToolStdinEntry::into_native),
        stdout: invocation
            .stdout
            .as_ref()
            .map(ToolStdoutWriterEntry::native_writer),
    };
    let result = await_native_entity_body(&runner_abort, async {
        match handler {
            Some(handler) => {
                handler
                    .invoke(retained.context_mut(), native_invocation)
                    .await
            }
            None => mcp::invoke(retained.context_mut(), native_invocation).await,
        }
    })
    .await;
    retained
        .context_mut()
        .durable_ctx_mut()
        .set_invocation_principal(None);
    let parent_end = retained.prepare_parent_end().await;
    let result = select_native_body_result(
        result,
        parent_end,
        operation.has_pending_live_admission_rejection().await,
    );
    let result = match result {
        Ok(result) => {
            if let Some(error) = stdout_limit_error(
                stdout_observer
                    .as_ref()
                    .and_then(AttachmentObserver::terminal_snapshot)
                    .is_some_and(|terminal| terminal.host_resource_exhausted),
            ) {
                encode_tool_operation_terminal(Err(error)).await
            } else {
                encode_tool_operation_terminal(validate_native_tool_output(
                    result,
                    &output_contract,
                ))
                .await
            }
        }
        Err(error) => Err(error),
    };
    let retained = Box::new(retained) as Box<dyn RetainedEntityStore>;
    (result, Some(retained))
}

fn select_native_body_result<T>(
    body: Result<T, WorkerExecutorError>,
    parent_end: Result<(), WorkerExecutorError>,
    live_admission_rejected: bool,
) -> Result<T, WorkerExecutorError> {
    if live_admission_rejected {
        return Err(crate::durable_host::tool_attachment_live_admission_rejected_error());
    }
    match body {
        Ok(result) => parent_end.map(|()| result),
        Err(error) => Err(error),
    }
}

async fn await_native_entity_body<T>(
    runner_abort: &tokio_util::sync::CancellationToken,
    body: impl Future<Output = Result<T, WorkerExecutorError>>,
) -> Result<T, WorkerExecutorError> {
    tokio::select! {
        biased;
        _ = runner_abort.cancelled() => Err(WorkerExecutorError::runtime("native entity body was aborted")),
        result = body => result,
    }
}

async fn guest_trap_stdout_failure(
    operation: &operation::OwnerToolOperation,
    trap: crate::model::TrapType,
    replaying_completed: bool,
    local_cancellation_selected: bool,
) -> ByteStreamFailure {
    if replaying_completed {
        ByteStreamFailure::Failed("tool sidecar trapped".to_string())
    } else if local_cancellation_selected || !operation.select_trap(trap).await {
        ByteStreamFailure::Cancelled
    } else {
        ByteStreamFailure::Failed("tool sidecar trapped".to_string())
    }
}

fn resource_exhausted_terminal() -> SerializableToolOperationTerminal {
    SerializableToolOperationTerminal {
        body_execution: SerializableEntityBodyExecution::Skipped,
        result: Err(SerializableToolRpcError::ResourceExhausted(
            "tool stdin exceeded the attachment byte limit".to_string(),
        )),
    }
}

async fn resource_exhausted_without_body()
-> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    encode_tool_terminal(
        resource_exhausted_terminal(),
        "failed to encode resource-exhausted tool terminal",
    )
    .await
}

fn terminal_from_response(
    response: &HostResponseEntityInvocation,
) -> Result<Arc<SerializableToolOperationTerminal>, WorkerExecutorError> {
    let value = response.result.as_ref().map_err(|error| {
        WorkerExecutorError::runtime(format!("entity invocation terminal failed: {error}"))
    })?;
    SerializableToolOperationTerminal::from_value(value.value())
        .map(Arc::new)
        .map_err(|error| {
            WorkerExecutorError::runtime(format!("invalid durable tool terminal: {error}"))
        })
}

fn recorded_tool_body_is_skipped(
    recorded: &RecordedEntityTerminal,
) -> Result<bool, WorkerExecutorError> {
    terminal_from_response(match recorded {
        RecordedEntityTerminal::Completed(response)
        | RecordedEntityTerminal::Cancelled(response) => response,
    })
    .map(|terminal| terminal.body_execution == SerializableEntityBodyExecution::Skipped)
}

fn skipped_attachment_failure(terminal: &SerializableToolOperationTerminal) -> ByteStreamFailure {
    match &terminal.result {
        Err(SerializableToolRpcError::ResourceExhausted(_)) => ByteStreamFailure::ResourceExhausted,
        Err(SerializableToolRpcError::Cancelled) => ByteStreamFailure::Cancelled,
        _ => ByteStreamFailure::Failed("tool body was skipped during replay".to_string()),
    }
}

struct SkippedToolAttachmentEndpoints {
    stdin: Option<ToolStdinEntry>,
    stdout: Option<ToolStdoutWriterEntry>,
}

impl SkippedToolAttachmentEndpoints {
    fn controllers(&self) -> (Option<AttachmentController>, Option<AttachmentController>) {
        (
            self.stdin.as_ref().map(ToolStdinEntry::controller),
            self.stdout.as_ref().map(ToolStdoutWriterEntry::controller),
        )
    }

    fn publish_failure(
        self,
        controllers: &(Option<AttachmentController>, Option<AttachmentController>),
        failure: ByteStreamFailure,
    ) {
        for controller in controllers.0.iter().chain(controllers.1.iter()) {
            let _ = controller.host_fail(failure.clone());
            controller.publish_no_body_terminal();
        }
        drop(self);
    }
}

fn publish_no_body_terminals(
    stdin: Option<&AttachmentController>,
    stdout: Option<&AttachmentController>,
) {
    for controller in stdin.into_iter().chain(stdout) {
        controller.publish_no_body_terminal();
    }
}

fn settle_resource_exhausted_admission(
    operation: &operation::OwnerToolOperation,
    filesystem: golem_common::model::entity::FilesystemCapability,
    deferred_admission: &Arc<operation::DeferredAdmissionTable>,
    deferred_cleanup: &mut Option<DeferredAdmissionCleanup>,
    parent: &crate::worker::owner_lane::OwnerInvocationId,
    start: golem_common::model::oplog::OplogIndex,
    call_mode: EntityCallMode,
) -> anyhow::Result<()> {
    if !operation.transition_admission(
        operation::BodyAdmissionState::Staging,
        operation::BodyAdmissionState::SettledWithoutBody,
    ) {
        return Err(anyhow!(
            "tool invocation was fenced before resource-exhausted no-body admission"
        ));
    }
    if filesystem == golem_common::model::entity::FilesystemCapability::Incapable {
        return Ok(());
    }
    if !deferred_admission.settle_staging(
        parent,
        start,
        operation::DeferredAdmissionReadiness::SettledWithoutBody,
    ) {
        return Err(anyhow!(
            "tool invocation lost its deferred no-body admission"
        ));
    }
    if call_mode == EntityCallMode::Synchronous
        && !deferred_admission.remove_settled_without_body(parent, start)
    {
        return Err(anyhow!(
            "synchronous tool invocation lost its no-body admission"
        ));
    }
    deferred_cleanup
        .as_mut()
        .expect("capable call must own deferred admission")
        .disarm();
    Ok(())
}

async fn complete_resource_exhausted_without_body<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    stdin: Option<&AttachmentController>,
    stdout: Option<&AttachmentController>,
) -> anyhow::Result<ToolInvokeResponse>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let admission_rejection_preselected = operation
        .take_live_attachment_admission_rejection()
        .await
        .is_some();
    if !admission_rejection_preselected && !operation.begin_ordinary() {
        return Err(anyhow!(
            "tool invocation was fenced before no-body completion"
        ));
    }
    let fallback_terminal = Arc::new(resource_exhausted_terminal());
    let response = match resource_exhausted_without_body().await {
        Ok(response) => response,
        Err(error) => {
            operation.resolve_ordinary(fallback_terminal, false).await;
            let _ = operation.select_infrastructure(error.clone()).await;
            return Err(error.into());
        }
    };
    let terminal = match terminal_from_response(&response) {
        Ok(terminal) => terminal,
        Err(error) => {
            operation.resolve_ordinary(fallback_terminal, false).await;
            let _ = operation.select_infrastructure(error.clone()).await;
            return Err(error.into());
        }
    };
    let outcome = durability
        .complete_without_body_access(accessor, accessor.getter(), response)
        .await;
    match outcome {
        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(
            response,
            _,
        )) => {
            operation.resolve_ordinary(terminal, true).await;
            operation.settle().await;
            let terminal = terminal_from_response(&response)?;
            let failure = skipped_attachment_failure(&terminal);
            for controller in stdin.into_iter().chain(stdout) {
                if matches!(failure, ByteStreamFailure::ResourceExhausted) {
                    controller.complete_rejected_live_memory_accounting();
                } else {
                    let _ = controller.host_fail(failure.clone());
                }
            }
            publish_no_body_terminals(stdin, stdout);
            decode_tool_terminal(*response).map_err(Into::into)
        }
        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(
            response,
            _,
        )) => {
            operation.resolve_ordinary(terminal, false).await;
            let _ = operation.begin_cancel();
            operation.resolve_cancel(true).await;
            operation.settle().await;
            let terminal = terminal_from_response(&response)?;
            let failure = skipped_attachment_failure(&terminal);
            for controller in stdin.into_iter().chain(stdout) {
                let _ = controller.host_fail(failure.clone());
            }
            publish_no_body_terminals(stdin, stdout);
            decode_tool_terminal(*response).map_err(Into::into)
        }
        Err(error) => {
            operation.resolve_ordinary(terminal, false).await;
            let _ = operation.select_infrastructure(error.clone()).await;
            Err(error.into())
        }
    }
}

async fn execute_recorded_skipped_tool_call<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    stdin: Option<Resource<ToolStdinEntry>>,
    stdout: Option<Resource<ToolStdoutEntry>>,
    recorded: RecordedEntityTerminal,
    deferred_admission_inserted: bool,
    deferred_cleanup: &mut Option<DeferredAdmissionCleanup>,
) -> anyhow::Result<ToolInvokeResponse>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let response = match &recorded {
        RecordedEntityTerminal::Completed(response)
        | RecordedEntityTerminal::Cancelled(response) => response,
    };
    let terminal = terminal_from_response(response)?;
    if terminal.body_execution != SerializableEntityBodyExecution::Skipped {
        return Err(anyhow!(
            "recorded no-body tool replay selected an executed terminal"
        ));
    }
    let failure = skipped_attachment_failure(&terminal);
    let endpoints = accessor.with(|mut access| -> anyhow::Result<_> {
        let ctx = access.get();
        let stdin = stdin.map(|stdin| ctx.table().delete(stdin)).transpose()?;
        let stdout = stdout
            .map(|stdout| ctx.table().delete(stdout).map(ToolStdoutEntry::into_writer))
            .transpose()?;
        Ok(SkippedToolAttachmentEndpoints { stdin, stdout })
    })?;
    let controllers = endpoints.controllers();
    if !operation.attach(controllers.0.clone(), controllers.1.clone())
        || !operation.transition_admission(
            operation::BodyAdmissionState::Staging,
            operation::BodyAdmissionState::SettledWithoutBody,
        )
    {
        return Err(anyhow!(
            "tool invocation was fenced before recorded no-body replay"
        ));
    }
    if deferred_admission_inserted {
        let parent = durability.parent().clone();
        let start = durability.scope().invocation_id().start_index();
        let settled = accessor.with(|mut access| {
            access
                .get()
                .owner_execution
                .deferred_tool_admission()
                .settle_staging(
                    &parent,
                    start,
                    operation::DeferredAdmissionReadiness::SettledWithoutBody,
                )
        });
        if !settled {
            return Err(anyhow!(
                "recorded no-body replay lost its deferred admission"
            ));
        }
        deferred_cleanup
            .as_mut()
            .expect("eager deferred admission must own cleanup")
            .disarm();
    }
    let outcome = match recorded {
        RecordedEntityTerminal::Completed(response) => {
            if !operation.begin_ordinary() {
                return Err(anyhow!(
                    "tool invocation lost ordinary no-body replay arbitration"
                ));
            }
            let outcome = durability
                .complete_without_body_access(accessor, accessor.getter(), response)
                .await;
            match &outcome {
                Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(
                    _,
                    _,
                )) => {
                    operation.resolve_ordinary(terminal, true).await;
                }
                _ => operation.resolve_ordinary(terminal, false).await,
            }
            outcome
        }
        RecordedEntityTerminal::Cancelled(_) => {
            if !operation.begin_cancel() {
                return Err(anyhow!(
                    "tool invocation lost cancelled no-body replay arbitration"
                ));
            }
            let outcome = durability
                .cancel_without_body_access(accessor, accessor.getter())
                .await;
            operation
                .resolve_cancel(matches!(
                    &outcome,
                    Ok(
                        crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(
                            _,
                            _
                        )
                    )
                ))
                .await;
            outcome
        }
    }?;
    operation.settle().await;
    endpoints.publish_failure(&controllers, failure);
    match outcome {
        crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(response, _)
        | crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(response, _) => {
            decode_tool_terminal(*response).map_err(Into::into)
        }
    }
}

struct DeferredAdmissionCleanup {
    table: Arc<operation::DeferredAdmissionTable>,
    parent: crate::worker::owner_lane::OwnerInvocationId,
    start: Option<golem_common::model::oplog::OplogIndex>,
}

impl DeferredAdmissionCleanup {
    fn disarm(&mut self) {
        self.start = None;
    }
}

impl Drop for DeferredAdmissionCleanup {
    fn drop(&mut self) {
        if let Some(start) = self.start.take() {
            self.table.remove(&self.parent, start);
        }
    }
}

async fn cancel_tool_before_body<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    deferred_admission: &Arc<operation::DeferredAdmissionTable>,
    deferred_cleanup: &mut Option<DeferredAdmissionCleanup>,
    parent: &crate::worker::owner_lane::OwnerInvocationId,
    start: golem_common::model::oplog::OplogIndex,
    admission: operation::BodyAdmissionState,
    stdin: Option<&AttachmentController>,
    stdout: Option<&AttachmentController>,
) -> anyhow::Result<ToolInvokeResponse>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let expected_readiness = match admission {
        operation::BodyAdmissionState::Staging => operation::DeferredAdmissionReadiness::Staging,
        operation::BodyAdmissionState::Ready => operation::DeferredAdmissionReadiness::Ready,
        _ => unreachable!("no-body cancellation requires deferred admission"),
    };
    if !deferred_admission.settle_operation_without_body(
        parent,
        start,
        expected_readiness,
        &operation,
        admission,
    ) {
        if operation.admission_if_active() == Some(operation::BodyAdmissionState::Registered) {
            deferred_cleanup
                .as_mut()
                .expect("capable call must own deferred admission")
                .disarm();
            return cancel_registered_tool_before_body(
                accessor, durability, operation, stdin, stdout,
            )
            .await;
        }
        return Err(anyhow!(
            "cancelled tool invocation lost its deferred admission"
        ));
    }
    deferred_cleanup
        .as_mut()
        .expect("capable call must own deferred admission")
        .disarm();
    if !operation.begin_cancel() {
        return Err(anyhow!(
            "tool invocation lost cancellation terminal arbitration"
        ));
    }
    let outcome = durability
        .cancel_without_body_access(accessor, accessor.getter())
        .await;
    operation
        .resolve_cancel(matches!(
            &outcome,
            Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(_, _))
        ))
        .await;
    let outcome = outcome?;
    operation.settle().await;
    publish_no_body_terminals(stdin, stdout);
    match outcome {
        crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(response, _)
        | crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(response, _) => {
            decode_tool_terminal(*response).map_err(Into::into)
        }
    }
}

async fn cancel_registered_tool_before_body<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    stdin: Option<&AttachmentController>,
    stdout: Option<&AttachmentController>,
) -> anyhow::Result<ToolInvokeResponse>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    if !operation.begin_cancel() {
        return Err(anyhow!(
            "tool invocation lost cancellation terminal arbitration"
        ));
    }
    let outcome = durability
        .cancel_without_body_access(accessor, accessor.getter())
        .await;
    operation
        .resolve_cancel(matches!(
            &outcome,
            Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(_, _))
        ))
        .await;
    let outcome = outcome?;
    operation.settle().await;
    publish_no_body_terminals(stdin, stdout);
    match outcome {
        crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(response, _)
        | crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(response, _) => {
            decode_tool_terminal(*response).map_err(Into::into)
        }
    }
}

async fn execute_accepted_tool_call<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    accepted: AcceptedToolCall,
    stdout: Option<Resource<ToolStdoutEntry>>,
    execution: Option<&ToolExecution>,
    mut completed_supervisor_started: Option<oneshot::Sender<()>>,
) -> anyhow::Result<ToolInvokeResponse>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let replaying_completed = accepted.durability.scope().mode()
        == golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted;
    let mut reconstruction_hold = replaying_completed
        .then(|| accepted.durability.historical_reconstruction_hold())
        .flatten();
    let completed_supervisor = replaying_completed.then(oneshot::channel);
    let (inner_supervisor_started, mut inner_supervisor_ready) = match completed_supervisor {
        Some((started, ready)) => (Some(started), Some(ready)),
        None => (None, None),
    };
    let cleanup_operation = accepted.operation.clone();
    let (primary, active_agents, owner_id, owner_execution, owner_operations) =
        accessor.with(|mut access| {
            let ctx = access.get();
            (
                ctx.public_state.worker(),
                ctx.public_state.worker().active_agents(),
                ctx.state.owned_agent_id.clone(),
                ctx.owner_execution.clone(),
                ctx.owner_execution.tool_operations(),
            )
        });
    let failed_resources = Arc::new(Mutex::new(None));
    let execution_future = Box::pin(execute_accepted_tool_call_inner(
        accessor,
        accepted,
        stdout,
        execution,
        failed_resources.clone(),
        inner_supervisor_started,
    ));
    let execution_future = async {
        if execution.is_some_and(|execution| !execution.cancellable) {
            crate::durable_host::without_entity_cancellation(execution_future).await
        } else {
            execution_future.await
        }
    };
    tokio::pin!(execution_future);
    let mut result = if let Some(ready) = inner_supervisor_ready.as_mut() {
        tokio::select! {
            biased;
            started = ready => {
                if started.is_ok() {
                    if let Some(started) = completed_supervisor_started.take() {
                        let _ = started.send(());
                    }
                    drop(reconstruction_hold.take());
                }
                execution_future.await
            }
            result = &mut execution_future => result,
        }
    } else {
        execution_future.await
    };
    if let Err(error) = &mut result {
        let mut failed_resources = failed_resources.lock().unwrap().take();
        if let Some(failed_resources) = failed_resources.as_mut() {
            if !failed_resources.parent_end_attempted {
                if let Err(preparation_error) =
                    failed_resources.resources.prepare_parent_end().await
                    && owner_operations.selected_owner_failure().is_none()
                {
                    *error = preparation_error.into();
                }
                failed_resources.parent_end_attempted = true;
            }
            failed_resources.resources.release_for_owner_failure();
        }
        if matches!(
            cleanup_operation.winner_if_active(),
            Some(operation::ToolOperationWinner::SelectingCancelled)
        ) {
            cleanup_operation.resolve_cancel(false).await;
        }
        let active_agent = active_agents
            .try_get_active_agent(&owner_id)
            .await
            .filter(|active_agent| Arc::ptr_eq(&active_agent.execution(), &owner_execution));
        let requested_winner = match &active_agent {
            Some(_) => {
                let infrastructure = error
                    .downcast_ref::<WorkerExecutorError>()
                    .cloned()
                    .unwrap_or_else(|| WorkerExecutorError::runtime(error.to_string()));
                operation::OwnerFailureWinner::Infrastructure(infrastructure)
            }
            None => operation::OwnerFailureWinner::Lifecycle(InterruptKind::Interrupt(
                golem_common::model::Timestamp::now_utc(),
            )),
        };
        tracing::debug!(
            owner_id = %owner_id,
            owner_generation_active = active_agent.is_some(),
            requested_failure_kind = requested_winner.kind_label(),
            "Classifying accepted tool operation failure"
        );
        cleanup_operation.select_failure(requested_winner).await;
        let winner = owner_operations
            .selected_owner_failure()
            .expect("failed accepted tool operation must select an owner winner");
        let preserve_exact_trap = matches!(&winner, operation::OwnerFailureWinner::Trap(_));
        let owner_failure_cleanup = cleanup_operation.claim_owner_failure_cleanup();
        if owner_failure_cleanup.is_some() {
            if let Some(active_agent) = active_agent {
                active_agent.fence_entity_bodies(winner).await;
            } else {
                owner_operations.close_failed_attachments();
                owner_operations.drain_owner_failure_lanes().await;
            }
        }
        if let Some(failed_resources) = failed_resources
            && let Err(settlement_error) =
                failed_resources.resources.settle_after_parent_end().await
            && !preserve_exact_trap
        {
            *error = settlement_error.into();
        }
        cleanup_operation.settle().await;
        if let Some(owner_failure_cleanup) = owner_failure_cleanup {
            owner_operations.wait_owner_settled().await;
            owner_operations.complete_owner_failure_cleanup(owner_failure_cleanup);
            primary.interrupt_current_execution();
        }
    }
    drop(reconstruction_hold);
    result
}

async fn execute_accepted_tool_call_inner<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    accepted: AcceptedToolCall,
    stdout: Option<Resource<ToolStdoutEntry>>,
    execution: Option<&ToolExecution>,
    failed_resources: Arc<Mutex<Option<FailedRetainedEntityResources>>>,
    completed_supervisor_started: Option<oneshot::Sender<()>>,
) -> anyhow::Result<ToolInvokeResponse>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let AcceptedToolCall {
        mut durability,
        operation,
        stdin,
        deferred_admission_inserted,
        mut incapable_lane_ticket,
        attachment_counterparty,
    } = accepted;
    let parent = durability.parent().clone();
    let start = durability.scope().invocation_id().start_index();
    let filesystem = durability.scope().activation().filesystem();
    let call_mode = durability.call_mode();
    let mut eager_deferred_cleanup = deferred_admission_inserted.then(|| {
        let table =
            accessor.with(|mut access| access.get().owner_execution.deferred_tool_admission());
        DeferredAdmissionCleanup {
            table,
            parent: parent.clone(),
            start: Some(start),
        }
    });
    if let Some(recorded) = durability
        .recorded_terminal_access(accessor, accessor.getter())
        .await?
        && recorded_tool_body_is_skipped(&recorded)?
    {
        return execute_recorded_skipped_tool_call(
            accessor,
            durability,
            operation,
            stdin,
            stdout,
            recorded,
            deferred_admission_inserted,
            &mut eager_deferred_cleanup,
        )
        .await;
    }
    let pre_body_live_admission_cancelled =
        if filesystem == golem_common::model::entity::FilesystemCapability::Capable {
            match durability
                .enter_incomplete_live_repair_before_body_access(accessor, accessor.getter())
                .await?
            {
                IncompleteLiveRepairBeforeBody::Ready(ready) => {
                    durability = ready;
                    false
                }
                IncompleteLiveRepairBeforeBody::Cancelled(cancelled) => {
                    durability = cancelled;
                    true
                }
            }
        } else {
            false
        };
    let execution_mode = durability.scope().mode();
    let discard_stdout = call_mode == EntityCallMode::FireAndForget
        && matches!(
            durability.operation(),
            EntityInvocationDescriptor::Tool(descriptor) if descriptor.declares_stdout
        );
    let (
        stdin,
        stdout,
        active_agents,
        owner_id,
        owner_component_metadata,
        lane,
        owner_execution,
        owner_operations,
        deferred_admission,
    ) = accessor.with(|mut access| -> anyhow::Result<_> {
        let ctx = access.get();
        let stdin = stdin.map(|stdin| ctx.table().delete(stdin)).transpose()?;
        let stdout = stdout
            .map(|stdout| ctx.table().delete(stdout).map(ToolStdoutEntry::into_writer))
            .transpose()?;
        let stdout = if stdout.is_none() && discard_stdout {
            Some(ToolStdoutWriterEntry::discard(AttachmentMemory::for_store(
                ctx.public_state.worker().active_agents(),
                execution_mode == InvocationExecutionMode::Live,
            )))
        } else {
            stdout
        };
        Ok((
            stdin,
            stdout,
            ctx.public_state.worker().active_agents(),
            ctx.state.owned_agent_id.clone(),
            Arc::new(ctx.owner_component_metadata().clone()),
            ctx.owner_execution.lane(),
            ctx.owner_execution.clone(),
            ctx.owner_execution.tool_operations(),
            ctx.owner_execution.deferred_tool_admission(),
        ))
    })?;
    let stdin_controller = stdin.as_ref().map(ToolStdinEntry::controller);
    let stdout_controller = stdout.as_ref().map(ToolStdoutWriterEntry::controller);
    let stdout_completion_only = stdout
        .as_ref()
        .is_some_and(ToolStdoutWriterEntry::completion_only);
    // Guest counterparties need the same owner lane as a capable body. Session journals
    // progress independently, so their streams stay live while the body owns the lane.
    let stage_attachments = filesystem
        == golem_common::model::entity::FilesystemCapability::Capable
        && attachment_counterparty == ToolAttachmentCounterparty::Guest;
    if !operation.attach(stdin_controller.clone(), stdout_controller.clone()) {
        return Err(anyhow!("owner generation fenced tool invocation"));
    }
    let mut deferred_cleanup =
        if filesystem == golem_common::model::entity::FilesystemCapability::Capable {
            if !deferred_admission_inserted && !deferred_admission.insert(parent.clone(), start) {
                return Err(anyhow!(
                    "capable tool operation was already registered for deferred admission"
                ));
            }
            eager_deferred_cleanup.take().or_else(|| {
                Some(DeferredAdmissionCleanup {
                    table: deferred_admission.clone(),
                    parent: parent.clone(),
                    start: Some(start),
                })
            })
        } else {
            None
        };

    if pre_body_live_admission_cancelled {
        return Box::pin(cancel_tool_before_body(
            accessor,
            durability,
            operation,
            &deferred_admission,
            &mut deferred_cleanup,
            &parent,
            start,
            operation::BodyAdmissionState::Staging,
            stdin_controller.as_ref(),
            stdout_controller.as_ref(),
        ))
        .await;
    }

    if filesystem == golem_common::model::entity::FilesystemCapability::Incapable
        && execution.is_some_and(ToolExecution::is_cancelled)
    {
        if !operation.transition_admission(
            operation::BodyAdmissionState::Staging,
            operation::BodyAdmissionState::SettledWithoutBody,
        ) {
            return Err(anyhow!(
                "tool invocation was fenced before pre-dispatch cancellation"
            ));
        }
        return cancel_registered_tool_before_body(
            accessor,
            durability,
            operation,
            stdin_controller.as_ref(),
            stdout_controller.as_ref(),
        )
        .await;
    }

    if execution_mode == InvocationExecutionMode::Live {
        match operation.activate_live_attachment_memory_accounting().await {
            operation::ToolLiveAdmissionOutcome::Admitted => {}
            operation::ToolLiveAdmissionOutcome::ResourceExhausted => {
                settle_resource_exhausted_admission(
                    &operation,
                    filesystem,
                    &deferred_admission,
                    &mut deferred_cleanup,
                    &parent,
                    start,
                    call_mode,
                )?;
                return complete_resource_exhausted_without_body(
                    accessor,
                    durability,
                    operation,
                    stdin_controller.as_ref(),
                    stdout_controller.as_ref(),
                )
                .await;
            }
            operation::ToolLiveAdmissionOutcome::Cancelled => {
                if filesystem == golem_common::model::entity::FilesystemCapability::Capable {
                    return Box::pin(cancel_tool_before_body(
                        accessor,
                        durability,
                        operation,
                        &deferred_admission,
                        &mut deferred_cleanup,
                        &parent,
                        start,
                        operation::BodyAdmissionState::Staging,
                        stdin_controller.as_ref(),
                        stdout_controller.as_ref(),
                    ))
                    .await;
                }
                if !operation.transition_admission(
                    operation::BodyAdmissionState::Staging,
                    operation::BodyAdmissionState::SettledWithoutBody,
                ) {
                    return Err(anyhow!(
                        "tool invocation was fenced before pre-dispatch cancellation"
                    ));
                }
                return Box::pin(cancel_registered_tool_before_body(
                    accessor,
                    durability,
                    operation,
                    stdin_controller.as_ref(),
                    stdout_controller.as_ref(),
                ))
                .await;
            }
            operation::ToolLiveAdmissionOutcome::Fenced => {
                return Err(anyhow!(
                    "tool invocation was fenced during live-memory admission"
                ));
            }
        }
    }

    if let Some(stdin) = &stdin_controller {
        if stage_attachments {
            stdin.configure_completion();
        } else {
            stdin.configure_live();
        }
    }
    if let Some(stdout) = &stdout_controller {
        if stage_attachments || stdout_completion_only {
            stdout.configure_completion();
        } else {
            stdout.configure_live();
        }
    }
    match filesystem {
        golem_common::model::entity::FilesystemCapability::Incapable => {
            if !operation.transition_admission(
                operation::BodyAdmissionState::Staging,
                operation::BodyAdmissionState::Running,
            ) {
                return Err(anyhow!("tool invocation was fenced before dispatch"));
            }
        }
        golem_common::model::entity::FilesystemCapability::Capable => {
            if execution.is_some_and(ToolExecution::is_cancelled) {
                return cancel_tool_before_body(
                    accessor,
                    durability,
                    operation,
                    &deferred_admission,
                    &mut deferred_cleanup,
                    &parent,
                    start,
                    operation::BodyAdmissionState::Staging,
                    stdin_controller.as_ref(),
                    stdout_controller.as_ref(),
                )
                .await;
            }
            if stage_attachments && let Some(stdin) = &stdin_controller {
                let stdin_observer = stdin.observer();
                let terminal = match execution {
                    Some(execution) => {
                        tokio::select! {
                            terminal = stdin_observer.wait_terminal() => terminal,
                            _ = execution.cancelled() => {
                                return cancel_tool_before_body(
                                    accessor,
                                    durability,
                                    operation,
                                    &deferred_admission,
                                    &mut deferred_cleanup,
                                    &parent,
                                    start,
                                    operation::BodyAdmissionState::Staging,
                                    stdin_controller.as_ref(),
                                    stdout_controller.as_ref(),
                                ).await;
                            }
                        }
                    }
                    None => stdin_observer.wait_terminal().await,
                };
                if execution.is_some_and(ToolExecution::is_cancelled) {
                    return cancel_tool_before_body(
                        accessor,
                        durability,
                        operation,
                        &deferred_admission,
                        &mut deferred_cleanup,
                        &parent,
                        start,
                        operation::BodyAdmissionState::Staging,
                        stdin_controller.as_ref(),
                        stdout_controller.as_ref(),
                    )
                    .await;
                }
                let host_resource_exhausted = stdin
                    .observer()
                    .terminal_snapshot()
                    .is_some_and(|terminal| terminal.host_resource_exhausted);
                if host_resource_exhausted {
                    settle_resource_exhausted_admission(
                        &operation,
                        filesystem,
                        &deferred_admission,
                        &mut deferred_cleanup,
                        &parent,
                        start,
                        call_mode,
                    )?;
                    return complete_resource_exhausted_without_body(
                        accessor,
                        durability,
                        operation,
                        stdin_controller.as_ref(),
                        stdout_controller.as_ref(),
                    )
                    .await;
                }
                tracing::debug!(
                    terminal = ?attachment::terminal_metadata(&terminal),
                    "Capable tool stdin staging reached its terminal"
                );
                stdin.publish_completion();
            }
            if !operation.transition_admission(
                operation::BodyAdmissionState::Staging,
                operation::BodyAdmissionState::Ready,
            ) || !deferred_admission.settle_staging(
                &parent,
                start,
                operation::DeferredAdmissionReadiness::Ready,
            ) {
                return Err(anyhow!(
                    "tool invocation was fenced before lane registration"
                ));
            }
            if call_mode == EntityCallMode::Synchronous {
                deferred_admission
                    .wait_and_register_cohort(
                        &parent,
                        operation::DeferredAdmissionCohort::ResultAwait(start),
                        &[start],
                        &owner_operations,
                        &lane,
                    )
                    .await?;
                deferred_cleanup
                    .as_mut()
                    .expect("capable call must own deferred admission")
                    .disarm();
            } else {
                if execution.is_none() {
                    return Err(anyhow!(
                        "detached capable tool operation has no execution owner"
                    ));
                }
                let execution = execution.expect("validated detached execution owner");
                let registered = tokio::select! {
                    registered = operation.wait_until_registered() => registered,
                    _ = execution.cancelled() => {
                        return if operation.admission_if_active()
                            == Some(operation::BodyAdmissionState::Ready)
                        {
                            cancel_tool_before_body(
                                accessor,
                                durability,
                                operation,
                                &deferred_admission,
                                &mut deferred_cleanup,
                                &parent,
                                start,
                                operation::BodyAdmissionState::Ready,
                                stdin_controller.as_ref(),
                                stdout_controller.as_ref(),
                            ).await
                        } else {
                            deferred_cleanup
                                .as_mut()
                                .expect("capable call must own deferred admission")
                                .disarm();
                            cancel_registered_tool_before_body(
                                accessor,
                                durability,
                                operation,
                                stdin_controller.as_ref(),
                                stdout_controller.as_ref(),
                            ).await
                        };
                    }
                };
                if !registered {
                    return Err(anyhow!("tool invocation was fenced before registration"));
                }
                deferred_cleanup
                    .as_mut()
                    .expect("capable call must own deferred admission")
                    .disarm();
            }
            let acquired = match execution {
                Some(execution) => {
                    tokio::select! {
                        biased;
                        _ = execution.cancelled() => {
                            return cancel_registered_tool_before_body(
                                accessor,
                                durability,
                                operation,
                                stdin_controller.as_ref(),
                                stdout_controller.as_ref(),
                            ).await;
                        }
                        acquired = operation.acquire_registered_body() => acquired?,
                    }
                }
                None => operation.acquire_registered_body().await?,
            };
            if !acquired {
                return Err(anyhow!("tool invocation was fenced before lane grant"));
            }
            if execution.is_some_and(ToolExecution::is_cancelled) {
                return cancel_registered_tool_before_body(
                    accessor,
                    durability,
                    operation,
                    stdin_controller.as_ref(),
                    stdout_controller.as_ref(),
                )
                .await;
            }
        }
    }

    let active_agent = active_agents
        .try_get_active_agent(&owner_id)
        .await
        .filter(|active_agent| Arc::ptr_eq(&active_agent.execution(), &owner_execution))
        .ok_or_else(|| anyhow!("active owner disappeared before tool sidecar dispatch"))?;
    let context = operation.context();
    let position = durability.resolved_position().clone();
    let leaf_position = (position.plan().len() - 1) as u32;
    let AgentEntity::Tool(tool_name) = position
        .plan()
        .layer(leaf_position)
        .map_err(anyhow::Error::msg)?
        .activation()
        .entity()
    else {
        return Err(anyhow!("tool invocation plan has no terminal tool"));
    };
    let EntityInvocationDescriptor::Tool(descriptor) = &context.descriptor;
    let scope = durability.scope().clone();
    let result_streams = streams::session(&active_agent.primary(), &scope).await?;
    let caller_revision =
        accessor.with(|mut access| access.get().state.component_metadata.revision);
    let has_input_streams =
        crate::durable_host::schema_value_stream::contains_stream(context.input.value());
    let input =
        streams::materialize_input(&result_streams, caller_revision, &context.input).await?;
    let sidecar = ToolSidecarInvocation {
        tool_name: tool_name.to_string(),
        position,
        command_path: descriptor.command_path.clone(),
        input,
        stdin,
        stdout,
        principal: context.principal.clone(),
        output_contract: descriptor.output_contract.clone(),
        result_streams: result_streams.clone(),
    };
    let replaying_completed =
        scope.mode() == golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted;
    let operation_for_body = operation.clone();
    let terminal = Arc::new(std::sync::Mutex::new(None));
    let terminal_for_finalize = terminal.clone();
    let operation_for_finalize = operation.clone();
    let terminal_for_completed_failure = terminal.clone();
    let operation_for_completed_failure = operation.clone();
    let cancellation = execution
        .filter(|execution| execution.cancellable)
        .map(|execution| execution.cancel.clone());
    let finalize = move |result: Result<HostResponseEntityInvocation, WorkerExecutorError>| {
        let operation = operation_for_finalize;
        async move {
            let (result, admission_rejection_preselected) = match result {
                Err(error)
                    if crate::durable_host::is_tool_attachment_live_admission_rejection(&error) =>
                {
                    operation
                        .take_live_attachment_admission_rejection()
                        .await
                        .ok_or_else(|| {
                            WorkerExecutorError::runtime(
                                "tool attachment admission rejection lost its operation marker",
                            )
                        })?;
                    let result = resource_exhausted_without_body().await;
                    if result.is_err() {
                        operation
                            .resolve_ordinary(Arc::new(resource_exhausted_terminal()), false)
                            .await;
                    }
                    (result, true)
                }
                result => (result, false),
            };
            match &result {
                Ok(response) => {
                    let selected = match terminal_from_response(response) {
                        Ok(selected) => selected,
                        Err(error) => {
                            if admission_rejection_preselected {
                                operation
                                    .resolve_ordinary(
                                        Arc::new(resource_exhausted_terminal()),
                                        false,
                                    )
                                    .await;
                            }
                            return Err(error);
                        }
                    };
                    if !admission_rejection_preselected && !operation.begin_ordinary() {
                        return Err(WorkerExecutorError::runtime(
                            "tool operation terminal lost owner arbitration",
                        ));
                    }
                    *terminal_for_finalize.lock().unwrap() = Some(selected);
                }
                Err(error)
                    if !replaying_completed
                        && matches!(
                            operation.winner_if_active(),
                            Some(operation::ToolOperationWinner::Open)
                        ) =>
                {
                    let _ = operation.select_infrastructure(error.clone()).await;
                }
                Err(_) => {}
            }
            result
        }
    };
    let body = match scope.activation().source() {
        golem_common::model::entity::EntityActivationSource::Component { .. } => {
            let invoke = ToolSidecarBody {
                invocation: sidecar,
                operation: operation_for_body,
                cancellation,
            };
            match filesystem {
                golem_common::model::entity::FilesystemCapability::Incapable => active_agent
                    .start_registered_entity_invocation(
                        scope,
                        owner_component_metadata,
                        call_mode,
                        incapable_lane_ticket
                            .take()
                            .expect("incapable tool body must own its admission ticket"),
                        move |instance, store| invoke.invoke(instance, store),
                        finalize,
                    ),
                golem_common::model::entity::FilesystemCapability::Capable => active_agent
                    .start_pre_acquired_entity_invocation(
                        scope,
                        owner_component_metadata,
                        call_mode,
                        invoke,
                        finalize,
                    ),
            }
        }
        golem_common::model::entity::EntityActivationSource::Host { .. } => {
            let worker = active_agent.primary();
            match filesystem {
                golem_common::model::entity::FilesystemCapability::Incapable => active_agent
                    .start_registered_native_entity_invocation(
                        scope,
                        call_mode,
                        incapable_lane_ticket
                            .take()
                            .expect("incapable native tool body must own its admission ticket"),
                        move |scope, registration, runner_abort| {
                            Box::pin(invoke_native_tool(
                                worker,
                                owner_component_metadata,
                                scope,
                                registration,
                                sidecar,
                                operation_for_body,
                                cancellation,
                                runner_abort,
                            ))
                        },
                        finalize,
                    ),
                golem_common::model::entity::FilesystemCapability::Capable => active_agent
                    .start_native_entity_invocation(
                        None,
                        scope,
                        call_mode,
                        move |scope, registration, runner_abort| {
                            Box::pin(invoke_native_tool(
                                worker,
                                owner_component_metadata,
                                scope,
                                registration,
                                sidecar,
                                operation_for_body,
                                cancellation,
                                runner_abort,
                            ))
                        },
                        finalize,
                    ),
            }
        }
    };
    let body = match body {
        Ok(body) => body,
        Err(error) => {
            let _ = operation.select_infrastructure(error.clone()).await;
            return Err(error.into());
        }
    };
    let outcome = durability
        .drive_access(
            accessor,
            accessor.getter(),
            body,
            execution
                .filter(|execution| execution.cancellable)
                .map(|execution| execution.cancel.clone()),
            move || {
                if let Some(started) = completed_supervisor_started {
                    let _ = started.send(());
                }
            },
            {
                let operation = operation.clone();
                move || {
                    operation.begin_cancel();
                }
            },
            move |error| async move {
                let terminal = terminal_for_completed_failure.lock().unwrap().take();
                if let Some(terminal) = terminal {
                    operation_for_completed_failure
                        .resolve_ordinary(terminal, false)
                        .await;
                }
                let _ = operation_for_completed_failure
                    .select_infrastructure(error)
                    .await;
            },
        )
        .await;
    match outcome {
        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Completed(
            response,
            resources,
        )) => {
            let live_admission_rejected = operation.live_attachment_admission_was_rejected().await;
            let terminal = terminal.lock().unwrap().take().ok_or_else(|| {
                anyhow!("completed tool body did not select an operation terminal")
            })?;
            if live_admission_rejected {
                operation.complete_rejected_live_attachment_memory_accounting();
            }
            let resources = match resources {
                Some(mut retained) => {
                    if let Err(error) = retained.prepare_parent_end().await {
                        operation.resolve_ordinary(terminal, false).await;
                        *failed_resources.lock().unwrap() = Some(FailedRetainedEntityResources {
                            resources: retained,
                            parent_end_attempted: true,
                        });
                        return Err(error.into());
                    }
                    Some(retained)
                }
                None => None,
            };
            operation.resolve_ordinary(terminal, true).await;
            if let Some(resources) = resources {
                resources.settle_after_parent_end().await?;
            }
            operation.settle().await;
            if has_input_streams
                || result_streams
                    .persisted_result()
                    .await
                    .map_err(anyhow::Error::msg)?
                    .is_some()
            {
                result_streams
                    .complete()
                    .await
                    .map_err(anyhow::Error::msg)?;
            }
            if (stage_attachments || stdout_completion_only)
                && let Some(stdout) = &stdout_controller
            {
                stdout.publish_completion();
            }
            streams::restore_response(&result_streams, decode_tool_terminal(*response)?)
                .await
                .map_err(Into::into)
        }
        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(
            response,
            resources,
        )) => {
            let live_admission_rejected = operation.live_attachment_admission_was_rejected().await;
            let response_terminal = terminal_from_response(&response)?;
            let no_body =
                response_terminal.body_execution == SerializableEntityBodyExecution::Skipped;
            let selected_terminal = terminal.lock().unwrap().take();
            if let Some(terminal) = selected_terminal {
                operation.resolve_ordinary(terminal, false).await;
            }
            let _ = operation.begin_cancel();
            let resources = match resources {
                Some(mut retained) => {
                    if let Err(error) = retained.prepare_parent_end().await {
                        *failed_resources.lock().unwrap() = Some(FailedRetainedEntityResources {
                            resources: retained,
                            parent_end_attempted: true,
                        });
                        return Err(error.into());
                    }
                    Some(retained)
                }
                None => None,
            };
            operation.resolve_cancel(true).await;
            if let Some(resources) = resources {
                resources.settle_after_parent_end().await?;
            }
            if live_admission_rejected {
                let failure = skipped_attachment_failure(&response_terminal);
                for controller in stdin_controller.iter().chain(stdout_controller.iter()) {
                    let _ = controller.host_fail(failure.clone());
                }
            }
            if has_input_streams
                || result_streams
                    .persisted_result()
                    .await
                    .map_err(anyhow::Error::msg)?
                    .is_some()
            {
                result_streams
                    .fail("tool invocation was cancelled".to_string())
                    .await
                    .map_err(anyhow::Error::msg)?;
            }
            operation.settle().await;
            if no_body {
                publish_no_body_terminals(stdin_controller.as_ref(), stdout_controller.as_ref());
            } else if (stage_attachments || stdout_completion_only)
                && let Some(stdout) = &stdout_controller
            {
                stdout.publish_completion();
            }
            decode_tool_terminal(*response).map_err(Into::into)
        }
        Err(failure) => {
            let selected_terminal = terminal.lock().unwrap().take();
            if let Some(terminal) = selected_terminal {
                operation.resolve_ordinary(terminal, false).await;
            }
            if let Some(resources) = failure.resources {
                *failed_resources.lock().unwrap() = Some(FailedRetainedEntityResources {
                    resources,
                    parent_end_attempted: false,
                });
            }
            Err(failure.error.into())
        }
    }
}

async fn dispatch_tool_attempt<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    attempt: ToolInvocationAttempt,
    command_path: Vec<String>,
    stdin: Option<Resource<ToolStdinEntry>>,
    has_stdout: bool,
    call_mode: EntityCallMode,
    pinned_activation: Option<Arc<ToolActivationSnapshot>>,
    principal: Option<Principal>,
    attachment_counterparty: ToolAttachmentCounterparty,
) -> anyhow::Result<ToolCallDispatch>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let has_stdin = stdin.is_some();
    if accessor.with(|mut access| !access.get().state.is_live()) {
        let identity = attempt.claim_identity(&command_path, has_stdin, has_stdout, call_mode);
        match EntityInvocationDurability::replay_tool_access(
            accessor,
            accessor.getter(),
            attempt.parent.clone(),
            &attempt.key_context,
            identity,
        )
        .await?
        {
            ToolInvocationReplayOutcome::Rejected(response) => {
                return Ok(ToolCallDispatch::Rejected {
                    response: Box::new(decode_tool_terminal(*response)?),
                    stdin,
                });
            }
            ToolInvocationReplayOutcome::Accepted(durability) => {
                let durability = *durability;
                let descriptor = durability.operation().clone();
                let input = attempt
                    .input
                    .expect("accepted replay validated the attempt input");
                let operation = accessor.with(|mut access| {
                    access.get().owner_execution.tool_operations().create(
                        operation::OwnerToolOperationContext {
                            parent: durability.parent().clone(),
                            call_mode: durability.call_mode(),
                            activation: durability.scope().activation().clone(),
                            calling_principal: durability.scope().calling_principal().clone(),
                            principal: durability.principal().clone(),
                            descriptor,
                            input,
                        },
                    )
                });
                let operation = operation
                    .accept(durability.scope().invocation_id().clone())
                    .ok_or_else(|| anyhow!("owner generation fenced replayed tool invocation"))?;
                return Ok(ToolCallDispatch::Accepted(Box::new(AcceptedToolCall {
                    durability,
                    operation,
                    stdin,
                    deferred_admission_inserted: false,
                    incapable_lane_ticket: None,
                    attachment_counterparty,
                })));
            }
            ToolInvocationReplayOutcome::ReplayEnded => {}
        }
    }

    let parent = attempt.parent.clone();
    let key_context = attempt.key_context.clone();
    match prepare_tool_call(
        accessor,
        attempt,
        command_path,
        stdin,
        has_stdout,
        call_mode,
        pinned_activation,
        principal,
    )
    .await?
    {
        ToolCallPreparation::Rejected { request, stdin } => {
            let response =
                record_tool_rejection_access(accessor, accessor.getter(), parent, *request).await?;
            Ok(ToolCallDispatch::Rejected {
                response: Box::new(decode_tool_terminal(response)?),
                stdin,
            })
        }
        ToolCallPreparation::Ready(prepared) => {
            let context = prepared.operation.context();
            let durability = EntityInvocationDurability::start_live_access(
                accessor,
                accessor.getter(),
                context.parent.clone(),
                &key_context,
                context.activation.entity(),
                context.calling_principal.clone(),
                context.principal.clone(),
                context.call_mode,
                context.descriptor.clone(),
                prepared.plan.clone(),
                context.input.clone(),
            )
            .await?;
            let operation = prepared
                .operation
                .accept(durability.scope().invocation_id().clone())
                .ok_or_else(|| anyhow!("owner generation fenced tool invocation"))?;
            let _permit = prepared.permit;
            Ok(ToolCallDispatch::Accepted(Box::new(AcceptedToolCall {
                durability,
                operation,
                stdin: prepared.stdin,
                deferred_admission_inserted: false,
                incapable_lane_ticket: None,
                attachment_counterparty,
            })))
        }
    }
}

async fn dispatch_tool_call<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    target: ToolCallTarget,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<Resource<ToolStdinEntry>>,
    has_stdout: bool,
    call_mode: EntityCallMode,
) -> anyhow::Result<ToolCallDispatch>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let attempt = read_tool_attempt(accessor, target, input)?;
    dispatch_tool_attempt(
        accessor,
        attempt,
        command_path,
        stdin,
        has_stdout,
        call_mode,
        None,
        None,
        ToolAttachmentCounterparty::Guest,
    )
    .await
}

async fn release_capable_tool_cohort<U, D, Ctx>(
    accessor: &Accessor<U, D>,
    get_ctx: fn(&mut U) -> &mut DurableWorkerCtx<Ctx>,
    parent: &crate::worker::owner_lane::OwnerInvocationId,
    cohort: operation::DeferredAdmissionCohort,
    starts: &[golem_common::model::oplog::OplogIndex],
) -> anyhow::Result<Option<crate::worker::owner_lane::OwnerLaneWait>>
where
    U: Send + 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    if starts.is_empty() {
        return Ok(None);
    }
    let (deferred, operations, lane) = accessor.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.owner_execution.deferred_tool_admission(),
            ctx.owner_execution.tool_operations(),
            ctx.owner_execution.lane(),
        )
    });
    release_capable_tool_cohort_owned(&deferred, &operations, &lane, parent, cohort, starts)
        .await
        .map_err(Into::into)
}

pub(crate) async fn release_capable_tool_cohort_owned(
    deferred: &operation::DeferredAdmissionTable,
    operations: &operation::OwnerToolOperations,
    lane: &crate::worker::owner_lane::OwnerLane,
    parent: &crate::worker::owner_lane::OwnerInvocationId,
    cohort: operation::DeferredAdmissionCohort,
    starts: &[golem_common::model::oplog::OplogIndex],
) -> Result<Option<crate::worker::owner_lane::OwnerLaneWait>, WorkerExecutorError> {
    if starts.is_empty() {
        return Ok(None);
    }
    deferred
        .wait_and_register_cohort(parent, cohort, starts, operations, lane)
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
}

pub(crate) async fn prepare_tool_parent_end_owned(
    execution: &crate::worker::instance::OwnerExecution,
    parent: crate::worker::owner_lane::OwnerInvocationId,
) -> Result<(), WorkerExecutorError> {
    let deferred = execution.deferred_tool_admission();
    let Some(starts) = deferred.close_parent_and_snapshot(&parent) else {
        return Ok(());
    };
    release_capable_tool_cohort_owned(
        &deferred,
        &execution.tool_operations(),
        &execution.lane(),
        &parent,
        operation::DeferredAdmissionCohort::ParentEnd,
        &starts,
    )
    .await?;
    Ok(())
}

pub(crate) async fn settle_tool_children_owned(
    execution: &crate::worker::instance::OwnerExecution,
    parent: crate::worker::owner_lane::OwnerInvocationId,
) -> Result<(), WorkerExecutorError> {
    let operations = execution.tool_operations();
    let deferred = execution.deferred_tool_admission();
    operations.wait_parent_settled(&parent).await;
    if !deferred.clear_closed_parent(&parent) {
        return Err(WorkerExecutorError::runtime(
            "tool parent settled with deferred admissions remaining",
        ));
    }
    Ok(())
}

async fn get_tool_invoke_results<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    futures: &[Resource<FutureInvokeResultEntry>],
) -> anyhow::Result<Vec<Result<InvocationResult, ToolRpcError>>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let plans = accessor.with(|mut access| -> anyhow::Result<Vec<_>> {
        futures
            .iter()
            .map(|future| {
                let entry = access.get().table().get(future)?;
                Ok(match &entry.state {
                    FutureToolInvokeState::Ready(response) => {
                        FutureToolInvokeGet::Ready(response.clone())
                    }
                    FutureToolInvokeState::Active(execution) => execution.get_plan(),
                })
            })
            .collect()
    })?;

    get_tool_invoke_plans(accessor, plans).await
}

async fn get_tool_invoke_plans<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    plans: Vec<FutureToolInvokeGet>,
) -> anyhow::Result<Vec<Result<InvocationResult, ToolRpcError>>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let responses = await_tool_invoke_plans(accessor, plans).await?;
    let mut projected = Vec::with_capacity(responses.len());
    for response in responses {
        let response = admit_tool_response_secret_holds(accessor, response).await?;
        projected.push(project_tool_response(accessor, response));
    }
    Ok(projected)
}

async fn await_tool_invoke_plans<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    mut plans: Vec<FutureToolInvokeGet>,
) -> anyhow::Result<Vec<ToolInvokeResponse>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let current_parent = accessor.with(|mut access| access.get().owner_invocation_id())?;
    let mut observed = Vec::new();
    let mut guards = Vec::new();
    for plan in &mut plans {
        let FutureToolInvokeGet::Active(execution) = plan else {
            continue;
        };
        if let Some(result) = execution.result_snapshot() {
            observed.push(execution.clone());
            *plan = match result {
                Ok(response) => FutureToolInvokeGet::Ready(Box::new(response)),
                Err(error) => FutureToolInvokeGet::Failed(error.to_string()),
            };
            continue;
        }
        if execution
            .get_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            *plan =
                FutureToolInvokeGet::Ready(Box::new(Err(SerializableToolRpcError::ProtocolError(
                    "tool invocation future already has an outstanding get".to_string(),
                ))));
        } else {
            observed.push(execution.clone());
            guards.push(ToolExecutionGetGuard(execution.clone()));
        }
    }

    let incapable_wait = accessor.with(|mut access| {
        access.get().owner_execution.lane().await_invocations(
            &current_parent,
            observed
                .iter()
                .filter(|&execution| {
                    execution.parent == current_parent
                        && execution.filesystem
                            == golem_common::model::entity::FilesystemCapability::Incapable
                })
                .map(|execution| {
                    crate::worker::owner_lane::OwnerInvocationId::Entity(
                        execution.invocation.clone(),
                    )
                }),
        )
    })?;

    for plan in &mut plans {
        let FutureToolInvokeGet::Active(execution) = plan else {
            continue;
        };
        if let Some(result) = execution.result_snapshot() {
            *plan = match result {
                Ok(response) => FutureToolInvokeGet::Ready(Box::new(response)),
                Err(error) => FutureToolInvokeGet::Failed(error.to_string()),
            };
        }
    }

    let lane_wait =
        if let Some(capable_starts) = capable_result_await_cohort(&plans, &current_parent) {
            let cohort_start = capable_starts[0];
            release_capable_tool_cohort(
                accessor,
                accessor.getter(),
                &current_parent,
                operation::DeferredAdmissionCohort::ResultAwait(cohort_start),
                &capable_starts,
            )
            .await?
        } else {
            None
        };

    let responses = futures::future::join_all(plans.into_iter().map(|plan| async move {
        match plan {
            FutureToolInvokeGet::Ready(response) => Ok(response),
            FutureToolInvokeGet::Failed(error) => Err(anyhow!(error)),
            FutureToolInvokeGet::Active(execution) => execution.result().await.map(Box::new),
        }
    }))
    .await;
    if let Some(lane_wait) = lane_wait {
        lane_wait.wait().await;
    }
    incapable_wait.wait().await;

    drop(guards);
    responses
        .into_iter()
        .map(|response| response.map(|response| *response))
        .collect()
}

/// Invokes a component-backed tool directly from the owner invocation driver.
///
/// The activation and optional byte stream mappings are pinned at queue admission. Stream
/// readers are reconstructed from the owner's session journal, not from the client connection.
pub(crate) async fn invoke_external_tool<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    activation: Arc<ToolActivationSnapshot>,
    tool_name: ToolName,
    command_path: Vec<String>,
    input: ModelTypedSchemaValue,
    stdin: bool,
    stdout: bool,
    principal: Principal,
) -> Result<anyhow::Result<ToolInvokeResponse>, GuestCallSettlementError> {
    run_guest_call_settled(
        store,
        async move |accessor| -> anyhow::Result<ToolInvokeResponse> {
            let accessor =
                accessor.with_getter::<HasSelf<DurableWorkerCtx<Ctx>>>(|ctx| ctx.durable_ctx_mut());
            let (result, receiver) = oneshot::channel();
            accessor.spawn(NativeToolTask {
                activation,
                tool_name,
                command_path,
                input,
                stdin,
                stdout,
                principal,
                result,
            });
            receiver
                .await
                .map_err(|_| anyhow!("native tool task ended without a result"))
        },
    )
    .await
}

struct NativeToolTask {
    activation: Arc<ToolActivationSnapshot>,
    tool_name: ToolName,
    command_path: Vec<String>,
    input: ModelTypedSchemaValue,
    stdin: bool,
    stdout: bool,
    principal: Principal,
    result: oneshot::Sender<ToolInvokeResponse>,
}

impl<Ctx: WorkerCtx> AccessorTask<Ctx, HasSelf<DurableWorkerCtx<Ctx>>> for NativeToolTask {
    async fn run(
        self,
        accessor: &Accessor<Ctx, HasSelf<DurableWorkerCtx<Ctx>>>,
    ) -> wasmtime::Result<()> {
        let Self {
            activation,
            tool_name,
            command_path,
            input,
            stdin: has_stdin,
            stdout: has_stdout,
            principal,
            result,
        } = self;
        let (worker, key) = accessor.with(|mut access| {
            let ctx = access.get();
            (
                ctx.public_state.worker(),
                ctx.state.get_current_idempotency_key(),
            )
        });
        let key = key.ok_or_else(|| wasmtime::Error::msg("native tool has no invocation key"))?;
        let session = worker
            .native_tool_session(&key)
            .await
            .map_err(|error| wasmtime::Error::from_anyhow(error.into()))?;
        let input = if session.is_some() {
            ToolInvocationInput::from_value(input.value())
                .map_err(|error| wasmtime::Error::msg(error.to_string()))?
        } else {
            ToolInvocationInput {
                arguments: input,
                stdin: None,
            }
        };
        let output_handle = session.as_ref().and_then(|(prepared, streams)| {
            prepared
                .stream_mappings
                .iter()
                .find(|mapping| mapping.role == SessionStreamRole::Output)
                .and_then(|mapping| streams.handle(mapping.transport_stream_id))
        });
        if input.stdin.is_some() != has_stdin || output_handle.is_some() != has_stdout {
            return Err(wasmtime::Error::msg(
                "native tool byte streams disagree with its accepted session",
            ));
        }
        let stdin = if let Some(stdin) = input.stdin {
            let endpoint = stdin
                .take_host_endpoint::<DurableInputEndpoint>()
                .map_err(wasmtime::Error::msg)?;
            let reader = accessor.with(|mut access| {
                let ctx = access.get();
                let producer = DurableByteInputProducer(
                    DurableInputProducer::new(endpoint).with_drop_cleanup(
                        ctx.state
                            .dropped_call_event_sender()
                            .expect("dropped-call event sender is always available"),
                        ctx.stream_runtime_teardown_probe(),
                    ),
                );
                StreamReader::new(&mut access, producer)
            })?;
            Some(
                create_native_underlying_stdin(accessor, reader)
                    .map_err(wasmtime::Error::from_anyhow)?,
            )
        } else {
            None
        };
        let (stdout, drain) = if let Some((_, streams)) = &session
            && let Some(handle) = &output_handle
        {
            let (stdout, consumer) =
                create_stdout_attachment(accessor, false).map_err(wasmtime::Error::from_anyhow)?;
            let endpoint = accessor.with(|mut access| -> wasmtime::Result<_> {
                let capacity = access.get().live_stream_event_capacity();
                let runtime_teardown = access.get().stream_runtime_teardown_probe();
                let (sink, stream) = byte_output_stream_pair(capacity, runtime_teardown)
                    .map_err(wasmtime::Error::msg)?;
                let reader = StreamReader::new(&mut access, consumer.into_raw_stream_producer())?;
                reader.pipe(&mut access, sink)?;
                stream
                    .take_host_endpoint::<LiveStreamEndpoint>()
                    .map_err(wasmtime::Error::msg)
            })?;
            (
                Some(stdout),
                Some((streams.clone(), handle.clone(), endpoint)),
            )
        } else {
            (None, None)
        };
        let (target, parent, attempt_ordinal, key_context, calling_principal) = accessor
            .with(|mut access| {
                let ctx = access.get();
                let rpc = tool_rpc_for_current_owner(ctx, tool_name)?;
                let parent = ctx.owner_invocation_id()?;
                let next_ordinal = ctx
                    .state
                    .tool_invocation_attempt_ordinals
                    .entry(parent.clone())
                    .or_default();
                let attempt_ordinal = *next_ordinal;
                *next_ordinal = next_ordinal
                    .checked_add(1)
                    .ok_or_else(|| anyhow!("tool invocation attempt ordinal overflow"))?;
                let target = ToolCallTarget::Ambient(rpc);
                let calling_principal = target.calling_principal(ctx);
                let key_context = EntityInvocationKeyContext::capture(ctx, attempt_ordinal)?;
                Ok::<_, anyhow::Error>((
                    target,
                    parent,
                    attempt_ordinal,
                    key_context,
                    calling_principal,
                ))
            })
            .map_err(wasmtime::Error::from_anyhow)?;
        let stdin_rep = stdin.as_ref().map(Resource::rep);
        let stdout_rep = stdout.as_ref().map(Resource::rep);
        let execute = async {
            let dispatch = dispatch_tool_attempt(
                accessor,
                ToolInvocationAttempt {
                    target,
                    input: Ok(input.arguments),
                    rejection: None,
                    key_context,
                    parent,
                    attempt_ordinal,
                    calling_principal,
                },
                command_path,
                stdin,
                has_stdout,
                EntityCallMode::Synchronous,
                Some(activation),
                Some(principal),
                ToolAttachmentCounterparty::SessionJournal,
            )
            .await
            .map_err(|error| {
                cleanup_failed_tool_dispatch(accessor, stdin_rep, stdout_rep, error)
            })?;
            let response = match dispatch {
                ToolCallDispatch::Rejected { response, stdin } => {
                    close_stdin(accessor, stdin)?;
                    reject_stdout(accessor, stdout)?;
                    *response
                }
                ToolCallDispatch::Accepted(mut accepted) => {
                    register_tool_admission(accessor, &mut accepted)?;
                    execute_accepted_tool_call(accessor, *accepted, stdout, None, None).await?
                }
            };
            Ok::<_, anyhow::Error>(response)
        };
        let drain = async {
            if let Some((streams, handle, endpoint)) = drain {
                streams
                    .drain_registered_output(
                        handle,
                        endpoint,
                        Arc::new(SchemaGraph::empty()),
                        SchemaType::u8(),
                    )
                    .await
                    .map_err(anyhow::Error::msg)?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (response, ()) =
            tokio::try_join!(execute, drain).map_err(wasmtime::Error::from_anyhow)?;
        let response = admit_tool_response_secret_holds(accessor, response)
            .await
            .map_err(wasmtime::Error::from_anyhow)?;
        if let Some((prepared, streams)) = &session {
            let value = ToolInvocationOutput {
                outcome: response.clone(),
                stdout: prepared
                    .stream_mappings
                    .iter()
                    .find(|mapping| mapping.role == SessionStreamRole::Output)
                    .map(|mapping| {
                        SchemaValueStream::from_host_endpoint(
                            crate::durable_host::durable_session::RegisteredOutputStream {
                                transport_stream_id: mapping.transport_stream_id,
                            },
                        )
                    }),
            }
            .into_typed_schema_value()
            .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
            streams
                .materialize_result(
                    value.value().clone(),
                    value.graph(),
                    value.root_type(),
                    prepared.attempt.invocation.target_component_revision,
                )
                .await
                .map_err(wasmtime::Error::msg)?;
        }
        result
            .send(response)
            .map_err(|_| wasmtime::Error::msg("native tool result receiver dropped"))?;
        Ok(())
    }
}

pub(crate) async fn prepare_tool_parent_end<Ctx: WorkerCtx>(
    store: &mut wasmtime::StoreContextMut<'_, Ctx>,
    parent: crate::worker::owner_lane::OwnerInvocationId,
) -> Result<(), WorkerExecutorError> {
    store
        .as_context_mut()
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            let execution =
                accessor.with(|mut access| access.data_mut().durable_ctx().owner_execution.clone());
            prepare_tool_parent_end_owned(&execution, parent)
                .await
                .map_err(|error| wasmtime::Error::msg(error.to_string()))
        })
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
}

pub(crate) async fn settle_tool_children<Ctx: WorkerCtx>(
    store: &mut wasmtime::StoreContextMut<'_, Ctx>,
    parent: crate::worker::owner_lane::OwnerInvocationId,
) -> Result<(), WorkerExecutorError> {
    let execution = store.data().durable_ctx().owner_execution.clone();
    store
        .as_context_mut()
        .run_concurrent(async move |_accessor| -> wasmtime::Result<()> {
            settle_tool_children_owned(&execution, parent)
                .await
                .map_err(|error| wasmtime::Error::msg(error.to_string()))
        })
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
}

fn register_tool_admission<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    accepted: &mut AcceptedToolCall,
) -> anyhow::Result<()>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    if accepted.operation.context().activation.filesystem()
        == golem_common::model::entity::FilesystemCapability::Capable
    {
        let parent = accepted.durability.parent().clone();
        let start = accepted.durability.scope().invocation_id().start_index();
        accepted.deferred_admission_inserted = accessor.with(|mut access| {
            access
                .get()
                .owner_execution
                .deferred_tool_admission()
                .insert(parent, start)
        });
    } else {
        let lane = accessor.with(|mut access| access.get().owner_execution.lane());
        let parent = accepted.durability.parent();
        let invocation = accepted.durability.scope().invocation_id().clone();
        let mode = accepted.durability.call_mode();
        accepted.incapable_lane_ticket = Some(lane.register_entity(
            parent.clone(),
            invocation.clone(),
            mode,
            golem_common::model::entity::FilesystemCapability::Incapable,
        )?);
    }
    Ok(())
}

async fn spawn_tool_execution<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    mut accepted: AcceptedToolCall,
    stdout: Option<Resource<ToolStdoutEntry>>,
) -> anyhow::Result<Arc<ToolExecution>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    register_tool_admission(accessor, &mut accepted)?;
    let inherited_cancellation = accessor.with(|mut access| access.get().entity_cancellation());
    let execution = ToolExecution::new(&accepted, inherited_cancellation);
    let completed_supervisor = (accepted.durability.scope().mode()
        == golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted
        && accepted.operation.context().activation.filesystem()
            == golem_common::model::entity::FilesystemCapability::Incapable)
        .then(oneshot::channel);
    let (completed_supervisor_started, completed_supervisor_ready) = match completed_supervisor {
        Some((started, ready)) => (Some(started), Some(ready)),
        None => (None, None),
    };
    accessor.with(|mut access| {
        access.spawn(ToolExecutionTask::<Ctx> {
            accepted,
            stdout,
            execution: execution.clone(),
            completed_supervisor_started,
            _ctx: std::marker::PhantomData,
        });
    });
    if let Some(ready) = completed_supervisor_ready
        && ready.await.is_err()
    {
        return match execution.result().await {
            Err(error) => Err(error),
            Ok(_) => Err(anyhow!(
                "completed reconstruction finished without starting its owner supervisor"
            )),
        };
    }
    Ok(execution)
}

fn close_stdin<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    stdin: Option<Resource<ToolStdinEntry>>,
) -> anyhow::Result<()>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    if let Some(stdin) = stdin {
        accessor.with(|mut access| access.get().table().delete(stdin).map(|_| ()))?;
    }
    Ok(())
}

fn reject_stdout<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    stdout: Option<Resource<ToolStdoutEntry>>,
) -> anyhow::Result<()>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    if let Some(stdout) = stdout {
        let stdout = accessor.with(|mut access| access.get().table().delete(stdout))?;
        stdout.reject_unconfigured();
    }
    Ok(())
}

fn cleanup_tool_endpoints(
    error: anyhow::Error,
    cleanup_stdin: impl FnOnce() -> anyhow::Result<()>,
    cleanup_stdout: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Error {
    let mut cleanup_failures = Vec::new();
    if let Err(cleanup) = cleanup_stdin() {
        cleanup_failures.push(format!("stdin cleanup failed: {cleanup}"));
    }
    if let Err(cleanup) = cleanup_stdout() {
        cleanup_failures.push(format!("stdout cleanup failed: {cleanup}"));
    }
    if cleanup_failures.is_empty() {
        error
    } else {
        error.context(cleanup_failures.join("; "))
    }
}

fn cleanup_failed_tool_dispatch<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    stdin: Option<u32>,
    stdout: Option<u32>,
    error: anyhow::Error,
) -> anyhow::Error
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    cleanup_tool_endpoints(
        error,
        || close_stdin(accessor, stdin.map(Resource::new_own)),
        || reject_stdout(accessor, stdout.map(Resource::new_own)),
    )
}

fn create_underlying_stdin<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    source: StreamReader<Result<Vec<u8>, ByteStreamFailure>>,
) -> anyhow::Result<Resource<ToolStdinEntry>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let ctx = access.get();
        let memory = AttachmentMemory::for_store(
            ctx.public_state.worker().active_agents(),
            ctx.state.is_live(),
        );
        let max_attachment_bytes = ctx.state.config.limits.max_tool_attachment_bytes;
        let (producer, consumer, observer) = attachment_pair(max_attachment_bytes, memory);
        let stdin = ctx.table().push(ToolStdinEntry { consumer })?;
        let (items, received) = mpsc::unbounded_channel();
        if let Err(error) = source.pipe(&mut access, ToolStdinStreamConsumer::new(items, observer))
        {
            return Err(cleanup_tool_endpoints(
                error.into(),
                || {
                    access.get().table().delete(stdin)?;
                    Ok(())
                },
                || Ok(()),
            ));
        }
        access.spawn(ToolStdinStreamPumpTask::<Ctx> {
            producer,
            items: received,
            _ctx: std::marker::PhantomData,
        });
        Ok(stdin)
    })
}

fn create_native_underlying_stdin<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    source: StreamReader<u8>,
) -> anyhow::Result<Resource<ToolStdinEntry>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let ctx = access.get();
        let memory = AttachmentMemory::for_store(
            ctx.public_state.worker().active_agents(),
            ctx.state.is_live(),
        );
        let max_attachment_bytes = ctx.state.config.limits.max_tool_attachment_bytes;
        let (producer, consumer, observer) = attachment_pair(max_attachment_bytes, memory);
        let stdin = ctx.table().push(ToolStdinEntry { consumer })?;
        let (items, received) = mpsc::unbounded_channel();
        if let Err(error) = source.pipe(
            &mut access,
            UnderlyingToolStdinStreamConsumer::new(items, observer, max_attachment_bytes),
        ) {
            return Err(cleanup_tool_endpoints(
                error.into(),
                || {
                    access.get().table().delete(stdin)?;
                    Ok(())
                },
                || Ok(()),
            ));
        }
        access.spawn(ToolStdinStreamPumpTask::<Ctx> {
            producer,
            items: received,
            _ctx: std::marker::PhantomData,
        });
        Ok(stdin)
    })
}

fn create_stdout_attachment<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    completion_only: bool,
) -> anyhow::Result<(Resource<ToolStdoutEntry>, AttachmentConsumer)>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let ctx = access.get();
        let memory = AttachmentMemory::for_store(
            ctx.public_state.worker().active_agents(),
            ctx.state.is_live(),
        );
        let (producer, consumer, _) =
            attachment_pair(ctx.state.config.limits.max_tool_attachment_bytes, memory);
        let target = ctx.table().push(ToolStdoutEntry {
            producer: Some(producer),
            completion_only,
        })?;
        Ok((target, consumer))
    })
}

async fn invoke_tool_terminal<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    rpc: ToolRpcEntry,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<Resource<ToolStdinEntry>>,
    stdout: Option<Resource<ToolStdoutEntry>>,
) -> anyhow::Result<ToolInvokeResponse>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let stdout_requested = stdout.is_some();
    let stdin_rep = stdin.as_ref().map(Resource::rep);
    let stdout_rep = stdout.as_ref().map(Resource::rep);
    let dispatch = dispatch_tool_call(
        accessor,
        ToolCallTarget::Ambient(rpc),
        command_path,
        input,
        stdin,
        stdout_requested,
        EntityCallMode::Synchronous,
    )
    .await;
    let response = match dispatch {
        Err(error) => {
            return Err(cleanup_failed_tool_dispatch(
                accessor, stdin_rep, stdout_rep, error,
            ));
        }
        Ok(dispatch) => match dispatch {
            ToolCallDispatch::Rejected { response, stdin } => {
                close_stdin(accessor, stdin)?;
                reject_stdout(accessor, stdout)?;
                *response
            }
            ToolCallDispatch::Accepted(accepted) => {
                let execution = spawn_tool_execution(accessor, *accepted, stdout).await?;
                let mut results =
                    await_tool_invoke_plans(accessor, vec![execution.get_plan()]).await?;
                results.pop().expect("one tool invocation has one result")
            }
        },
    };
    admit_tool_response_secret_holds(accessor, response).await
}

impl TryFrom<&DiscoveredTool> for WitRegisteredTool {
    type Error = anyhow::Error;

    fn try_from(value: &DiscoveredTool) -> Result<Self, Self::Error> {
        let name = value.definition.name().unwrap_or("<unnamed tool>");
        let definition = WitTool::try_from(&value.definition).with_context(|| {
            format!(
                "failed to encode discovered tool '{name}' implemented by component {}",
                value.implemented_by
            )
        })?;

        Ok(Self {
            lookup_name: value.lookup_name.clone(),
            definition,
            implemented_by: value.implemented_by.into(),
        })
    }
}

fn classify_tool_discovery_error(error: &ToolDiscoveryError) -> HostFailureKind {
    match error {
        ToolDiscoveryError::Retrieval(_) => HostFailureKind::Transient,
        ToolDiscoveryError::Mcp(error) => match error {
            golem_service_base::clients::registry::RegistryServiceError::InternalServerError(_)
            | golem_service_base::clients::registry::RegistryServiceError::InternalClientError(_) => {
                HostFailureKind::Transient
            }
            _ => HostFailureKind::Permanent,
        },
        ToolDiscoveryError::AgentContextRequired
        | ToolDiscoveryError::MissingDeploymentRevision { .. }
        | ToolDiscoveryError::InconsistentSnapshot { .. } => HostFailureKind::Permanent,
    }
}

fn terminal_tool_discovery_error(message: String) -> anyhow::Error {
    anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Permanent,
        message,
    })
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    async fn observe_mcp_tools(
        &mut self,
        deployment: Option<&ToolDeploymentState>,
        name: Option<&ToolName>,
    ) -> Result<
        Vec<golem_common::model::oplog::payload::types::SerializableMcpImportDiscovery>,
        ToolDiscoveryError,
    > {
        use golem_common::model::mcp_import::{McpImportSource, mcp_import_bridge_source};
        use golem_common::model::oplog::payload::types::{
            SerializableDiscoveredTools, SerializableMcpImportDiscovery,
        };

        let Some(deployment) = deployment else {
            return Ok(Vec::new());
        };
        if deployment.mcp_imports.is_empty()
            || name.is_some_and(|name| deployment.registered_tools.contains_key(name))
        {
            return Ok(Vec::new());
        }
        let auth = self
            .capture_agent_auth_ctx_at_boundary()
            .await?
            .ok_or(ToolDiscoveryError::AgentContextRequired)?;
        let mut observations = Vec::new();
        for index in 0..deployment.mcp_imports.len() {
            let source = McpImportSource {
                environment_id: self.state.owned_agent_id.environment_id,
                deployment_revision: deployment.deployment_revision,
                import_index: index.try_into().map_err(|_| {
                    ToolDiscoveryError::InconsistentSnapshot {
                        details: "too many MCP imports".into(),
                    }
                })?,
                upstream_tool_name: String::new(),
            };
            let observation = self
                .state
                .environment_state_service
                .resolve_mcp_import(&source, &auth, false)
                .await
                .map_err(ToolDiscoveryError::Mcp)?;
            let found = name.is_some_and(|name| {
                observation
                    .tools
                    .iter()
                    .any(|tool| tool.definition.name() == Some(name.as_str()))
            });
            observations.push(SerializableMcpImportDiscovery {
                import_index: source.import_index,
                tools: SerializableDiscoveredTools(
                    observation
                        .tools
                        .into_iter()
                        .map(|tool| {
                            DiscoveredTool::new(tool.definition, mcp_import_bridge_source())
                        })
                        .collect(),
                ),
                exclusions: observation
                    .diagnostics
                    .into_iter()
                    .map(|diagnostic| (diagnostic.upstream_name, diagnostic.reason))
                    .collect(),
            });
            if found {
                break;
            }
        }
        Ok(observations)
    }

    async fn rehydrate_tool_discovery(
        &mut self,
        revision: Option<u64>,
        live: Option<Arc<ToolDeploymentState>>,
    ) -> anyhow::Result<Option<Arc<ToolDeploymentState>>> {
        let Some(revision) = revision else {
            return Ok(None);
        };
        let revision = revision.try_into().map_err(|error| {
            terminal_tool_discovery_error(format!(
                "recorded invalid tool deployment revision {revision}: {error}"
            ))
        })?;
        if let Some(deployment) = live.filter(|state| state.deployment_revision == revision) {
            return Ok(Some(deployment));
        }
        self.state
            .environment_state_service
            .get_tool_deployment_state_at_revision(
                self.state.owned_agent_id.environment_id,
                revision,
            )
            .await
            .map(Some)
            .map_err(|error| {
                anyhow::Error::new(ClassifiedHostError {
                    kind: classify_tool_discovery_error(&error),
                    message: format!(
                        "failed to rehydrate recorded tool deployment revision {revision}: {error}"
                    ),
                })
            })
    }

    pub(crate) async fn get_all_tools_model(&mut self) -> anyhow::Result<Vec<Arc<DiscoveredTool>>> {
        let environment_id = self.state.owned_agent_id.environment_id;
        let owner_component_metadata = self.owner_component_metadata();
        let component_id = owner_component_metadata.id;
        let component_revision = owner_component_metadata.revision;
        let binding_owner = match self.owner_context() {
            ResolvedOwnerContext::Agent(agent) => ToolBindingOwner::AgentType {
                agent_type_name: agent.agent_type.clone(),
            },
            ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                ToolBindingOwner::ComponentBaseline { component_id }
            }
        };

        let mut handle = DurableCallSession::<GolemToolGetAllTools, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let mut live_deployment = None;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = {
                loop {
                    let result = self
                        .state
                        .environment_state_service
                        .get_live_tool_deployment_state(
                            environment_id,
                            component_id,
                            component_revision,
                        )
                        .await;
                    let result = async {
                        let deployment = result?;
                        get_accessible_tools_from_deployment(
                            deployment.as_deref(),
                            &binding_owner,
                        )?;
                        let dynamic_tools =
                            self.observe_mcp_tools(deployment.as_deref(), None).await?;
                        let deployment_revision = deployment
                            .as_ref()
                            .map(|deployment| deployment.deployment_revision.get());
                        live_deployment = deployment;
                        Ok(SerializableToolDiscoverySnapshot {
                            deployment_revision,
                            dynamic_tools,
                        })
                    }
                    .await;
                    if matches!(&result, Err(ToolDiscoveryError::Mcp(golem_service_base::clients::registry::RegistryServiceError::LimitExceeded(_)))) {
                        return Err(handle.trap(GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted));
                    }
                    match handle
                        .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                        .await?
                    {
                        InternalRetryResult::Persist => break result,
                        InternalRetryResult::RetryInternally => continue,
                    }
                }
            };

            handle
                .complete(
                    self,
                    HostResponseGolemToolTools {
                        result: result.map_err(|error| error.to_string()),
                    },
                )
                .await?
        };

        let snapshot = response.result.map_err(|error| {
            terminal_tool_discovery_error(format!(
                "failed to discover tools for owner '{binding_owner:?}' in environment '{environment_id}': {error}"
            ))
        })?;
        let deployment = self
            .rehydrate_tool_discovery(snapshot.deployment_revision, live_deployment)
            .await?;
        merge_discovered_tools(deployment.as_deref(), &binding_owner, None, snapshot)
            .map_err(|error| terminal_tool_discovery_error(error.to_string()))
    }

    pub(crate) async fn get_tool_model(
        &mut self,
        tool_name: String,
    ) -> anyhow::Result<Option<Arc<DiscoveredTool>>> {
        let valid_tool_name = ToolName::try_from(tool_name.as_str()).ok();
        let environment_id = self.state.owned_agent_id.environment_id;
        let owner_component_metadata = self.owner_component_metadata();
        let component_id = owner_component_metadata.id;
        let component_revision = owner_component_metadata.revision;
        let binding_owner = match self.owner_context() {
            ResolvedOwnerContext::Agent(agent) => ToolBindingOwner::AgentType {
                agent_type_name: agent.agent_type.clone(),
            },
            ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                ToolBindingOwner::ComponentBaseline { component_id }
            }
        };

        let mut handle = DurableCallSession::<GolemToolGetTool, NotCancellable>::start(
            self,
            HostRequestGolemToolGetTool {
                name: tool_name.clone(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let mut live_deployment = None;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = {
                if valid_tool_name.is_none() {
                    Ok(SerializableToolDiscoverySnapshot {
                        deployment_revision: None,
                        dynamic_tools: Vec::new(),
                    })
                } else {
                    loop {
                        let result = self
                            .state
                            .environment_state_service
                            .get_live_tool_deployment_state(
                                environment_id,
                                component_id,
                                component_revision,
                            )
                            .await;
                        let result = async {
                            let deployment = result?;
                            if let Some(valid_tool_name) = &valid_tool_name {
                                get_accessible_tool_from_deployment(
                                    deployment.as_deref(),
                                    &binding_owner,
                                    valid_tool_name,
                                )?;
                            }
                            let dynamic_tools = self
                                .observe_mcp_tools(deployment.as_deref(), valid_tool_name.as_ref())
                                .await?;
                            let deployment_revision = deployment
                                .as_ref()
                                .map(|deployment| deployment.deployment_revision.get());
                            live_deployment = deployment;
                            Ok(SerializableToolDiscoverySnapshot {
                                deployment_revision,
                                dynamic_tools,
                            })
                        }
                        .await;
                        if matches!(&result, Err(ToolDiscoveryError::Mcp(golem_service_base::clients::registry::RegistryServiceError::LimitExceeded(_)))) {
                            return Err(handle.trap(GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted));
                        }
                        match handle
                            .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                            .await?
                        {
                            InternalRetryResult::Persist => break result,
                            InternalRetryResult::RetryInternally => continue,
                        }
                    }
                }
            };

            handle
                .complete(
                    self,
                    HostResponseGolemToolTool {
                        result: result.map_err(|error| error.to_string()),
                    },
                )
                .await?
        };

        let snapshot = response.result.map_err(|error| {
            terminal_tool_discovery_error(format!(
                "failed to discover tool '{}' for owner '{binding_owner:?}' in environment '{environment_id}': {error}",
                tool_name
            ))
        })?;
        let Some(valid_tool_name) = valid_tool_name else {
            return Ok(None);
        };
        let deployment = self
            .rehydrate_tool_discovery(snapshot.deployment_revision, live_deployment)
            .await?;
        Ok(merge_discovered_tools(
            deployment.as_deref(),
            &binding_owner,
            Some(&valid_tool_name),
            snapshot,
        )
        .map_err(|error| terminal_tool_discovery_error(error.to_string()))?
        .into_iter()
        .find(|tool| tool.lookup_name == valid_tool_name.as_str()))
    }
}

fn merge_discovered_tools(
    deployment: Option<&ToolDeploymentState>,
    owner: &ToolBindingOwner,
    selected_name: Option<&ToolName>,
    snapshot: SerializableToolDiscoverySnapshot,
) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
    let mut tools = match selected_name {
        Some(name) => get_accessible_tool_from_deployment(deployment, owner, name)?
            .into_iter()
            .collect(),
        None => get_accessible_tools_from_deployment(deployment, owner)?,
    };
    // A native name remains reserved even when it is not bound to this agent.
    let mut names = deployment
        .into_iter()
        .flat_map(|state| state.registered_tools.keys())
        .map(|name| name.as_str().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    for observation in snapshot.dynamic_tools {
        for mut tool in observation.tools.0 {
            if let Some(name) = tool.definition.name()
                && selected_name.is_none_or(|selected| selected.as_str() == name)
                && names.insert(name.to_owned())
            {
                let tool_name = ToolName::try_from(name)
                    .map_err(|details| ToolDiscoveryError::InconsistentSnapshot { details })?;
                if let Some(deployment) = deployment {
                    let configuration = &deployment.tool_middleware_configuration;
                    let environment_binding = configuration.environment_bindings.get(&tool_name);
                    let owner_binding = match owner {
                        ToolBindingOwner::AgentType { agent_type_name } => configuration
                            .agent_bindings
                            .get(agent_type_name)
                            .and_then(|bindings| bindings.get(&tool_name)),
                        ToolBindingOwner::ComponentBaseline { .. } => None,
                    };
                    let (config, readable, revealable) =
                        crate::services::environment_state::mcp_binding_scopes(
                            environment_binding,
                            owner_binding,
                        );
                    let compiled = golem_common::model::tool_middleware::compile::compile_discovered_tool_middleware_chain(
                        deployment.deployment_revision,
                        &tool.definition,
                        owner,
                        &tool_name,
                        &config,
                        &readable,
                        &revealable,
                        &deployment.registered_tool_middlewares.values().cloned().collect::<Vec<_>>(),
                        &configuration.universal,
                        environment_binding,
                        owner_binding,
                        configuration.compatibility_mode,
                    );
                    if !compiled.errors.is_empty() {
                        return Err(ToolDiscoveryError::InconsistentSnapshot {
                            details: format!(
                                "middleware for dynamic tool '{tool_name}' is incompatible: {}",
                                compiled
                                    .errors
                                    .iter()
                                    .map(|diagnostic| diagnostic.message.as_str())
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            ),
                        });
                    }
                    if let Some(chain) = compiled.chains.into_iter().next() {
                        tool.definition = chain.effective_definition;
                    }
                }
                tools.push(Arc::new(tool));
            }
        }
    }
    Ok(tools)
}

impl<Ctx: WorkerCtx> HostToolStdinWriter for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolStdinWriterEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolStdinWriterWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn write(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdinWriterEntry>,
        bytes: Vec<u8>,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        let writer = accessor.with(|mut access| {
            Ok::<_, anyhow::Error>(access.get().table().get(&self_)?.producer.writer())
        })?;
        Ok(writer.write(bytes).await)
    }

    async fn finish(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdinWriterEntry>,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        accessor.with(|mut access| Ok(access.get().table().get(&self_)?.producer.writer().finish()))
    }

    async fn fail(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdinWriterEntry>,
        reason: ByteStreamFailure,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        accessor.with(|mut access| {
            Ok(access
                .get()
                .table()
                .get(&self_)?
                .producer
                .writer()
                .fail(reason))
        })
    }
}

impl<Ctx: WorkerCtx> HostToolStdin for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolStdinEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostToolStdinClosed for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolStdinClosedEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolStdinClosedWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn wait(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdinClosedEntry>,
    ) -> anyhow::Result<ByteStreamCloseCause> {
        let observer = accessor.with(|mut access| {
            Ok::<_, anyhow::Error>(access.get().table().get(&self_)?.observer.clone())
        })?;
        Ok(observer.wait_terminal().await)
    }
}

impl<Ctx: WorkerCtx> HostToolStdout for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolStdoutEntry>) -> anyhow::Result<()> {
        let stdout = self.table().delete(rep)?;
        stdout.abandon_unconfigured();
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostToolStdoutWriter for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolStdoutWriterEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolStdoutWriterWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn write(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdoutWriterEntry>,
        bytes: Vec<u8>,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        let writer = accessor.with(|mut access| {
            Ok::<_, anyhow::Error>(access.get().table().get(&self_)?.producer.writer())
        })?;
        Ok(writer.write(bytes).await)
    }

    async fn finish(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdoutWriterEntry>,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        accessor.with(|mut access| Ok(access.get().table().get(&self_)?.producer.writer().finish()))
    }

    async fn fail(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdoutWriterEntry>,
        reason: ByteStreamFailure,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        accessor.with(|mut access| {
            Ok(access
                .get()
                .table()
                .get(&self_)?
                .producer
                .writer()
                .fail(reason))
        })
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_tools(&mut self) -> anyhow::Result<Vec<WitRegisteredTool>> {
        self.get_all_tools_model()
            .await?
            .iter()
            .map(|tool| WitRegisteredTool::try_from(tool.as_ref()))
            .collect()
    }

    async fn get_tool(&mut self, name: String) -> anyhow::Result<Option<WitRegisteredTool>> {
        self.get_tool_model(name)
            .await?
            .as_ref()
            .map(|tool| WitRegisteredTool::try_from(tool.as_ref()))
            .transpose()
    }

    async fn create_stdin(
        &mut self,
    ) -> anyhow::Result<(
        Resource<ToolStdinWriterEntry>,
        Resource<ToolStdinEntry>,
        Resource<ToolStdinClosedEntry>,
    )> {
        let memory = AttachmentMemory::for_store(
            self.public_state.worker().active_agents(),
            self.state.is_live(),
        );
        let (producer, consumer, observer) =
            attachment_pair(self.state.config.limits.max_tool_attachment_bytes, memory);
        let writer = self.table().push(ToolStdinWriterEntry { producer })?;
        let source = match self.table().push(ToolStdinEntry { consumer }) {
            Ok(source) => source,
            Err(error) => {
                self.table().delete(writer)?;
                return Err(error.into());
            }
        };
        let closed = match self.table().push(ToolStdinClosedEntry { observer }) {
            Ok(closed) => closed,
            Err(error) => {
                self.table().delete(source)?;
                self.table().delete(writer)?;
                return Err(error.into());
            }
        };
        Ok((writer, source, closed))
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn create_stdin_from_stream(
        accessor: &Accessor<U, Self>,
        source: StreamReader<Result<Vec<u8>, ByteStreamFailure>>,
    ) -> anyhow::Result<Resource<ToolStdinEntry>> {
        accessor.with(|mut access| {
            let ctx = access.get();
            let memory = AttachmentMemory::for_store(
                ctx.public_state.worker().active_agents(),
                ctx.state.is_live(),
            );
            let (producer, consumer, observer) =
                attachment_pair(ctx.state.config.limits.max_tool_attachment_bytes, memory);
            let stdin = ctx.table().push(ToolStdinEntry { consumer })?;
            let (items, received) = mpsc::unbounded_channel();
            if let Err(error) =
                source.pipe(&mut access, ToolStdinStreamConsumer::new(items, observer))
            {
                access.get().table().delete(stdin)?;
                return Err(error.into());
            }
            access.spawn(ToolStdinStreamPumpTask::<Ctx> {
                producer,
                items: received,
                _ctx: std::marker::PhantomData,
            });
            Ok(stdin)
        })
    }

    async fn create_stdout(
        accessor: &Accessor<U, Self>,
    ) -> anyhow::Result<(
        Resource<ToolStdoutEntry>,
        StreamReader<Result<Vec<u8>, ByteStreamFailure>>,
    )> {
        accessor.with(|mut access| {
            let ctx = access.get();
            let memory = AttachmentMemory::for_store(
                ctx.public_state.worker().active_agents(),
                ctx.state.is_live(),
            );
            let (producer, consumer, _) =
                attachment_pair(ctx.state.config.limits.max_tool_attachment_bytes, memory);
            let target = ctx.table().push(ToolStdoutEntry {
                producer: Some(producer),
                completion_only: false,
            })?;
            match StreamReader::new(&mut access, consumer.into_stream_producer()) {
                Ok(reader) => Ok((target, reader)),
                Err(error) => {
                    access.get().table().delete(target)?;
                    Err(error.into())
                }
            }
        })
    }

    async fn get_invoke_results(
        accessor: &Accessor<U, Self>,
        futures: Vec<Resource<FutureInvokeResultEntry>>,
    ) -> anyhow::Result<Vec<Result<InvocationResult, ToolRpcError>>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host", "get-invoke-results");
        });
        get_tool_invoke_results(accessor, &futures).await
    }
}

impl<Ctx: WorkerCtx> HostUnderlyingTool for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<UnderlyingToolEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostUnderlying for DurableWorkerCtx<Ctx> {}
impl<Ctx: WorkerCtx> HostToolStreams for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> HostToolCommon for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostUnderlyingToolWithStore<U> for ToolCommonHost<Ctx> {
    async fn invoke(
        accessor: &Accessor<U, Self>,
        self_: Resource<UnderlyingToolEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<StreamReader<Result<Vec<u8>, ByteStreamFailure>>>,
    ) -> anyhow::Result<(
        Resource<UnderlyingInvokeResultEntry>,
        Option<StreamReader<Result<Vec<u8>, ByteStreamFailure>>>,
    )> {
        let accessor = accessor.with_getter::<HasSelf<DurableWorkerCtx<Ctx>>>(accessor.getter());
        let accessor = &accessor;
        let underlying = accessor
            .with(|mut access| Ok::<_, anyhow::Error>(access.get().table().get(&self_)?.clone()))?;
        let definition = underlying.next_definition()?;
        let mut attempt = read_tool_attempt(
            accessor,
            ToolCallTarget::Underlying(underlying.clone()),
            input,
        )?;
        let mut command_path = command_path;
        let mut response_edge = None;
        if let Ok(input) = &attempt.input {
            let mut streams =
                crate::durable_host::schema_value_stream::ExecutorProjectionStreams::new(accessor);
            match boundary::prepare_underlying_tool_call(
                &underlying.position,
                command_path.clone(),
                input.clone(),
                &mut streams,
            ) {
                Ok(prepared) => {
                    command_path = prepared.command_path;
                    attempt.input = Ok(prepared.input);
                    response_edge = Some(prepared.response_edge);
                }
                Err(error) => attempt.rejection = Some(error),
            }
        }
        let stdout_requested = attempt
            .input
            .as_ref()
            .ok()
            .and_then(|input| resolve_tool_command(definition, &command_path, input).ok())
            .and_then(|command| command.stdout_required)
            .is_some();
        let stdin = stdin
            .map(|source| create_underlying_stdin(accessor, source))
            .transpose()?;
        let stdin_rep = stdin.as_ref().map(Resource::rep);
        let (stdout, stdout_consumer) = if stdout_requested {
            let (stdout, consumer) = create_stdout_attachment(accessor, false)
                .map_err(|error| cleanup_failed_tool_dispatch(accessor, stdin_rep, None, error))?;
            (Some(stdout), Some(consumer))
        } else {
            (None, None)
        };
        let stdout_rep = stdout.as_ref().map(Resource::rep);
        let dispatch = dispatch_tool_attempt(
            accessor,
            attempt,
            command_path,
            stdin,
            stdout_requested,
            EntityCallMode::Asynchronous,
            None,
            None,
            ToolAttachmentCounterparty::Guest,
        )
        .await;
        let state = match dispatch {
            Err(error) => {
                return Err(cleanup_failed_tool_dispatch(
                    accessor, stdin_rep, stdout_rep, error,
                ));
            }
            Ok(ToolCallDispatch::Rejected { response, stdin }) => {
                close_stdin(accessor, stdin)?;
                reject_stdout(accessor, stdout)?;
                FutureToolInvokeState::Ready(response)
            }
            Ok(ToolCallDispatch::Accepted(accepted)) => FutureToolInvokeState::Active(
                spawn_tool_execution(accessor, *accepted, stdout).await?,
            ),
        };
        let result = accessor.with(|mut access| {
            access.get().table().push(UnderlyingInvokeResultEntry {
                state,
                response_edge,
            })
        })?;
        let stdout = stdout_consumer
            .map(|consumer| {
                accessor.with(|mut access| {
                    StreamReader::new(&mut access, consumer.into_stream_producer())
                })
            })
            .transpose()?;
        Ok((result, stdout))
    }
}

impl<Ctx: WorkerCtx> HostUnderlyingInvokeResult for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, self_: Resource<UnderlyingInvokeResultEntry>) -> anyhow::Result<()> {
        if let FutureToolInvokeState::Active(execution) = &self.table().get(&self_)?.state {
            execution.cancel();
        }
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<UnderlyingInvokeResultEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostUnderlyingInvokeResultWithStore<U>
    for ToolCommonHost<Ctx>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        self_: Resource<UnderlyingInvokeResultEntry>,
    ) -> anyhow::Result<Result<Option<TypedSchemaValue>, UnderlyingError>> {
        let accessor = accessor.with_getter::<HasSelf<DurableWorkerCtx<Ctx>>>(accessor.getter());
        let (plan, response_edge) = accessor.with(|mut access| {
            let entry = access.get().table().get(&self_)?;
            let plan = match &entry.state {
                FutureToolInvokeState::Ready(response) => {
                    FutureToolInvokeGet::Ready(response.clone())
                }
                FutureToolInvokeState::Active(execution) => execution.get_plan(),
            };
            Ok::<_, anyhow::Error>((plan, entry.response_edge.clone()))
        })?;
        let mut responses = await_tool_invoke_plans(&accessor, vec![plan]).await?;
        let response = responses
            .pop()
            .expect("a scalar underlying result await must return one response");
        let response = if let Some(edge) = response_edge {
            let mut streams =
                crate::durable_host::schema_value_stream::ExecutorProjectionStreams::new(&accessor);
            boundary::project_underlying_tool_response(response, &edge, &mut streams)
        } else {
            response
        };
        let response = admit_tool_response_secret_holds(&accessor, response).await?;
        let response = project_tool_response(&accessor, response);
        match response {
            Ok(result) => Ok(Ok(result.result)),
            Err(ToolRpcError::RemoteToolError(error)) => Ok(Err(UnderlyingError::ToolError(error))),
            Err(ToolRpcError::Cancelled) => Ok(Err(UnderlyingError::Cancelled)),
            Err(ToolRpcError::ResourceExhausted(message)) => {
                Ok(Err(UnderlyingError::ResourceExhausted(message)))
            }
            Err(ToolRpcError::ProtocolError(message)) => {
                Ok(Err(UnderlyingError::ProtocolError(message)))
            }
            Err(ToolRpcError::Denied(message)) => Ok(Err(UnderlyingError::Denied(message))),
            Err(ToolRpcError::RemoteInternalError(message)) => {
                Ok(Err(UnderlyingError::InternalError(message)))
            }
            Err(ToolRpcError::NotFound(message)) => {
                Ok(Err(UnderlyingError::InternalError(message)))
            }
        }
    }
}

impl<Ctx: WorkerCtx> HostToolRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, tool_name: String) -> anyhow::Result<Resource<ToolRpcEntry>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "new");
        let tool_name = ToolName::try_from(tool_name).map_err(anyhow::Error::msg)?;
        let rpc = tool_rpc_for_current_owner(self, tool_name)?;
        Ok(self.table().push(rpc)?)
    }

    async fn create(
        &mut self,
        tool_name: String,
    ) -> anyhow::Result<Result<Resource<ToolRpcEntry>, ToolRpcError>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "create");
        let tool_name = match ToolName::try_from(tool_name) {
            Ok(name) => name,
            Err(error) => return Ok(Err(ToolRpcError::ProtocolError(error.to_string()))),
        };
        let rpc = match tool_rpc_for_current_owner(self, tool_name) {
            Ok(rpc) => rpc,
            Err(error) => return Ok(Err(ToolRpcError::ProtocolError(error.to_string()))),
        };
        Ok(Ok(self.table().push(rpc)?))
    }

    async fn drop(&mut self, rep: Resource<ToolRpcEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::tool-rpc", "drop");
        let _ = self.table().delete(rep);
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolRpcWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn invoke(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolRpcEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<Resource<ToolStdinEntry>>,
    ) -> anyhow::Result<Result<(), ToolRpcError>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "invoke");
        });
        let rpc = tool_rpc_resource(accessor, &self_)?;
        let stdin_rep = stdin.as_ref().map(Resource::rep);
        let dispatch = dispatch_tool_call(
            accessor,
            ToolCallTarget::Ambient(rpc),
            command_path,
            input,
            stdin,
            false,
            EntityCallMode::FireAndForget,
        )
        .await;
        match dispatch {
            Err(error) => Err(cleanup_failed_tool_dispatch(
                accessor, stdin_rep, None, error,
            )),
            Ok(dispatch) => match dispatch {
                ToolCallDispatch::Rejected { response, stdin } => {
                    close_stdin(accessor, stdin)?;
                    Ok(accessor.with(|mut access| {
                        project_tool_unit((*response).map(|_| ()), access.get())
                    }))
                }
                ToolCallDispatch::Accepted(accepted) => {
                    spawn_tool_execution(accessor, *accepted, None).await?;
                    Ok(Ok(()))
                }
            },
        }
    }

    async fn invoke_and_await(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolRpcEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<Resource<ToolStdinEntry>>,
        stdout: Option<Resource<ToolStdoutEntry>>,
    ) -> anyhow::Result<Result<InvocationResult, ToolRpcError>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "invoke-and-await");
        });
        let rpc = tool_rpc_resource(accessor, &self_)?;
        let response =
            invoke_tool_terminal(accessor, rpc, command_path, input, stdin, stdout).await?;
        Ok(project_tool_response(accessor, response))
    }

    async fn async_invoke_and_await(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolRpcEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<Resource<ToolStdinEntry>>,
        stdout: Option<Resource<ToolStdoutEntry>>,
    ) -> anyhow::Result<Resource<FutureInvokeResultEntry>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "async-invoke-and-await");
        });
        let rpc = tool_rpc_resource(accessor, &self_)?;
        let stdout_requested = stdout.is_some();
        let stdin_rep = stdin.as_ref().map(Resource::rep);
        let stdout_rep = stdout.as_ref().map(Resource::rep);
        let dispatch = dispatch_tool_call(
            accessor,
            ToolCallTarget::Ambient(rpc),
            command_path,
            input,
            stdin,
            stdout_requested,
            EntityCallMode::Asynchronous,
        )
        .await;
        match dispatch {
            Err(error) => Err(cleanup_failed_tool_dispatch(
                accessor, stdin_rep, stdout_rep, error,
            )),
            Ok(dispatch) => match dispatch {
                ToolCallDispatch::Rejected { response, stdin } => {
                    close_stdin(accessor, stdin)?;
                    reject_stdout(accessor, stdout)?;
                    let response = admit_tool_response_secret_holds(accessor, *response).await?;
                    accessor.with(|mut access| {
                        Ok(access.get().table().push(FutureInvokeResultEntry {
                            state: FutureToolInvokeState::Ready(Box::new(response)),
                        })?)
                    })
                }
                ToolCallDispatch::Accepted(accepted) => {
                    let execution = spawn_tool_execution(accessor, *accepted, stdout).await?;
                    accessor.with(|mut access| {
                        Ok(access.get().table().push(FutureInvokeResultEntry {
                            state: FutureToolInvokeState::Active(execution),
                        })?)
                    })
                }
            },
        }
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureInvokeResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        self_: Resource<FutureInvokeResultEntry>,
    ) -> anyhow::Result<Result<InvocationResult, ToolRpcError>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::future-invoke-result", "get");
        });
        let mut responses = get_tool_invoke_results(accessor, &[self_]).await?;
        Ok(responses
            .pop()
            .expect("a scalar tool result await must return one response"))
    }
}

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, self_: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "cancel");
        if let FutureToolInvokeState::Active(execution) = &self.table().get(&self_)?.state {
            execution.cancel();
        }
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ResolvedToolCommand, SkippedToolAttachmentEndpoints, ToolStdinEntry,
        ToolStdinStreamConsumer, ToolStdoutWriterEntry, WitRegisteredTool,
        await_native_entity_body, caller_tool_owner, classify_tool_discovery_error,
        cleanup_tool_endpoints, merge_discovered_tools, option_args, recorded_tool_body_is_skipped,
        resolve_tool_command, select_native_body_result, stdout_limit_error,
        terminal_tool_discovery_error, validate_declared_tool_error, validate_declared_tool_result,
        validate_native_tool_output, validate_stream_attachments,
    };
    use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
    use crate::durable_host::entity::RecordedEntityTerminal;
    use crate::durable_host::tool::attachment::{
        AttachmentMemory, ToolAttachmentModeMetadata, attachment_pair,
    };

    use crate::preview2::golem::tool::host::{ByteStreamCloseCause, ByteStreamFailure};
    use crate::services::environment_state::ToolDiscoveryError;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::application::ApplicationName;
    use golem_common::model::card::owner::ToolOwnerPattern;
    use golem_common::model::card::{
        ToolArgPattern, ToolIdentifier, ToolValueLiteral, ToolValuePattern,
    };
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::entity::{EntityCallMode, NamedToolErrorSchema, ToolOutputContract};
    use golem_common::model::environment::EnvironmentName;
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::oplog::HostResponseEntityInvocation;
    use golem_common::model::oplog::payload::types::{
        SerializableCustomToolError, SerializableEntityBodyExecution, SerializableToolError,
        SerializableToolOperationTerminal, SerializableToolResultValue, SerializableToolRpcError,
        SerializableToolStructuredResult,
    };
    use golem_common::model::oplog::payload::types::{
        SerializableDiscoveredTools, SerializableMcpImportDiscovery,
        SerializableToolDiscoverySnapshot,
    };
    use golem_common::model::tool::{
        RegisteredTool, ToolBindingInput, ToolBindingOwner, ToolDeploymentState, ToolName,
        ToolProvisionConfig, ToolSource,
    };
    use golem_common::model::tool_middleware::{
        RegisteredToolMiddleware, ToolMiddlewareInstallation, ToolMiddlewareName,
        ToolMiddlewareSource,
    };
    use golem_common::schema::tool::{
        CommandBody, CommandNode, CommandTree, Constraint, DiscoveredTool, Doc, Globals,
        MonomorphicToolMiddlewareScope, OptionShape, OptionSpec, Positional, Positionals, Ref,
        Repetition, Tool, ToolMiddleware, ToolMiddlewareScope,
    };
    use golem_common::schema::{
        IntoTypedSchemaValue, MetadataEnvelope, SchemaGraph, SchemaType, SchemaTypeDef,
        SchemaValue, TypeId, TypedSchemaValue,
    };
    use golem_schema::schema::SchemaValueStream;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Poll;
    use test_r::test;
    use test_r::timeout;
    use tokio::sync::mpsc;
    use wasmtime::component::{Component, Linker, StreamReader};
    use wasmtime::{Config, Engine, Store};

    #[test]
    #[timeout("10s")]
    async fn pending_native_entity_setup_is_cancelled_by_runner_abort() {
        let runner_abort = tokio_util::sync::CancellationToken::new();
        let setup_polled = Arc::new(tokio::sync::Notify::new());
        let setup_polled_by_future = setup_polled.clone();
        let abort = runner_abort.clone();
        let cancel = tokio::spawn(async move {
            setup_polled.notified().await;
            abort.cancel();
        });
        let setup = std::future::poll_fn(move |_| {
            setup_polled_by_future.notify_one();
            Poll::<Result<(), WorkerExecutorError>>::Pending
        });

        let error = await_native_entity_body(&runner_abort, setup)
            .await
            .unwrap_err();
        cancel.await.unwrap();

        assert!(format!("{error}").contains("native entity body was aborted"));
    }

    #[test]
    async fn pre_cancelled_runner_abort_does_not_poll_native_handler() {
        let runner_abort = tokio_util::sync::CancellationToken::new();
        runner_abort.cancel();
        let handler_polls = Arc::new(AtomicUsize::new(0));
        let handler_polls_by_future = handler_polls.clone();
        let handler = std::future::poll_fn(move |_| {
            handler_polls_by_future.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Ok::<_, WorkerExecutorError>(()))
        });

        let error = await_native_entity_body(&runner_abort, handler)
            .await
            .unwrap_err();

        assert!(format!("{error}").contains("native entity body was aborted"));
        assert_eq!(handler_polls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn native_post_body_admission_rejection_overrides_a_caught_handler_error() {
        let result =
            select_native_body_result(Ok("handler returned success"), Ok(()), true).unwrap_err();

        assert!(crate::durable_host::is_tool_attachment_live_admission_rejection(&result));
    }

    #[test]
    fn native_body_failure_precedes_parent_cleanup_failure() {
        let result = select_native_body_result::<()>(
            Err(WorkerExecutorError::runtime("body failed")),
            Err(WorkerExecutorError::runtime("cleanup failed")),
            false,
        )
        .unwrap_err();

        assert!(format!("{result}").contains("body failed"));
    }

    #[test]
    fn native_parent_cleanup_failure_replaces_a_successful_declared_result() {
        let declared_error = SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::InvalidResult("declared failure".to_string()),
        ));
        let result = select_native_body_result(
            Ok(Err::<SerializableToolStructuredResult, _>(declared_error)),
            Err(WorkerExecutorError::runtime("cleanup failed")),
            false,
        )
        .unwrap_err();

        assert!(format!("{result}").contains("cleanup failed"));
    }

    #[test]
    fn host_stdout_exhaustion_precedes_a_declared_tool_error() {
        let declared = SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::InvalidResult("declared failure".to_string()),
        ));
        let selected = stdout_limit_error(true).unwrap_or(declared);
        assert!(matches!(
            selected,
            SerializableToolRpcError::ResourceExhausted(_)
        ));
        assert!(stdout_limit_error(false).is_none());
    }

    fn typed_wasmtime_writer_component(engine: &Engine) -> Component {
        Component::new(
            engine,
            r#"
(component
  (import "attach" (func $attach))
  (core func $attach (canon lower (func $attach)))
  (core module $memory
    (memory (export "mem") 1)
    (data (i32.const 128) "\01\02\03\00\04\05\06\00\07\08\09\00\0a\0b\0c")
  )
  (core instance $memory (instantiate $memory))
  (core module $core
    (import "" "mem" (memory 1))
    (import "" "stream.new" (func $stream.new (result i64)))
    (import "" "stream.write-async" (func $stream.write-async (param i32 i32 i32) (result i32)))
    (import "" "stream.write-sync" (func $stream.write-sync (param i32 i32 i32) (result i32)))
    (import "" "stream.cancel-write" (func $stream.cancel-write (param i32) (result i32)))
    (import "" "stream.drop-writable" (func $stream.drop-writable (param i32)))
    (import "" "attach" (func $attach))
    (global $writer (mut i32) (i32.const 0))
    (func $set-item (param $item i32) (param $bytes i32)
      (i32.store8 (local.get $item) (i32.const 0))
      (i32.store (i32.add (local.get $item) (i32.const 4)) (local.get $bytes))
      (i32.store (i32.add (local.get $item) (i32.const 8)) (i32.const 3))
    )
    (func (export "start") (result i32)
      (local $pair i64)
      (call $set-item (i32.const 0) (i32.const 128))
      (call $set-item (i32.const 16) (i32.const 132))
      (call $set-item (i32.const 32) (i32.const 136))
      (call $set-item (i32.const 48) (i32.const 140))
      (local.set $pair (call $stream.new))
      (global.set $writer (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
      (i32.wrap_i64 (local.get $pair))
    )
    (func (export "cancel-without-acknowledgement")
      (local $result i32)
      (local.set $result (call $stream.write-async (global.get $writer) (i32.const 48) (i32.const 1)))
      (if (i32.ne (local.get $result) (i32.const -1)) (then unreachable))
      (call $attach)
      (local.set $result (call $stream.cancel-write (global.get $writer)))
      (if (i32.ne (local.get $result) (i32.const 2)) (then unreachable))
    )
    (func (export "attach") (call $attach))
    (func (export "write-first")
      (local $result i32)
      (local.set $result (call $stream.write-sync (global.get $writer) (i32.const 0) (i32.const 1)))
      (if (i32.ne (local.get $result) (i32.const 16)) (then unreachable))
    )
    (func (export "cancel-with-pending-acknowledgement")
      (local $result i32)
      (local.set $result (call $stream.write-async (global.get $writer) (i32.const 16) (i32.const 1)))
      (if (i32.ne (local.get $result) (i32.const -1)) (then unreachable))
      (local.set $result (call $stream.cancel-write (global.get $writer)))
      (if (i32.ne (local.get $result) (i32.const 2)) (then unreachable))
    )
    (func (export "resume-and-close")
      (local $result i32)
      (local.set $result (call $stream.write-sync (global.get $writer) (i32.const 32) (i32.const 1)))
      (if (i32.ne (local.get $result) (i32.const 16)) (then unreachable))
      (call $stream.drop-writable (global.get $writer))
    )
  )
  (type $failure (variant
    (case "cancelled")
    (case "abandoned")
    (case "resource-exhausted")
    (case "failed" string)
  ))
  (export $failure-export "byte-stream-failure" (type $failure))
  (type $item (result (list u8) (error $failure-export)))
  (export $item-export "byte-stream-item" (type $item))
  (type $stream (stream $item-export))
  (core func $stream.new (canon stream.new $stream))
  (core func $stream.write-async (canon stream.write $stream async (memory $memory "mem")))
  (core func $stream.write-sync (canon stream.write $stream (memory $memory "mem")))
  (core func $stream.cancel-write (canon stream.cancel-write $stream))
  (core func $stream.drop-writable (canon stream.drop-writable $stream))
  (core instance $core (instantiate $core (with "" (instance
    (export "mem" (memory $memory "mem"))
    (export "stream.new" (func $stream.new))
    (export "stream.write-async" (func $stream.write-async))
    (export "stream.write-sync" (func $stream.write-sync))
    (export "stream.cancel-write" (func $stream.cancel-write))
    (export "stream.drop-writable" (func $stream.drop-writable))
    (export "attach" (func $attach))
  ))))
  (func (export "start") async (result (stream $item-export))
    (canon lift (core func $core "start")))
  (func (export "cancel-without-acknowledgement") async
    (canon lift (core func $core "cancel-without-acknowledgement")))
  (func (export "attach") async
    (canon lift (core func $core "attach")))
  (func (export "write-first") async
    (canon lift (core func $core "write-first")))
  (func (export "cancel-with-pending-acknowledgement") async
    (canon lift (core func $core "cancel-with-pending-acknowledgement")))
  (func (export "resume-and-close") async
    (canon lift (core func $core "resume-and-close")))
)
            "#,
        )
        .unwrap()
    }

    async fn assert_typed_stdin_operation_cancellation(pending_acknowledgement: bool) {
        let (_attachment_producer, _attachment_consumer, observer) =
            attachment_pair(3, AttachmentMemory::inert());
        let (items, mut received) = mpsc::unbounded_channel();
        let mut config = Config::new();
        config.concurrency_support(true);
        config.wasm_component_model_more_async_builtins(true);
        let engine = Engine::new(&config).unwrap();
        let component = typed_wasmtime_writer_component(&engine);
        let mut store = Store::new(&engine, ());
        let reader_slot = std::sync::Arc::new(std::sync::Mutex::new(
            None::<StreamReader<Result<Vec<u8>, ByteStreamFailure>>>,
        ));
        let reader_slot_for_attach = reader_slot.clone();
        let observer_during_cancellation = observer.clone();
        let mut linker = Linker::new(&engine);
        linker
            .root()
            .func_wrap("attach", move |mut store, (): ()| {
                reader_slot_for_attach
                    .lock()
                    .unwrap()
                    .take()
                    .expect("typed stream reader was not ready to attach")
                    .pipe(
                        &mut store,
                        ToolStdinStreamConsumer::new(items.clone(), observer.clone()),
                    )?;
                Ok(())
            })
            .unwrap();
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let start = instance
            .get_typed_func::<(), (StreamReader<Result<Vec<u8>, ByteStreamFailure>>,)>(
                &mut store, "start",
            )
            .unwrap();
        let attach = instance
            .get_typed_func::<(), ()>(&mut store, "attach")
            .unwrap();
        let write_first = instance
            .get_typed_func::<(), ()>(&mut store, "write-first")
            .unwrap();
        let cancel = instance
            .get_typed_func::<(), ()>(
                &mut store,
                if pending_acknowledgement {
                    "cancel-with-pending-acknowledgement"
                } else {
                    "cancel-without-acknowledgement"
                },
            )
            .unwrap();
        let resume = instance
            .get_typed_func::<(), ()>(&mut store, "resume-and-close")
            .unwrap();

        store
            .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
                let (reader,) = start.call_concurrent(accessor, ()).await?;
                *reader_slot.lock().unwrap() = Some(reader);
                let first = if pending_acknowledgement {
                    attach.call_concurrent(accessor, ()).await?;
                    write_first.call_concurrent(accessor, ()).await?;
                    let first = received.recv().await.unwrap();
                    assert_eq!(first.item.as_ref().unwrap(), &vec![1, 2, 3]);
                    Some(first)
                } else {
                    None
                };
                cancel.call_concurrent(accessor, ()).await?;
                assert!(received.try_recv().is_err());
                assert!(observer_during_cancellation.terminal_snapshot().is_none());
                if let Some(first) = first {
                    first.acknowledged.send(()).unwrap();
                }

                resume.call_concurrent(accessor, ()).await?;
                let resumed = received.recv().await.unwrap();
                assert_eq!(resumed.item.unwrap(), vec![7, 8, 9]);
                let _ = resumed.acknowledged.send(());
                assert!(received.try_recv().is_err());
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    async fn cancelling_typed_stdin_without_acknowledgement_resumes_without_false_eof() {
        assert_typed_stdin_operation_cancellation(false).await;
    }

    #[test]
    async fn cancelling_typed_stdin_with_pending_acknowledgement_preserves_it_and_resumes() {
        assert_typed_stdin_operation_cancellation(true).await;
    }

    #[test]
    fn failed_dispatch_attempts_both_endpoint_cleanups_and_preserves_the_original_error() {
        let attempted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stdin_attempted = attempted.clone();
        let stdout_attempted = attempted.clone();
        let error = cleanup_tool_endpoints(
            anyhow::anyhow!("dispatch failed"),
            move || {
                stdin_attempted.lock().unwrap().push("stdin");
                Err(anyhow::anyhow!("missing stdin"))
            },
            move || {
                stdout_attempted.lock().unwrap().push("stdout");
                Err(anyhow::anyhow!("missing stdout"))
            },
        );

        assert_eq!(*attempted.lock().unwrap(), vec!["stdin", "stdout"]);
        let message = format!("{error:#}");
        assert!(message.contains("dispatch failed"));
        assert!(message.contains("stdin cleanup failed: missing stdin"));
        assert!(message.contains("stdout cleanup failed: missing stdout"));
    }

    #[test]
    fn completed_skipped_terminal_selects_no_body_replay() {
        let response = HostResponseEntityInvocation {
            result: Ok(SerializableToolOperationTerminal {
                body_execution: SerializableEntityBodyExecution::Skipped,
                result: Err(SerializableToolRpcError::ResourceExhausted(
                    "recorded limit".to_string(),
                )),
            }
            .into_typed_schema_value()
            .unwrap()),
        };
        assert!(
            recorded_tool_body_is_skipped(&RecordedEntityTerminal::Completed(response)).unwrap()
        );
    }

    #[test]
    fn skipped_replay_publishes_recorded_failure_before_endpoint_roles_drop() {
        for failure in [
            ByteStreamFailure::ResourceExhausted,
            ByteStreamFailure::Cancelled,
        ] {
            let (stdin_producer, stdin_consumer, stdin_observer) =
                attachment_pair(16, AttachmentMemory::inert());
            let (stdout_producer, stdout_consumer, stdout_observer) =
                attachment_pair(16, AttachmentMemory::inert());
            let endpoints = SkippedToolAttachmentEndpoints {
                stdin: Some(ToolStdinEntry {
                    consumer: stdin_consumer,
                }),
                stdout: Some(ToolStdoutWriterEntry {
                    producer: stdout_producer,
                    completion_only: false,
                }),
            };
            let controllers = endpoints.controllers();

            endpoints.publish_failure(&controllers, failure.clone());

            assert_eq!(
                controllers.0.as_ref().unwrap().metadata().mode,
                ToolAttachmentModeMetadata::TerminalOnly
            );
            assert_eq!(
                controllers.1.as_ref().unwrap().metadata().mode,
                ToolAttachmentModeMetadata::TerminalOnly
            );
            for observer in [&stdin_observer, &stdout_observer] {
                let Some(ByteStreamCloseCause::Failed(actual)) = observer.terminal() else {
                    panic!("skipped replay endpoint did not retain its recorded failure");
                };
                assert!(matches!(
                    (&failure, actual),
                    (
                        ByteStreamFailure::ResourceExhausted,
                        ByteStreamFailure::ResourceExhausted
                    ) | (ByteStreamFailure::Cancelled, ByteStreamFailure::Cancelled)
                ));
            }

            drop(stdin_producer);
            drop(stdout_consumer);
        }
    }

    fn registered_tool() -> (RegisteredTool, ComponentId) {
        let component_id = ComponentId::new();
        let definition = Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "search".to_string(),
                    aliases: vec!["find".to_string()],
                    doc: Doc {
                        summary: "Search documents".to_string(),
                        description: "Searches indexed documents".to_string(),
                        examples: Vec::new(),
                    },
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: Some(CommandBody {
                        positionals: Positionals {
                            fixed: vec![Positional {
                                name: "query".to_string(),
                                doc: Doc::default(),
                                value_name: Some("QUERY".to_string()),
                                type_: SchemaType::string().with_metadata(MetadataEnvelope {
                                    doc: Some("Text to search for".to_string()),
                                    aliases: vec!["term".to_string()],
                                    ..Default::default()
                                }),
                                default: None,
                                required: true,
                                accepts_stdio: false,
                            }],
                            tail: None,
                        },
                        options: Vec::new(),
                        flags: Vec::new(),
                        constraints: Vec::new(),
                        stdin: None,
                        stdout: None,
                        result: None,
                        errors: Vec::new(),
                        annotations: None,
                    }),
                }],
            },
            schema: SchemaGraph::empty(),
        };

        (
            RegisteredTool {
                deployment_revision: DeploymentRevision::try_from(1_u64).unwrap(),
                release_id: None,
                definition,
                provision: ToolProvisionConfig::default(),
                component_bindings: Default::default(),
                source: ToolSource::Component {
                    component_id,
                    component_revision: ComponentRevision::try_from(7_u64).unwrap(),
                    component_name: ComponentName("search-tools".to_string()),
                },
                owner_account_id: AccountId::new(),
                owner_account_email: AccountEmail::new("owner@example.com"),
                metadata_version: "0.1.0".to_string(),
                metadata_digest: Default::default(),
            },
            component_id,
        )
    }

    fn dynamic_middleware_deployment(expected: Tool) -> (ToolDeploymentState, ToolBindingOwner) {
        let revision = DeploymentRevision::INITIAL;
        let tool_name = ToolName::try_from("search").unwrap();
        let middleware_name = ToolMiddlewareName::try_from("project").unwrap();
        let mut presented = expected.clone();
        presented.commands.nodes[0].name = "effective-search".to_string();
        presented.commands.nodes[0].doc.summary = "Effective projected metadata".to_string();
        let registration = RegisteredToolMiddleware {
            deployment_revision: revision,
            release_id: None,
            definition: ToolMiddleware {
                name: middleware_name.to_string(),
                version: "1.0.0".to_string(),
                aliases: Vec::new(),
                doc: Doc::default(),
                parameter_schema: SchemaGraph::empty(),
                scope: ToolMiddlewareScope::Monomorphic(Box::new(MonomorphicToolMiddlewareScope {
                    presented,
                    expected: Some(expected),
                })),
            },
            provision: ToolProvisionConfig::default(),
            source: ToolMiddlewareSource::Component {
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::INITIAL,
                component_name: ComponentName("middleware:project".to_string()),
            },
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("middleware@example.com"),
            metadata_version: "0.1.0".to_string(),
            metadata_digest: Default::default(),
        };
        let installation = ToolMiddlewareInstallation {
            name: middleware_name.clone(),
            version: Some("1.0.0".to_string()),
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            account: Some(AccountEmail::new("middleware@example.com")),
            secret_keys_readable: None,
            secret_keys_revealable: None,
            filesystem_access: Default::default(),
        };
        let agent = AgentTypeName("Agent".to_string());
        let owner = ToolBindingOwner::AgentType {
            agent_type_name: agent.clone(),
        };
        (
            ToolDeploymentState {
                deployment_revision: revision,
                registered_tools: BTreeMap::new(),
                tool_bindings: BTreeMap::new(),
                mcp_imports: Vec::new(),
                tool_middleware_configuration:
                    golem_common::model::tool_middleware::ToolMiddlewareConfiguration {
                        universal: Vec::new(),
                        compatibility_mode: Default::default(),
                        environment_bindings: BTreeMap::from([(
                            tool_name,
                            ToolBindingInput {
                                middleware: Some(vec![installation]),
                                ..Default::default()
                            },
                        )]),
                        agent_bindings: BTreeMap::new(),
                    },
                registered_tool_middlewares: BTreeMap::from([(middleware_name, registration)]),
                tool_middleware_chains: BTreeMap::new(),
            },
            owner,
        )
    }

    fn discovery_snapshot(tools: Vec<DiscoveredTool>) -> SerializableToolDiscoverySnapshot {
        SerializableToolDiscoverySnapshot {
            deployment_revision: Some(DeploymentRevision::INITIAL.get()),
            dynamic_tools: vec![SerializableMcpImportDiscovery {
                import_index: 0,
                tools: SerializableDiscoveredTools(tools),
                exclusions: Vec::new(),
            }],
        }
    }

    #[test]
    fn dynamic_discovery_presents_effective_metadata_without_changing_lookup_name() {
        use golem_common::model::mcp_import::mcp_import_bridge_source;
        use golem_mcp_import::tool::{Limits, ProjectedTool};

        let projected = ProjectedTool::new(
            &serde_json::json!({
                "name": "search",
                "description": "Genuine upstream metadata",
                "inputSchema": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"],
                    "additionalProperties": false
                },
                "outputSchema": { "type": "string" }
            }),
            "search",
            Limits::default(),
        )
        .unwrap();
        let discovered =
            DiscoveredTool::new(projected.definition.clone(), mcp_import_bridge_source());
        let (deployment, owner) = dynamic_middleware_deployment(projected.definition);
        let selected = ToolName::try_from("search").unwrap();

        for tools in [
            merge_discovered_tools(
                Some(&deployment),
                &owner,
                None,
                discovery_snapshot(vec![discovered.clone()]),
            )
            .unwrap(),
            merge_discovered_tools(
                Some(&deployment),
                &owner,
                Some(&selected),
                discovery_snapshot(vec![discovered]),
            )
            .unwrap(),
        ] {
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].lookup_name, "search");
            assert_eq!(tools[0].definition.name(), Some("effective-search"));
            assert_eq!(
                tools[0].definition.commands.nodes[0].doc.summary,
                "Effective projected metadata"
            );
        }
    }

    #[test]
    fn dynamic_discovery_rejects_first_incompatible_collision_without_selecting_later_tool() {
        use golem_common::model::mcp_import::mcp_import_bridge_source;
        use golem_mcp_import::tool::{Limits, ProjectedTool};

        let compatible = ProjectedTool::new(
            &serde_json::json!({
                "name": "search",
                "inputSchema": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"],
                    "additionalProperties": false
                }
            }),
            "search",
            Limits::default(),
        )
        .unwrap();
        let changed = ProjectedTool::new(
            &serde_json::json!({
                "name": "search",
                "inputSchema": {
                    "type": "object",
                    "properties": { "query": { "type": "integer" } },
                    "required": ["query"],
                    "additionalProperties": false
                }
            }),
            "search",
            Limits::default(),
        )
        .unwrap();
        let (deployment, owner) = dynamic_middleware_deployment(compatible.definition.clone());
        let selected = ToolName::try_from("search").unwrap();
        let error = merge_discovered_tools(
            Some(&deployment),
            &owner,
            Some(&selected),
            discovery_snapshot(vec![
                DiscoveredTool::new(changed.definition, mcp_import_bridge_source()),
                DiscoveredTool::new(compatible.definition, mcp_import_bridge_source()),
            ]),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ToolDiscoveryError::InconsistentSnapshot { .. }
        ));
        assert!(
            error
                .to_string()
                .contains("middleware for dynamic tool 'search' is incompatible")
        );
    }

    #[test]
    fn registered_component_tool_converts_to_discovery_wit_record() {
        let (registered, component_id) = registered_tool();
        let expected_definition = registered.definition.clone();

        let discovered = DiscoveredTool::from(registered);
        assert_eq!(discovered.lookup_name, "search");
        assert_eq!(discovered.definition, expected_definition);
        assert_eq!(discovered.implemented_by, component_id);

        let wit = WitRegisteredTool::try_from(&discovered).unwrap();
        assert_eq!(
            Tool::try_from(&wit.definition).unwrap(),
            expected_definition
        );
        assert_eq!(ComponentId::from(wit.implemented_by), component_id);
        assert_eq!(wit.lookup_name, "search");
    }

    #[test]
    fn tool_authorization_owner_is_entirely_caller_derived() {
        let caller_account = AccountEmail::new("consumer@example.com");
        let publisher_account = AccountEmail::new("publisher@example.com");
        let application = ApplicationName::try_from("consumer-application").unwrap();
        let environment = EnvironmentName::try_from("consumer-environment").unwrap();
        let component = ComponentName("consumer:agent".to_string());
        let tool_name = ToolName::try_from("search").unwrap();

        let owner = caller_tool_owner(
            &caller_account,
            &application,
            &environment,
            &component,
            &tool_name,
        );

        assert_eq!(
            owner,
            ToolOwnerPattern::Tool {
                account: caller_account.clone(),
                application,
                environment,
                component,
                tool: tool_name.to_string(),
            }
        );
        let ToolOwnerPattern::Tool { account, .. } = owner else {
            panic!("authorization owner must identify one caller-owned tool")
        };
        assert_eq!(account, caller_account);
        assert_ne!(account, publisher_account);
    }

    #[test]
    fn command_resolution_validates_path_schema_value_and_constraints() {
        let (registered, _) = registered_tool();
        let graph = registered
            .definition
            .canonical_input_record_schema(0)
            .unwrap();
        let input = TypedSchemaValue::new(
            graph,
            SchemaValue::Record {
                fields: vec![SchemaValue::String("needle".to_string())],
            },
        );

        let resolved = resolve_tool_command(&registered.definition, &[], &input).unwrap();
        assert_eq!(
            resolved.args,
            vec![ToolArgPattern::Positional(ToolValuePattern::Literal(
                ToolValueLiteral("\"needle\"".to_string())
            ))]
        );
        assert!(matches!(
            resolve_tool_command(&registered.definition, &["missing".to_string()], &input),
            Err(SerializableToolError::InvalidCommandPath(_))
        ));

        let wrong_schema = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::bool()),
            SchemaValue::Bool(true),
        );
        assert!(matches!(
            resolve_tool_command(&registered.definition, &[], &wrong_schema),
            Err(SerializableToolError::InvalidInput(_))
        ));

        let mut constrained = registered.definition.clone();
        constrained.commands.nodes[0]
            .body
            .as_mut()
            .unwrap()
            .constraints = vec![Constraint::RequiresAll(vec![Ref::Present(
            "not-supplied".to_string(),
        )])];
        assert!(matches!(
            resolve_tool_command(&constrained, &[], &input),
            Err(SerializableToolError::ConstraintViolation(_))
        ));
    }

    #[test]
    fn command_resolution_omits_optional_values_and_renders_supplied_values() {
        let (mut registered, _) = registered_tool();
        let body = registered.definition.commands.nodes[0]
            .body
            .as_mut()
            .unwrap();
        body.positionals.fixed[0].required = false;
        body.options.push(OptionSpec {
            long: "mode".to_string(),
            short: None,
            aliases: Vec::new(),
            doc: Doc::default(),
            value_name: None,
            shape: OptionShape::Scalar(SchemaType::string()),
            default: None,
            required: false,
            env_var: None,
        });
        let graph = registered
            .definition
            .canonical_input_record_schema(0)
            .unwrap();
        for (fields, expected) in [
            (
                vec![
                    SchemaValue::Option { inner: None },
                    SchemaValue::Option { inner: None },
                ],
                Vec::<ToolArgPattern>::new(),
            ),
            (
                vec![
                    SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::String("needle".to_string()))),
                    },
                    SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::String("fast".to_string()))),
                    },
                ],
                vec![
                    ToolArgPattern::Positional(ToolValuePattern::Literal(ToolValueLiteral(
                        "\"needle\"".to_string(),
                    ))),
                    ToolArgPattern::LongFlag {
                        name: ToolIdentifier("mode".to_string()),
                        value: Some(ToolValuePattern::Literal(ToolValueLiteral(
                            "\"fast\"".to_string(),
                        ))),
                    },
                ],
            ),
        ] {
            let input = TypedSchemaValue::new(graph.clone(), SchemaValue::Record { fields });
            let resolved = resolve_tool_command(&registered.definition, &[], &input).unwrap();
            assert_eq!(resolved.args, expected);
        }
    }

    #[test]
    fn stream_arguments_require_opaque_permission_without_losing_literal_constraints() {
        use golem_common::model::card::{
            ResourcePattern, ToolInvocationPattern, ToolResourcePattern,
        };

        let (mut registered, _) = registered_tool();
        let body = registered.definition.commands.nodes[0]
            .body
            .as_mut()
            .unwrap();
        let mut stream_argument = body.positionals.fixed[0].clone();
        stream_argument.name = "input".to_string();
        stream_argument.type_ = SchemaType::stream(Some(SchemaType::u32()));
        body.positionals.fixed.push(stream_argument);
        let input = TypedSchemaValue::new(
            registered
                .definition
                .canonical_input_record_schema(0)
                .unwrap(),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("needle".to_string()),
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(())),
                ],
            },
        );
        let resolved = resolve_tool_command(&registered.definition, &[], &input).unwrap();
        assert_eq!(
            resolved.args,
            vec![
                ToolArgPattern::Positional(ToolValuePattern::Literal(ToolValueLiteral(
                    "\"needle\"".to_string()
                ))),
                ToolArgPattern::Positional(ToolValuePattern::Star),
            ]
        );
        let target = ToolResourcePattern::Invocation(ToolInvocationPattern {
            command_path: Some(vec![ToolIdentifier("consume".to_string())]),
            args: resolved.args,
        });
        assert!(
            ToolResourcePattern::parse_resource("consume \"needle\" *")
                .unwrap()
                .subsumes(&target)
        );
        assert!(
            !ToolResourcePattern::parse_resource("consume \"other\" *")
                .unwrap()
                .subsumes(&target)
        );
        assert!(
            !ToolResourcePattern::parse_resource("consume \"needle\" literal")
                .unwrap()
                .subsumes(&target)
        );
        assert!(ToolResourcePattern::AnyInvocation.subsumes(&target));
    }

    #[test]
    fn present_optional_option_value_renders_its_inner_type() {
        let (registered, _) = registered_tool();
        let option = OptionSpec {
            long: "limit".to_string(),
            short: None,
            aliases: vec![],
            doc: Doc::default(),
            value_name: None,
            default: None,
            required: false,
            env_var: None,
            shape: OptionShape::Scalar(SchemaType::option(SchemaType::s64())),
        };
        assert!(
            option_args(
                &registered.definition,
                &option,
                &SchemaValue::Option { inner: None }
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            option_args(
                &registered.definition,
                &option,
                &SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::S64(3)))
                }
            )
            .unwrap(),
            vec![ToolArgPattern::LongFlag {
                name: ToolIdentifier("limit".to_string()),
                value: Some(ToolValuePattern::Literal(ToolValueLiteral("3".to_string())))
            }]
        );
    }

    #[test]
    fn present_optional_option_with_referenced_option_type_preserves_inner_option() {
        let (mut registered, _) = registered_tool();
        let optional_limit = TypeId::new("optional-limit");
        registered.definition.schema.defs.push(SchemaTypeDef {
            id: optional_limit.clone(),
            name: Some("OptionalLimit".to_string()),
            body: SchemaType::option(SchemaType::s64()),
        });
        let option = OptionSpec {
            long: "limit".to_string(),
            short: None,
            aliases: vec![],
            doc: Doc::default(),
            value_name: None,
            default: None,
            required: false,
            env_var: None,
            shape: OptionShape::Scalar(SchemaType::ref_to(optional_limit)),
        };
        let value = SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::S64(3))),
            })),
        };

        assert_eq!(
            option_args(&registered.definition, &option, &value).unwrap(),
            vec![ToolArgPattern::LongFlag {
                name: ToolIdentifier("limit".to_string()),
                value: Some(ToolValuePattern::Literal(ToolValueLiteral(
                    "some(3)".to_string()
                )))
            }]
        );
    }

    #[test]
    fn stream_option_values_preserve_repetition_boundaries() {
        use golem_common::schema::tool::RepeatableListShape;

        let (registered, _) = registered_tool();
        let mut option = OptionSpec {
            long: "input".to_string(),
            short: None,
            aliases: vec![],
            doc: Doc::default(),
            value_name: None,
            default: None,
            required: true,
            env_var: None,
            shape: OptionShape::RepeatableList(RepeatableListShape {
                repetition: Repetition::Repeated,
                item_type: SchemaType::option(SchemaType::stream(Some(SchemaType::u32()))),
            }),
        };
        let value = SchemaValue::List {
            elements: vec![
                SchemaValue::Option { inner: None },
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::Stream(
                        SchemaValueStream::from_host_endpoint(()),
                    ))),
                },
            ],
        };
        let repeated = option_args(&registered.definition, &option, &value).unwrap();
        assert_eq!(repeated.len(), 2);
        assert!(matches!(
            &repeated[0],
            ToolArgPattern::LongFlag {
                value: Some(ToolValuePattern::Literal(_)),
                ..
            }
        ));
        let opaque = ToolArgPattern::LongFlag {
            name: ToolIdentifier("input".to_string()),
            value: Some(ToolValuePattern::Star),
        };
        assert_eq!(repeated[1], opaque);
        for repetition in [Repetition::Delimited(','), Repetition::Either(':')] {
            let OptionShape::RepeatableList(shape) = &mut option.shape else {
                unreachable!()
            };
            shape.repetition = repetition;
            assert_eq!(
                option_args(&registered.definition, &option, &value).unwrap(),
                vec![opaque.clone()]
            );
        }
    }

    #[test]
    fn declared_tool_error_validation_uses_name_before_payload() {
        let string = SchemaGraph::anonymous(SchemaType::string());
        let contract = ToolOutputContract {
            result: None,
            errors: vec![
                NamedToolErrorSchema {
                    name: "first".to_string(),
                    payload: string.clone(),
                },
                NamedToolErrorSchema {
                    name: "second".to_string(),
                    payload: string.clone(),
                },
            ],
        };
        let error = SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                name: "second".to_string(),
                payload: TypedSchemaValue::new(string, SchemaValue::String("details".to_string())),
            })),
        ));

        assert_eq!(
            validate_declared_tool_error(error.clone(), &contract),
            error
        );
    }

    #[test]
    fn declared_payloadless_tool_error_requires_unit_payload() {
        let unit = SchemaGraph::anonymous(SchemaType::tuple(Vec::new()));
        let contract = ToolOutputContract {
            result: None,
            errors: vec![NamedToolErrorSchema {
                name: "not-found".to_string(),
                payload: unit.clone(),
            }],
        };
        let error = SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                name: "not-found".to_string(),
                payload: TypedSchemaValue::new(
                    unit,
                    SchemaValue::Tuple {
                        elements: Vec::new(),
                    },
                ),
            })),
        ));

        assert_eq!(
            validate_declared_tool_error(error.clone(), &contract),
            error
        );
    }

    #[test]
    fn ordinary_payloadless_tool_result_remains_valid() {
        let contract = ToolOutputContract {
            result: None,
            errors: Vec::new(),
        };

        assert_eq!(validate_declared_tool_result(None, &contract), Ok(None));
    }

    #[test]
    fn native_output_validation_accepts_a_typed_declared_result() {
        let schema = SchemaGraph::anonymous(SchemaType::u64());
        let value = TypedSchemaValue::new(schema.clone(), SchemaValue::U64(42));
        let result = SerializableToolStructuredResult {
            result: Some(SerializableToolResultValue::from_typed(&value).unwrap()),
        };
        let contract = ToolOutputContract {
            result: Some(schema),
            errors: Vec::new(),
        };

        assert_eq!(
            validate_native_tool_output(Ok(result.clone()), &contract),
            Ok(result)
        );
    }

    #[test]
    fn native_output_validation_rejects_undeclared_error_name_and_wrong_result_schema() {
        let string = SchemaGraph::anonymous(SchemaType::string());
        let contract = ToolOutputContract {
            result: Some(string.clone()),
            errors: vec![NamedToolErrorSchema {
                name: "declared".to_string(),
                payload: string,
            }],
        };
        let undeclared = SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                name: "other".to_string(),
                payload: TypedSchemaValue::new(
                    SchemaGraph::anonymous(SchemaType::string()),
                    SchemaValue::String("details".to_string()),
                ),
            })),
        ));
        let wrong_result = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::bool()),
            SchemaValue::Bool(true),
        );

        for result in [
            validate_native_tool_output(Err(undeclared), &contract),
            validate_native_tool_output(
                Ok(SerializableToolStructuredResult {
                    result: Some(SerializableToolResultValue::from_typed(&wrong_result).unwrap()),
                }),
                &contract,
            ),
        ] {
            assert!(matches!(
                result,
                Err(SerializableToolRpcError::RemoteToolError(error))
                    if matches!(*error, SerializableToolError::InvalidResult(_))
            ));
        }
    }

    #[test]
    fn stream_attachment_validation_uses_actual_attachments_and_call_mode() {
        let command = ResolvedToolCommand {
            command_index: 0,
            args: Vec::new(),
            stdin_required: None,
            stdout_required: None,
        };
        assert!(matches!(
            validate_stream_attachments(
                &command,
                &["search".to_string()],
                true,
                false,
                EntityCallMode::Synchronous,
            ),
            Err(SerializableToolRpcError::ProtocolError(_))
        ));
        assert!(matches!(
            validate_stream_attachments(
                &command,
                &["search".to_string()],
                false,
                true,
                EntityCallMode::Synchronous,
            ),
            Err(SerializableToolRpcError::ProtocolError(_))
        ));
        assert!(
            validate_stream_attachments(
                &command,
                &["search".to_string()],
                false,
                false,
                EntityCallMode::FireAndForget,
            )
            .is_ok()
        );
    }

    #[test]
    fn tool_discovery_error_classification_preserves_integrity_semantics() {
        let retrieval = ToolDiscoveryError::Retrieval(WorkerExecutorError::runtime("offline"));
        let agent_context_required = ToolDiscoveryError::AgentContextRequired;
        let inconsistent = ToolDiscoveryError::InconsistentSnapshot {
            details: "dangling binding".to_string(),
        };

        assert_eq!(
            classify_tool_discovery_error(&retrieval),
            HostFailureKind::Transient
        );
        assert_eq!(
            classify_tool_discovery_error(&agent_context_required),
            HostFailureKind::Permanent
        );
        assert_eq!(
            classify_tool_discovery_error(&inconsistent),
            HostFailureKind::Permanent
        );
    }

    #[test]
    fn terminal_tool_discovery_errors_remain_permanent() {
        let error = terminal_tool_discovery_error("discovery failed".to_string());
        let classified = error.downcast_ref::<ClassifiedHostError>().unwrap();

        assert_eq!(classified.kind, HostFailureKind::Permanent);
        assert_eq!(classified.message, "discovery failed");
    }

    #[test]
    fn discovered_tool_wit_encoding_error_has_tool_and_source_context() {
        let (registered, component_id) = registered_tool();
        let mut discovered = DiscoveredTool::from(registered);
        let duplicate_id = TypeId::new("duplicate");
        let duplicate_definition = SchemaTypeDef {
            id: duplicate_id,
            name: Some("Duplicate".to_string()),
            body: SchemaType::string(),
        };
        discovered.definition.schema.defs =
            vec![duplicate_definition.clone(), duplicate_definition];

        let error = match WitRegisteredTool::try_from(&discovered) {
            Ok(_) => panic!("duplicate schema definitions must fail WIT encoding"),
            Err(error) => error,
        };
        let message = format!("{error:#}");

        assert!(message.contains("discovered tool 'search'"));
        assert!(message.contains(&component_id.to_string()));
        assert!(message.contains("duplicate type id: duplicate"));
    }
}

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
pub(crate) mod operation;

pub use attachment::{
    ToolAttachmentMetadata, ToolAttachmentModeMetadata, ToolAttachmentTerminalMetadata,
};
pub use operation::{
    ToolBodyAdmissionMetadata, ToolOperationLaneMetadata, ToolOperationMetadata,
    ToolOperationSetMetadata, ToolOperationWinnerMetadata, ToolOwnerFailureMetadata,
};

use crate::durable_host::authorization::targets::tool_target;
use crate::durable_host::concurrent::{
    CallReplayOutcome, DurableCallSession, NotCancellable,
    authorize_live_permissions_at_serialized_access,
};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
use crate::durable_host::entity::{
    EntityInvocationDurability, RecordedEntityTerminal, ToolInvocationReplayOutcome,
    encode_tool_terminal, record_tool_rejection_access,
};
use crate::durable_host::secrets::secret_hold_targets_for_value;
use crate::durable_host::tool::attachment::{
    AttachmentConsumer, AttachmentController, AttachmentMemory, AttachmentObserver,
    AttachmentProducer, AttachmentStreamProducer, attachment_pair, discard_producer,
};
use crate::durable_host::{
    DurabilityHost, DurableWorkerCtx, InternalRetryResult, LiveAuthorizationPermit,
};
use crate::preview2::golem::tool::host::{
    ByteStreamCloseCause, ByteStreamFailure, Host, HostFutureInvokeResult,
    HostFutureInvokeResultWithStore, HostToolRpc, HostToolRpcWithStore, HostToolStdin,
    HostToolStdinClosed, HostToolStdinClosedWithStore, HostToolStdinWriter,
    HostToolStdinWriterWithStore, HostToolStdout, HostToolStdoutWriter,
    HostToolStdoutWriterWithStore, HostWithStore, InvocationResult,
    RegisteredTool as WitRegisteredTool, RpcError, StreamWriteError, TypedSchemaValue,
};
use crate::preview2::tool_guest::exports::golem::tool::guest as tool_guest_exports;
use crate::services::environment_state::ToolDiscoveryError;
use crate::services::{HasActiveAgents, HasWorker};
use crate::worker::instance::EntityInvocationBody;
use crate::worker::invocation::{
    GuestCallSettlementError, InvokeResult, finish_invocation_and_get_fuel_consumption,
    prepare_guest_call, run_guest_call_settled,
};
use crate::workerctx::WorkerCtx;
use anyhow::{Context, anyhow};
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::{AgentPrincipal, AgentTypeName, Principal};
use golem_common::model::card::owner::ToolOwnerPattern;
use golem_common::model::entity::{
    AgentEntity, EntityCallMode, EntityInvocationDescriptor, EntityInvocationDescriptorIdentity,
    EntityInvocationRequestIdentity, ToolInputDecodeFailure, ToolInvocationClaimIdentity,
    ToolInvocationDescriptor, ToolInvocationDescriptorIdentity, ToolInvocationRejectedIdentity,
};
use golem_common::model::oplog::host_functions::{GolemToolGetAllTools, GolemToolGetTool};
use golem_common::model::oplog::payload::types::{
    SerializableEntityBodyExecution, SerializableToolError, SerializableToolInvocationResult,
    SerializableToolOperationTerminal, SerializableToolResultValue, SerializableToolRpcError,
    SerializableToolStructuredResult,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemToolGetTool, HostRequestGolemToolInvocationRejected,
    HostRequestNoInput, HostResponseEntityInvocation, HostResponseGolemToolTool,
    HostResponseGolemToolTools,
};
use golem_common::model::tool::{RegisteredTool, ToolName, ToolSource};
use golem_common::schema::render::cli_text::value_to_cli_text_unredacted;
use golem_common::schema::tool::DiscoveredTool;
use golem_common::schema::tool::canonical::CanonicalSurfaceRef;
use golem_common::schema::tool::wit::wire::{
    Host as HostToolCommon, HostUnderlyingTool, HostUnderlyingToolWithStore, Tool as WitTool,
    ToolError, UnderlyingTool,
};
use golem_common::schema::tool::{
    Constraint, FlagShape, OptionShape, OptionSpec, Quantifier, Ref, Repetition, Tool,
};
use golem_common::schema::validation::{is_equivalent_cross_graph, validate_value};
use golem_common::schema::wit::{decode_graph, decode_value_with, encode_graph, encode_value_with};
use golem_common::schema::{
    FromSchema, SchemaType, SchemaValue, TypedSchemaValue as ModelTypedSchemaValue,
};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
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
    golem_common::schema::tool::wit::wire::add_to_linker::<_, ToolCommonHost<Ctx>>(linker, get)
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
    agent_type: AgentTypeName,
}

/// Host-side resource table entry backing the
/// `golem:tool/host.future-invoke-result` resource.
pub struct FutureInvokeResultEntry {
    state: FutureToolInvokeState,
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
}

type ToolInvokeResponse = Result<SerializableToolInvocationResult, SerializableToolRpcError>;

async fn admit_tool_response_secret_holds<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: ToolInvokeResponse,
) -> Result<ToolInvokeResponse, WorkerExecutorError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let targets = accessor.with(|mut access| {
        let ctx = access.get();
        let value = match &response {
            Ok(result) => result.result.as_ref(),
            Err(SerializableToolRpcError::RemoteToolError(error)) => match error.as_ref() {
                SerializableToolError::CustomError(value) => Some(value.as_ref()),
                _ => None,
            },
            _ => None,
        };
        match value {
            Some(value) => secret_hold_targets_for_value(ctx, value.value()),
            None => Ok(Vec::new()),
        }
    })?;
    if targets.is_empty() {
        return Ok(response);
    }
    match authorize_live_permissions_at_serialized_access(accessor, accessor.getter(), &targets)
        .await?
    {
        Ok(_) => Ok(response),
        Err(_) => Ok(Err(SerializableToolRpcError::Denied(
            "permission denied".to_string(),
        ))),
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
        if &execution.parent != current_parent {
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
        match self.result_snapshot() {
            Some(Ok(result)) => FutureToolInvokeGet::Ready(Box::new(result)),
            Some(Err(error)) => FutureToolInvokeGet::Failed(error.to_string()),
            None => FutureToolInvokeGet::Active(self.clone()),
        }
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
) -> Result<String, String> {
    value_to_cli_text_unredacted(&tool.schema, type_, value).map_err(|error| error.to_string())
}

fn option_args(
    tool: &Tool,
    option: &OptionSpec,
    value: &SchemaValue,
) -> Result<Vec<String>, String> {
    let prefix = format!("--{}=", option.long);
    match &option.shape {
        OptionShape::Scalar(type_) | OptionShape::OptionalScalar(type_) => Ok(vec![format!(
            "{prefix}{}",
            render_tool_value(tool, type_, value)?
        )]),
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
                Repetition::Repeated => Ok(rendered
                    .into_iter()
                    .map(|value| format!("{prefix}{value}"))
                    .collect()),
                Repetition::Delimited(separator) | Repetition::Either(separator) => Ok((!rendered
                    .is_empty())
                .then(|| format!("{prefix}{}", rendered.join(&separator.to_string())))
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
                    Ok(format!(
                        "{}={}",
                        render_tool_value(tool, key_type, key)?,
                        render_tool_value(tool, value_type, value)?
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            match shape.repetition {
                Repetition::Repeated => Ok(rendered
                    .into_iter()
                    .map(|value| format!("{prefix}{value}"))
                    .collect()),
                Repetition::Delimited(separator) | Repetition::Either(separator) => Ok((!rendered
                    .is_empty())
                .then(|| format!("{prefix}{}", rendered.join(&separator.to_string())))
                .into_iter()
                .collect()),
            }
        }
    }
}

fn flag_args(
    flag: &golem_common::schema::tool::FlagSpec,
    value: &SchemaValue,
) -> Result<Vec<String>, String> {
    match (flag.shape, value) {
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(value)) if *value == shape.default => {
            Ok(Vec::new())
        }
        (FlagShape::BoolFlag(_), SchemaValue::Bool(true)) => Ok(vec![format!("--{}", flag.long)]),
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(false)) if shape.negatable => {
            Ok(vec![format!("--no-{}", flag.long)])
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
            Ok((0..*count).map(|_| format!("--{}", flag.long)).collect())
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
    args: Vec<String>,
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

fn ref_matches(
    reference: &Ref,
    tool: &Tool,
    command_index: usize,
    surfaces: &[CanonicalSurfaceRef],
    values: &[golem_common::schema::tool::canonical::CanonicalInputValue],
) -> bool {
    let (name, expected) = match reference {
        Ref::Present(name) => (name, None),
        Ref::ValueIs(value) => (&value.name, Some(&value.value)),
    };
    let Some((index, value)) = values
        .iter()
        .enumerate()
        .find(|(_, value)| value.name == *name || value.aliases.contains(name))
    else {
        return false;
    };
    if let Some(expected) = expected {
        return schema_value_matches(&value.value, expected);
    }
    surface_is_present(tool, command_index, surfaces[index], &value.value)
}

fn schema_value_matches(value: &SchemaValue, expected: &SchemaValue) -> bool {
    if value == expected {
        return true;
    }
    match value {
        SchemaValue::Option { inner } => inner
            .as_deref()
            .is_some_and(|value| schema_value_matches(value, expected)),
        SchemaValue::List { elements } | SchemaValue::FixedList { elements } => elements
            .iter()
            .any(|value| schema_value_matches(value, expected)),
        SchemaValue::Map { entries } => entries
            .iter()
            .any(|(_, value)| schema_value_matches(value, expected)),
        _ => false,
    }
}

fn value_is_present(value: &SchemaValue, default: Option<&SchemaValue>) -> bool {
    if default.is_some_and(|default| value == default) {
        return false;
    }
    match value {
        SchemaValue::Option { inner } => inner.is_some(),
        SchemaValue::List { elements } | SchemaValue::FixedList { elements } => {
            !elements.is_empty()
        }
        SchemaValue::Map { entries } => !entries.is_empty(),
        SchemaValue::Bool(value) => *value,
        SchemaValue::U32(value) => *value != 0,
        _ => true,
    }
}

fn surface_is_present(
    tool: &Tool,
    command_index: usize,
    surface: CanonicalSurfaceRef,
    value: &SchemaValue,
) -> bool {
    let body = || {
        tool.commands.nodes[command_index]
            .body
            .as_ref()
            .expect("canonical input surfaces only resolve command bodies")
    };
    match surface {
        CanonicalSurfaceRef::GlobalOption { node, index } => value_is_present(
            value,
            tool.commands.nodes[node].globals.options[index]
                .default
                .as_ref(),
        ),
        CanonicalSurfaceRef::BodyOption { index } => {
            value_is_present(value, body().options[index].default.as_ref())
        }
        CanonicalSurfaceRef::GlobalFlag { node, index } => {
            flag_is_present(&tool.commands.nodes[node].globals.flags[index].shape, value)
        }
        CanonicalSurfaceRef::BodyFlag { index } => {
            flag_is_present(&body().flags[index].shape, value)
        }
        CanonicalSurfaceRef::BodyPositional { index } => {
            value_is_present(value, body().positionals.fixed[index].default.as_ref())
        }
        CanonicalSurfaceRef::BodyTail => value_is_present(value, None),
    }
}

fn flag_is_present(shape: &FlagShape, value: &SchemaValue) -> bool {
    match (shape, value) {
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(value)) => *value != shape.default,
        (FlagShape::CountFlag(_), SchemaValue::U32(value)) => *value != 0,
        _ => false,
    }
}

fn quantified_refs(
    quantifier: Quantifier,
    refs: &[Ref],
    tool: &Tool,
    command_index: usize,
    surfaces: &[CanonicalSurfaceRef],
    values: &[golem_common::schema::tool::canonical::CanonicalInputValue],
) -> bool {
    match quantifier {
        Quantifier::All => refs
            .iter()
            .all(|reference| ref_matches(reference, tool, command_index, surfaces, values)),
        Quantifier::Any => refs
            .iter()
            .any(|reference| ref_matches(reference, tool, command_index, surfaces, values)),
    }
}

fn validate_tool_constraints(
    tool: &Tool,
    command_index: usize,
    constraints: &[Constraint],
    surfaces: &[CanonicalSurfaceRef],
    values: &[golem_common::schema::tool::canonical::CanonicalInputValue],
) -> Result<(), String> {
    for (index, constraint) in constraints.iter().enumerate() {
        let satisfied = match constraint {
            Constraint::RequiresAll(refs) => {
                quantified_refs(Quantifier::All, refs, tool, command_index, surfaces, values)
            }
            Constraint::AllOrNone(refs) => {
                let present = refs
                    .iter()
                    .filter(|reference| {
                        ref_matches(reference, tool, command_index, surfaces, values)
                    })
                    .count();
                present == 0 || present == refs.len()
            }
            Constraint::RequiresAny(refs) => {
                quantified_refs(Quantifier::Any, refs, tool, command_index, surfaces, values)
            }
            Constraint::MutexGroups(groups) => {
                groups
                    .iter()
                    .filter(|group| {
                        quantified_refs(
                            Quantifier::All,
                            &group.refs,
                            tool,
                            command_index,
                            surfaces,
                            values,
                        )
                    })
                    .count()
                    <= 1
            }
            Constraint::Implies(implies) => {
                !quantified_refs(
                    implies.lhs_quant,
                    &implies.lhs,
                    tool,
                    command_index,
                    surfaces,
                    values,
                ) || quantified_refs(
                    implies.rhs_quant,
                    &implies.rhs,
                    tool,
                    command_index,
                    surfaces,
                    values,
                )
            }
            Constraint::Forbids(forbids) => {
                !quantified_refs(
                    forbids.lhs_quant,
                    &forbids.lhs,
                    tool,
                    command_index,
                    surfaces,
                    values,
                ) || !quantified_refs(
                    Quantifier::Any,
                    &forbids.rhs,
                    tool,
                    command_index,
                    surfaces,
                    values,
                )
            }
        };
        if !satisfied {
            return Err(format!("tool command constraint {index} is not satisfied"));
        }
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
    validate_tool_constraints(tool, command_index, &body.constraints, &surfaces, &values)
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
            CanonicalSurfaceRef::BodyPositional { index } => args.push(
                render_tool_value(tool, &body.positionals.fixed[index].type_, &field.value)
                    .map_err(SerializableToolError::InvalidInput)?,
            ),
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
                    args.push(separator.clone());
                }
                args.extend(
                    elements
                        .iter()
                        .map(|value| render_tool_value(tool, &tail.item_type, value))
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
        value: encode_value_with(value.value(), ctx).map_err(|error| error.to_string())?,
    })
}

fn project_tool_error<Ctx: WorkerCtx>(
    error: SerializableToolError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> RpcError {
    let error = match error {
        SerializableToolError::InvalidToolName(value) => {
            crate::preview2::golem::tool::host::ToolError::InvalidToolName(value)
        }
        SerializableToolError::InvalidCommandPath(value) => {
            crate::preview2::golem::tool::host::ToolError::InvalidCommandPath(value)
        }
        SerializableToolError::InvalidInput(value) => {
            crate::preview2::golem::tool::host::ToolError::InvalidInput(value)
        }
        SerializableToolError::ConstraintViolation(value) => {
            crate::preview2::golem::tool::host::ToolError::ConstraintViolation(value)
        }
        SerializableToolError::InvalidResult(value) => {
            crate::preview2::golem::tool::host::ToolError::InvalidResult(value)
        }
        SerializableToolError::CustomError(value) => match encode_typed_tool_value(&value, ctx) {
            Ok(value) => crate::preview2::golem::tool::host::ToolError::CustomError(value),
            Err(error) => return RpcError::ProtocolError(error),
        },
    };
    RpcError::RemoteToolError(error)
}

fn project_tool_rpc_error<Ctx: WorkerCtx>(
    error: SerializableToolRpcError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> RpcError {
    match error {
        SerializableToolRpcError::ProtocolError(value) => RpcError::ProtocolError(value),
        SerializableToolRpcError::Denied(value) => RpcError::Denied(value),
        SerializableToolRpcError::NotFound(value) => RpcError::NotFound(value),
        SerializableToolRpcError::RemoteInternalError(value) => {
            RpcError::RemoteInternalError(value)
        }
        SerializableToolRpcError::RemoteToolError(error) => project_tool_error(*error, ctx),
        SerializableToolRpcError::Cancelled => RpcError::Cancelled,
        SerializableToolRpcError::ResourceExhausted(value) => RpcError::ResourceExhausted(value),
    }
}

fn project_tool_response_value<Ctx: WorkerCtx>(
    response: ToolInvokeResponse,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<(Option<TypedSchemaValue>, Option<Vec<u8>>), RpcError> {
    response
        .map_err(|error| project_tool_rpc_error(error, ctx))
        .and_then(|response| {
            let result = response
                .result
                .as_ref()
                .map(|value| encode_typed_tool_value(value, ctx))
                .transpose()
                .map_err(RpcError::ProtocolError)?;
            Ok((result, response.stdout))
        })
}

fn project_tool_response<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: ToolInvokeResponse,
) -> Result<InvocationResult, RpcError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let (result, stdout) = project_tool_response_value(response, access.get())?;
        let stdout = stdout
            .map(|bytes| StreamReader::new(&mut access, bytes))
            .transpose()
            .map_err(|error| RpcError::RemoteInternalError(error.to_string()))?;
        Ok(InvocationResult { result, stdout })
    })
}

fn project_tool_unit<Ctx: WorkerCtx>(
    response: Result<(), SerializableToolRpcError>,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<(), RpcError> {
    response.map_err(|error| project_tool_rpc_error(error, ctx))
}

fn tool_owner<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    rpc: &ToolRpcEntry,
    registered_tool: &RegisteredTool,
) -> ToolOwnerPattern {
    let component = ctx.component_metadata();
    let ToolSource::Component { component_name, .. } = &registered_tool.source;
    ToolOwnerPattern::Tool {
        account: registered_tool.owner_account_email.clone(),
        application: component.application_name.clone(),
        environment: component.environment_name.clone(),
        component: component_name.clone(),
        tool: rpc.tool_name.to_string(),
    }
}

struct PreparedToolCall {
    stdin: Option<Resource<ToolStdinEntry>>,
    permit: LiveAuthorizationPermit,
    operation: operation::ProvisionalOwnerToolOperation,
}

enum ToolCallPreparation {
    Ready(PreparedToolCall),
    Rejected {
        request: Box<HostRequestGolemToolInvocationRejected>,
        stdin: Option<Resource<ToolStdinEntry>>,
    },
}

struct AcceptedToolCall {
    durability: EntityInvocationDurability,
    operation: operation::OwnerToolOperation,
    stdin: Option<Resource<ToolStdinEntry>>,
    deferred_admission_inserted: bool,
}

enum ToolCallDispatch {
    Rejected {
        response: Box<ToolInvokeResponse>,
        stdin: Option<Resource<ToolStdinEntry>>,
    },
    Accepted(Box<AcceptedToolCall>),
}

struct ToolInvocationAttempt {
    rpc: ToolRpcEntry,
    input: Result<ModelTypedSchemaValue, ToolInputDecodeFailure>,
    parent: crate::worker::owner_lane::OwnerInvocationId,
    attempt_ordinal: u64,
}

impl ToolInvocationAttempt {
    fn claim_identity(
        &self,
        command_path: &[String],
        has_stdin: bool,
        has_stdout: bool,
        call_mode: EntityCallMode,
    ) -> ToolInvocationClaimIdentity {
        let calling_principal = Principal::Agent(AgentPrincipal {
            agent_id: self.rpc.owner.owner_id.agent_id.clone(),
        });
        let input = self.input.as_ref().ok().cloned();
        ToolInvocationClaimIdentity {
            accepted: input.clone().map(|input| EntityInvocationRequestIdentity {
                entity: AgentEntity::Tool(self.rpc.tool_name.clone()),
                calling_principal,
                call_mode,
                operation: Some(EntityInvocationDescriptorIdentity::Tool(
                    ToolInvocationDescriptorIdentity {
                        attempt_ordinal: self.attempt_ordinal,
                        command_path: command_path.to_vec(),
                        has_stdin,
                        has_stdout,
                    },
                )),
                input,
            }),
            rejected: ToolInvocationRejectedIdentity {
                attempt_ordinal: self.attempt_ordinal,
                tool_name: self.rpc.tool_name.clone(),
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
    let agent_type = ctx
        .parsed_agent_id()
        .map(|agent_id| agent_id.agent_type)
        .ok_or_else(|| anyhow!("tool RPC resources require an agent owner"))?;
    Ok(ToolRpcEntry {
        tool_name,
        owner: ToolRpcOwnerContext {
            owner_id: ctx.state.owned_agent_id.clone(),
            agent_type,
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
    rpc: ToolRpcEntry,
    input: TypedSchemaValue,
) -> anyhow::Result<ToolInvocationAttempt>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let ctx = access.get();
        if ctx.state.owned_agent_id != rpc.owner.owner_id {
            return Err(anyhow!(
                "tool RPC resource belongs to a different owner runtime"
            ));
        }
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
        Ok(ToolInvocationAttempt {
            rpc,
            input: decode_typed_tool_value(input, ctx),
            parent,
            attempt_ordinal,
        })
    })
}

fn rejected_tool_call(
    rpc: &ToolRpcEntry,
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
            tool_name: rpc.tool_name.to_string(),
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

async fn prepare_tool_call<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    attempt: ToolInvocationAttempt,
    command_path: Vec<String>,
    stdin: Option<Resource<ToolStdinEntry>>,
    stdout_requested: bool,
    call_mode: EntityCallMode,
) -> anyhow::Result<ToolCallPreparation>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let has_stdin = stdin.is_some();
    let ToolInvocationAttempt {
        rpc,
        input,
        parent,
        attempt_ordinal,
    } = attempt;
    let environment_state_service =
        accessor.with(|mut access| access.get().state.environment_state_service.clone());
    let input = match input {
        Ok(input) => input,
        Err(error) => {
            return Ok(rejected_tool_call(
                &rpc,
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

    let activation_snapshot = match environment_state_service
        .get_tool_activation(
            rpc.owner.owner_id.environment_id,
            &rpc.owner.agent_type,
            &rpc.tool_name,
        )
        .await
    {
        Ok(Some(activation)) => activation,
        Ok(None) => {
            return Ok(rejected_tool_call(
                &rpc,
                attempt_ordinal,
                &command_path,
                Some(input),
                None,
                has_stdin,
                stdout_requested,
                call_mode,
                SerializableToolRpcError::Denied(format!(
                    "tool '{}' is not accessible to agent type '{}'",
                    rpc.tool_name, rpc.owner.agent_type
                )),
                stdin,
            ));
        }
        Err(error) => {
            let kind = classify_tool_discovery_error(&error);
            return Err(anyhow::Error::new(ClassifiedHostError {
                kind,
                message: error.to_string(),
            }));
        }
    };
    let registered_tool = activation_snapshot.registered_tool().clone();

    let command = match resolve_tool_command(&registered_tool.definition, &command_path, &input) {
        Ok(command) => command,
        Err(error) => {
            return Ok(rejected_tool_call(
                &rpc,
                attempt_ordinal,
                &command_path,
                Some(input),
                None,
                has_stdin,
                stdout_requested,
                call_mode,
                SerializableToolRpcError::RemoteToolError(Box::new(error)),
                stdin,
            ));
        }
    };
    if let Err(error) = validate_stream_attachments(
        &command,
        &command_path,
        has_stdin,
        stdout_requested,
        call_mode,
    ) {
        return Ok(rejected_tool_call(
            &rpc,
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
    let declares_stdout = command.stdout_required.is_some();
    let args = command.args;

    let target = accessor.with(|mut access| {
        let owner = tool_owner(access.get(), &rpc, &registered_tool);
        let command_path = command_path.iter().map(String::as_str).collect::<Vec<_>>();
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        tool_target(owner, &command_path, &args)
    });
    let target = match target {
        Ok(target) => target,
        Err(error) => {
            return Ok(rejected_tool_call(
                &rpc,
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
    let permit = match authorize_live_permissions_at_serialized_access(
        accessor,
        accessor.getter(),
        &[target],
    )
    .await?
    {
        Ok(permit) => permit,
        Err(error) => {
            return Ok(rejected_tool_call(
                &rpc,
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
    };

    let activation = Arc::new(
        activation_snapshot
            .into_entity_activation()
            .map_err(|error| {
                anyhow::Error::new(ClassifiedHostError {
                    kind: classify_tool_discovery_error(&error),
                    message: error.to_string(),
                })
            })?,
    );
    let descriptor = EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
        attempt_ordinal,
        command_path,
        args,
        has_stdin,
        has_stdout: stdout_requested,
        declares_stdout,
    });
    let calling_principal = Principal::Agent(AgentPrincipal {
        agent_id: rpc.owner.owner_id.agent_id.clone(),
    });
    let operation = accessor.with(|mut access| {
        let ctx = access.get();
        let principal = ctx.invocation_principal();
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

    Ok(ToolCallPreparation::Ready(PreparedToolCall {
        stdin,
        permit,
        operation,
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
                result,
                stdout: None,
            }))
        }
        Err(error) => Ok(Err(error)),
    }
}

fn decode_guest_tool_error<Ctx: WorkerCtx>(
    error: crate::preview2::golem::tool::host::ToolError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> SerializableToolRpcError {
    let error = match error {
        crate::preview2::golem::tool::host::ToolError::InvalidToolName(value) => {
            SerializableToolError::InvalidToolName(value)
        }
        crate::preview2::golem::tool::host::ToolError::InvalidCommandPath(value) => {
            SerializableToolError::InvalidCommandPath(value)
        }
        crate::preview2::golem::tool::host::ToolError::InvalidInput(value) => {
            SerializableToolError::InvalidInput(value)
        }
        crate::preview2::golem::tool::host::ToolError::ConstraintViolation(value) => {
            SerializableToolError::ConstraintViolation(value)
        }
        crate::preview2::golem::tool::host::ToolError::InvalidResult(value) => {
            SerializableToolError::InvalidResult(value)
        }
        crate::preview2::golem::tool::host::ToolError::CustomError(value) => {
            match decode_typed_tool_value(value, ctx) {
                Ok(value) => SerializableToolError::CustomError(Box::new(value)),
                Err(_) => {
                    return SerializableToolRpcError::ProtocolError(
                        "tool guest returned an invalid custom-error payload".to_string(),
                    );
                }
            }
        }
    };
    SerializableToolRpcError::RemoteToolError(Box::new(error))
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
    command_path: Vec<String>,
    input: ModelTypedSchemaValue,
    stdin: Option<ToolStdinEntry>,
    stdout: Option<ToolStdoutWriterEntry>,
    principal: Principal,
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

async fn invoke_tool_sidecar<Ctx: WorkerCtx>(
    instance: &wasmtime::component::Instance,
    store: &mut Store<Ctx>,
    invocation: ToolSidecarInvocation,
    operation: operation::OwnerToolOperation,
    cancellation: Option<tokio_util::sync::CancellationToken>,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    let ToolSidecarInvocation {
        tool_name,
        command_path,
        input,
        stdin,
        stdout,
        principal,
    } = invocation;
    let mut store = store.as_context_mut();
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
    let instance_pre = instance.instance_pre(&store);
    let indices = tool_guest_exports::GuestIndices::new(&instance_pre).map_err(|error| {
        WorkerExecutorError::invalid_request(format!("tool guest export not available: {error}"))
    })?;
    let guest = indices.load(&mut store, instance).map_err(|error| {
        WorkerExecutorError::invalid_request(format!("failed to load tool guest export: {error}"))
    })?;
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
    let display_name = format!("golem:tool/guest.invoke({tool_name})");
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
    let result = run_guest_call_settled(&mut store, async move |accessor| {
        guest
            .call_invoke(
                accessor,
                tool_name,
                command_path,
                input,
                stdin,
                stdout,
                principal,
            )
            .await
    })
    .await;
    store
        .data_mut()
        .durable_ctx_mut()
        .set_invocation_principal(None);
    let finish = finish_invocation_and_get_fuel_consumption(&mut store, &display_name).await;
    let parent_end = prepare_tool_parent_end(&mut store, parent).await;
    drop(cancellation_epoch);
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
            match result {
                Ok(result) => {
                    let result = result
                        .result
                        .map(|value| {
                            decode_typed_tool_value(value, store.data_mut().durable_ctx_mut())
                        })
                        .transpose()
                        .map_err(|_| {
                            WorkerExecutorError::runtime(
                                "tool guest returned an invalid result payload",
                            )
                        })?
                        .map(|value| SerializableToolResultValue::from_typed(&value))
                        .transpose()
                        .map_err(|error| {
                            WorkerExecutorError::runtime(format!(
                                "failed to encode durable tool result payload: {error}"
                            ))
                        })?;
                    encode_tool_operation_terminal(Ok(SerializableToolStructuredResult { result }))
                        .await
                }
                Err(error) => {
                    encode_tool_operation_terminal(Err(decode_guest_tool_error(
                        error,
                        store.data_mut().durable_ctx_mut(),
                    )))
                    .await
                }
            }
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

async fn resource_exhausted_without_body()
-> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    encode_tool_terminal(
        SerializableToolOperationTerminal {
            body_execution: SerializableEntityBodyExecution::Skipped,
            result: Err(SerializableToolRpcError::ResourceExhausted(
                "tool stdin exceeded the attachment byte limit".to_string(),
            )),
        },
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
    let execution_future = execute_accepted_tool_call_inner(
        accessor,
        accepted,
        stdout,
        execution,
        failed_resources.clone(),
        inner_supervisor_started,
    );
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
        let selected_owner_failure = owner_operations
            .select_owner_failure(requested_winner)
            .await;
        let winner = owner_operations
            .selected_owner_failure()
            .expect("failed accepted tool operation must select an owner winner");
        let preserve_exact_trap = matches!(&winner, operation::OwnerFailureWinner::Trap(_));
        let owns_owner_failure_cleanup = selected_owner_failure
            || preserve_exact_trap
                && matches!(
                    cleanup_operation.winner_if_active(),
                    Some(operation::ToolOperationWinner::Trap)
                );
        if owns_owner_failure_cleanup {
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
        if owns_owner_failure_cleanup {
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
    } = accepted;
    let parent = durability.parent().clone();
    let start = durability.scope().invocation_id().start_index();
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
    let filesystem = durability.scope().activation().filesystem();
    if filesystem == golem_common::model::entity::FilesystemCapability::Capable {
        durability = durability
            .enter_incomplete_live_repair_before_body_access(accessor, accessor.getter())
            .await?;
    }
    let call_mode = durability.call_mode();
    let discard_stdout = call_mode == EntityCallMode::FireAndForget
        && matches!(
            durability.operation(),
            Some(EntityInvocationDescriptor::Tool(descriptor)) if descriptor.declares_stdout
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
            Some(ToolStdoutWriterEntry::discard(AttachmentMemory::tracked(
                ctx.public_state.worker().active_agents(),
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

    match filesystem {
        golem_common::model::entity::FilesystemCapability::Incapable => {
            if let Some(stdin) = &stdin_controller {
                stdin.configure_live();
            }
            if let Some(stdout) = &stdout_controller {
                if stdout_completion_only {
                    stdout.configure_completion();
                } else {
                    stdout.configure_live();
                }
            }
            if !operation.transition_admission(
                operation::BodyAdmissionState::Staging,
                operation::BodyAdmissionState::Running,
            ) {
                return Err(anyhow!("tool invocation was fenced before dispatch"));
            }
        }
        golem_common::model::entity::FilesystemCapability::Capable => {
            if let Some(stdin) = &stdin_controller {
                stdin.configure_completion();
            }
            if let Some(stdout) = &stdout_controller {
                stdout.configure_completion();
            }
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
            if let Some(stdin) = &stdin_controller {
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
                    operation.transition_admission(
                        operation::BodyAdmissionState::Staging,
                        operation::BodyAdmissionState::SettledWithoutBody,
                    );
                    if !deferred_admission.settle_staging(
                        &parent,
                        start,
                        operation::DeferredAdmissionReadiness::SettledWithoutBody,
                    ) {
                        return Err(anyhow!(
                            "tool invocation lost its deferred no-body admission"
                        ));
                    }
                    if call_mode == EntityCallMode::Synchronous
                        && !deferred_admission.remove_settled_without_body(&parent, start)
                    {
                        return Err(anyhow!(
                            "synchronous tool invocation lost its no-body admission"
                        ));
                    }
                    deferred_cleanup
                        .as_mut()
                        .expect("capable call must own deferred admission")
                        .disarm();
                    let response = resource_exhausted_without_body().await?;
                    let terminal = terminal_from_response(&response)?;
                    if !operation.begin_ordinary() {
                        return Err(anyhow!(
                            "tool invocation was fenced before no-body completion"
                        ));
                    }
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
                            if let Some(stdout) = &stdout_controller {
                                let _ = stdout.host_fail(ByteStreamFailure::ResourceExhausted);
                            }
                            publish_no_body_terminals(
                                stdin_controller.as_ref(),
                                stdout_controller.as_ref(),
                            );
                            return decode_tool_terminal(*response).map_err(Into::into);
                        }
                        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(
                            response,
                            _,
                        )) => {
                            operation.resolve_ordinary(terminal, false).await;
                            let _ = operation.begin_cancel();
                            operation.resolve_cancel(true).await;
                            operation.settle().await;
                            publish_no_body_terminals(
                                stdin_controller.as_ref(),
                                stdout_controller.as_ref(),
                            );
                            return decode_tool_terminal(*response).map_err(Into::into);
                        }
                        Err(error) => {
                            operation.resolve_ordinary(terminal, false).await;
                            let _ = operation.select_infrastructure(error.clone()).await;
                            return Err(error.into());
                        }
                    }
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
    let AgentEntity::Tool(tool_name) = context.activation.entity() else {
        return Err(anyhow!("tool invocation activation does not target a tool"));
    };
    let EntityInvocationDescriptor::Tool(descriptor) = &context.descriptor;
    let sidecar = ToolSidecarInvocation {
        tool_name: tool_name.to_string(),
        command_path: descriptor.command_path.clone(),
        input: context.input.clone(),
        stdin,
        stdout,
        principal: context.principal.clone(),
    };
    let scope = durability.scope().clone();
    let replaying_completed =
        scope.mode() == golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted;
    let operation_for_body = operation.clone();
    let terminal = Arc::new(std::sync::Mutex::new(None));
    let terminal_for_finalize = terminal.clone();
    let operation_for_finalize = operation.clone();
    let terminal_for_completed_failure = terminal.clone();
    let operation_for_completed_failure = operation.clone();
    let invoke = ToolSidecarBody {
        invocation: sidecar,
        operation: operation_for_body,
        cancellation: execution
            .filter(|execution| execution.cancellable)
            .map(|execution| execution.cancel.clone()),
    };
    let finalize = move |result: Result<HostResponseEntityInvocation, WorkerExecutorError>| {
        let operation = operation_for_finalize;
        async move {
            match &result {
                Ok(response) => {
                    let selected = terminal_from_response(response)?;
                    if !operation.begin_ordinary() {
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
    let body = match filesystem {
        golem_common::model::entity::FilesystemCapability::Incapable => active_agent
            .start_entity_invocation(
                parent,
                scope,
                owner_component_metadata,
                call_mode,
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
            let terminal = terminal.lock().unwrap().take().ok_or_else(|| {
                anyhow!("completed tool body did not select an operation terminal")
            })?;
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
            if (filesystem == golem_common::model::entity::FilesystemCapability::Capable
                || stdout_completion_only)
                && let Some(stdout) = &stdout_controller
            {
                stdout.publish_completion();
            }
            decode_tool_terminal(*response).map_err(Into::into)
        }
        Ok(crate::durable_host::entity::EntityInvocationDurabilityOutcome::Cancelled(
            response,
            resources,
        )) => {
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
            operation.settle().await;
            if (filesystem == golem_common::model::entity::FilesystemCapability::Capable
                || stdout_completion_only)
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

async fn dispatch_tool_call<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    rpc: ToolRpcEntry,
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
    let has_stdin = stdin.is_some();
    let attempt = read_tool_attempt(accessor, rpc, input)?;
    if accessor.with(|mut access| !access.get().state.is_live()) {
        let identity = attempt.claim_identity(&command_path, has_stdin, has_stdout, call_mode);
        match EntityInvocationDurability::replay_tool_access(
            accessor,
            accessor.getter(),
            attempt.parent.clone(),
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
                let descriptor = durability.operation().cloned().ok_or_else(|| {
                    anyhow!("recorded tool entity invocation has no operation descriptor")
                })?;
                let operation = accessor.with(|mut access| {
                    access.get().owner_execution.tool_operations().create(
                        operation::OwnerToolOperationContext {
                            parent: durability.parent().clone(),
                            call_mode: durability.call_mode(),
                            activation: durability.scope().activation().clone(),
                            calling_principal: durability.scope().calling_principal().clone(),
                            principal: durability.principal().clone(),
                            descriptor,
                            input: durability.input().clone(),
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
                })));
            }
            ToolInvocationReplayOutcome::ReplayEnded => {}
        }
    }

    let parent = attempt.parent.clone();
    match prepare_tool_call(
        accessor,
        attempt,
        command_path,
        stdin,
        has_stdout,
        call_mode,
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
                context.activation.entity(),
                context.activation.clone(),
                context.calling_principal.clone(),
                context.principal.clone(),
                context.call_mode,
                Some(context.descriptor.clone()),
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
            })))
        }
    }
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
    deferred
        .wait_and_register_cohort(parent, cohort, starts, &operations, &lane)
        .await
        .map_err(Into::into)
}

async fn get_tool_invoke_results<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    futures: &[Resource<FutureInvokeResultEntry>],
) -> anyhow::Result<Vec<Result<InvocationResult, RpcError>>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let mut plans = accessor.with(|mut access| -> anyhow::Result<Vec<_>> {
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

    let mut guards = Vec::new();
    for plan in &mut plans {
        let FutureToolInvokeGet::Active(execution) = plan else {
            continue;
        };
        if execution.result_snapshot().is_some() {
            *plan = execution.get_plan();
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
            guards.push(ToolExecutionGetGuard(execution.clone()));
        }
    }

    for plan in &mut plans {
        let FutureToolInvokeGet::Active(execution) = plan else {
            continue;
        };
        if execution.result_snapshot().is_some() {
            *plan = execution.get_plan();
        }
    }

    let current_parent = accessor.with(|mut access| access.get().owner_invocation_id())?;
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

    let mut projected = Vec::with_capacity(responses.len());
    for response in responses {
        let response = admit_tool_response_secret_holds(accessor, *response?).await?;
        projected.push(project_tool_response(accessor, response));
    }
    drop(guards);
    Ok(projected)
}

pub(crate) async fn prepare_tool_parent_end<Ctx: WorkerCtx>(
    store: &mut wasmtime::StoreContextMut<'_, Ctx>,
    parent: crate::worker::owner_lane::OwnerInvocationId,
) -> Result<(), WorkerExecutorError> {
    store
        .as_context_mut()
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            let starts = accessor.with(|mut access| {
                access
                    .data_mut()
                    .durable_ctx()
                    .owner_execution
                    .deferred_tool_admission()
                    .close_parent_and_snapshot(&parent)
            });
            let Some(starts) = starts else {
                return Ok(());
            };
            release_capable_tool_cohort(
                accessor,
                |ctx: &mut Ctx| ctx.durable_ctx_mut(),
                &parent,
                operation::DeferredAdmissionCohort::ParentEnd,
                &starts,
            )
            .await
            .map(|_| ())
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
    let (operations, deferred) = {
        let owner_execution = &store.data().durable_ctx().owner_execution;
        (
            owner_execution.tool_operations(),
            owner_execution.deferred_tool_admission(),
        )
    };
    store
        .as_context_mut()
        .run_concurrent(async move |_accessor| -> wasmtime::Result<()> {
            operations.wait_parent_settled(&parent).await;
            if !deferred.clear_closed_parent(&parent) {
                return Err(wasmtime::Error::msg(
                    "tool parent settled with deferred admissions remaining",
                ));
            }
            Ok(())
        })
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
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
    }
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
    source: StreamReader<u8>,
) -> anyhow::Result<Resource<ToolStdinEntry>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let ctx = access.get();
        let memory = AttachmentMemory::tracked(ctx.public_state.worker().active_agents());
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

fn create_underlying_stdout<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
) -> anyhow::Result<(Resource<ToolStdoutEntry>, AttachmentConsumer)>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let ctx = access.get();
        let memory = AttachmentMemory::tracked(ctx.public_state.worker().active_agents());
        let (producer, consumer, _) =
            attachment_pair(ctx.state.config.limits.max_tool_attachment_bytes, memory);
        let target = ctx.table().push(ToolStdoutEntry {
            producer: Some(producer),
            completion_only: true,
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
        rpc,
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
                spawn_tool_execution(accessor, *accepted, stdout)
                    .await?
                    .result()
                    .await?
            }
        },
    };
    admit_tool_response_secret_holds(accessor, response)
        .await
        .map_err(Into::into)
}

fn project_underlying_tool_response<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: ToolInvokeResponse,
    stdout: Option<AttachmentConsumer>,
) -> anyhow::Result<Result<InvocationResult, ToolError>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    match project_tool_response(accessor, response) {
        Ok(mut response) => {
            if let Some(stdout) = stdout {
                response.stdout = Some(accessor.with(|mut access| {
                    StreamReader::new(&mut access, stdout.into_raw_stream_producer())
                })?);
            }
            Ok(Ok(response))
        }
        Err(RpcError::RemoteToolError(error)) => Ok(Err(error)),
        Err(error) => Err(anyhow!("underlying tool invocation failed: {error:?}")),
    }
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
            definition,
            implemented_by: value.implemented_by.into(),
        })
    }
}

fn classify_tool_discovery_error(error: &ToolDiscoveryError) -> HostFailureKind {
    match error {
        ToolDiscoveryError::Retrieval(_) => HostFailureKind::Transient,
        ToolDiscoveryError::AgentContextRequired
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
    pub(crate) async fn get_all_tools_model(&mut self) -> anyhow::Result<Vec<Arc<DiscoveredTool>>> {
        let agent_type = self.parsed_agent_id().map(|agent_id| agent_id.agent_type);
        let environment_id = self.state.owned_agent_id.environment_id;
        let component_id = self.state.owned_agent_id.agent_id.component_id;
        let component_revision = self.state.component_metadata.revision;

        let mut handle = DurableCallSession::<GolemToolGetAllTools, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = if let Some(agent_type) = &agent_type {
                loop {
                    let result = self
                        .state
                        .environment_state_service
                        .get_accessible_tools(
                            environment_id,
                            component_id,
                            component_revision,
                            agent_type,
                        )
                        .await;
                    match handle
                        .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                        .await?
                    {
                        InternalRetryResult::Persist => break result,
                        InternalRetryResult::RetryInternally => continue,
                    }
                }
            } else {
                Err(ToolDiscoveryError::AgentContextRequired)
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

        response.result.map_err(|error| {
            let agent_type = agent_type
                .as_ref()
                .map_or("<missing>", |agent_type| agent_type.0.as_str());
            terminal_tool_discovery_error(format!(
                "failed to discover tools for agent type '{}' in environment '{environment_id}': {error}",
                agent_type
            ))
        })
    }

    pub(crate) async fn get_tool_model(
        &mut self,
        tool_name: String,
    ) -> anyhow::Result<Option<Arc<DiscoveredTool>>> {
        let agent_type = self.parsed_agent_id().map(|agent_id| agent_id.agent_type);
        let valid_tool_name = ToolName::try_from(tool_name.as_str()).ok();
        let environment_id = self.state.owned_agent_id.environment_id;
        let component_id = self.state.owned_agent_id.agent_id.component_id;
        let component_revision = self.state.component_metadata.revision;

        let mut handle = DurableCallSession::<GolemToolGetTool, NotCancellable>::start(
            self,
            HostRequestGolemToolGetTool {
                name: tool_name.clone(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = if let Some(agent_type) = &agent_type {
                if let Some(valid_tool_name) = &valid_tool_name {
                    loop {
                        let result = self
                            .state
                            .environment_state_service
                            .get_accessible_tool(
                                environment_id,
                                component_id,
                                component_revision,
                                agent_type,
                                valid_tool_name,
                            )
                            .await;
                        match handle
                            .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                            .await?
                        {
                            InternalRetryResult::Persist => break result,
                            InternalRetryResult::RetryInternally => continue,
                        }
                    }
                } else {
                    Ok(None)
                }
            } else {
                Err(ToolDiscoveryError::AgentContextRequired)
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

        response.result.map_err(|error| {
            let agent_type = agent_type
                .as_ref()
                .map_or("<missing>", |agent_type| agent_type.0.as_str());
            terminal_tool_discovery_error(format!(
                "failed to discover tool '{}' for agent type '{}' in environment '{environment_id}': {error}",
                tool_name, agent_type
            ))
        })
    }
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
        let memory = AttachmentMemory::tracked(self.public_state.worker().active_agents());
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
            let memory = AttachmentMemory::tracked(ctx.public_state.worker().active_agents());
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
            let memory = AttachmentMemory::tracked(ctx.public_state.worker().active_agents());
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
    ) -> anyhow::Result<Vec<Result<InvocationResult, RpcError>>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host", "get-invoke-results");
        });
        get_tool_invoke_results(accessor, &futures).await
    }
}

impl<Ctx: WorkerCtx> HostUnderlyingTool for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> HostToolCommon for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostUnderlyingToolWithStore<U> for ToolCommonHost<Ctx> {
    async fn drop(
        accessor: &Accessor<U, Self>,
        rep: Resource<UnderlyingTool>,
    ) -> anyhow::Result<()> {
        accessor.with(|mut access| {
            let _ = access.get().table().delete(rep);
            Ok(())
        })
    }

    async fn invoke(
        accessor: &Accessor<U, Self>,
        self_: Resource<UnderlyingTool>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<StreamReader<u8>>,
    ) -> anyhow::Result<Result<InvocationResult, ToolError>> {
        let accessor = accessor.with_getter::<HasSelf<DurableWorkerCtx<Ctx>>>(accessor.getter());
        let accessor = &accessor;
        let underlying = accessor
            .with(|mut access| Ok::<_, anyhow::Error>(access.get().table().get(&self_)?.clone()))?;
        let rpc = accessor.with(|mut access| {
            let tool_name = ToolName::try_from(underlying.tool_name).map_err(anyhow::Error::msg)?;
            tool_rpc_for_current_owner(access.get(), tool_name)
        })?;
        let stdin = stdin
            .map(|stdin| create_underlying_stdin(accessor, stdin))
            .transpose()?;
        let stdin_rep = stdin.as_ref().map(Resource::rep);
        let (stdout, stdout_consumer) = if underlying.has_stdout {
            let (stdout, consumer) = match create_underlying_stdout(accessor) {
                Ok(stdout) => stdout,
                Err(error) => {
                    return Err(cleanup_failed_tool_dispatch(
                        accessor, stdin_rep, None, error,
                    ));
                }
            };
            (Some(stdout), Some(consumer))
        } else {
            (None, None)
        };
        let response =
            invoke_tool_terminal(accessor, rpc, command_path, input, stdin, stdout).await?;
        project_underlying_tool_response(accessor, response, stdout_consumer)
    }
}

impl<Ctx: WorkerCtx> HostToolRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, tool_name: String) -> anyhow::Result<Resource<ToolRpcEntry>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "new");
        let tool_name = ToolName::try_from(tool_name).map_err(|error| anyhow!(error))?;
        let rpc = tool_rpc_for_current_owner(self, tool_name)?;
        Ok(self.table().push(rpc)?)
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
    ) -> anyhow::Result<Result<(), RpcError>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "invoke");
        });
        let rpc = tool_rpc_resource(accessor, &self_)?;
        let stdin_rep = stdin.as_ref().map(Resource::rep);
        let dispatch = dispatch_tool_call(
            accessor,
            rpc,
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
    ) -> anyhow::Result<Result<InvocationResult, RpcError>> {
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
            rpc,
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
    ) -> anyhow::Result<Result<InvocationResult, RpcError>> {
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
        ToolStdinStreamConsumer, ToolStdoutWriterEntry, UnderlyingToolStdinStreamConsumer,
        WitRegisteredTool, classify_tool_discovery_error, cleanup_tool_endpoints,
        recorded_tool_body_is_skipped, resolve_tool_command, stdout_limit_error,
        terminal_tool_discovery_error, validate_stream_attachments,
    };
    use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
    use crate::durable_host::entity::RecordedEntityTerminal;
    use crate::durable_host::tool::attachment::{
        AttachmentMemory, ToolAttachmentModeMetadata, attachment_pair,
    };
    use crate::preview2::golem::tool::host::{ByteStreamCloseCause, ByteStreamFailure};
    use crate::services::environment_state::ToolDiscoveryError;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::entity::EntityCallMode;
    use golem_common::model::oplog::HostResponseEntityInvocation;
    use golem_common::model::oplog::payload::types::{
        SerializableEntityBodyExecution, SerializableToolError, SerializableToolOperationTerminal,
        SerializableToolRpcError,
    };
    use golem_common::model::tool::{RegisteredTool, ToolProvisionConfig, ToolSource};
    use golem_common::schema::tool::{
        CommandBody, CommandNode, CommandTree, Constraint, DiscoveredTool, Doc, Globals,
        Positional, Positionals, Ref, Tool,
    };
    use golem_common::schema::{
        IntoTypedSchemaValue, MetadataEnvelope, SchemaGraph, SchemaType, SchemaTypeDef,
        SchemaValue, TypeId, TypedSchemaValue,
    };
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use test_r::test;
    use tokio::sync::mpsc;
    use wasmtime::component::{
        Component, Destination, Linker, StreamProducer, StreamReader, StreamResult,
    };
    use wasmtime::{Config, Engine, Store, StoreContextMut};

    struct OneBufferProducer {
        buffer: Option<bytes::Bytes>,
    }

    impl<D> StreamProducer<D> for OneBufferProducer {
        type Item = u8;
        type Buffer = bytes::Bytes;

        fn poll_produce<'a>(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            store: StoreContextMut<'a, D>,
            mut destination: Destination<'a, Self::Item, Self::Buffer>,
            finish: bool,
        ) -> Poll<wasmtime::Result<StreamResult>> {
            if finish {
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }
            if destination.remaining(store) == Some(0) {
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            match self.buffer.take() {
                Some(buffer) => {
                    destination.set_buffer(buffer);
                    Poll::Ready(Ok(StreamResult::Completed))
                }
                None => Poll::Ready(Ok(StreamResult::Dropped)),
            }
        }
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

    fn raw_wasmtime_writer_component(engine: &Engine) -> Component {
        Component::new(
            engine,
            r#"
(component
  (import "attach" (func $attach))
  (core func $attach (canon lower (func $attach)))
  (core module $memory (memory (export "mem") 1))
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
    (data (i32.const 0) "\01\02\03\04\05\06\07\08\09\0a\0b\0c")
    (func (export "start") (result i32)
      (local $pair i64)
      (local.set $pair (call $stream.new))
      (global.set $writer (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
      (i32.wrap_i64 (local.get $pair))
    )
    (func (export "cancel-without-acknowledgement")
      (local $result i32)
      (local.set $result (call $stream.write-async (global.get $writer) (i32.const 9) (i32.const 3)))
      (if (i32.ne (local.get $result) (i32.const -1)) (then unreachable))
      (call $attach)
      (local.set $result (call $stream.cancel-write (global.get $writer)))
      (if (i32.ne (local.get $result) (i32.const 2)) (then unreachable))
    )
    (func (export "attach") (call $attach))
    (func (export "write-first")
      (local $result i32)
      (local.set $result (call $stream.write-sync (global.get $writer) (i32.const 0) (i32.const 3)))
      (if (i32.ne (local.get $result) (i32.const 48)) (then unreachable))
    )
    (func (export "cancel-with-pending-acknowledgement")
      (local $result i32)
      (local.set $result (call $stream.write-async (global.get $writer) (i32.const 3) (i32.const 3)))
      (if (i32.ne (local.get $result) (i32.const -1)) (then unreachable))
      (local.set $result (call $stream.cancel-write (global.get $writer)))
      (if (i32.ne (local.get $result) (i32.const 2)) (then unreachable))
    )
    (func (export "resume-and-close")
      (local $result i32)
      (local.set $result (call $stream.write-sync (global.get $writer) (i32.const 6) (i32.const 3)))
      (if (i32.ne (local.get $result) (i32.const 48)) (then unreachable))
      (call $stream.drop-writable (global.get $writer))
    )
  )
  (type $stream (stream u8))
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
  (func (export "start") async (result (stream u8))
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

    async fn assert_raw_stdin_operation_cancellation(pending_acknowledgement: bool) {
        let (_attachment_producer, _attachment_consumer, observer) =
            attachment_pair(3, AttachmentMemory::inert());
        let (items, mut received) = mpsc::unbounded_channel();
        let mut config = Config::new();
        config.concurrency_support(true);
        config.wasm_component_model_more_async_builtins(true);
        let engine = Engine::new(&config).unwrap();
        let component = raw_wasmtime_writer_component(&engine);
        let mut store = Store::new(&engine, ());
        let reader_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<StreamReader<u8>>));
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
                    .expect("raw stream reader was not ready to attach")
                    .pipe(
                        &mut store,
                        UnderlyingToolStdinStreamConsumer::new(items.clone(), observer.clone(), 3),
                    )?;
                Ok(())
            })
            .unwrap();
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let start = instance
            .get_typed_func::<(), (StreamReader<u8>,)>(&mut store, "start")
            .unwrap();
        let write_first = instance
            .get_typed_func::<(), ()>(&mut store, "write-first")
            .unwrap();
        let attach = instance
            .get_typed_func::<(), ()>(&mut store, "attach")
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
    async fn cancelling_raw_stdin_without_acknowledgement_resumes_without_false_eof() {
        assert_raw_stdin_operation_cancellation(false).await;
    }

    #[test]
    async fn cancelling_raw_stdin_with_pending_acknowledgement_preserves_it_and_resumes() {
        assert_raw_stdin_operation_cancellation(true).await;
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
    async fn underlying_stdin_splits_large_source_and_waits_for_each_acknowledgement() {
        let (_attachment_producer, _attachment_consumer, observer) =
            attachment_pair(3, AttachmentMemory::inert());
        let (items, mut received) = mpsc::unbounded_channel();
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());

        let chunks = store
            .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<Vec<u8>>> {
                accessor.with(|mut store| {
                    StreamReader::new(
                        &mut store,
                        OneBufferProducer {
                            buffer: Some(bytes::Bytes::from_static(b"1234567")),
                        },
                    )?
                    .pipe(
                        &mut store,
                        UnderlyingToolStdinStreamConsumer::new(items, observer, 3),
                    )
                })?;

                let mut chunks = Vec::new();
                let mut received_bytes = 0;
                while let Some(item) = received.recv().await {
                    let chunk = item.item.unwrap();
                    received_bytes += chunk.len();
                    chunks.push(chunk);
                    if received_bytes < 7 {
                        assert!(
                            tokio::time::timeout(
                                std::time::Duration::from_millis(25),
                                received.recv()
                            )
                            .await
                            .is_err(),
                            "the next chunk arrived before the previous chunk was acknowledged"
                        );
                        item.acknowledged.send(()).unwrap();
                    } else {
                        let _ = item.acknowledged.send(());
                    }
                }
                Ok(chunks)
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            chunks,
            vec![b"123".to_vec(), b"456".to_vec(), b"7".to_vec()]
        );
    }

    #[test]
    async fn underlying_stdin_cancellation_wakes_a_pending_acknowledgement() {
        let (attachment_producer, _attachment_consumer, observer) =
            attachment_pair(3, AttachmentMemory::inert());
        let (items, mut received) = mpsc::unbounded_channel();
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());

        store
            .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
                accessor.with(|mut store| {
                    StreamReader::new(
                        &mut store,
                        OneBufferProducer {
                            buffer: Some(bytes::Bytes::from_static(b"123456")),
                        },
                    )?
                    .pipe(
                        &mut store,
                        UnderlyingToolStdinStreamConsumer::new(items, observer, 3),
                    )
                })?;

                let first = received.recv().await.unwrap();
                assert_eq!(first.item.unwrap(), b"123");
                attachment_producer.cancel().unwrap();
                assert!(
                    tokio::time::timeout(std::time::Duration::from_secs(1), received.recv())
                        .await
                        .unwrap()
                        .is_none()
                );
                drop(first.acknowledged);
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();
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
                definition,
                provision: ToolProvisionConfig::default(),
                source: ToolSource::Component {
                    component_id,
                    component_revision: ComponentRevision::try_from(7_u64).unwrap(),
                    component_name: ComponentName("search-tools".to_string()),
                },
                owner_account_id: AccountId::new(),
                owner_account_email: AccountEmail::new("owner@example.com"),
                metadata_version: "0.1.0".to_string(),
            },
            component_id,
        )
    }

    #[test]
    fn registered_component_tool_converts_to_discovery_wit_record() {
        let (registered, component_id) = registered_tool();
        let expected_definition = registered.definition.clone();

        let discovered = DiscoveredTool::from(registered);
        assert_eq!(discovered.definition, expected_definition);
        assert_eq!(discovered.implemented_by, component_id);

        let wit = WitRegisteredTool::try_from(&discovered).unwrap();
        assert_eq!(
            Tool::try_from(&wit.definition).unwrap(),
            expected_definition
        );
        assert_eq!(ComponentId::from(wit.implemented_by), component_id);
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
        assert_eq!(resolved.args, vec!["\"needle\""]);
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
    fn stream_attachment_validation_uses_actual_attachments_and_call_mode() {
        let command = ResolvedToolCommand {
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

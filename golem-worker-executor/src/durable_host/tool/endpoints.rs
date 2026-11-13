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

use super::StreamCancelReason;
use super::{
    Accessor, Arc, AsContextMut, AttachmentConsumer, AttachmentController, AttachmentMemory,
    AttachmentObserver, AttachmentStreamProducer, ByteStreamFailure, Cancellable,
    DurableByteInputProducer, DurableCallSession, DurableFunctionType, DurableInputEndpoint,
    DurableInputProducer, DurableWorkerCtx, GolemToolResponseSecretHoldAdmission, HasActiveAgents,
    HasSelf, HasWorker, HostRequestGolemToolResponseSecretHoldAdmission,
    HostResponseGolemToolResponseSecretHoldAdmission, NativeToolOutput, NativeToolStdin, OnceLock,
    Pin, Poll, Resource, SerializableToolError, SerializableToolRpcError, Source, StoreContextMut,
    StreamConsumer, StreamReader, StreamResult, TaskContext, ToolBodyStdin, ToolCallStdin,
    ToolInvokeResponse, ToolOutputEntry, ToolOutputWriterEntry, ToolStdinEntry,
    ToolStdinStreamConsumer, ToolStdinStreamItem, ToolStdinStreamPumpTask,
    UnderlyingToolStdinStreamConsumer, WorkerCtx, WorkerExecutorError, attachment_pair,
    authorize_live_permissions_at_serialized_access, discard_producer, durability, input, mpsc,
    oneshot, secret_hold_targets_for_value,
};

impl<Ctx: WorkerCtx> ToolStdinStreamPumpTask<Ctx> {
    pub(super) async fn run<U: Send + 'static>(
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
        // A tool's stdin has a failure item, so a client's cancel reaches the tool as
        // `cancelled`. Every other end of the source is a clean end.
        let cancelled = matches!(
            self.source_cancel.as_ref().and_then(|reason| reason.get()),
            Some(StreamCancelReason::Cancelled)
        );
        let _ = if cancelled {
            self.producer.fail(ByteStreamFailure::Cancelled)
        } else {
            self.producer.finish()
        };
        Ok(())
    }
}

impl ToolStdinEntry {
    pub(crate) fn controller(&self) -> AttachmentController {
        self.consumer.controller()
    }

    pub(crate) fn into_stream_producer(self) -> AttachmentStreamProducer {
        self.consumer.into_stream_producer()
    }

    pub(super) fn into_native<Ctx: WorkerCtx>(
        self,
        ctx: &DurableWorkerCtx<Ctx>,
        tasks: crate::worker::tasks::NativeReconstructionSpawner,
    ) -> anyhow::Result<NativeToolStdin> {
        Ok(NativeToolStdin {
            controller: self.consumer.controller(),
            input: input::NativeAttachmentInput::new(self.consumer, ctx)?,
            tasks,
        })
    }
}

impl ToolBodyStdin {
    pub(super) fn controller(&self) -> Option<AttachmentController> {
        match self {
            Self::Attachment(stdin) => Some(stdin.controller()),
            Self::Durable(_) => None,
        }
    }
}

impl ToolStdinStreamConsumer {
    pub(super) fn new(
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

impl UnderlyingToolStdinStreamConsumer {
    pub(super) fn new(
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

impl ToolOutputEntry {
    pub(crate) fn into_writer(mut self) -> ToolOutputWriterEntry {
        ToolOutputWriterEntry {
            producer: self
                .producer
                .take()
                .expect("output target already consumed"),
            completion_only: self.completion_only,
        }
    }
}

impl Drop for ToolOutputEntry {
    fn drop(&mut self) {
        if let Some(producer) = &self.producer {
            let _ = producer.abandon_unconfigured();
        }
    }
}

impl ToolOutputWriterEntry {
    pub(crate) fn discard(memory: AttachmentMemory) -> Self {
        Self {
            producer: discard_producer(memory),
            completion_only: false,
        }
    }

    pub(crate) fn controller(&self) -> AttachmentController {
        self.producer.controller()
    }

    pub(super) fn completion_only(&self) -> bool {
        self.completion_only
    }

    pub(super) fn native_writer(
        &self,
        tasks: crate::worker::tasks::NativeReconstructionSpawner,
    ) -> NativeToolOutput {
        NativeToolOutput {
            writer: self.producer.writer(),
            tasks,
        }
    }
}

pub(super) fn materialize_output_writer(
    output: Option<ToolOutputEntry>,
    discard: bool,
    memory: impl FnOnce() -> AttachmentMemory,
) -> Option<ToolOutputWriterEntry> {
    output
        .map(ToolOutputEntry::into_writer)
        .or_else(|| discard.then(|| ToolOutputWriterEntry::discard(memory())))
}

pub(super) fn native_output_handles(
    stdout: Option<&ToolOutputWriterEntry>,
    stderr: Option<&ToolOutputWriterEntry>,
    tasks: &crate::worker::tasks::NativeReconstructionSpawner,
) -> (Option<NativeToolOutput>, Option<NativeToolOutput>) {
    (
        stdout.map(|output| output.native_writer(tasks.clone())),
        stderr.map(|output| output.native_writer(tasks.clone())),
    )
}

pub(super) async fn admit_tool_response_secret_holds<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: ToolInvokeResponse,
    invocation_id: crate::durable_host::concurrent::InvocationIdSource,
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
        DurableCallSession::<GolemToolResponseSecretHoldAdmission, Cancellable>::invoke_access_with_options(
            accessor,
            accessor.getter(),
            request,
            DurableFunctionType::ReadLocal,
            crate::durable_host::concurrent::AccessClaimOptions {
                invocation_id,
                ..Default::default()
            },
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

pub(super) fn close_stdin<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    stdin: Option<ToolCallStdin>,
) -> anyhow::Result<()>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    match stdin {
        Some(ToolCallStdin::Attachment(stdin)) => {
            accessor.with(|mut access| access.get().table().delete(stdin).map(|_| ()))?;
        }
        // The end of the call's session cancels the durable stdin that nobody read.
        Some(ToolCallStdin::Durable(_)) | None => {}
    }
    Ok(())
}

pub(super) fn reject_output<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    output: Option<Resource<ToolOutputEntry>>,
) -> anyhow::Result<()>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    if let Some(output) = output {
        accessor.with(|mut access| {
            let ctx = access.get();
            let mut output = ctx.table().delete(output)?;
            durability::dispose_producer(
                ctx,
                output
                    .producer
                    .take()
                    .expect("output target already consumed"),
                ByteStreamFailure::Failed("tool invocation rejected".to_string()),
                true,
            )
        })?;
    }
    Ok(())
}

pub(super) fn cleanup_tool_endpoints(
    error: anyhow::Error,
    cleanup_stdin: impl FnOnce() -> anyhow::Result<()>,
    cleanup_stdout: impl FnOnce() -> anyhow::Result<()>,
    cleanup_stderr: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Error {
    let mut cleanup_failures = Vec::new();
    if let Err(cleanup) = cleanup_stdin() {
        cleanup_failures.push(format!("stdin cleanup failed: {cleanup}"));
    }
    if let Err(cleanup) = cleanup_stdout() {
        cleanup_failures.push(format!("stdout cleanup failed: {cleanup}"));
    }
    if let Err(cleanup) = cleanup_stderr() {
        cleanup_failures.push(format!("stderr cleanup failed: {cleanup}"));
    }
    if cleanup_failures.is_empty() {
        error
    } else {
        error.context(cleanup_failures.join("; "))
    }
}

pub(super) fn cleanup_failed_tool_dispatch<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    stdin: Option<u32>,
    stdout: Option<u32>,
    stderr: Option<u32>,
    error: anyhow::Error,
) -> anyhow::Error
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    cleanup_tool_endpoints(
        error,
        || {
            close_stdin(
                accessor,
                stdin.map(|rep| ToolCallStdin::Attachment(Resource::new_own(rep))),
            )
        },
        || reject_output(accessor, stdout.map(Resource::new_own)),
        || reject_output(accessor, stderr.map(Resource::new_own)),
    )
}

pub(super) fn create_underlying_stdin<U, Ctx>(
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
                || Ok(()),
            ));
        }
        let task = ToolStdinStreamPumpTask::<Ctx> {
            producer,
            items: received,
            source_cancel: None,
            _ctx: std::marker::PhantomData,
        };
        access.spawn(async move |access| task.run(access).await)?;
        Ok(stdin)
    })
}

/// Feeds the durable stdin of a call that a client started into an attachment for a native
/// body. The host reads the durable input; dropping the reader cancels it for the client.
pub(super) fn create_native_underlying_stdin<Ctx>(
    accessor: &Accessor<Ctx, HasSelf<DurableWorkerCtx<Ctx>>>,
    endpoint: DurableInputEndpoint,
) -> anyhow::Result<Resource<ToolStdinEntry>>
where
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let source_cancel = Arc::new(OnceLock::new());
        let ctx = access.get();
        let producer = DurableByteInputProducer(
            DurableInputProducer::new(endpoint)
                .with_drop_cleanup(
                    ctx.state
                        .dropped_call_event_sender()
                        .expect("dropped-call event sender is always available"),
                    ctx.stream_runtime_teardown_probe(),
                )
                .with_producer_cancel_observer(source_cancel.clone()),
        );
        let source = StreamReader::new(&mut access, producer)?;
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
                || Ok(()),
            ));
        }
        let task = ToolStdinStreamPumpTask::<Ctx> {
            producer,
            items: received,
            source_cancel: Some(source_cancel),
            _ctx: std::marker::PhantomData,
        };
        access.spawn(async move |access| task.run(access).await)?;
        Ok(stdin)
    })
}

pub(super) fn create_output_attachment<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    completion_only: bool,
) -> anyhow::Result<(Resource<ToolOutputEntry>, AttachmentConsumer)>
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
        let target = ctx.table().push(ToolOutputEntry {
            producer: Some(producer),
            completion_only,
        })?;
        Ok((target, consumer))
    })
}

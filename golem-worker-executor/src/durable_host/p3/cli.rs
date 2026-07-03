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

use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::durable_host::p3::{DurableP3, DurableP3View, durable_worker_ctx};
use crate::model::event::InternalWorkerEvent;
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use crate::workerctx::{LogEventEmitBehaviour, PublicWorkerIo};
use bytes::BytesMut;
use golem_common::model::oplog::{LogLevel, OplogEntry};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::oneshot;
use wasmtime::AsContextMut as _;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Accessor, AccessorTask, Destination, FutureReader, Resource, Source, StreamConsumer,
    StreamProducer, StreamReader, StreamResult,
};
use wasmtime_wasi::cli::{StdinStream, WasiCliView};
use wasmtime_wasi::p3::bindings::cli::types::ErrorCode;
use wasmtime_wasi::p3::bindings::cli::{
    environment, exit, stderr, stdin, stdout, terminal_input, terminal_output, terminal_stderr,
    terminal_stdin, terminal_stdout,
};
use wasmtime_wasi::p3::cli::{TerminalInput, TerminalOutput};

#[derive(Clone, Copy)]
enum StandardStream {
    Stdout,
    Stderr,
}

const STDIO_BUFFER_CAPACITY: usize = 8192;

fn io_error_to_error_code(error: std::io::Error) -> ErrorCode {
    match error.kind() {
        std::io::ErrorKind::BrokenPipe => ErrorCode::Pipe,
        other => {
            tracing::warn!("stdio error: {other}");
            ErrorCode::Io
        }
    }
}

/// Plain pass-through producer for standard input.
///
/// Standard input is never recorded in the oplog: on replay the underlying host stream is read
/// again, exactly like the P2 implementation. This mirrors the default wasmtime p3 stdin producer.
struct StdinStreamProducer {
    rx: Pin<Box<dyn AsyncRead + Send + Sync>>,
    result_tx: Option<oneshot::Sender<ErrorCode>>,
}

impl<D> StreamProducer<D> for StdinStreamProducer {
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let mut dst = dst.as_direct(store, STDIO_BUFFER_CAPACITY);
        let mut buf = ReadBuf::new(dst.remaining());
        match self.rx.as_mut().poll_read(cx, &mut buf) {
            Poll::Ready(Ok(())) if buf.filled().is_empty() => {
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Ok(())) => {
                let n = buf.filled().len();
                dst.mark_written(n);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Err(error)) => {
                if let Some(result_tx) = self.result_tx.take() {
                    let _ = result_tx.send(io_error_to_error_code(error));
                }
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Accumulates the bytes written via a single `write-via-stream` call.
///
/// All bytes of one call are coalesced into a single log event. The emitted log message therefore
/// depends only on the total bytes the (deterministic) guest wrote in that call, never on
/// non-deterministic host chunk boundaries. This keeps the message-hash based replay deduplication
/// (`seen_log`) correct across recovery re-runs.
struct CapturingOutputStreamConsumer {
    contents: Vec<u8>,
    bytes_tx: Option<oneshot::Sender<Vec<u8>>>,
}

impl CapturingOutputStreamConsumer {
    fn new(bytes_tx: oneshot::Sender<Vec<u8>>) -> Self {
        Self {
            contents: Vec::new(),
            bytes_tx: Some(bytes_tx),
        }
    }

    fn close(&mut self) {
        if let Some(bytes_tx) = self.bytes_tx.take() {
            let _ = bytes_tx.send(std::mem::take(&mut self.contents));
        }
    }
}

impl<D> StreamConsumer<D> for CapturingOutputStreamConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut src = src.as_direct(store);
        let bytes = src.remaining();
        if bytes.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let len = bytes.len();
        self.contents.extend_from_slice(bytes);
        src.mark_read(len);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for CapturingOutputStreamConsumer {
    fn drop(&mut self) {
        self.close();
    }
}

struct StdioWriteTask<Ctx> {
    stream: StandardStream,
    bytes_rx: oneshot::Receiver<Vec<u8>>,
    result_tx: oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for StdioWriteTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let contents = self.bytes_rx.await.unwrap_or_default();
        if !contents.is_empty() {
            let event = match self.stream {
                StandardStream::Stdout => InternalWorkerEvent::stdout(contents),
                StandardStream::Stderr => InternalWorkerEvent::stderr(contents),
            };
            emit_log_event_access::<Ctx, U>(accessor, event).await;
        }
        let _ = self.result_tx.send(Ok(Ok(())));
        Ok(())
    }
}

async fn wait_stdio_task_result(
    result_rx: oneshot::Receiver<wasmtime::Result<Result<(), ErrorCode>>>,
) -> wasmtime::Result<Result<(), ErrorCode>> {
    result_rx
        .await
        .unwrap_or_else(|_| Err(wasmtime::Error::msg("stdio task dropped")))
}

async fn emit_log_event_access<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    event: InternalWorkerEvent,
) {
    if let Some(entry) = event.as_oplog_entry()
        && let OplogEntry::Log {
            level,
            context,
            message,
            ..
        } = &entry
    {
        let (has_oplog_processor, owned_agent_id, public_state, replay_state, oplog, is_live) =
            accessor.with(|mut access| {
                let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                (
                    ctx.state.component_metadata.metadata.has_oplog_processor(),
                    ctx.owned_agent_id.clone(),
                    ctx.public_state.clone(),
                    ctx.state.replay_state.clone(),
                    ctx.state.oplog.clone(),
                    ctx.state.is_live(),
                )
            });

        if has_oplog_processor {
            match level {
                LogLevel::Stdout | LogLevel::Debug | LogLevel::Trace => {
                    tracing::debug!(
                        plugin_agent = %owned_agent_id,
                        context,
                        "Plugin: {message}"
                    );
                }
                LogLevel::Stderr | LogLevel::Info => {
                    tracing::info!(
                        plugin_agent = %owned_agent_id,
                        context,
                        "Plugin: {message}"
                    );
                }
                LogLevel::Warn => {
                    tracing::warn!(
                        plugin_agent = %owned_agent_id,
                        context,
                        "Plugin: {message}"
                    );
                }
                LogLevel::Error | LogLevel::Critical => {
                    tracing::error!(
                        plugin_agent = %owned_agent_id,
                        context,
                        "Plugin: {message}"
                    );
                }
            }
        }

        match Ctx::LOG_EVENT_EMIT_BEHAVIOUR {
            LogEventEmitBehaviour::LiveOnly => {
                if is_live {
                    if !replay_state.seen_log(*level, context, message).await {
                        public_state.event_service().emit_event(event.clone(), true);
                        public_state.worker().add_to_oplog(entry).await;
                    } else {
                        public_state
                            .event_service()
                            .emit_event(event.clone(), false);
                        replay_state.remove_seen_log(*level, context, message).await;
                    }
                }
            }
            LogEventEmitBehaviour::Always => {
                public_state.event_service().emit_event(event.clone(), true);

                if is_live && !replay_state.seen_log(*level, context, message).await {
                    oplog.add(entry).await;
                }
            }
        }
    }
}

async fn write_standard_stream_via_stream<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    data: StreamReader<u8>,
    stream: StandardStream,
) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let (bytes_tx, bytes_rx) = oneshot::channel();
    let (result_tx, result_rx) = oneshot::channel();
    accessor.with(|mut store| {
        data.pipe(&mut store, CapturingOutputStreamConsumer::new(bytes_tx))?;
        store.spawn(StdioWriteTask::<Ctx> {
            stream,
            bytes_rx,
            result_tx,
            _phantom: PhantomData,
        });
        FutureReader::new(&mut store, wait_stdio_task_result(result_rx))
    })
}

impl<Ctx: WorkerCtx> environment::Host for DurableP3View<'_, Ctx> {
    fn get_environment(&mut self) -> wasmtime::Result<Vec<(String, String)>> {
        environment::Host::get_environment(&mut WasiCliView::cli(self.0))
    }

    fn get_arguments(&mut self) -> wasmtime::Result<Vec<String>> {
        environment::Host::get_arguments(&mut WasiCliView::cli(self.0))
    }

    fn get_initial_cwd(&mut self) -> wasmtime::Result<Option<String>> {
        environment::Host::get_initial_cwd(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> exit::Host for DurableP3View<'_, Ctx> {
    fn exit(&mut self, status: Result<(), ()>) -> wasmtime::Result<()> {
        exit::Host::exit(&mut WasiCliView::cli(self.0), status)
    }

    fn exit_with_code(&mut self, status_code: u8) -> wasmtime::Result<()> {
        exit::Host::exit_with_code(&mut WasiCliView::cli(self.0), status_code)
    }
}

impl<Ctx: WorkerCtx> terminal_input::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> terminal_input::HostTerminalInput for DurableP3View<'_, Ctx> {
    fn drop(&mut self, rep: Resource<TerminalInput>) -> wasmtime::Result<()> {
        terminal_input::HostTerminalInput::drop(&mut WasiCliView::cli(self.0), rep)
    }
}

impl<Ctx: WorkerCtx> terminal_output::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> terminal_output::HostTerminalOutput for DurableP3View<'_, Ctx> {
    fn drop(&mut self, rep: Resource<TerminalOutput>) -> wasmtime::Result<()> {
        terminal_output::HostTerminalOutput::drop(&mut WasiCliView::cli(self.0), rep)
    }
}

impl<Ctx: WorkerCtx> terminal_stdin::Host for DurableP3View<'_, Ctx> {
    fn get_terminal_stdin(&mut self) -> wasmtime::Result<Option<Resource<TerminalInput>>> {
        terminal_stdin::Host::get_terminal_stdin(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> terminal_stdout::Host for DurableP3View<'_, Ctx> {
    fn get_terminal_stdout(&mut self) -> wasmtime::Result<Option<Resource<TerminalOutput>>> {
        terminal_stdout::Host::get_terminal_stdout(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> terminal_stderr::Host for DurableP3View<'_, Ctx> {
    fn get_terminal_stderr(&mut self) -> wasmtime::Result<Option<Resource<TerminalOutput>>> {
        terminal_stderr::Host::get_terminal_stderr(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> stdin::Host for DurableP3View<'_, Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> stdin::HostWithStore<U> for DurableP3<Ctx> {
    async fn read_via_stream(
        accessor: &Accessor<U, Self>,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), ErrorCode>>)> {
        accessor.with(|mut store| {
            let ctx = durable_worker_ctx::<Ctx, U>(store.data_mut());
            if ctx.stdin.is_disabled() {
                return Err(wasmtime::Error::msg("standard input is disabled"));
            }
            let rx = ctx.stdin.async_stream();

            let (result_tx, result_rx) = oneshot::channel();
            let stream = StreamReader::new(
                &mut store,
                StdinStreamProducer {
                    rx: Box::into_pin(rx),
                    result_tx: Some(result_tx),
                },
            )?;
            let future = FutureReader::new(&mut store, async move {
                wasmtime::error::Ok(match result_rx.await {
                    Ok(error) => Err(error),
                    Err(_) => Ok(()),
                })
            })?;
            Ok((stream, future))
        })
    }
}

impl<Ctx: WorkerCtx> stdout::Host for DurableP3View<'_, Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> stdout::HostWithStore<U> for DurableP3<Ctx> {
    async fn write_via_stream(
        accessor: &Accessor<U, Self>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>> {
        write_standard_stream_via_stream::<Ctx, U>(accessor, data, StandardStream::Stdout).await
    }
}

impl<Ctx: WorkerCtx> stderr::Host for DurableP3View<'_, Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> stderr::HostWithStore<U> for DurableP3<Ctx> {
    async fn write_via_stream(
        accessor: &Accessor<U, Self>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>> {
        write_standard_stream_via_stream::<Ctx, U>(accessor, data, StandardStream::Stderr).await
    }
}

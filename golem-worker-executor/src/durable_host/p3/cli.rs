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

use std::io::Cursor;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable};
use crate::durable_host::p3::{DurableP3, DurableP3View, durable_worker_ctx};
use crate::model::event::InternalWorkerEvent;
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use crate::workerctx::{LogEventEmitBehaviour, PublicWorkerIo};
use bytes::BytesMut;
use golem_common::model::oplog::host_functions::{
    P3CliStderrWriteViaStream, P3CliStdinReadViaStream, P3CliStdoutWriteViaStream,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestNoInput, HostResponseP3CliStream, LogLevel,
    OplogEntry,
};
use tokio::io::{AsyncRead, ReadBuf};
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

#[derive(Clone)]
struct CapturedStream {
    contents: Vec<u8>,
    result: Result<(), ErrorCode>,
}

enum StdinReadStreamMode {
    Replayed(CapturedStream),
    Live,
    Error(String),
}

enum StdinInputStreamProducerState {
    AwaitingReplay {
        rx: tokio::sync::oneshot::Receiver<StdinReadStreamMode>,
        live: Option<Pin<Box<dyn AsyncRead + Send + Sync>>>,
        result_tx: Option<tokio::sync::oneshot::Sender<CapturedStream>>,
    },
    Replayed {
        contents: Cursor<BytesMut>,
        result: Result<(), ErrorCode>,
        result_tx: Option<tokio::sync::oneshot::Sender<CapturedStream>>,
    },
    Live(RecordingInputStreamProducer),
    Done,
}

struct StdinInputStreamProducer {
    state: StdinInputStreamProducerState,
}

impl StdinInputStreamProducer {
    fn replaying_or_live(
        live: Box<dyn AsyncRead + Send + Sync>,
        result_tx: tokio::sync::oneshot::Sender<CapturedStream>,
        rx: tokio::sync::oneshot::Receiver<StdinReadStreamMode>,
    ) -> Self {
        Self {
            state: StdinInputStreamProducerState::AwaitingReplay {
                rx,
                live: Some(Box::into_pin(live)),
                result_tx: Some(result_tx),
            },
        }
    }

    fn live(
        live: Box<dyn AsyncRead + Send + Sync>,
        result_tx: tokio::sync::oneshot::Sender<CapturedStream>,
    ) -> Self {
        Self {
            state: StdinInputStreamProducerState::Live(RecordingInputStreamProducer::new(
                live, result_tx,
            )),
        }
    }

    fn close_replayed(
        contents: &mut Cursor<BytesMut>,
        result: &Result<(), ErrorCode>,
        result_tx: &mut Option<tokio::sync::oneshot::Sender<CapturedStream>>,
    ) {
        if let Some(result_tx) = result_tx.take() {
            let bytes = contents.get_ref();
            let position = contents.position() as usize;
            let result = if position >= bytes.len() {
                result.clone()
            } else {
                Ok(())
            };
            let _ = result_tx.send(CapturedStream {
                contents: bytes[..position.min(bytes.len())].to_vec(),
                result,
            });
        }
    }
}

impl<D> StreamProducer<D> for StdinInputStreamProducer {
    type Item = u8;
    type Buffer = Cursor<BytesMut>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        loop {
            match &mut self.state {
                StdinInputStreamProducerState::AwaitingReplay {
                    rx,
                    live,
                    result_tx,
                } => match Pin::new(rx).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(StdinReadStreamMode::Replayed(captured))) => {
                        let result_tx = result_tx
                            .take()
                            .expect("stdin result sender available for replay");
                        self.state = StdinInputStreamProducerState::Replayed {
                            contents: Cursor::new(BytesMut::from(captured.contents.as_slice())),
                            result: captured.result,
                            result_tx: Some(result_tx),
                        };
                    }
                    Poll::Ready(Ok(StdinReadStreamMode::Live)) => {
                        let live = live
                            .take()
                            .expect("live stdin stream available for incomplete replay");
                        let result_tx = result_tx
                            .take()
                            .expect("stdin result sender available for incomplete replay");
                        self.state =
                            StdinInputStreamProducerState::Live(RecordingInputStreamProducer {
                                rx: live,
                                contents: Vec::new(),
                                result_tx: Some(result_tx),
                            });
                    }
                    Poll::Ready(Ok(StdinReadStreamMode::Error(error))) => {
                        self.state = StdinInputStreamProducerState::Done;
                        return Poll::Ready(Err(wasmtime::Error::msg(error)));
                    }
                    Poll::Ready(Err(_)) => {
                        self.state = StdinInputStreamProducerState::Done;
                        return Poll::Ready(Err(wasmtime::Error::msg("stdio replay task dropped")));
                    }
                },
                StdinInputStreamProducerState::Replayed {
                    contents,
                    result,
                    result_tx,
                } => {
                    if dst.remaining(store.as_context_mut()) == Some(0) {
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }

                    let mut dst = dst.as_direct(store, STDIO_BUFFER_CAPACITY);
                    let bytes = contents.get_ref();
                    let position = contents.position() as usize;
                    if position >= bytes.len() {
                        Self::close_replayed(contents, result, result_tx);
                        self.state = StdinInputStreamProducerState::Done;
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }

                    let remaining = &bytes[position..];
                    let n = remaining.len().min(dst.remaining().len());
                    dst.remaining()[..n].copy_from_slice(&remaining[..n]);
                    dst.mark_written(n);
                    contents.set_position((position + n) as u64);
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                StdinInputStreamProducerState::Live(producer) => {
                    return Pin::new(producer).poll_produce(cx, store, dst, finish);
                }
                StdinInputStreamProducerState::Done => {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }
    }
}

impl Drop for StdinInputStreamProducer {
    fn drop(&mut self) {
        match &mut self.state {
            StdinInputStreamProducerState::AwaitingReplay { result_tx, .. } => {
                if let Some(result_tx) = result_tx.take() {
                    let _ = result_tx.send(CapturedStream {
                        contents: Vec::new(),
                        result: Ok(()),
                    });
                }
            }
            StdinInputStreamProducerState::Replayed {
                contents,
                result,
                result_tx,
            } => Self::close_replayed(contents, result, result_tx),
            StdinInputStreamProducerState::Live(_) | StdinInputStreamProducerState::Done => {}
        }
    }
}

struct RecordingInputStreamProducer {
    rx: Pin<Box<dyn AsyncRead + Send + Sync>>,
    contents: Vec<u8>,
    result_tx: Option<tokio::sync::oneshot::Sender<CapturedStream>>,
}

impl RecordingInputStreamProducer {
    fn new(
        rx: Box<dyn AsyncRead + Send + Sync>,
        result_tx: tokio::sync::oneshot::Sender<CapturedStream>,
    ) -> Self {
        Self {
            rx: Box::into_pin(rx),
            contents: Vec::new(),
            result_tx: Some(result_tx),
        }
    }

    fn close(&mut self, result: CapturedStream) {
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(result);
        }
    }
}

impl<D> StreamProducer<D> for RecordingInputStreamProducer {
    type Item = u8;
    type Buffer = Cursor<BytesMut>;

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
                let contents = std::mem::take(&mut self.contents);
                self.close(CapturedStream {
                    contents,
                    result: Ok(()),
                });
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Ok(())) => {
                let n = buf.filled().len();
                self.contents.extend_from_slice(buf.filled());
                dst.mark_written(n);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Err(error)) => {
                let contents = std::mem::take(&mut self.contents);
                self.close(CapturedStream {
                    contents,
                    result: Err(io_error_to_error_code(error)),
                });
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for RecordingInputStreamProducer {
    fn drop(&mut self) {
        if self.result_tx.is_some() {
            let contents = std::mem::take(&mut self.contents);
            self.close(CapturedStream {
                contents,
                result: Ok(()),
            });
        }
    }
}

fn io_error_to_error_code(error: std::io::Error) -> ErrorCode {
    match error.kind() {
        std::io::ErrorKind::BrokenPipe => ErrorCode::Pipe,
        other => {
            tracing::warn!("stdio error: {other}");
            ErrorCode::Io
        }
    }
}

struct CapturingOutputStreamConsumer {
    contents: Vec<u8>,
    result_tx: Option<tokio::sync::oneshot::Sender<CapturedStream>>,
}

impl CapturingOutputStreamConsumer {
    fn new(result_tx: tokio::sync::oneshot::Sender<CapturedStream>) -> Self {
        Self {
            contents: Vec::new(),
            result_tx: Some(result_tx),
        }
    }

    fn close(&mut self, result: CapturedStream) {
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(result);
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
        if self.result_tx.is_some() {
            let contents = std::mem::take(&mut self.contents);
            self.close(CapturedStream {
                contents,
                result: Ok(()),
            });
        }
    }
}

struct StdinReadTask<Ctx> {
    call: CallHandle<P3CliStdinReadViaStream, Cancellable>,
    bytes_rx: tokio::sync::oneshot::Receiver<CapturedStream>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

struct StdinReadReplayTask<Ctx> {
    call: CallHandle<P3CliStdinReadViaStream, Cancellable>,
    stream_mode_tx: tokio::sync::oneshot::Sender<StdinReadStreamMode>,
    bytes_rx: tokio::sync::oneshot::Receiver<CapturedStream>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> StdinReadReplayTask<Ctx> {
    fn new(
        call: CallHandle<P3CliStdinReadViaStream, Cancellable>,
        stream_mode_tx: tokio::sync::oneshot::Sender<StdinReadStreamMode>,
        bytes_rx: tokio::sync::oneshot::Receiver<CapturedStream>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            stream_mode_tx,
            bytes_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx> StdinReadTask<Ctx> {
    fn new(
        call: CallHandle<P3CliStdinReadViaStream, Cancellable>,
        bytes_rx: tokio::sync::oneshot::Receiver<CapturedStream>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            bytes_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for StdinReadTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let result =
            complete_stdio_read::<Ctx, U>(accessor, self.call, self.bytes_rx, &self.result_tx)
                .await;
        if !self.result_tx.is_closed() {
            let _ = self.result_tx.send(result);
        }
        Ok(())
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for StdinReadReplayTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        match self
            .call
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(wasmtime::Error::from)
        {
            Ok(CallReplayOutcome::Replayed(response)) => {
                let captured = CapturedStream {
                    contents: response.contents,
                    result: response.result.map_err(Into::into),
                };
                let _ = self
                    .stream_mode_tx
                    .send(StdinReadStreamMode::Replayed(captured));
                let result = match self.bytes_rx.await {
                    Ok(result) => Ok(result.result),
                    Err(_) => Err(wasmtime::Error::msg("stdio replay stream dropped")),
                };
                let _ = self.result_tx.send(result);
            }
            Ok(CallReplayOutcome::Incomplete(call)) => {
                let _ = self.stream_mode_tx.send(StdinReadStreamMode::Live);
                let result =
                    complete_stdio_read::<Ctx, U>(accessor, call, self.bytes_rx, &self.result_tx)
                        .await;
                if !self.result_tx.is_closed() {
                    let _ = self.result_tx.send(result);
                }
            }
            Err(error) => {
                let error = error.to_string();
                let _ = self
                    .stream_mode_tx
                    .send(StdinReadStreamMode::Error(error.clone()));
                let _ = self.result_tx.send(Err(wasmtime::Error::msg(error)));
            }
        }
        Ok(())
    }
}

async fn complete_stdio_read<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3CliStdinReadViaStream, Cancellable>,
    bytes_rx: tokio::sync::oneshot::Receiver<CapturedStream>,
    result_tx: &tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
) -> wasmtime::Result<Result<(), ErrorCode>>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let captured = match bytes_rx.await {
        Ok(result) => result,
        Err(_) => CapturedStream {
            contents: Vec::new(),
            result: Err(ErrorCode::Io),
        },
    };
    let response = HostResponseP3CliStream {
        contents: captured.contents,
        result: captured.result.map_err(Into::into),
    };

    if result_tx.is_closed() {
        call.cancel_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            Some(response.clone()),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        return Ok(response.result.map(|_| ()).map_err(Into::into));
    }

    let response = call
        .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)?;

    Ok(response.result.map(|_| ()).map_err(Into::into))
}

struct StdioWriteTask<Ctx, Pair>
where
    Pair: HostPayloadPair,
{
    stream: StandardStream,
    call: CallHandle<Pair, Cancellable>,
    bytes_rx: tokio::sync::oneshot::Receiver<CapturedStream>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
    _phantom: PhantomData<fn() -> (Ctx, Pair)>,
}

impl<Ctx, Pair> StdioWriteTask<Ctx, Pair>
where
    Pair: HostPayloadPair,
{
    fn new(
        stream: StandardStream,
        call: CallHandle<Pair, Cancellable>,
        bytes_rx: tokio::sync::oneshot::Receiver<CapturedStream>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
    ) -> Self {
        Self {
            stream,
            call,
            bytes_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, Pair, U> AccessorTask<U, DurableP3<Ctx>> for StdioWriteTask<Ctx, Pair>
where
    Ctx: WorkerCtx,
    Pair:
        HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseP3CliStream> + Send + 'static,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let result = complete_stdio_write::<Ctx, U, Pair>(
            accessor,
            self.stream,
            self.call,
            self.bytes_rx,
            &self.result_tx,
        )
        .await;
        if !self.result_tx.is_closed() {
            let _ = self.result_tx.send(result);
        }
        Ok(())
    }
}

async fn complete_stdio_write<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    stream: StandardStream,
    mut call: CallHandle<Pair, Cancellable>,
    bytes_rx: tokio::sync::oneshot::Receiver<CapturedStream>,
    result_tx: &tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
) -> wasmtime::Result<Result<(), ErrorCode>>
where
    Ctx: WorkerCtx,
    Pair:
        HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseP3CliStream> + Send + 'static,
    U: 'static,
{
    let captured = match bytes_rx.await {
        Ok(result) => result,
        Err(_) => CapturedStream {
            contents: Vec::new(),
            result: Err(ErrorCode::Io),
        },
    };
    let response = HostResponseP3CliStream {
        contents: captured.contents,
        result: captured.result.map_err(Into::into),
    };

    if result_tx.is_closed() {
        call.cancel_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            Some(response.clone()),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        return Ok(response.result.map_err(Into::into));
    }

    if !call.is_live() {
        match call
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(wasmtime::Error::from)?
        {
            CallReplayOutcome::Replayed(response) => {
                return apply_replayed_stdio_write_response::<Ctx, U>(accessor, stream, response)
                    .await;
            }
            CallReplayOutcome::Incomplete(live_call) => {
                call = live_call;
            }
        }
    }

    emit_standard_stream_event::<Ctx, U>(accessor, stream, response.contents.clone()).await?;

    if result_tx.is_closed() {
        call.cancel_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            Some(response.clone()),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        return Ok(response.result.map_err(Into::into));
    }

    let response = call
        .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)?;

    Ok(response.result.map_err(Into::into))
}

async fn apply_replayed_stdio_write_response<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    stream: StandardStream,
    response: HostResponseP3CliStream,
) -> wasmtime::Result<Result<(), ErrorCode>>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    emit_replayed_standard_stream_event::<Ctx, U>(accessor, stream, response.contents).await?;
    Ok(response.result.map_err(Into::into))
}

async fn wait_stdio_task_result(
    result_rx: tokio::sync::oneshot::Receiver<wasmtime::Result<Result<(), ErrorCode>>>,
) -> wasmtime::Result<Result<(), ErrorCode>> {
    result_rx
        .await
        .unwrap_or_else(|_| Err(wasmtime::Error::msg("stdio task dropped")))
}

async fn emit_standard_stream_event<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    stream: StandardStream,
    contents: Vec<u8>,
) -> wasmtime::Result<()> {
    let event = match stream {
        StandardStream::Stdout => InternalWorkerEvent::stdout(contents),
        StandardStream::Stderr => InternalWorkerEvent::stderr(contents),
    };
    emit_log_event_access::<Ctx, U>(accessor, event).await;
    Ok(())
}

async fn emit_replayed_standard_stream_event<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    stream: StandardStream,
    contents: Vec<u8>,
) -> wasmtime::Result<()> {
    let event = match stream {
        StandardStream::Stdout => InternalWorkerEvent::stdout(contents),
        StandardStream::Stderr => InternalWorkerEvent::stderr(contents),
    };

    if let Some(entry) = event.as_oplog_entry()
        && let OplogEntry::Log {
            level,
            context,
            message,
            ..
        } = &entry
    {
        let (has_oplog_processor, owned_agent_id, public_state, replay_state) =
            accessor.with(|mut access| {
                let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                (
                    ctx.state.component_metadata.metadata.has_oplog_processor(),
                    ctx.owned_agent_id.clone(),
                    ctx.public_state.clone(),
                    ctx.state.replay_state.clone(),
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

        public_state
            .event_service()
            .emit_event(event.clone(), false);
        replay_state.remove_seen_log(*level, context, message).await;
    }

    Ok(())
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

async fn start_stdio_read_call<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
) -> wasmtime::Result<CallHandle<P3CliStdinReadViaStream, Cancellable>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    CallHandle::<P3CliStdinReadViaStream, Cancellable>::start_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        HostRequestNoInput {},
        DurableFunctionType::ReadLocal,
    )
    .await
    .map_err(wasmtime::Error::from)
}

async fn write_standard_stream_via_stream<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    data: StreamReader<u8>,
    stream: StandardStream,
) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>>
where
    Ctx: WorkerCtx,
    Pair:
        HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseP3CliStream> + Send + 'static,
    U: Send + 'static,
{
    let (bytes_tx, bytes_rx) = tokio::sync::oneshot::channel();
    accessor
        .with(|mut store| data.pipe(&mut store, CapturingOutputStreamConsumer::new(bytes_tx)))?;

    let call = CallHandle::<Pair, Cancellable>::start_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        HostRequestNoInput {},
        DurableFunctionType::ReadLocal,
    )
    .await
    .map_err(wasmtime::Error::from)?;

    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    accessor.with(|mut store| {
        store.spawn(StdioWriteTask::<Ctx, Pair>::new(
            stream, call, bytes_rx, result_tx,
        ));

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

impl<Ctx: WorkerCtx> stdin::HostWithStore for DurableP3<Ctx> {
    async fn read_via_stream<U: Send + 'static>(
        accessor: &Accessor<U, Self>,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), ErrorCode>>)> {
        let stdin = accessor.with(|mut store| {
            let ctx = durable_worker_ctx::<Ctx, U>(store.data_mut());
            if ctx.stdin.is_disabled() {
                return Err(wasmtime::Error::msg("standard input is disabled"));
            }
            Ok(ctx.stdin.async_stream())
        })?;

        let call = start_stdio_read_call::<Ctx, U>(accessor).await?;

        let (bytes_tx, bytes_rx) = tokio::sync::oneshot::channel();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        accessor.with(|mut store| {
            if call.is_live() {
                store.spawn(StdinReadTask::<Ctx>::new(call, bytes_rx, result_tx));
                let stream =
                    StreamReader::new(&mut store, StdinInputStreamProducer::live(stdin, bytes_tx))?;
                let future = FutureReader::new(&mut store, wait_stdio_task_result(result_rx))?;
                Ok((stream, future))
            } else {
                let (stream_mode_tx, stream_mode_rx) = tokio::sync::oneshot::channel();
                store.spawn(StdinReadReplayTask::<Ctx>::new(
                    call,
                    stream_mode_tx,
                    bytes_rx,
                    result_tx,
                ));

                let stream = StreamReader::new(
                    &mut store,
                    StdinInputStreamProducer::replaying_or_live(stdin, bytes_tx, stream_mode_rx),
                )?;
                let future = FutureReader::new(&mut store, wait_stdio_task_result(result_rx))?;
                Ok((stream, future))
            }
        })
    }
}

impl<Ctx: WorkerCtx> stdout::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> stdout::HostWithStore for DurableP3<Ctx> {
    async fn write_via_stream<U: Send + 'static>(
        accessor: &Accessor<U, Self>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>> {
        write_standard_stream_via_stream::<Ctx, U, P3CliStdoutWriteViaStream>(
            accessor,
            data,
            StandardStream::Stdout,
        )
        .await
    }
}

impl<Ctx: WorkerCtx> stderr::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> stderr::HostWithStore for DurableP3<Ctx> {
    async fn write_via_stream<U: Send + 'static>(
        accessor: &Accessor<U, Self>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>> {
        write_standard_stream_via_stream::<Ctx, U, P3CliStderrWriteViaStream>(
            accessor,
            data,
            StandardStream::Stderr,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::types::SerializableP3CliErrorCode;
    use golem_common::model::oplog::{HostRequest, HostResponse, host_functions};
    use test_r::test;

    #[test]
    fn p3_cli_host_payload_pairs_roundtrip() {
        assert_host_payload_pair_roundtrip::<P3CliStdinReadViaStream>(
            HostRequestNoInput {},
            HostResponseP3CliStream {
                contents: b"stdin prefix".to_vec(),
                result: Err(SerializableP3CliErrorCode::Io),
            },
        );
        assert_host_payload_pair_roundtrip::<P3CliStdoutWriteViaStream>(
            HostRequestNoInput {},
            HostResponseP3CliStream {
                contents: b"stdout bytes".to_vec(),
                result: Ok(()),
            },
        );
        assert_host_payload_pair_roundtrip::<P3CliStderrWriteViaStream>(
            HostRequestNoInput {},
            HostResponseP3CliStream {
                contents: b"stderr prefix".to_vec(),
                result: Err(SerializableP3CliErrorCode::Pipe),
            },
        );
    }

    #[test]
    fn p3_cli_error_payload_mapping_roundtrips_stdio_codes() {
        assert_p3_cli_error_roundtrip(ErrorCode::Io);
        assert_p3_cli_error_roundtrip(ErrorCode::IllegalByteSequence);
        assert_p3_cli_error_roundtrip(ErrorCode::Pipe);
    }

    fn assert_host_payload_pair_roundtrip<Pair>(request: Pair::Req, response: Pair::Resp)
    where
        Pair: HostPayloadPair,
        Pair::Req: Clone + std::fmt::Debug + PartialEq + TryFrom<HostRequest, Error = String>,
        Pair::Resp: Clone + std::fmt::Debug + PartialEq,
    {
        let request_payload: HostRequest = request.clone().into();
        let request_bytes = desert_rust::serialize_to_byte_vec(&request_payload).unwrap();
        let request_roundtrip: HostRequest = desert_rust::deserialize(&request_bytes).unwrap();
        assert_eq!(Pair::Req::try_from(request_roundtrip).unwrap(), request);

        let response_payload: HostResponse = response.clone().into();
        let response_bytes = desert_rust::serialize_to_byte_vec(&response_payload).unwrap();
        let response_roundtrip: HostResponse = desert_rust::deserialize(&response_bytes).unwrap();
        assert_eq!(Pair::Resp::try_from(response_roundtrip).unwrap(), response);

        let function_name_bytes =
            desert_rust::serialize_to_byte_vec(&Pair::HOST_FUNCTION_NAME).unwrap();
        let function_name_roundtrip: host_functions::HostFunctionName =
            desert_rust::deserialize(&function_name_bytes).unwrap();
        assert_eq!(function_name_roundtrip, Pair::HOST_FUNCTION_NAME);
    }

    fn assert_p3_cli_error_roundtrip(error: ErrorCode) {
        let serialized = SerializableP3CliErrorCode::from(error);
        let replayed = ErrorCode::from(serialized);
        assert_eq!(format!("{replayed:?}"), format!("{error:?}"));
    }
}

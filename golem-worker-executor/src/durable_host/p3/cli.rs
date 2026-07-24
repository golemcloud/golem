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

use crate::durable_host::logging::policy as logging_policy;
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, observe_function_call,
    observe_function_call_store,
};
use crate::durable_host::tail_work::TailActivity;
use crate::model::event::InternalWorkerEvent;
use crate::workerctx::WorkerCtx;
use bytes::BytesMut;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{mpsc, oneshot};
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

/// Maximum number of bytes captured from a guest stdout/stderr stream per emitted log event.
///
/// This bounds host memory: per open stdio stream at most one accumulator of this size plus one
/// unacknowledged chunk of this size is buffered, regardless of how much the guest writes.
///
/// Chunk boundaries must stay deterministic because replay deduplication (`seen_log`) matches
/// log entries by message hash: bytes are accumulated across writes and a chunk is emitted
/// exactly when the accumulator is full (minus at most 3 bytes held back so a multi-byte UTF-8
/// scalar is never split, see [`utf8_safe_split_point`]), with the remainder flushed when the
/// stream closes. Boundaries are therefore a pure function of the cumulative byte stream and
/// this constant — independent of the guest's write sizes and of host-side producer buffer
/// segmentation. Changing this value only weakens deduplication across recoveries spanning the
/// change (extra duplicated log entries), it does not affect correctness of replay.
const STDIO_LOG_CHUNK_MAX_BYTES: usize = 8192;

/// The longest prefix of `buf` that does not end in the middle of what could be a valid
/// multi-byte UTF-8 scalar. At most 3 trailing bytes are held back; if the trailing bytes
/// cannot be a prefix of a valid scalar anyway, the full length is returned (splitting invalid
/// data is no worse than the lossy conversion it later goes through).
fn utf8_safe_split_point(buf: &[u8]) -> usize {
    let len = buf.len();
    // Find the lead byte of the last (potential) scalar by skipping at most 3 continuation
    // bytes from the end.
    let mut start = len;
    while start > 0 && len - start < 3 && buf[start - 1] & 0b1100_0000 == 0b1000_0000 {
        start -= 1;
    }
    if start == 0 {
        return len;
    }
    let lead = buf[start - 1];
    let scalar_len = if lead & 0b1000_0000 == 0 {
        1
    } else if lead & 0b1110_0000 == 0b1100_0000 {
        2
    } else if lead & 0b1111_0000 == 0b1110_0000 {
        3
    } else if lead & 0b1111_1000 == 0b1111_0000 {
        4
    } else {
        // Not a valid lead byte: the tail is not valid UTF-8, split at the cap.
        return len;
    };
    let tail_len = len - (start - 1);
    if tail_len < scalar_len {
        start - 1
    } else {
        len
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

/// One capped chunk of guest stdout/stderr bytes handed to the emitting [`StdioWriteTask`].
///
/// `ack_tx` is signalled once the chunk's log event has been emitted, at which point the
/// consumer marks the bytes as read and accepts more input. This limits host buffering to one
/// in-flight chunk per stream.
struct StdioChunk {
    contents: Vec<u8>,
    ack_tx: oneshot::Sender<()>,
}

/// Streams the bytes written via a single `write-via-stream` call to the emitting task in
/// bounded, acknowledged chunks (at most [`STDIO_LOG_CHUNK_MAX_BYTES`] each, at most one in
/// flight), instead of accumulating the whole stream in host memory.
///
/// Chunk boundaries — and therefore the emitted log messages — are a deterministic function of
/// the cumulative byte stream and the fixed cap: bytes are accumulated in `buffer` and a chunk
/// is emitted exactly when the accumulator is full (adjusted by [`utf8_safe_split_point`] so a
/// multi-byte UTF-8 scalar is never split), with the remainder flushed on drop, i.e. when the
/// stream closes. Boundaries do not depend on the guest's write sizes or on host-side producer
/// buffer segmentation (`write-via-stream` may be fed by a host-produced stream, whose source
/// buffer boundaries are timing-dependent). This keeps the message-hash based replay
/// deduplication (`seen_log`) correct across recovery re-runs.
///
/// Memory stays bounded because consuming more input requires the previously emitted chunk to
/// be acknowledged first: at most one full accumulator plus one in-flight chunk per stream.
struct CapturingOutputStreamConsumer {
    chunks_tx: Option<mpsc::UnboundedSender<StdioChunk>>,
    buffer: Vec<u8>,
    pending_ack: Option<oneshot::Receiver<()>>,
}

impl CapturingOutputStreamConsumer {
    fn new(chunks_tx: mpsc::UnboundedSender<StdioChunk>) -> Self {
        Self {
            chunks_tx: Some(chunks_tx),
            buffer: Vec::new(),
            pending_ack: None,
        }
    }
}

impl<D> StreamConsumer<D> for CapturingOutputStreamConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut src = src.as_direct(store);

        // Wait for the in-flight chunk to be emitted before consuming more input. The
        // receiver must be polled (not just stored) so its waker is registered; otherwise
        // the write task's acknowledgement could be missed, hanging the stream. No source
        // bytes are read before this resolves, so a pending poll leaves the source intact.
        if let Some(ack_rx) = &mut self.pending_ack {
            match Pin::new(ack_rx).poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(())) => {
                    self.pending_ack = None;
                }
                Poll::Ready(Err(_)) => {
                    self.pending_ack = None;
                    self.chunks_tx.take();
                    self.buffer.clear();
                    let remaining = src.remaining().len();
                    src.mark_read(remaining);
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }

        let bytes = src.remaining();
        if bytes.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        if self.chunks_tx.is_none() {
            let remaining = bytes.len();
            src.mark_read(remaining);
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        let take = bytes
            .len()
            .min(STDIO_LOG_CHUNK_MAX_BYTES - self.buffer.len());
        self.buffer.extend_from_slice(&bytes[..take]);
        src.mark_read(take);

        if self.buffer.len() == STDIO_LOG_CHUNK_MAX_BYTES {
            let split = utf8_safe_split_point(&self.buffer);
            let contents: Vec<u8> = self.buffer.drain(..split).collect();
            let (ack_tx, ack_rx) = oneshot::channel();
            let chunks_tx = self.chunks_tx.as_ref().expect("checked above");
            if chunks_tx.send(StdioChunk { contents, ack_tx }).is_err() {
                self.chunks_tx.take();
                self.buffer.clear();
                let remaining = src.remaining().len();
                src.mark_read(remaining);
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            self.pending_ack = Some(ack_rx);
            // The fresh receiver is polled (registering its waker) on the next
            // `poll_consume` call, before any further input is consumed.
        }

        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for CapturingOutputStreamConsumer {
    fn drop(&mut self) {
        // The stream is closing: flush the accumulated remainder as the final chunk. The
        // channel is unbounded, so this cannot block; ordering after a still-unacknowledged
        // in-flight chunk is preserved by the channel.
        if let Some(chunks_tx) = self.chunks_tx.take()
            && !self.buffer.is_empty()
        {
            let (ack_tx, _ack_rx) = oneshot::channel();
            let _ = chunks_tx.send(StdioChunk {
                contents: std::mem::take(&mut self.buffer),
                ack_tx,
            });
        }
    }
}

struct StdioWriteTask<Ctx> {
    stream: StandardStream,
    chunks_rx: mpsc::UnboundedReceiver<StdioChunk>,
    result_tx: oneshot::Sender<wasmtime::Result<Result<(), ErrorCode>>>,
    activity: TailActivity,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for StdioWriteTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let Self {
            stream,
            mut chunks_rx,
            result_tx,
            activity,
            _phantom,
        } = self;
        // Safe park: waiting for guest-produced stream bytes (or the stream's end).
        while let Some(StdioChunk { contents, ack_tx }) = activity.park(chunks_rx.recv()).await {
            let event = match stream {
                StandardStream::Stdout => InternalWorkerEvent::stdout(contents),
                StandardStream::Stderr => InternalWorkerEvent::stderr(contents),
            };
            emit_log_event_access::<Ctx, U>(accessor, event).await;
            let _ = ack_tx.send(());
        }
        let _ = result_tx.send(Ok(Ok(())));
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

    logging_policy::emit_log_event_with_state::<Ctx>(
        event,
        has_oplog_processor,
        &owned_agent_id,
        &public_state,
        &replay_state,
        &oplog,
        is_live,
    )
    .await;
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
    let (chunks_tx, chunks_rx) = mpsc::unbounded_channel();
    let (result_tx, result_rx) = oneshot::channel();
    accessor.with(|mut store| {
        data.pipe(&mut store, CapturingOutputStreamConsumer::new(chunks_tx))?;
        let activity = durable_worker_ctx::<Ctx, U>(store.data_mut())
            .tail_work_tracker()
            .activity();
        store.spawn(StdioWriteTask::<Ctx> {
            stream,
            chunks_rx,
            result_tx,
            activity,
            _phantom: PhantomData,
        });
        FutureReader::new(&mut store, wait_stdio_task_result(result_rx))
    })
}

impl<Ctx: WorkerCtx> environment::Host for DurableP3View<'_, Ctx> {
    fn get_environment(&mut self) -> wasmtime::Result<Vec<(String, String)>> {
        observe_function_call(&*self.0, "cli::environment", "get-environment");
        self.0.durable_ctx().build_enriched_environment()
    }

    fn get_arguments(&mut self) -> wasmtime::Result<Vec<String>> {
        observe_function_call(&*self.0, "cli::environment", "get-arguments");
        environment::Host::get_arguments(&mut WasiCliView::cli(self.0))
    }

    fn get_initial_cwd(&mut self) -> wasmtime::Result<Option<String>> {
        observe_function_call(&*self.0, "cli::environment", "get-initial-cwd");
        environment::Host::get_initial_cwd(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> exit::Host for DurableP3View<'_, Ctx> {
    fn exit(&mut self, status: Result<(), ()>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "cli::exit", "exit");
        exit::Host::exit(&mut WasiCliView::cli(self.0), status)
    }

    fn exit_with_code(&mut self, status_code: u8) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "cli::exit", "exit-with-code");
        exit::Host::exit_with_code(&mut WasiCliView::cli(self.0), status_code)
    }
}

impl<Ctx: WorkerCtx> terminal_input::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> terminal_input::HostTerminalInput for DurableP3View<'_, Ctx> {
    fn drop(&mut self, rep: Resource<TerminalInput>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "cli::terminal-input", "drop");
        terminal_input::HostTerminalInput::drop(&mut WasiCliView::cli(self.0), rep)
    }
}

impl<Ctx: WorkerCtx> terminal_output::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> terminal_output::HostTerminalOutput for DurableP3View<'_, Ctx> {
    fn drop(&mut self, rep: Resource<TerminalOutput>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "cli::terminal-output", "drop");
        terminal_output::HostTerminalOutput::drop(&mut WasiCliView::cli(self.0), rep)
    }
}

impl<Ctx: WorkerCtx> terminal_stdin::Host for DurableP3View<'_, Ctx> {
    fn get_terminal_stdin(&mut self) -> wasmtime::Result<Option<Resource<TerminalInput>>> {
        observe_function_call(&*self.0, "cli::terminal-stdin", "get-terminal-stdin");
        terminal_stdin::Host::get_terminal_stdin(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> terminal_stdout::Host for DurableP3View<'_, Ctx> {
    fn get_terminal_stdout(&mut self) -> wasmtime::Result<Option<Resource<TerminalOutput>>> {
        observe_function_call(&*self.0, "cli::terminal-stdout", "get-terminal-stdout");
        terminal_stdout::Host::get_terminal_stdout(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> terminal_stderr::Host for DurableP3View<'_, Ctx> {
    fn get_terminal_stderr(&mut self) -> wasmtime::Result<Option<Resource<TerminalOutput>>> {
        observe_function_call(&*self.0, "cli::terminal-stderr", "get-terminal-stderr");
        terminal_stderr::Host::get_terminal_stderr(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> stdin::Host for DurableP3View<'_, Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> stdin::HostWithStore<U> for DurableP3<Ctx> {
    async fn read_via_stream(
        accessor: &Accessor<U, Self>,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), ErrorCode>>)> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "cli::stdin",
                "read-via-stream",
            )
        });
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
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "cli::stdout",
                "write-via-stream",
            )
        });
        write_standard_stream_via_stream::<Ctx, U>(accessor, data, StandardStream::Stdout).await
    }
}

impl<Ctx: WorkerCtx> stderr::Host for DurableP3View<'_, Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> stderr::HostWithStore<U> for DurableP3<Ctx> {
    async fn write_via_stream(
        accessor: &Accessor<U, Self>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "cli::stderr",
                "write-via-stream",
            )
        });
        write_standard_stream_via_stream::<Ctx, U>(accessor, data, StandardStream::Stderr).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;
    use test_r::timeout;
    use wasmtime::component::StreamReader;
    use wasmtime::{Config, Engine, Store};

    /// Splitting never happens in the middle of a multi-byte UTF-8 scalar, at most 3 bytes
    /// are held back, and invalid trailing bytes are split at the cap.
    #[test]
    fn utf8_safe_split_point_respects_scalar_boundaries() {
        // Pure ASCII: no hold-back.
        assert_eq!(utf8_safe_split_point(b"abcd"), 4);
        // Complete multi-byte scalars at the end: no hold-back.
        assert_eq!(utf8_safe_split_point("abé".as_bytes()), 4);
        assert_eq!(utf8_safe_split_point("a€".as_bytes()), 4);
        assert_eq!(utf8_safe_split_point("😀".as_bytes()), 4);
        // Incomplete scalars at the end: the whole scalar prefix is held back.
        let euro = "€".as_bytes(); // 3 bytes
        assert_eq!(
            utf8_safe_split_point(&[b"ab" as &[u8], &euro[..1]].concat()),
            2
        );
        assert_eq!(
            utf8_safe_split_point(&[b"ab" as &[u8], &euro[..2]].concat()),
            2
        );
        let emoji = "😀".as_bytes(); // 4 bytes
        assert_eq!(
            utf8_safe_split_point(&[b"a" as &[u8], &emoji[..3]].concat()),
            1
        );
        // Invalid data (lone/too many continuation bytes): split at the cap.
        assert_eq!(utf8_safe_split_point(&[b'a', 0x80, 0x80, 0x80, 0x80]), 5);
        assert_eq!(utf8_safe_split_point(&[0x80, 0x80]), 2);
    }

    /// A stream larger than the per-chunk cap piped into
    /// [`CapturingOutputStreamConsumer`] must arrive as multiple capped chunks
    /// whose concatenation is the full stream, with the channel closing after
    /// the source is exhausted. This proves the capture path never buffers the
    /// whole stream: each chunk is at most [`STDIO_LOG_CHUNK_MAX_BYTES`] and the
    /// consumer waits for an acknowledgement before producing the next one.
    #[test]
    #[timeout("10s")]
    async fn capturing_output_stream_consumer_emits_capped_acknowledged_chunks() {
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());

        let input: Vec<u8> = (0..(STDIO_LOG_CHUNK_MAX_BYTES * 3 + 100))
            .map(|i| (i % 251) as u8)
            .collect();
        let expected = input.clone();

        let (chunks_tx, chunk_rx) = mpsc::unbounded_channel::<StdioChunk>();

        let chunks = store
            .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<Vec<u8>>> {
                let mut chunk_rx = chunk_rx;
                accessor.with(|mut store| {
                    let stream = StreamReader::new(&mut store, input)?;
                    stream.pipe(&mut store, CapturingOutputStreamConsumer::new(chunks_tx))
                })?;

                let mut chunks = Vec::new();
                while let Some(StdioChunk { contents, ack_tx }) = chunk_rx.recv().await {
                    // At most one chunk may be in flight before it is acknowledged
                    // (`Disconnected` is fine: the final remainder is flushed when the
                    // consumer is dropped, after which the channel is closed).
                    assert!(matches!(
                        chunk_rx.try_recv(),
                        Err(mpsc::error::TryRecvError::Empty)
                            | Err(mpsc::error::TryRecvError::Disconnected)
                    ));
                    chunks.push(contents);
                    // The ack receiver of the final flushed chunk is already gone.
                    let _ = ack_tx.send(());
                }
                Ok(chunks)
            })
            .await
            .unwrap()
            .unwrap();

        assert!(chunks.len() >= 4);
        for chunk in &chunks {
            assert!(!chunk.is_empty());
            assert!(chunk.len() <= STDIO_LOG_CHUNK_MAX_BYTES);
        }
        assert_eq!(chunks.concat(), expected);
    }

    /// A host-side producer that hands the same byte stream to the pipe in a configurable
    /// segmentation, emulating timing-dependent producer buffer boundaries of host-to-host
    /// streams (e.g. an HTTP body or socket stream written to stdout).
    struct SegmentedProducer {
        data: Vec<u8>,
        pos: usize,
        segments: Vec<usize>,
        seg_idx: usize,
    }

    impl<D> StreamProducer<D> for SegmentedProducer {
        type Item = u8;
        type Buffer = BytesMut;

        fn poll_produce<'a>(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            store: StoreContextMut<'a, D>,
            dst: Destination<'a, Self::Item, Self::Buffer>,
            _finish: bool,
        ) -> Poll<wasmtime::Result<StreamResult>> {
            if self.pos >= self.data.len() {
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            let segment = self.segments[self.seg_idx % self.segments.len()];
            self.seg_idx += 1;
            let end = (self.pos + segment).min(self.data.len());
            let mut dst = dst.as_direct(store, end - self.pos);
            let n = dst.remaining().len().min(end - self.pos);
            let pos = self.pos;
            dst.remaining()[..n].copy_from_slice(&self.data[pos..pos + n]);
            dst.mark_written(n);
            self.pos += n;
            Poll::Ready(Ok(StreamResult::Completed))
        }
    }

    async fn collect_chunks(input: Vec<u8>, segments: Vec<usize>) -> Vec<Vec<u8>> {
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());

        let (chunks_tx, chunk_rx) = mpsc::unbounded_channel::<StdioChunk>();

        store
            .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<Vec<u8>>> {
                let mut chunk_rx = chunk_rx;
                accessor.with(|mut store| {
                    let stream = StreamReader::new(
                        &mut store,
                        SegmentedProducer {
                            data: input,
                            pos: 0,
                            segments,
                            seg_idx: 0,
                        },
                    )?;
                    stream.pipe(&mut store, CapturingOutputStreamConsumer::new(chunks_tx))
                })?;

                let mut chunks = Vec::new();
                while let Some(StdioChunk { contents, ack_tx }) = chunk_rx.recv().await {
                    chunks.push(contents);
                    let _ = ack_tx.send(());
                }
                Ok(chunks)
            })
            .await
            .unwrap()
            .unwrap()
    }

    /// Chunk boundaries must be a pure function of the cumulative byte stream: the same
    /// bytes delivered through differently segmented host producers must yield identical
    /// chunk vectors (replay dedup hashes chunk contents), and multi-byte UTF-8 scalars
    /// must never be split across chunks even when they straddle the cap.
    #[test]
    #[timeout("10s")]
    async fn capturing_output_stream_consumer_chunks_are_segmentation_independent() {
        // Multi-byte scalars guaranteed to straddle the 8192-byte cap repeatedly. The input
        // itself ends on a scalar boundary (whole repetitions), so every chunk — including the
        // final flushed remainder — must be valid UTF-8.
        let pattern = "é€😀abc"; // 12 bytes per repetition
        let repetitions = (STDIO_LOG_CHUNK_MAX_BYTES * 5 / 2) / pattern.len() + 1;
        let input: Vec<u8> = pattern.repeat(repetitions).into_bytes();
        assert!(input.len() > 2 * STDIO_LOG_CHUNK_MAX_BYTES);

        let whole = collect_chunks(input.clone(), vec![input.len()]).await;
        let medium = collect_chunks(input.clone(), vec![4097]).await;
        let ragged = collect_chunks(input.clone(), vec![1, 7, 4096, 3]).await;

        assert_eq!(whole.concat(), input);
        assert_eq!(whole, medium);
        assert_eq!(whole, ragged);
        assert!(whole.len() >= 3);
        for chunk in &whole {
            assert!(chunk.len() <= STDIO_LOG_CHUNK_MAX_BYTES);
            // No multi-byte scalar was split at a chunk boundary.
            assert!(std::str::from_utf8(chunk).is_ok());
        }
    }

    /// When the emitting task goes away (the chunk receiver is dropped), the
    /// consumer must tear the stream down instead of hanging or accumulating
    /// bytes.
    #[test]
    #[timeout("10s")]
    async fn capturing_output_stream_consumer_tears_down_when_receiver_is_dropped() {
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());

        let input: Vec<u8> = vec![42u8; STDIO_LOG_CHUNK_MAX_BYTES * 2];

        let (chunks_tx, chunk_rx) = mpsc::unbounded_channel::<StdioChunk>();

        store
            .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
                let mut chunk_rx = chunk_rx;
                accessor.with(|mut store| {
                    let stream = StreamReader::new(&mut store, input)?;
                    stream.pipe(&mut store, CapturingOutputStreamConsumer::new(chunks_tx))
                })?;

                // Receive the first chunk, then drop the receiver without
                // acknowledging: the consumer must observe the dropped ack and
                // stop, letting the pipe finish.
                let first = chunk_rx.recv().await;
                assert!(first.is_some());
                drop(first);
                drop(chunk_rx);

                // Let the event loop drive the consumer to completion.
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();
    }
}

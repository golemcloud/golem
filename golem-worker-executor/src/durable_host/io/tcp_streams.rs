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

//! Durable P2 TCP stream I/O. Each read, skip, write, flush and write permit of a TCP stream is
//! one durable call. Reads record the bytes (skips the count), writes record only the result and
//! the count, never the bytes. A splice is the permit, the read and the write it is made of.
//!
//! Replay returns the recorded results and touches no stream: the connection of a replayed
//! stream is lost, and its stream is a stand-in from `sockets::tcp`. An incomplete call runs on
//! that stand-in again, which fails with `connection-reset` and sends nothing. A replayed failure
//! counts as the stand-in's first failure, so its next operation reports a closed stream.

use crate::durable_host::concurrent::{CallReplayOutcome, DurableCallSession, NotCancellable};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
use crate::durable_host::io::native_wait::NativeIoWait;
use crate::durable_host::io::streams::interruptible_tcp_input;
use crate::durable_host::sockets::tcp::{LostTcpInputStream, LostTcpOutputStream};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    SocketsTcpInputStreamBlockingRead, SocketsTcpInputStreamBlockingSkip,
    SocketsTcpInputStreamRead, SocketsTcpInputStreamSkip, SocketsTcpOutputStreamBlockingFlush,
    SocketsTcpOutputStreamBlockingWriteAndFlush, SocketsTcpOutputStreamBlockingWriteZeroesAndFlush,
    SocketsTcpOutputStreamCheckWrite, SocketsTcpOutputStreamFlush, SocketsTcpOutputStreamWrite,
    SocketsTcpOutputStreamWriteReady, SocketsTcpOutputStreamWriteZeroes,
};
use golem_common::model::oplog::types::SerializableTcpStreamError;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestNoInput, HostResponseSocketsTcpStreamCount,
    HostResponseSocketsTcpStreamFlush, HostResponseSocketsTcpStreamRead,
};
use golem_service_base::error::worker_executor::InterruptKind;
use wasmtime::component::{Resource, ResourceTableError};
use wasmtime_wasi::StreamError;
use wasmtime_wasi::p2::bindings::io::streams::{
    HostInputStream, HostOutputStream, InputStream, OutputStream,
};

/// The response of a P2 TCP stream call.
pub(super) trait TcpStreamResponse {
    type Value: Clone;
    fn new(result: Result<Self::Value, SerializableTcpStreamError>) -> Self;
    fn into_result(self) -> Result<Self::Value, SerializableTcpStreamError>;
}

macro_rules! tcp_stream_response {
    ($response:ty, $value:ty) => {
        impl TcpStreamResponse for $response {
            type Value = $value;
            fn new(result: Result<$value, SerializableTcpStreamError>) -> Self {
                Self { result }
            }
            fn into_result(self) -> Result<$value, SerializableTcpStreamError> {
                self.result
            }
        }
    };
}

tcp_stream_response!(HostResponseSocketsTcpStreamRead, Vec<u8>);
tcp_stream_response!(HostResponseSocketsTcpStreamCount, u64);
tcp_stream_response!(HostResponseSocketsTcpStreamFlush, ());

#[derive(Clone, Copy)]
enum TcpStream {
    Input(u32),
    Output(u32),
}

impl TcpStream {
    /// Whether the stream is the stand-in of a connection that a reconstruction lost.
    fn is_lost<Ctx: WorkerCtx>(self, ctx: &mut DurableWorkerCtx<Ctx>) -> wasmtime::Result<bool> {
        Ok(match self {
            Self::Input(rep) => ctx
                .table()
                .get(&Resource::<InputStream>::new_borrow(rep))?
                .as_any()
                .is::<LostTcpInputStream>(),
            Self::Output(rep) => ctx
                .table()
                .get(&Resource::<OutputStream>::new_borrow(rep))?
                .as_any()
                .is::<LostTcpOutputStream>(),
        })
    }

    /// Makes a lost stand-in fail its first operation, which a replayed failure stands for, so
    /// its next operation reports a closed stream, as the native stream did after that failure.
    fn report_replayed_failure<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> wasmtime::Result<()> {
        match self {
            Self::Input(rep) => {
                let _ = wasmtime_wasi::p2::InputStream::read(
                    &mut **ctx
                        .table()
                        .get_mut(&Resource::<InputStream>::new_borrow(rep))?,
                    0,
                );
            }
            Self::Output(rep) => {
                let _ = wasmtime_wasi::p2::OutputStream::check_write(
                    &mut **ctx
                        .table()
                        .get_mut(&Resource::<OutputStream>::new_borrow(rep))?,
                );
            }
        }
        Ok(())
    }
}

fn connection_lost() -> StreamError {
    StreamError::LastOperationFailed(wasmtime::Error::from(std::io::Error::from(
        std::io::ErrorKind::ConnectionReset,
    )))
}

/// Runs one TCP stream operation as a durable call. Live, `native` runs on the stream. A
/// completed call returns its recorded result. An incomplete call runs `native` again only on a
/// lost stand-in; a native stream reports the lost connection without any I/O. The guest gets the
/// recorded form of the result also live, so live and replay give it the same error.
async fn durable<Pair, Ctx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    stream: TcpStream,
    function_type: DurableFunctionType,
    native: impl AsyncFnOnce(
        &mut DurableWorkerCtx<Ctx>,
    ) -> Result<<Pair::Resp as TcpStreamResponse>::Value, StreamError>,
) -> Result<<Pair::Resp as TcpStreamResponse>::Value, StreamError>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<Req = HostRequestNoInput>,
    Pair::Resp: TcpStreamResponse,
{
    let call = DurableCallSession::<Pair, NotCancellable>::start(
        ctx,
        HostRequestNoInput {},
        function_type,
    )
    .await
    .map_err(StreamError::Trap)?;
    let (mut call, live) = if call.is_live() {
        (call, true)
    } else {
        match call.replay(ctx).await? {
            CallReplayOutcome::Replayed(recorded) => {
                let result = recorded.into_result();
                if result.is_err() && stream.is_lost(ctx).map_err(StreamError::Trap)? {
                    stream
                        .report_replayed_failure(ctx)
                        .map_err(StreamError::Trap)?;
                }
                return result.map_err(StreamError::from);
            }
            CallReplayOutcome::Incomplete(call) => (call, false),
        }
    };
    let lost = !live
        && stream
            .is_lost(ctx)
            .map_err(|error| StreamError::Trap(wasmtime::Error::from_anyhow(call.trap(error))))?;
    let result = if live || lost {
        native(ctx).await
    } else {
        Err(connection_lost())
    };
    let recorded = match result {
        Ok(value) => Ok(value),
        Err(error) => match SerializableTcpStreamError::from_stream_error(error) {
            Ok(recorded) => Err(recorded),
            Err(trap) if trap.downcast_ref::<InterruptKind>().is_some() => {
                // The Start stays incomplete; replay completes it with the lost connection.
                call.abandon_for_trap();
                return Err(StreamError::Trap(trap));
            }
            Err(trap) => {
                return Err(StreamError::Trap(wasmtime::Error::from_anyhow(
                    call.trap(trap),
                )));
            }
        },
    };
    call.complete(
        ctx,
        <Pair::Resp as TcpStreamResponse>::new(recorded.clone()),
    )
    .await?;
    recorded.map_err(StreamError::from)
}

fn input(rep: u32) -> Resource<InputStream> {
    Resource::new_borrow(rep)
}

fn output(rep: u32) -> Resource<OutputStream> {
    Resource::new_borrow(rep)
}

pub(super) async fn read<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    len: u64,
) -> Result<Vec<u8>, StreamError> {
    durable::<SocketsTcpInputStreamRead, _>(
        ctx,
        TcpStream::Input(rep),
        DurableFunctionType::ReadRemote,
        async |ctx| HostInputStream::read(ctx.table(), input(rep), len).await,
    )
    .await
}

pub(super) async fn blocking_read<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    len: u64,
) -> Result<Vec<u8>, StreamError> {
    durable::<SocketsTcpInputStreamBlockingRead, _>(
        ctx,
        TcpStream::Input(rep),
        DurableFunctionType::ReadRemote,
        async |ctx| {
            let wait = NativeIoWait::new(ctx.interrupt_sources());
            interruptible_tcp_input(
                HostInputStream::blocking_read(ctx.table(), input(rep), len),
                wait,
            )
            .await
        },
    )
    .await
}

pub(super) async fn skip<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    len: u64,
) -> Result<u64, StreamError> {
    durable::<SocketsTcpInputStreamSkip, _>(
        ctx,
        TcpStream::Input(rep),
        DurableFunctionType::ReadRemote,
        async |ctx| HostInputStream::skip(ctx.table(), input(rep), len).await,
    )
    .await
}

pub(super) async fn blocking_skip<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    len: u64,
) -> Result<u64, StreamError> {
    durable::<SocketsTcpInputStreamBlockingSkip, _>(
        ctx,
        TcpStream::Input(rep),
        DurableFunctionType::ReadRemote,
        async |ctx| {
            let wait = NativeIoWait::new(ctx.interrupt_sources());
            interruptible_tcp_input(
                HostInputStream::blocking_skip(ctx.table(), input(rep), len),
                wait,
            )
            .await
        },
    )
    .await
}

pub(super) async fn check_write<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> Result<u64, StreamError> {
    durable::<SocketsTcpOutputStreamCheckWrite, _>(
        ctx,
        TcpStream::Output(rep),
        DurableFunctionType::ReadLocal,
        async |ctx| HostOutputStream::check_write(ctx.table(), output(rep)).await,
    )
    .await
}

/// Waits until the stream accepts a write and returns the permit, as a blocking splice does.
async fn write_ready<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> Result<u64, StreamError> {
    durable::<SocketsTcpOutputStreamWriteReady, _>(
        ctx,
        TcpStream::Output(rep),
        DurableFunctionType::ReadLocal,
        async |ctx| {
            let wait = NativeIoWait::new(ctx.interrupt_sources());
            let stream = ctx.table().get_mut(&output(rep))?;
            interruptible_tcp_input(stream.write_ready(), wait)
                .await
                .map(|permit| permit as u64)
        },
    )
    .await
}

pub(super) async fn write<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    contents: Vec<u8>,
) -> Result<(), StreamError> {
    let len = contents.len() as u64;
    durable::<SocketsTcpOutputStreamWrite, _>(
        ctx,
        TcpStream::Output(rep),
        DurableFunctionType::WriteRemote,
        async |ctx| {
            HostOutputStream::write(ctx.table(), output(rep), contents)
                .await
                .map(|()| len)
                .map_err(write_beyond_permit_is_permanent)
        },
    )
    .await
    .map(|_| ())
}

/// The native stream traps when the guest writes more than `check-write` permitted. That is a
/// deterministic bug of the guest, so the trap fails the invocation permanently: a retry would
/// replay the connect into a lost stream and turn the bug into a lost connection.
fn write_beyond_permit_is_permanent(error: StreamError) -> StreamError {
    match error {
        StreamError::Trap(trap) if trap.downcast_ref::<ResourceTableError>().is_none() => {
            StreamError::Trap(wasmtime::Error::from_anyhow(anyhow::Error::new(
                ClassifiedHostError {
                    kind: HostFailureKind::Permanent,
                    message: trap.to_string(),
                },
            )))
        }
        other => other,
    }
}

const MAX_BLOCKING_WRITE: usize = 4096;

/// Writes all of `contents` within the write permits of the stream, then flushes it.
pub(super) async fn blocking_write_and_flush<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    contents: Vec<u8>,
) -> Result<(), StreamError> {
    if contents.len() > MAX_BLOCKING_WRITE {
        return Err(StreamError::trap(
            "Buffer too large for blocking-write-and-flush (expected at most 4096)",
        ));
    }
    write_and_flush(ctx, rep, contents).await
}

/// Writes all of `contents` within the write permits of the stream, flushes it and waits until
/// the flush is done, as one `blocking_write_and_flush` call of any size.
async fn write_and_flush<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    contents: Vec<u8>,
) -> Result<(), StreamError> {
    let len = contents.len() as u64;
    durable::<SocketsTcpOutputStreamBlockingWriteAndFlush, _>(
        ctx,
        TcpStream::Output(rep),
        DurableFunctionType::WriteRemote,
        async |ctx| {
            let wait = NativeIoWait::new(ctx.interrupt_sources());
            let stream = ctx.table().get_mut(&output(rep))?;
            interruptible_tcp_input(stream.blocking_write_and_flush(contents.into()), wait)
                .await
                .map(|()| len)
        },
    )
    .await
    .map(|_| ())
}

pub(super) async fn write_zeroes<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    len: u64,
) -> Result<(), StreamError> {
    durable::<SocketsTcpOutputStreamWriteZeroes, _>(
        ctx,
        TcpStream::Output(rep),
        DurableFunctionType::WriteRemote,
        async |ctx| {
            HostOutputStream::write_zeroes(ctx.table(), output(rep), len)
                .await
                .map(|()| len)
                .map_err(write_beyond_permit_is_permanent)
        },
    )
    .await
    .map(|_| ())
}

pub(super) async fn blocking_write_zeroes_and_flush<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    len: u64,
) -> Result<(), StreamError> {
    if len > MAX_BLOCKING_WRITE as u64 {
        return Err(StreamError::trap(
            "Buffer too large for blocking-write-zeroes-and-flush (expected at most 4096)",
        ));
    }
    durable::<SocketsTcpOutputStreamBlockingWriteZeroesAndFlush, _>(
        ctx,
        TcpStream::Output(rep),
        DurableFunctionType::WriteRemote,
        async |ctx| {
            let wait = NativeIoWait::new(ctx.interrupt_sources());
            let stream = ctx.table().get_mut(&output(rep))?;
            let zeroes = bytes::Bytes::from(vec![0; len as usize]);
            interruptible_tcp_input(stream.blocking_write_and_flush(zeroes), wait)
                .await
                .map(|()| len)
        },
    )
    .await
    .map(|_| ())
}

pub(super) async fn flush<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> Result<(), StreamError> {
    durable::<SocketsTcpOutputStreamFlush, _>(
        ctx,
        TcpStream::Output(rep),
        DurableFunctionType::WriteRemote,
        async |ctx| HostOutputStream::flush(ctx.table(), output(rep)).await,
    )
    .await
}

pub(super) async fn blocking_flush<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> Result<(), StreamError> {
    durable::<SocketsTcpOutputStreamBlockingFlush, _>(
        ctx,
        TcpStream::Output(rep),
        DurableFunctionType::WriteRemote,
        async |ctx| {
            let wait = NativeIoWait::new(ctx.interrupt_sources());
            interruptible_tcp_input(
                HostOutputStream::blocking_flush(ctx.table(), output(rep)),
                wait,
            )
            .await
        },
    )
    .await
}

/// A splice with a TCP stream on at least one side, made of the calls of its parts: the write
/// permit of the destination, the read of at most that many bytes from the source, and the write
/// of the bytes read. A blocking splice writes with `blocking-write-and-flush`, as Wasmtime's
/// does, so the bytes are sent when it returns. The TCP parts are durable calls; the other side
/// runs through the durable host as a direct call on that stream would.
pub(super) async fn splice<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    dest: u32,
    src: u32,
    len: u64,
    blocking: bool,
) -> Result<u64, StreamError> {
    let tcp_dest = ctx.state.open_tcp_output_streams.contains(&dest);
    let tcp_src = ctx.state.open_tcp_input_streams.contains(&src);
    let permit = match (tcp_dest, blocking) {
        (true, true) => write_ready(ctx, dest).await?,
        (true, false) => check_write(ctx, dest).await?,
        (false, true) => {
            if ctx.state.open_filesystem_output_streams.contains_key(&dest) {
                let interrupt = ctx.create_interrupt_signal();
                super::streams::observe_file_stream(
                    interrupt,
                    ctx.table().get_mut(&output(dest))?.write_ready(),
                )
                .await? as u64
            } else {
                ctx.table().get_mut(&output(dest))?.write_ready().await? as u64
            }
        }
        (false, false) => HostOutputStream::check_write(ctx, output(dest)).await?,
    };
    let len = len.min(permit);
    if len == 0 {
        return Ok(0);
    }
    let contents = match (tcp_src, blocking) {
        (true, true) => blocking_read(ctx, src, len).await?,
        (true, false) => read(ctx, src, len).await?,
        (false, true) => HostInputStream::blocking_read(ctx, input(src), len).await?,
        (false, false) => HostInputStream::read(ctx, input(src), len).await?,
    };
    if contents.is_empty() {
        return Ok(0);
    }
    let written = contents.len() as u64;
    match (tcp_dest, blocking) {
        (true, true) => write_and_flush(ctx, dest, contents).await?,
        (true, false) => write(ctx, dest, contents).await?,
        (false, true) => {
            HostOutputStream::blocking_write_and_flush(ctx, output(dest), contents).await?
        }
        (false, false) => HostOutputStream::write(ctx, output(dest), contents).await?,
    }
    Ok(written)
}

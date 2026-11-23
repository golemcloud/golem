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
use std::sync::OnceLock;
use std::task::{Context, Poll};

use crate::durable_host::attribution::with_import_access;
use crate::durable_host::concurrent::{
    AccessClaimOptions, Cancellable, CompletionDelivery, DeferredCallReplayOutcome, DemandDelivery,
    DurableCallSession, DurableDemandItem, DurableDemandStream, InvocationIdSource, NotCancellable,
    ScopeReplayRecovery, access_invocation_id, closed_demand, deliver_demand, demand_channel,
    reserve_invocation_id_before_await, resolve_current_observational_owner,
};
use crate::durable_host::durability::{
    DurabilityHost, DurableCallTrapContext, GuestTaskContext, find_durable_call_trap_context,
    mark_durable_call_trap_context,
};
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, observe_function_call,
    observe_function_call_store, run_deniable_access, wasi_sockets_view,
};
use crate::durable_host::sockets::{SocketEntry, SocketPhase, TcpSocketOptions};
use crate::durable_host::tail_work::TailActivity;
use crate::durable_host::{DurableWorkerCtx, TcpSocketStreamDirection};
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use bytes::Bytes;
use golem_common::model::oplog::host_functions::{
    P3SocketsTypesTcpSocketBind, P3SocketsTypesTcpSocketConnect, P3SocketsTypesTcpSocketReceive,
    P3SocketsTypesTcpSocketReceiveAcquire, P3SocketsTypesTcpSocketReceiveChunk,
    P3SocketsTypesTcpSocketSend, P3SocketsTypesTcpSocketSendAcquire,
    P3SocketsTypesTcpSocketSendChunk,
};
use golem_common::model::oplog::types::{SerializableP3SocketErrorCode, SerializableP3TcpChunk};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestNoInput, HostRequestP3SocketsBind,
    HostRequestP3SocketsConnect, HostResponseP3SocketsBind, HostResponseP3SocketsTcpAcquire,
    HostResponseP3SocketsTcpConnect, HostResponseP3SocketsTcpReceive,
    HostResponseP3SocketsTcpReceiveChunk, HostResponseP3SocketsTcpSend,
    HostResponseP3SocketsTcpSendChunk, OplogIndex,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;
use wasmtime::AsContextMut;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Access, Accessor, Destination, FutureConsumer, FutureReader, Resource, Source, StreamConsumer,
    StreamProducer, StreamReader, StreamResult,
};
use wasmtime_wasi::p3::bindings::sockets::types;
use wasmtime_wasi::p3::sockets::{SocketError, SocketResult};
use wasmtime_wasi::sockets::{TcpSendStream, TcpSocket, WasiSockets, WasiSocketsView};

use super::serialize_socket_error;

fn tcp_entry<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> wasmtime::Result<&mut SocketEntry> {
    if !ctx.state.socket_entries.contains_key(&rep) {
        // Native accept creates resources outside `create`.
        let entry = {
            let mut wasi = ctx.as_wasi_view();
            let mut view = wasi.sockets();
            let family =
                types::HostTcpSocket::get_address_family(&mut view, Resource::new_borrow(rep))?;
            let mut entry = SocketEntry::new(family);
            entry.local_address =
                types::HostTcpSocket::get_local_address(&mut view, Resource::new_borrow(rep))
                    .ok()
                    .map(Into::into);
            entry.remote_address =
                types::HostTcpSocket::get_remote_address(&mut view, Resource::new_borrow(rep))
                    .ok()
                    .map(Into::into);
            entry.phase = if entry.remote_address.is_some() {
                SocketPhase::Connected
            } else if entry.local_address.is_some() {
                SocketPhase::Bound
            } else {
                SocketPhase::Created
            };
            entry
        };
        ctx.state.socket_entries.insert(rep, entry);
    }
    Ok(ctx
        .state
        .socket_entries
        .get_mut(&rep)
        .expect("TCP socket entry"))
}

fn tcp_options<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> SocketResult<&mut TcpSocketOptions> {
    let entry = tcp_entry(ctx, rep).map_err(SocketError::trap)?;
    match &entry.phase {
        SocketPhase::Closed(error) => Err(types::ErrorCode::from(error.clone()).into()),
        SocketPhase::Connecting => Err(types::ErrorCode::InvalidState.into()),
        _ => Ok(&mut entry.options),
    }
}

/// The permission target of a connect or datagram from the Store to `address`.
pub(super) fn socket_target<Ctx: WorkerCtx, U: 'static>(
    store: &Accessor<U, DurableP3<Ctx>>,
    address: std::net::SocketAddr,
) -> Result<golem_common::model::card::PermissionTarget, String> {
    store.with(|mut access| durable_worker_ctx::<Ctx, U>(access.data_mut()).socket_target(address))
}

fn serialize_tcp_stream_result(
    result: Result<(), types::ErrorCode>,
) -> Result<(), SerializableP3SocketErrorCode> {
    result.map_err(Into::into)
}

fn deserialize_tcp_stream_result(
    result: Result<(), SerializableP3SocketErrorCode>,
) -> Result<(), types::ErrorCode> {
    result.map_err(Into::into)
}

/// Bridges the upstream socket's transmission-result `FutureReader` into a
/// `oneshot` the durable task awaits. Shared by `send` (live) and `receive`
/// (live).
struct TcpSocketResultConsumer {
    result_tx: Option<oneshot::Sender<Result<(), types::ErrorCode>>>,
}

impl TcpSocketResultConsumer {
    fn new(result_tx: oneshot::Sender<Result<(), types::ErrorCode>>) -> Self {
        Self {
            result_tx: Some(result_tx),
        }
    }

    fn send(&mut self, result: Result<(), types::ErrorCode>) {
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(result);
        }
    }
}

impl<D> FutureConsumer<D> for TcpSocketResultConsumer {
    type Item = Result<(), types::ErrorCode>;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<D>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<()>> {
        if finish {
            self.send(Err(types::ErrorCode::ConnectionBroken));
            return Poll::Ready(Ok(()));
        }

        let mut result = None;
        source.read(store.as_context_mut(), &mut result)?;
        if let Some(result) = result {
            self.send(result);
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}

/// The durable receive task's reply to a single guest stream demand.
enum TcpReceiveReply {
    /// One non-empty chunk, already persisted as a `receive-chunk` child before
    /// being handed back for delivery to the guest.
    Data(Bytes),
    /// The receive stream reached its terminal (clean close or socket error);
    /// there are no more bytes. The producer acks before reporting EOF so the
    /// task only finalizes the parent once the terminal is observed by the guest.
    End { ack: oneshot::Sender<()> },
    /// A durable failure occurred; the guest stream traps with this message,
    /// tagged with the failing call scope's trap context.
    Failed {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// A demand from the receive stream producer to the durable task for the next
/// chunk, carrying the channel the task replies on.
type TcpReceiveDemand = oneshot::Sender<TcpReceiveReply>;

/// Resolution delivered to the guest-facing send- or receive-result future once
/// the stream closes (or the durable task fails before recording the terminal).
enum TcpStreamResolution {
    /// The stream terminal: a clean close or a socket `ErrorCode`.
    Outcome(Result<(), types::ErrorCode>),
    /// A durability failure: the result future traps with this message, tagged
    /// with the failing call scope's trap context.
    Trap {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// Resolves the guest-facing send- or receive-result future. A durability failure traps
/// (carrying the failing call scope's trap context) rather than resolving to a
/// normal error that would mask it.
async fn wait_tcp_stream_result(
    result_rx: oneshot::Receiver<TcpStreamResolution>,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    match result_rx.await {
        Ok(TcpStreamResolution::Outcome(result)) => Ok(result),
        Ok(TcpStreamResolution::Trap {
            message,
            trap_context,
        }) => Err(wasmtime::Error::from_anyhow(
            mark_durable_call_trap_context(anyhow::Error::msg(message), trap_context),
        )),
        Err(_) => Err(wasmtime::Error::msg(
            "tcp socket durable task dropped before resolving result",
        )),
    }
}

/// Receive byte stream returned to the guest.
///
/// `receive` is a *synchronous* host function but durable persistence is async,
/// so the producer never touches the oplog (or the socket) itself. Instead it
/// bridges to the spawned [`TcpSocketReceiveTask`] with a demand/reply protocol
/// identical to the P3 HTTP `consume-body` body stream: when the guest needs
/// more bytes the producer sends a demand and parks; the task reads (live) or
/// replays (on replay) exactly one chunk, persists/claims it as a child durable
/// call, and replies with the bytes. The whole chunk is handed to the runtime's
/// buffer (`Destination::set_buffer`), which delivers it across however many
/// guest reads and only calls `poll_produce` again once it is fully drained — so
/// exactly one child chunk is produced per real demand, identically live and on
/// replay.
struct DurableTcpReceiveProducer {
    demand_tx: mpsc::Sender<TcpReceiveDemand>,
    pending: Option<oneshot::Receiver<TcpReceiveReply>>,
    finished: bool,
}

impl DurableTcpReceiveProducer {
    fn new(demand_tx: mpsc::Sender<TcpReceiveDemand>) -> Self {
        Self {
            demand_tx,
            pending: None,
            finished: false,
        }
    }
}

impl<D> StreamProducer<D> for DurableTcpReceiveProducer {
    type Item = u8;
    type Buffer = Bytes;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        loop {
            if self.finished {
                return Poll::Ready(Ok(StreamResult::Dropped));
            }

            if let Some(rx) = self.pending.as_mut() {
                match Pin::new(rx).poll(cx) {
                    Poll::Pending => {
                        // A demand is in flight: the task has been asked for (and
                        // will durably persist) exactly one chunk. We must deliver
                        // it to a guest read rather than abandon it, otherwise the
                        // recorded child chunk would have no matching delivery and
                        // replay would diverge. So even when the guest is trying to
                        // cancel (`finish`), wait for the in-flight chunk instead of
                        // returning `Cancelled`.
                        return Poll::Pending;
                    }
                    Poll::Ready(Ok(TcpReceiveReply::Data(bytes))) => {
                        self.pending = None;
                        if bytes.is_empty() {
                            continue;
                        }
                        dst.set_buffer(bytes);
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }
                    Poll::Ready(Ok(TcpReceiveReply::End { ack })) => {
                        self.pending = None;
                        self.finished = true;
                        // Acknowledge the terminal *before* reporting EOF so the
                        // task only resolves the result after this stream observes
                        // the terminal. A dropped `ack` receiver just means the task
                        // is already gone, which is harmless here.
                        let _ = ack.send(());
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }
                    Poll::Ready(Ok(TcpReceiveReply::Failed {
                        message,
                        trap_context,
                    })) => {
                        self.pending = None;
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::from_anyhow(
                            mark_durable_call_trap_context(
                                anyhow::Error::msg(message),
                                trap_context,
                            ),
                        )));
                    }
                    Poll::Ready(Err(_)) => {
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::msg(
                            "tcp receive durable task dropped before replying",
                        )));
                    }
                }
            }

            // No demand in flight.
            if dst.remaining(&mut store) == Some(0) {
                // Zero-length read: the guest is probing readiness, not reading.
                // Do not turn this into a durable socket read.
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            if finish {
                // The guest is cancelling a read and we have nothing buffered and
                // no demand in flight: report a cancelled (empty) read without
                // starting a new durable socket read.
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }

            let (tx, rx) = oneshot::channel();
            match self.demand_tx.try_send(tx) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.finished = true;
                    return Poll::Ready(Err(wasmtime::Error::msg(
                        "tcp receive durable task is gone",
                    )));
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // The producer keeps at most one demand in flight, so the
                    // capacity-1 channel can never be full here.
                    self.finished = true;
                    return Poll::Ready(Err(wasmtime::Error::msg(
                        "tcp receive demand channel unexpectedly full",
                    )));
                }
            }
            self.pending = Some(rx);
            // Loop to register the receiver's waker (the reply is not ready yet).
        }
    }
}

/// One unit read from the upstream socket by the durable receive task.
enum TcpReceiveFrame {
    /// A non-empty chunk.
    Data(Bytes),
    /// The receive stream closed, carrying its terminal result.
    End(Result<(), types::ErrorCode>),
}

/// One item produced per receive-loop iteration — after the chunk has been
/// persisted (live) or replayed (replay) — to be delivered to the guest stream.
enum ProducedTcpChunk {
    /// A non-empty chunk to hand to the guest.
    Data(Bytes),
    /// The recorded stream's terminal: there are no more chunks to deliver.
    Terminal,
}

/// Bridges the upstream socket receive stream into a bounded channel the durable
/// task drains on demand.
///
/// The consumer is *demand-gated*: it forwards nothing until the durable task
/// grants a permit (one per real durable demand) over `permit_rx`. Without this
/// gate the consumer would forward a chunk into the bounded channel as soon as
/// capacity existed — i.e. one chunk would be read from the socket and buffered
/// in Golem before any durable demand. Gating keeps the Golem-side channel empty
/// until a demand exists; the only remaining read-ahead is wasmtime's single
/// internal host buffer (one `poll_produce` worth), which cannot be avoided
/// without modifying the runtime. Empty reads are skipped (without consuming the
/// permit) so empty chunks are never persisted or delivered.
pub(super) struct TcpReceiveForwardConsumer {
    chunk_tx: PollSender<Vec<u8>>,
    permit_rx: mpsc::Receiver<()>,
    has_permit: bool,
}

impl TcpReceiveForwardConsumer {
    pub(super) fn new(chunk_tx: PollSender<Vec<u8>>, permit_rx: mpsc::Receiver<()>) -> Self {
        Self {
            chunk_tx,
            permit_rx,
            has_permit: false,
        }
    }
}

impl<D> StreamConsumer<D> for TcpReceiveForwardConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if !self.has_permit {
            match self.permit_rx.poll_recv(cx) {
                Poll::Ready(Some(())) => self.has_permit = true,
                // The durable task is gone and will accept no more chunks: tear
                // the upstream stream down (permanent, not a retryable cancel).
                Poll::Ready(None) => return Poll::Ready(Ok(StreamResult::Dropped)),
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
            }
        }

        let mut src = src.as_direct(store);
        let bytes = src.remaining();
        if bytes.is_empty() {
            // Keep the permit: the task is still waiting for one real chunk or the
            // terminal, so the permit must survive until a non-empty read or EOF.
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        match self.chunk_tx.poll_reserve(cx) {
            Poll::Ready(Ok(())) => {
                let chunk = bytes.to_vec();
                let len = chunk.len();
                match self.chunk_tx.send_item(chunk) {
                    Ok(()) => {
                        src.mark_read(len);
                        self.has_permit = false;
                        Poll::Ready(Ok(StreamResult::Completed))
                    }
                    Err(_) => Poll::Ready(Ok(StreamResult::Dropped)),
                }
            }
            Poll::Ready(Err(_)) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// The largest guest write a single `send-chunk` child carries. A longer write is accepted in
/// several chunks, each its own child.
const TCP_SEND_CHUNK_LIMIT: usize = 64 * 1024;

/// The durable send task's reply to one guest write.
enum TcpSendReply {
    /// The native sink accepted this many bytes of the write, recorded as a `send-chunk` child.
    Accepted(u64),
    /// The send failed or the connection is lost: the write is not accepted and the guest
    /// stream is closed.
    Rejected,
    /// A durable failure occurred; the guest stream traps with this message, tagged with the
    /// failing call scope's trap context.
    Failed {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// One guest write handed from the guest-facing consumer to the durable send task.
struct TcpSendDemand {
    bytes: Bytes,
    reply: oneshot::Sender<TcpSendReply>,
    /// Resolves when the guest cancels the write or drops its writer. A live write that has
    /// not taken a byte by then stops and is recorded as accepting none.
    cancel: oneshot::Receiver<()>,
}

/// A guest write whose `send-chunk` child the durable task is recording.
struct PendingTcpSendWrite {
    reply: oneshot::Receiver<TcpSendReply>,
    /// Taken when the guest cancels the write.
    cancel: Option<oneshot::Sender<()>>,
}

/// Consumer of the guest's outgoing byte stream.
///
/// The host owns the reader of the guest stream. Each non-empty guest write becomes one demand
/// to the spawned [`TcpSocketSendTask`], which records it as one `send-chunk` child and, only
/// when the send started live in this Store, writes it to the native socket. The write stays
/// pending until the task replies, so the guest observes exactly the recorded byte count, live
/// and on replay. A guest cancel asks the task to stop the native write; the write still
/// completes only with its recorded count, and reports `Cancelled` when that count is zero.
/// Dropping the consumer (the guest closing its writer) ends the demand stream.
struct DurableTcpSendConsumer {
    demand_tx: mpsc::Sender<TcpSendDemand>,
    pending: Option<PendingTcpSendWrite>,
    finished: bool,
}

impl DurableTcpSendConsumer {
    fn new(demand_tx: mpsc::Sender<TcpSendDemand>) -> Self {
        Self {
            demand_tx,
            pending: None,
            finished: false,
        }
    }
}

impl<D> StreamConsumer<D> for DurableTcpSendConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.finished {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        let mut src = src.as_direct(store);
        loop {
            if let Some(pending) = self.pending.as_mut() {
                // A write in flight is being recorded as a child. A cancel stops its native
                // write, but the guest still waits for the recorded count.
                if finish && let Some(cancel) = pending.cancel.take() {
                    let _ = cancel.send(());
                }
                let reply = match Pin::new(&mut pending.reply).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(reply) => reply,
                };
                let cancelled = pending.cancel.is_none();
                self.pending = None;
                return match reply {
                    Ok(TcpSendReply::Accepted(0)) if cancelled => {
                        Poll::Ready(Ok(StreamResult::Cancelled))
                    }
                    Ok(TcpSendReply::Accepted(0)) => {
                        self.finished = true;
                        Poll::Ready(Err(wasmtime::Error::msg(
                            "tcp send recorded no accepted bytes for a write the guest did not cancel",
                        )))
                    }
                    Ok(TcpSendReply::Accepted(count)) => {
                        let count = count as usize;
                        if count > src.remaining().len() {
                            self.finished = true;
                            return Poll::Ready(Err(wasmtime::Error::msg(format!(
                                "tcp send recorded {count} accepted bytes for a write of {}",
                                src.remaining().len()
                            ))));
                        }
                        src.mark_read(count);
                        Poll::Ready(Ok(StreamResult::Completed))
                    }
                    Ok(TcpSendReply::Rejected) => {
                        self.finished = true;
                        Poll::Ready(Ok(StreamResult::Dropped))
                    }
                    Ok(TcpSendReply::Failed {
                        message,
                        trap_context,
                    }) => {
                        self.finished = true;
                        Poll::Ready(Err(wasmtime::Error::from_anyhow(
                            mark_durable_call_trap_context(
                                anyhow::Error::msg(message),
                                trap_context,
                            ),
                        )))
                    }
                    Err(_) => {
                        self.finished = true;
                        Poll::Ready(Err(wasmtime::Error::msg(
                            "tcp send durable task dropped before replying",
                        )))
                    }
                };
            }

            let bytes = src.remaining();
            if bytes.is_empty() {
                // A zero-length write probes readiness; it is not a durable send.
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            if finish {
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }
            let bytes = Bytes::copy_from_slice(&bytes[..bytes.len().min(TCP_SEND_CHUNK_LIMIT)]);
            let (reply, reply_rx) = oneshot::channel();
            let (cancel_tx, cancel) = oneshot::channel();
            if self
                .demand_tx
                .try_send(TcpSendDemand {
                    bytes,
                    reply,
                    cancel,
                })
                .is_err()
            {
                // The consumer keeps at most one demand in flight, so the channel is never full;
                // a closed channel means the task already finished the send.
                self.finished = true;
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            self.pending = Some(PendingTcpSendWrite {
                reply: reply_rx,
                cancel: Some(cancel_tx),
            });
        }
    }
}

impl<Ctx: WorkerCtx> types::HostTcpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<TcpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "bind");
        let rep = socket.rep();
        let mut bound_live = false;
        let response = DurableCallSession::<P3SocketsTypesTcpSocketBind, NotCancellable>::invoke(
            self.0.durable_ctx_mut(),
            HostRequestP3SocketsBind {
                local_address: local_address.into(),
            },
            DurableFunctionType::WriteLocal,
            async |ctx| -> wasmtime::Result<_> {
                let result = if tcp_entry(ctx, rep)?.phase != SocketPhase::Created {
                    Err(types::ErrorCode::InvalidState.into())
                } else {
                    let mut wasi = ctx.as_wasi_view();
                    let mut view = wasi.sockets();
                    match types::HostTcpSocket::bind(&mut view, socket, local_address).await {
                        Ok(()) => {
                            bound_live = true;
                            types::HostTcpSocket::get_local_address(
                                &mut view,
                                Resource::new_borrow(rep),
                            )
                        }
                        Err(error) => Err(error),
                    }
                };
                Ok(HostResponseP3SocketsBind {
                    result: match result {
                        Ok(address) => Ok(address.into()),
                        Err(error) => Err(serialize_socket_error(error)?),
                    },
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        match response.result {
            Ok(address) => {
                let entry = self
                    .0
                    .durable_ctx_mut()
                    .state
                    .socket_entries
                    .get_mut(&rep)
                    .expect("TCP socket entry");
                entry.phase = SocketPhase::Bound;
                entry.local_address = Some(address);
                entry.native_bound = bound_live;
                Ok(())
            }
            Err(error) => Err(types::ErrorCode::from(error).into()),
        }
    }

    fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<TcpSocket>> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "create");
        let socket =
            types::HostTcpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)?;
        self.0
            .durable_ctx_mut()
            .state
            .socket_entries
            .insert(socket.rep(), SocketEntry::new(address_family));
        Ok(socket)
    }

    fn get_local_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "get-local-address");
        let entry = tcp_entry(self.0.durable_ctx_mut(), socket.rep()).map_err(SocketError::trap)?;
        match &entry.phase {
            SocketPhase::Closed(error) => return Err(types::ErrorCode::from(error.clone()).into()),
            SocketPhase::Connecting => return Err(types::ErrorCode::InvalidState.into()),
            _ => {}
        }
        entry
            .local_address
            .clone()
            .map(Into::into)
            .ok_or_else(|| types::ErrorCode::InvalidState.into())
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "get-remote-address");
        let entry = tcp_entry(self.0.durable_ctx_mut(), socket.rep()).map_err(SocketError::trap)?;
        if let SocketPhase::Closed(error) = &entry.phase {
            return Err(types::ErrorCode::from(error.clone()).into());
        }
        entry
            .remote_address
            .clone()
            .map(Into::into)
            .ok_or_else(|| types::ErrorCode::InvalidState.into())
    }

    fn get_is_listening(&mut self, socket: Resource<TcpSocket>) -> wasmtime::Result<bool> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "get-is-listening");
        types::HostTcpSocket::get_is_listening(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_address_family(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<types::IpAddressFamily> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "get-address-family");
        Ok(tcp_entry(self.0.durable_ctx_mut(), socket.rep())?.family)
    }

    fn set_listen_backlog_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "set-listen-backlog-size",
        );
        types::HostTcpSocket::set_listen_backlog_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    async fn get_keep_alive_enabled(&mut self, socket: Resource<TcpSocket>) -> SocketResult<bool> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-keep-alive-enabled",
        );
        let rep = socket.rep();
        if let Some(value) = tcp_options(self.0.durable_ctx_mut(), rep)?
            .keep_alive_enabled
            .as_ref()
            .and_then(OnceLock::get)
            .copied()
        {
            return Ok(value);
        }
        let value = types::HostTcpSocket::get_keep_alive_enabled(
            &mut WasiSocketsView::sockets(self.0),
            socket,
        )
        .await?;
        if let Some(cache) = &tcp_options(self.0.durable_ctx_mut(), rep)?.keep_alive_enabled {
            return Ok(*cache.get_or_init(|| value));
        }
        Ok(value)
    }

    fn set_keep_alive_enabled(
        &mut self,
        socket: Resource<TcpSocket>,
        value: bool,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "set-keep-alive-enabled",
        );
        let rep = socket.rep();
        tcp_options(self.0.durable_ctx_mut(), rep)?;
        types::HostTcpSocket::set_keep_alive_enabled(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )?;
        tcp_entry(self.0.durable_ctx_mut(), rep)
            .map_err(SocketError::trap)?
            .options
            .keep_alive_enabled = Some(OnceLock::new());
        Ok(())
    }

    async fn get_keep_alive_idle_time(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::Duration> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-keep-alive-idle-time",
        );
        let rep = socket.rep();
        if let Some(value) = tcp_options(self.0.durable_ctx_mut(), rep)?
            .keep_alive_idle_time
            .as_ref()
            .and_then(OnceLock::get)
            .copied()
        {
            return Ok(value);
        }
        let value = types::HostTcpSocket::get_keep_alive_idle_time(
            &mut WasiSocketsView::sockets(self.0),
            socket,
        )
        .await?;
        if let Some(cache) = &tcp_options(self.0.durable_ctx_mut(), rep)?.keep_alive_idle_time {
            return Ok(*cache.get_or_init(|| value));
        }
        Ok(value)
    }

    fn set_keep_alive_idle_time(
        &mut self,
        socket: Resource<TcpSocket>,
        value: types::Duration,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "set-keep-alive-idle-time",
        );
        let rep = socket.rep();
        tcp_options(self.0.durable_ctx_mut(), rep)?;
        types::HostTcpSocket::set_keep_alive_idle_time(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )?;
        tcp_entry(self.0.durable_ctx_mut(), rep)
            .map_err(SocketError::trap)?
            .options
            .keep_alive_idle_time = Some(OnceLock::new());
        Ok(())
    }

    async fn get_keep_alive_interval(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::Duration> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-keep-alive-interval",
        );
        let rep = socket.rep();
        if let Some(value) = tcp_options(self.0.durable_ctx_mut(), rep)?
            .keep_alive_interval
            .as_ref()
            .and_then(OnceLock::get)
            .copied()
        {
            return Ok(value);
        }
        let value = types::HostTcpSocket::get_keep_alive_interval(
            &mut WasiSocketsView::sockets(self.0),
            socket,
        )
        .await?;
        if let Some(cache) = &tcp_options(self.0.durable_ctx_mut(), rep)?.keep_alive_interval {
            return Ok(*cache.get_or_init(|| value));
        }
        Ok(value)
    }

    fn set_keep_alive_interval(
        &mut self,
        socket: Resource<TcpSocket>,
        value: types::Duration,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "set-keep-alive-interval",
        );
        let rep = socket.rep();
        tcp_options(self.0.durable_ctx_mut(), rep)?;
        types::HostTcpSocket::set_keep_alive_interval(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )?;
        tcp_entry(self.0.durable_ctx_mut(), rep)
            .map_err(SocketError::trap)?
            .options
            .keep_alive_interval = Some(OnceLock::new());
        Ok(())
    }

    async fn get_keep_alive_count(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u32> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-keep-alive-count",
        );
        let rep = socket.rep();
        if let Some(value) = tcp_options(self.0.durable_ctx_mut(), rep)?
            .keep_alive_count
            .as_ref()
            .and_then(OnceLock::get)
            .copied()
        {
            return Ok(value);
        }
        let value = types::HostTcpSocket::get_keep_alive_count(
            &mut WasiSocketsView::sockets(self.0),
            socket,
        )
        .await?;
        if let Some(cache) = &tcp_options(self.0.durable_ctx_mut(), rep)?.keep_alive_count {
            return Ok(*cache.get_or_init(|| value));
        }
        Ok(value)
    }

    fn set_keep_alive_count(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u32,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "set-keep-alive-count",
        );
        let rep = socket.rep();
        tcp_options(self.0.durable_ctx_mut(), rep)?;
        types::HostTcpSocket::set_keep_alive_count(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )?;
        tcp_entry(self.0.durable_ctx_mut(), rep)
            .map_err(SocketError::trap)?
            .options
            .keep_alive_count = Some(OnceLock::new());
        Ok(())
    }

    async fn get_hop_limit(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u8> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "get-hop-limit");
        let rep = socket.rep();
        if let Some(value) = tcp_options(self.0.durable_ctx_mut(), rep)?
            .hop_limit
            .as_ref()
            .and_then(OnceLock::get)
            .copied()
        {
            return Ok(value);
        }
        let value =
            types::HostTcpSocket::get_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
                .await?;
        if let Some(cache) = &tcp_options(self.0.durable_ctx_mut(), rep)?.hop_limit {
            return Ok(*cache.get_or_init(|| value));
        }
        Ok(value)
    }

    fn set_hop_limit(&mut self, socket: Resource<TcpSocket>, value: u8) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "set-hop-limit");
        let rep = socket.rep();
        tcp_options(self.0.durable_ctx_mut(), rep)?;
        types::HostTcpSocket::set_hop_limit(&mut WasiSocketsView::sockets(self.0), socket, value)?;
        tcp_entry(self.0.durable_ctx_mut(), rep)
            .map_err(SocketError::trap)?
            .options
            .hop_limit = Some(OnceLock::new());
        Ok(())
    }

    async fn get_receive_buffer_size(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-receive-buffer-size",
        );
        let rep = socket.rep();
        if let Some(value) = tcp_options(self.0.durable_ctx_mut(), rep)?
            .receive_buffer_size
            .as_ref()
            .and_then(OnceLock::get)
            .copied()
        {
            return Ok(value);
        }
        let value = types::HostTcpSocket::get_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
        )
        .await?;
        if let Some(cache) = &tcp_options(self.0.durable_ctx_mut(), rep)?.receive_buffer_size {
            return Ok(*cache.get_or_init(|| value));
        }
        Ok(value)
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "set-receive-buffer-size",
        );
        let rep = socket.rep();
        tcp_options(self.0.durable_ctx_mut(), rep)?;
        types::HostTcpSocket::set_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )?;
        tcp_entry(self.0.durable_ctx_mut(), rep)
            .map_err(SocketError::trap)?
            .options
            .receive_buffer_size = Some(OnceLock::new());
        Ok(())
    }

    async fn get_send_buffer_size(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-send-buffer-size",
        );
        let rep = socket.rep();
        if let Some(value) = tcp_options(self.0.durable_ctx_mut(), rep)?
            .send_buffer_size
            .as_ref()
            .and_then(OnceLock::get)
            .copied()
        {
            return Ok(value);
        }
        let value = types::HostTcpSocket::get_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
        )
        .await?;
        if let Some(cache) = &tcp_options(self.0.durable_ctx_mut(), rep)?.send_buffer_size {
            return Ok(*cache.get_or_init(|| value));
        }
        Ok(value)
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "set-send-buffer-size",
        );
        let rep = socket.rep();
        tcp_options(self.0.durable_ctx_mut(), rep)?;
        types::HostTcpSocket::set_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )?;
        tcp_entry(self.0.durable_ctx_mut(), rep)
            .map_err(SocketError::trap)?
            .options
            .send_buffer_size = Some(OnceLock::new());
        Ok(())
    }

    fn drop(&mut self, sock: Resource<TcpSocket>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "drop");
        self.0
            .durable_ctx_mut()
            .state
            .socket_entries
            .remove(&sock.rep());
        types::HostTcpSocket::drop(&mut WasiSocketsView::sockets(self.0), sock)
    }
}

struct TcpSocketSendTask<Ctx> {
    /// The native send stream, taken by the `send` import when its Store was live.
    sink: Option<TcpSendSink>,
    demand_rx: mpsc::Receiver<TcpSendDemand>,
    result_tx: oneshot::Sender<TcpStreamResolution>,
    observational_owner: Option<OplogIndex>,
    /// The invocation id of the `send` guest import that spawned the task.
    invocation_id: InvocationIdSource,
    activity: TailActivity,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> TcpSocketSendTask<Ctx> {
    fn new(
        sink: Option<TcpSendSink>,
        demand_rx: mpsc::Receiver<TcpSendDemand>,
        result_tx: oneshot::Sender<TcpStreamResolution>,
        observational_owner: Option<OplogIndex>,
        invocation_id: InvocationIdSource,
        activity: TailActivity,
    ) -> Self {
        Self {
            sink,
            demand_rx,
            result_tx,
            observational_owner,
            invocation_id,
            activity,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx> TcpSocketSendTask<Ctx>
where
    Ctx: WorkerCtx,
{
    async fn run<U: Send + 'static>(
        self,
        accessor: &Accessor<U, DurableP3<Ctx>>,
    ) -> wasmtime::Result<()> {
        run_tcp_socket_send::<Ctx, U>(
            accessor,
            self.sink,
            self.demand_rx,
            self.result_tx,
            self.observational_owner,
            self.invocation_id,
            self.activity,
        )
        .await
    }
}

/// The native socket's send stream, owned by a send that started live in this Store. An
/// `Err` keeps the error of a native stream that could not be taken; every write reports it.
struct TcpSendSink(Result<TcpSendStream, types::ErrorCode>);

impl TcpSendSink {
    /// Takes the native send stream of `socket`. The native stream owns the connection from
    /// then on, so the guest may drop the socket. No byte reaches the socket until the send
    /// task writes its first chunk.
    fn take<Ctx, U>(
        accessor: &Accessor<U, DurableP3<Ctx>>,
        socket: &Resource<TcpSocket>,
    ) -> wasmtime::Result<Self>
    where
        Ctx: WorkerCtx,
        U: Send + 'static,
    {
        accessor.with(|mut store| {
            let view = wasi_sockets_view::<Ctx, U>(store.data_mut());
            let socket = view.table.get_mut(socket)?;
            Ok(Self(socket.take_send_stream().map_err(Into::into)))
        })
    }

    /// Writes a prefix of `bytes` and returns its length. A write that is still pending has
    /// written nothing, so dropping this future cancels it without a partial effect.
    async fn write(&mut self, bytes: &[u8]) -> Result<u64, types::ErrorCode> {
        match &mut self.0 {
            Ok(stream) => std::future::poll_fn(|cx| stream.poll_write(cx, bytes))
                .await
                .map(|count| count as u64)
                .map_err(Into::into),
            Err(code) => Err(code.clone()),
        }
    }

    /// Drops the native send stream, which shuts the write side down, and returns the result
    /// of the send.
    fn close(self) -> Result<(), types::ErrorCode> {
        self.0.map(drop)
    }
}

/// Durable driver for a `send` socket stream.
///
/// The `send` parent owns the stream. Each guest write is one `send-chunk` child that records
/// the write's result and the number of bytes the native sink accepted, never the bytes. The
/// parent waits for the next guest write at a safe park point, so an open send does not keep
/// the agent busy. Only a parent that started live in this Store writes to the native socket:
/// a write that has no recorded child after replay, and a child left incomplete by a crash,
/// complete with `connection-broken` and are never written. The parent terminal is persisted
/// after every child terminal.
async fn run_tcp_socket_send<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    sink: Option<TcpSendSink>,
    demand_rx: mpsc::Receiver<TcpSendDemand>,
    result_tx: oneshot::Sender<TcpStreamResolution>,
    observational_owner: Option<OplogIndex>,
    invocation_id: InvocationIdSource,
    activity: TailActivity,
) -> wasmtime::Result<()>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let mut stream = match DurableDemandStream::<
        P3SocketsTypesTcpSocketSend,
        P3SocketsTypesTcpSocketSendChunk,
        TcpSendDemand,
    >::start(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        demand_rx,
        AccessClaimOptions {
            observational_owner,
            invocation_id,
            scope_replay_recovery: ScopeReplayRecovery::reexecute_when(async { Ok(()) }),
            ..Default::default()
        },
        HostRequestNoInput {},
    )
    .await
    {
        Ok(stream) => stream,
        Err(error) => {
            let trap_context = find_durable_call_trap_context(&error);
            return fail_tcp_stream_task(
                result_tx,
                wasmtime::Error::from_anyhow(error),
                trap_context,
            );
        }
    };

    // The parent `Start` is committed by `start`, before the native sink can see a byte. A
    // Store moves from replay to live only once, so a sink taken by the import belongs to a
    // parent that starts live; a parent that becomes live at the replay tail has no sink.
    let mut sink = sink.filter(|_| stream.is_live());

    // A parent that did not start live in this Store lost its connection.
    let mut terminal = if sink.is_some() {
        Ok(())
    } else {
        Err(types::ErrorCode::ConnectionBroken)
    };

    loop {
        // Safe park: waiting for the guest's next write.
        let Some(demand) = stream.next_demand(&activity).await else {
            break;
        };
        let TcpSendDemand {
            bytes,
            reply,
            mut cancel,
        } = demand;

        let item = match stream
            .next(
                accessor,
                durable_worker_ctx::<Ctx, U>,
                HostRequestNoInput {},
            )
            .await
        {
            Ok(item) => item,
            Err(error) => {
                let trap_context =
                    find_durable_call_trap_context(&error).unwrap_or_else(|| stream.trap_context());
                let _ = reply.send(TcpSendReply::Failed {
                    message: error.to_string(),
                    trap_context,
                });
                return fail_tcp_stream_task(
                    result_tx,
                    wasmtime::Error::from_anyhow(stream.trap(error)),
                    Some(trap_context),
                );
            }
        };

        let (result, delivery) = match item {
            DurableDemandItem::Replayed { response, delivery } => {
                (deserialize_tcp_send_chunk(response.result), delivery)
            }
            DurableDemandItem::Live(mut child) => {
                let result = match sink.take() {
                    // No native sink: the connection did not survive into this Store.
                    None => Err(types::ErrorCode::ConnectionBroken),
                    Some(mut live) => {
                        let interrupt = accessor.with(|mut access| {
                            durable_worker_ctx::<Ctx, U>(access.data_mut())
                                .create_interrupt_signal()
                        });
                        let written = tokio::select! {
                            biased;
                            written = live.write(&bytes) => written,
                            // The guest cancelled the write, or dropped its writer, before the
                            // socket took a byte: the write accepts none.
                            _ = &mut cancel => Ok(0),
                            kind = interrupt => {
                                // Leave both Starts incomplete; recovery completes them with
                                // connection loss and never writes the chunk again.
                                child.abandon_for_trap();
                                stream.abandon_for_trap();
                                return Err(wasmtime::Error::from_anyhow(kind.into()));
                            }
                        };
                        // A failed native stream is not used again.
                        if written.is_ok() {
                            sink = Some(live);
                        }
                        written
                    }
                };
                let delivery = match stream
                    .complete_item(
                        accessor,
                        durable_worker_ctx::<Ctx, U>,
                        child,
                        HostResponseP3SocketsTcpSendChunk {
                            result: result.clone().map_err(SerializableP3SocketErrorCode::from),
                        },
                    )
                    .await
                {
                    Ok(delivery) => delivery,
                    Err(error) => {
                        let trap_context = stream.trap_context();
                        let _ = reply.send(TcpSendReply::Failed {
                            message: error.to_string(),
                            trap_context,
                        });
                        stream.abandon_for_trap();
                        return fail_tcp_stream_task(
                            result_tx,
                            wasmtime::Error::from(error),
                            Some(trap_context),
                        );
                    }
                };
                (result, delivery)
            }
        };

        let reply_value = match result {
            Ok(count) => TcpSendReply::Accepted(count),
            Err(_) => TcpSendReply::Rejected,
        };
        match deliver_demand(&activity, reply, reply_value, delivery).await {
            Ok(DemandDelivery::Delivered) => {}
            // The guest dropped its writer before the result reached it. The discard is
            // recorded, and the send ends as if the writer had closed.
            Ok(DemandDelivery::Abandoned) => break,
            Err(error) => {
                let trap_context = stream.trap_context();
                return fail_tcp_stream_task(
                    result_tx,
                    wasmtime::Error::from_anyhow(stream.trap(error)),
                    Some(trap_context),
                );
            }
        }
        if let Err(code) = result {
            terminal = Err(code);
            break;
        }
    }

    // The guest closed its writer, or a write failed: shut the native send stream down.
    if let Some(live) = sink.take() {
        terminal = live.close();
    }

    let parent_trap_context = stream.trap_context();
    let outcome = match stream
        .finish(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            HostResponseP3SocketsTcpSend {
                result: serialize_tcp_stream_result(terminal),
            },
        )
        .await
    {
        Ok(response) => deserialize_tcp_stream_result(response.result),
        Err(error) => {
            return fail_tcp_stream_task(
                result_tx,
                wasmtime::Error::from(error),
                Some(parent_trap_context),
            );
        }
    };

    let _ = result_tx.send(TcpStreamResolution::Outcome(outcome));
    Ok(())
}

fn deserialize_tcp_send_chunk(
    result: Result<u64, SerializableP3SocketErrorCode>,
) -> Result<u64, types::ErrorCode> {
    result.map_err(Into::into)
}

/// The native socket's receive stream, owned by a receive that started live in this Store.
struct TcpReceiveSource {
    bytes: mpsc::Receiver<Vec<u8>>,
    permits: mpsc::Sender<()>,
    result: Option<oneshot::Receiver<Result<(), types::ErrorCode>>>,
}

impl TcpReceiveSource {
    /// Takes the native receive stream of `socket`. The native stream owns the connection from
    /// then on, so the guest may drop the socket. The upstream stream is piped into a
    /// demand-gated consumer: it forwards a chunk only after the receive task grants a permit,
    /// so nothing is read from the socket before a durable demand.
    async fn take<Ctx, U>(
        accessor: &Accessor<U, DurableP3<Ctx>>,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Self>
    where
        Ctx: WorkerCtx,
        U: Send + 'static,
    {
        let sockets = accessor.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
        let (socket_stream, result_future) =
            <WasiSockets as types::HostTcpSocketWithStore<U>>::receive(&sockets, socket).await?;
        let (chunk_tx, bytes) = mpsc::channel::<Vec<u8>>(1);
        let (permits, permit_rx) = mpsc::channel::<()>(1);
        let (result_tx, result) = oneshot::channel();
        accessor.with(|mut store| -> wasmtime::Result<()> {
            socket_stream.pipe(
                &mut store,
                TcpReceiveForwardConsumer::new(PollSender::new(chunk_tx), permit_rx),
            )?;
            result_future.pipe(&mut store, TcpSocketResultConsumer::new(result_tx))?;
            Ok(())
        })?;
        Ok(Self {
            bytes,
            permits,
            result: Some(result),
        })
    }
}

struct TcpSocketReceiveTask<Ctx> {
    source: Option<TcpReceiveSource>,
    demand_rx: mpsc::Receiver<TcpReceiveDemand>,
    result_tx: oneshot::Sender<TcpStreamResolution>,
    observational_owner: Option<OplogIndex>,
    /// The invocation id of the `receive` guest import that spawned the task.
    invocation_id: InvocationIdSource,
    activity: TailActivity,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> TcpSocketReceiveTask<Ctx> {
    fn new(
        source: Option<TcpReceiveSource>,
        demand_rx: mpsc::Receiver<TcpReceiveDemand>,
        result_tx: oneshot::Sender<TcpStreamResolution>,
        observational_owner: Option<OplogIndex>,
        invocation_id: InvocationIdSource,
        activity: TailActivity,
    ) -> Self {
        Self {
            source,
            demand_rx,
            result_tx,
            observational_owner,
            invocation_id,
            activity,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx> TcpSocketReceiveTask<Ctx>
where
    Ctx: WorkerCtx,
{
    async fn run<U: Send + 'static>(
        self,
        accessor: &Accessor<U, DurableP3<Ctx>>,
    ) -> wasmtime::Result<()> {
        let scope = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut())
                .public_state
                .worker()
                .release_scope()
        });
        observe_tcp_receive_driver(
            scope,
            run_tcp_socket_receive::<Ctx, U>(
                accessor,
                self.source,
                self.demand_rx,
                self.result_tx,
                self.observational_owner,
                self.invocation_id,
                self.activity,
            ),
        )
        .await
    }
}

async fn observe_tcp_receive_driver(
    scope: crate::metrics::resource_release::ReleaseScope,
    driver: impl Future<Output = wasmtime::Result<()>>,
) -> wasmtime::Result<()> {
    let mut exit = crate::metrics::resource_release::HostDriverExit::entered(&scope);
    let result = driver.await;
    exit.returned(result.is_err());
    result
}

/// Fail the durable `send` or `receive` task loudly on a durability-machinery error (an
/// oplog read/write/socket-setup failure), as opposed to a normal socket error.
///
/// Mirrors the P3 HTTP `consume-body` fail-loud contract: a durability failure
/// must not be turned into a normal terminal (which would commit a completed
/// parent marker after an incomplete child chunk). The caller leaves the parent
/// without a terminal marker (it traps/abandons the parent handle so no
/// `Cancelled` is written), and the guest-facing result future is resolved with
/// a [`TcpStreamResolution::Trap`] carrying the failing call scope's trap
/// context so it also fails loud with correct retry grouping. When `trap_context`
/// is `None` the sender is dropped, which still traps the result future loudly.
fn fail_tcp_stream_task(
    result_tx: oneshot::Sender<TcpStreamResolution>,
    error: wasmtime::Error,
    trap_context: Option<DurableCallTrapContext>,
) -> wasmtime::Result<()> {
    match trap_context {
        Some(trap_context) => {
            let _ = result_tx.send(TcpStreamResolution::Trap {
                message: "tcp socket durable persistence failed".to_string(),
                trap_context,
            });
        }
        None => drop(result_tx),
    }
    Err(error)
}

/// Durable driver for a `receive` socket stream.
///
/// Persists incoming socket bytes **chunk-by-chunk** under a single `receive`
/// parent call (mirroring the P3 HTTP incoming-body stream):
///
/// * the parent `P3SocketsTypesTcpSocketReceive` call owns the stream and
///   is finalized last with a marker carrying the terminal `Result<(), error>`;
/// * every delivered chunk is persisted as a `P3SocketsTypesTcpSocketReceiveChunk`
///   child (`Data`) before its bytes are handed to the guest;
/// * a requested EOF is persisted as an `End` child. Dropping an idle reader only
///   closes the parent; collection does not create a read demand.
///
/// Each child is produced in response to exactly one producer demand, so on
/// replay the same number of children are read back from the oplog and delivered
/// in the same order — no whole-stream buffering, bounded memory.
async fn run_tcp_socket_receive<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    source: Option<TcpReceiveSource>,
    demand_rx: mpsc::Receiver<TcpReceiveDemand>,
    result_tx: oneshot::Sender<TcpStreamResolution>,
    observational_owner: Option<OplogIndex>,
    invocation_id: InvocationIdSource,
    activity: TailActivity,
) -> wasmtime::Result<()>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    // Open the parent call. The demand stream owns it until every requested child has
    // reached its delivery boundary, so the parent terminal is necessarily written last.
    let mut stream = match DurableDemandStream::<
        P3SocketsTypesTcpSocketReceive,
        P3SocketsTypesTcpSocketReceiveChunk,
        TcpReceiveDemand,
    >::start(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        demand_rx,
        AccessClaimOptions {
            observational_owner,
            invocation_id,
            scope_replay_recovery: ScopeReplayRecovery::reexecute_when(async { Ok(()) }),
            ..Default::default()
        },
        HostRequestNoInput {},
    )
    .await
    {
        Ok(parent) => parent,
        Err(error) => {
            let trap_context = find_durable_call_trap_context(&error);
            return fail_tcp_stream_task(
                result_tx,
                wasmtime::Error::from_anyhow(error),
                trap_context,
            );
        }
    };

    // The parent `Start` is committed by `start`, before the source forwards a byte. A Store
    // moves from replay to live only once, so a source taken by the import belongs to a parent
    // that starts live; a parent that becomes live at the replay tail has no source.
    let source = source.filter(|_| stream.is_live());
    let (mut bytes_rx, permit_tx, mut socket_result_rx) = match source {
        Some(TcpReceiveSource {
            bytes,
            permits,
            result,
        }) => (Some(bytes), Some(permits), result),
        None => (None, None, None),
    };

    // A parent that did not start live in this Store lost its connection, including one whose
    // crash followed the final child but preceded the parent terminal.
    let mut terminal = if bytes_rx.is_some() {
        Ok(())
    } else {
        Err(types::ErrorCode::ConnectionBroken)
    };

    loop {
        // Safe park: waiting for the guest to demand the next chunk.
        let demand = stream.next_demand(&activity).await;
        if demand.is_none() {
            break;
        }

        let item = match stream
            .next(
                accessor,
                durable_worker_ctx::<Ctx, U>,
                HostRequestNoInput {},
            )
            .await
        {
            Ok(item) => item,
            Err(error) => {
                let trap_context =
                    find_durable_call_trap_context(&error).unwrap_or_else(|| stream.trap_context());
                if let Some(reply_tx) = demand {
                    let _ = reply_tx.send(TcpReceiveReply::Failed {
                        message: error.to_string(),
                        trap_context,
                    });
                }
                return fail_tcp_stream_task(
                    result_tx,
                    wasmtime::Error::from_anyhow(stream.trap(error)),
                    Some(trap_context),
                );
            }
        };

        // Produce the next item: replay the recorded child (replay) or read the
        // upstream socket and persist it (live).
        let (produced, delivery) = match item {
            DurableDemandItem::Replayed { response, delivery } => {
                let produced = match response.chunk {
                    SerializableP3TcpChunk::Data(bytes) => {
                        ProducedTcpChunk::Data(Bytes::from(bytes))
                    }
                    SerializableP3TcpChunk::End => ProducedTcpChunk::Terminal,
                };
                (produced, delivery)
            }
            DurableDemandItem::Live(mut child) => {
                // An abandoned read still owns its child. Complete that demand without
                // reading more of the socket when its reply endpoint has disappeared.
                let producer_gone = demand
                    .as_ref()
                    .map(|reply_tx| reply_tx.is_closed())
                    .unwrap_or(true);
                let frame = match bytes_rx.as_mut() {
                    // Recorded chunks remain replayable, but their socket does not survive
                    // reconstruction. Complete only the missing tail with explicit loss.
                    None => TcpReceiveFrame::End(Err(types::ErrorCode::ConnectionBroken)),
                    Some(_) if producer_gone => TcpReceiveFrame::End(Ok(())),
                    Some(bytes_rx) => {
                        // Grant the upstream consumer permission to forward exactly one
                        // chunk for this demand.
                        if let Some(permit_tx) = permit_tx.as_ref()
                            && let Err(mpsc::error::TrySendError::Full(())) = permit_tx.try_send(())
                        {
                            tracing::error!(
                                "tcp receive permit channel unexpectedly full; \
                             continuing with the already-queued permit"
                            );
                        }
                        let interrupt = accessor.with(|mut access| {
                            durable_worker_ctx::<Ctx, U>(access.data_mut())
                                .create_interrupt_signal()
                        });
                        let socket_result_rx = &mut socket_result_rx;
                        let read_frame = async {
                            match bytes_rx.recv().await {
                                Some(bytes) => TcpReceiveFrame::Data(Bytes::from(bytes)),
                                None => {
                                    let result = match socket_result_rx.take() {
                                        Some(rx) => rx
                                            .await
                                            .unwrap_or(Err(types::ErrorCode::ConnectionBroken)),
                                        None => Ok(()),
                                    };
                                    TcpReceiveFrame::End(result)
                                }
                            }
                        };
                        tokio::select! {
                            frame = read_frame => frame,
                            kind = interrupt => {
                                // Leave both Starts incomplete for call-local loss completion.
                                child.abandon_for_trap();
                                stream.abandon_for_trap();
                                return Err(wasmtime::Error::from_anyhow(kind.into()));
                            }
                        }
                    }
                };

                let chunk = match &frame {
                    TcpReceiveFrame::Data(bytes) => SerializableP3TcpChunk::Data(bytes.to_vec()),
                    TcpReceiveFrame::End(_) => SerializableP3TcpChunk::End,
                };
                let produced = match frame {
                    TcpReceiveFrame::Data(bytes) => ProducedTcpChunk::Data(bytes),
                    TcpReceiveFrame::End(result) => {
                        terminal = result;
                        ProducedTcpChunk::Terminal
                    }
                };
                let delivery = match stream
                    .complete_item(
                        accessor,
                        durable_worker_ctx::<Ctx, U>,
                        child,
                        HostResponseP3SocketsTcpReceiveChunk { chunk },
                    )
                    .await
                {
                    Ok(delivery) => delivery,
                    Err(error) => {
                        let trap_context = stream.trap_context();
                        if let Some(reply_tx) = demand {
                            let _ = reply_tx.send(TcpReceiveReply::Failed {
                                message: error.to_string(),
                                trap_context,
                            });
                        }
                        stream.abandon_for_trap();
                        return fail_tcp_stream_task(
                            result_tx,
                            wasmtime::Error::from(error),
                            Some(trap_context),
                        );
                    }
                };

                (produced, delivery)
            }
        };

        // Delivery is the same after a live read and a replayed child: each child's guest
        // delivery is recorded, and replay releases it at that recorded boundary.
        match produced {
            ProducedTcpChunk::Data(bytes) => {
                let result = match deliver_demand(
                    &activity,
                    demand.unwrap_or_else(closed_demand),
                    TcpReceiveReply::Data(bytes),
                    delivery,
                )
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        let trap_context = stream.trap_context();
                        return fail_tcp_stream_task(
                            result_tx,
                            wasmtime::Error::from_anyhow(stream.trap(error)),
                            Some(trap_context),
                        );
                    }
                };
                // The guest dropped its reader before the chunk reached it. The discard is
                // recorded, and the receive ends as if the reader had been dropped while idle.
                if matches!(result, DemandDelivery::Abandoned) {
                    break;
                }
            }
            ProducedTcpChunk::Terminal => {
                let (ack_tx, ack_rx) = oneshot::channel();
                match deliver_demand(
                    &activity,
                    demand.unwrap_or_else(closed_demand),
                    TcpReceiveReply::End { ack: ack_tx },
                    delivery,
                )
                .await
                {
                    Ok(DemandDelivery::Delivered) => {
                        let _ = ack_rx.await;
                    }
                    Ok(DemandDelivery::Abandoned) => {}
                    Err(error) => {
                        let trap_context = stream.trap_context();
                        return fail_tcp_stream_task(
                            result_tx,
                            wasmtime::Error::from_anyhow(stream.trap(error)),
                            Some(trap_context),
                        );
                    }
                }
                break;
            }
        }
    }

    // The parent terminal is persisted only after all child terminals and delivery boundaries.
    let parent_trap_context = stream.trap_context();
    let outcome = match stream
        .finish(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            HostResponseP3SocketsTcpReceive {
                result: serialize_tcp_stream_result(terminal),
            },
        )
        .await
    {
        Ok(response) => deserialize_tcp_stream_result(response.result),
        Err(error) => {
            return fail_tcp_stream_task(
                result_tx,
                wasmtime::Error::from(error),
                Some(parent_trap_context),
            );
        }
    };

    let _ = result_tx.send(TcpStreamResolution::Outcome(outcome));
    Ok(())
}

/// Durable one-shot acquisition of a TCP `send`/`receive` stream.
///
/// wasmtime's P3 TCP `send`/`receive` take the socket's send (resp. receive)
/// stream exactly once; a second call returns [`types::ErrorCode::InvalidState`].
/// The durable wrappers replay `send`/`receive` from the oplog and never invoke
/// the native host call on replay, so the native taken flag is not advanced
/// across a replay boundary. This helper records the acquisition outcome durably
/// and restores the entry's taken flags. Recorded acquisitions keep their original
/// outcomes; new acquisitions on a lost connection return `connection-broken`.
///
/// On the live path the phase check, the taken-flag check and the
/// shadow mutation all happen inside a single `accessor.with` closure with no
/// `.await` between them, so concurrent acquisitions on the same socket are
/// linearized — exactly one observes a free stream and acquires it. The recorded
/// outcome is reapplied to the shadow on both live and replay so replay
/// rehydrates the taken flags before any later live call runs.
///
/// `Ok(())` means the stream was acquired; `Err(code)` means it was not (either
/// already taken, or the socket was not connected / in an error state), in which
/// case the caller must surface `code` without invoking the native send/receive.
async fn tcp_acquire_stream<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    socket_rep: u32,
    direction: TcpSocketStreamDirection,
) -> wasmtime::Result<(Result<(), types::ErrorCode>, CompletionDelivery)>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    match direction {
        TcpSocketStreamDirection::Send => {
            tcp_run_acquire::<Ctx, U, P3SocketsTypesTcpSocketSendAcquire>(
                accessor, socket_rep, direction,
            )
            .await
        }
        TcpSocketStreamDirection::Receive => {
            tcp_run_acquire::<Ctx, U, P3SocketsTypesTcpSocketReceiveAcquire>(
                accessor, socket_rep, direction,
            )
            .await
        }
    }
}

async fn tcp_run_acquire<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    socket_rep: u32,
    direction: TcpSocketStreamDirection,
) -> wasmtime::Result<(Result<(), types::ErrorCode>, CompletionDelivery)>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
    Pair: HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseP3SocketsTcpAcquire>,
{
    // `ReadLocal`: the acquire is a local connectivity probe plus local shadow
    // bookkeeping with no external side effect, so it is safely re-executable on
    // an incomplete replay. The actual byte transmission is a `WriteRemote` call. The
    // recorded outcome is still persisted and replayed for determinism, exactly
    // like the clock `ReadLocal` calls.
    let call = DurableCallSession::<Pair, Cancellable>::start_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        HostRequestNoInput {},
        DurableFunctionType::ReadLocal,
    )
    .await?;
    let (call, replayed) = if call.is_live() {
        (Some(call), None)
    } else {
        match call
            .replay_access_deferred(accessor, durable_worker_ctx::<Ctx, U>)
            .await?
        {
            DeferredCallReplayOutcome::Replayed(response, delivery) => {
                (None, Some((response, delivery)))
            }
            DeferredCallReplayOutcome::Incomplete(call) => (Some(call), None),
        }
    };
    // The acquire is observed only when the send/receive host function returns its
    // handles. Keep delivery deferred while that function starts its durable task.
    let (response, delivery) = if let Some(mut call) = call {
        let response = accessor
            .with(
                |mut access| -> wasmtime::Result<HostResponseP3SocketsTcpAcquire> {
                    let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                    let phase = tcp_entry(ctx, socket_rep)?.phase.clone();
                    let result = if phase == SocketPhase::Lost {
                        Err(SerializableP3SocketErrorCode::from(
                            types::ErrorCode::ConnectionBroken,
                        ))
                    } else if ctx.is_tcp_stream_taken(socket_rep, direction) {
                        Err(SerializableP3SocketErrorCode::from(
                            types::ErrorCode::InvalidState,
                        ))
                    } else {
                        match phase {
                            SocketPhase::Connected => {
                                ctx.mark_tcp_stream_taken(socket_rep, direction);
                                Ok(())
                            }
                            SocketPhase::Closed(error) => Err(error),
                            _ => Err(SerializableP3SocketErrorCode::from(
                                types::ErrorCode::InvalidState,
                            )),
                        }
                    };
                    Ok(HostResponseP3SocketsTcpAcquire { result })
                },
            )
            .map_err(|error| wasmtime::Error::from_anyhow(call.trap(error)))?;
        call.complete_access_deferred(accessor, durable_worker_ctx::<Ctx, U>, response)
            .await?
    } else {
        replayed.expect("a completed acquire retains its delivery")
    };

    // Reapply the recorded outcome to the shadow so replay rehydrates the taken
    // flag before any later live call runs (idempotent on the live path, where
    // the closure above already set it).
    if response.result.is_ok() {
        accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut())
                .mark_tcp_stream_taken(socket_rep, direction);
        });
    }

    Ok((deserialize_tcp_stream_result(response.result), delivery))
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostTcpSocketWithStore<U> for DurableP3<Ctx> {
    async fn connect(
        store: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        reserve_invocation_id_before_await(store, durable_worker_ctx::<Ctx, U>)
            .map_err(|error| SocketError::trap(wasmtime::Error::from(error)))?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "sockets::types::tcp-socket",
                "connect",
            )
        });
        let targets = socket_target(store, remote_address.into()).map(|target| vec![target]);
        let request_address = remote_address;
        let rep = socket.rep();
        let mut connected_live = false;
        let response = run_deniable_access::<_, _, Ctx, P3SocketsTypesTcpSocketConnect, _>(
            store,
            HostRequestP3SocketsConnect {
                remote_address: request_address.into(),
            },
            DurableFunctionType::WriteRemote,
            targets,
            || async {
                let (phase, binding_lost) = store.with(|mut access| -> wasmtime::Result<_> {
                    let entry = tcp_entry(durable_worker_ctx::<Ctx, U>(access.data_mut()), rep)?;
                    Ok((
                        entry.phase.clone(),
                        entry.phase == SocketPhase::Bound && !entry.native_bound,
                    ))
                })?;
                if binding_lost {
                    // The recorded endpoint is observational state, not a reserved native port.
                    return Ok(HostResponseP3SocketsTcpConnect {
                        result: Err(types::ErrorCode::ConnectionBroken.into()),
                        closed: false,
                    });
                }
                if !matches!(phase, SocketPhase::Created | SocketPhase::Bound) {
                    return Ok(HostResponseP3SocketsTcpConnect {
                        result: Err(types::ErrorCode::InvalidState.into()),
                        closed: false,
                    });
                }
                store.with(|mut access| {
                    durable_worker_ctx::<Ctx, U>(access.data_mut())
                        .state
                        .socket_entries
                        .get_mut(&rep)
                        .expect("TCP socket entry")
                        .phase = SocketPhase::Connecting;
                });
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let result = <WasiSockets as types::HostTcpSocketWithStore<U>>::connect(
                    &sockets,
                    socket,
                    remote_address,
                )
                .await;
                let closed = if result.is_err() {
                    store.with(|mut access| -> wasmtime::Result<bool> {
                        let mut view = wasi_sockets_view::<Ctx, U>(access.data_mut());
                        match types::HostTcpSocket::get_remote_address(
                            &mut view,
                            Resource::new_borrow(rep),
                        ) {
                            Ok(_) => Ok(false),
                            Err(error) => Ok(serialize_socket_error(error)?
                                != SerializableP3SocketErrorCode::InvalidState),
                        }
                    })?
                } else {
                    false
                };
                store.with(|mut access| {
                    durable_worker_ctx::<Ctx, U>(access.data_mut())
                        .state
                        .socket_entries
                        .get_mut(&rep)
                        .expect("TCP socket entry")
                        .phase = phase;
                });
                Ok(HostResponseP3SocketsTcpConnect {
                    closed,
                    result: match result {
                        Ok(()) => {
                            connected_live = true;
                            store
                                .with(|mut access| -> wasmtime::Result<_> {
                                    let mut view = wasi_sockets_view::<Ctx, U>(access.data_mut());
                                    let local = types::HostTcpSocket::get_local_address(
                                        &mut view,
                                        Resource::new_borrow(rep),
                                    )
                                    .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                                    let remote = types::HostTcpSocket::get_remote_address(
                                        &mut view,
                                        Resource::new_borrow(rep),
                                    )
                                    .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                                    Ok((local.into(), remote.into()))
                                })
                                .map(Ok)?
                        }
                        Err(error) => Err(serialize_socket_error(error)?),
                    },
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        match response.result {
            Ok((local, remote)) => {
                store.with(|mut access| {
                    let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                    let entry = ctx
                        .state
                        .socket_entries
                        .get_mut(&rep)
                        .expect("TCP socket entry");
                    entry.local_address = Some(local);
                    entry.remote_address = Some(remote);
                    entry.phase = if connected_live {
                        SocketPhase::Connected
                    } else {
                        SocketPhase::Lost
                    };
                });
                Ok(())
            }
            Err(error) => {
                if response.closed {
                    store.with(|mut access| {
                        durable_worker_ctx::<Ctx, U>(access.data_mut())
                            .state
                            .socket_entries
                            .get_mut(&rep)
                            .expect("TCP socket entry")
                            .phase = SocketPhase::Closed(error.clone());
                    });
                }
                Err(types::ErrorCode::from(error).into())
            }
        }
    }

    /// Agents expose no ports, so `listen` is always refused and its implicit bind never happens.
    /// A socket that is recorded as bound but has no native bind after reconstruction reports
    /// `connection-broken` first, like the other operations on it.
    async fn listen(
        mut store: Access<'_, U, Self>,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<StreamReader<Resource<TcpSocket>>> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "sockets::types::tcp-socket",
            "listen",
        );
        let rep = socket.rep();
        {
            let mut store = store.as_context_mut();
            let entry = tcp_entry(durable_worker_ctx::<Ctx, U>(store.data_mut()), rep)
                .map_err(SocketError::trap)?;
            if entry.phase == SocketPhase::Bound && !entry.native_bound {
                return Err(types::ErrorCode::ConnectionBroken.into());
            }
        }
        Err(types::ErrorCode::AccessDenied.into())
    }

    async fn send(
        accessor: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
        mut data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let invocation_id = access_invocation_id(accessor, durable_worker_ctx::<Ctx, U>)?;
        let custom_invocation_context = accessor.guest_task_context::<GuestTaskContext>()?;
        let observational_owner = accessor.with(|mut access| {
            resolve_current_observational_owner(
                durable_worker_ctx::<Ctx, U>(access.data_mut()),
                custom_invocation_context.as_deref(),
            )
        })?;
        // One-shot acquire of the send stream. On a second call (or when the
        // socket is not connected) this returns an error, which we surface
        // without invoking the native send, mirroring the native `take_send_stream`
        // contract while keeping replay deterministic.
        let (acquired, delivery) =
            tcp_acquire_stream::<Ctx, U>(accessor, socket.rep(), TcpSocketStreamDirection::Send)
                .await?;
        let (future, delivery) = delivery
            .continue_with(async {
                if let Err(error) = acquired {
                    let future = accessor.with(|mut store| {
                        data.close(&mut store)?;
                        FutureReader::new(
                            &mut store,
                            async move { wasmtime::error::Ok(Err(error)) },
                        )
                    })?;
                    return Ok(future);
                }

                // Take the native send stream while this import still holds the socket borrow:
                // the guest may drop the socket as soon as the import returns, before the task
                // has committed the `send` `Start`.
                let live = accessor
                    .with(|mut store| durable_worker_ctx::<Ctx, U>(store.data_mut()).is_live());
                let sink = if live {
                    Some(TcpSendSink::take::<Ctx, U>(accessor, &socket)?)
                } else {
                    None
                };
                let (demand_tx, demand_rx) = demand_channel();
                let (result_tx, result_rx) = oneshot::channel();
                // The host owns the reader of the guest stream; the durable task appends the
                // `send` `Start` only after both guest-facing handles exist.
                let future = with_import_access(accessor, |mut store| {
                    let future = FutureReader::new(&mut store, wait_tcp_stream_result(result_rx))?;
                    data.pipe(&mut store, DurableTcpSendConsumer::new(demand_tx))?;
                    let activity = durable_worker_ctx::<Ctx, U>(store.data_mut())
                        .tail_work_tracker()
                        .activity();
                    let task = TcpSocketSendTask::<Ctx>::new(
                        sink,
                        demand_rx,
                        result_tx,
                        observational_owner,
                        invocation_id,
                        activity,
                    );
                    let captured = durable_worker_ctx::<Ctx, U>(store.data_mut())
                        .attribution
                        .capture();
                    store.spawn(async move |store| captured.run(task.run(store)).await)?;
                    wasmtime::error::Ok(future)
                })?;
                wasmtime::error::Ok(future)
            })
            .await?;
        delivery.deliver_at_accessor_terminal(accessor).await?;
        Ok(future)
    }

    async fn receive(
        accessor: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        let invocation_id = access_invocation_id(accessor, durable_worker_ctx::<Ctx, U>)?;
        let custom_invocation_context = accessor.guest_task_context::<GuestTaskContext>()?;
        let observational_owner = accessor.with(|mut access| {
            resolve_current_observational_owner(
                durable_worker_ctx::<Ctx, U>(access.data_mut()),
                custom_invocation_context.as_deref(),
            )
        })?;
        // One-shot acquire of the receive stream. On a second call (or when the
        // socket is not connected) this returns an error, surfaced as an empty
        // stream plus an immediately-failing result future, mirroring the native
        // `take_receive_stream` contract while keeping replay deterministic.
        let (acquired, delivery) =
            tcp_acquire_stream::<Ctx, U>(accessor, socket.rep(), TcpSocketStreamDirection::Receive)
                .await?;
        let (handles, delivery) = delivery
            .continue_with(async {
                if let Err(error) = acquired {
                    let handles = accessor.with(|mut store| {
                        wasmtime::error::Ok((
                            StreamReader::new(&mut store, std::iter::empty::<u8>())?,
                            FutureReader::new(&mut store, async move {
                                wasmtime::error::Ok(Err(error))
                            })?,
                        ))
                    })?;
                    return Ok(handles);
                }

                // Take the native receive stream while this import still holds the socket
                // borrow: the guest may drop the socket as soon as the import returns, before
                // the task has committed the `receive` `Start`.
                let live = accessor
                    .with(|mut store| durable_worker_ctx::<Ctx, U>(store.data_mut()).is_live());
                let source = if live {
                    Some(TcpReceiveSource::take::<Ctx, U>(accessor, socket).await?)
                } else {
                    None
                };
                // Capacity 1 suffices (and bounds memory as defense in depth): the
                // producer keeps at most one demand in flight at a time.
                let (demand_tx, demand_rx) = demand_channel();
                let (result_tx, result_rx) = oneshot::channel();

                // Build both guest-facing handles before spawning the durable task. The
                // task appends the `receive` `Start`; the guest cannot poll either handle
                // until this host call returns, so spawning first would risk committing a
                // `Start` with no terminal if a later handle construction fails.
                let handles = with_import_access(accessor, |mut store| {
                    let mut stream =
                        StreamReader::new(&mut store, DurableTcpReceiveProducer::new(demand_tx))?;
                    let future =
                        match FutureReader::new(&mut store, wait_tcp_stream_result(result_rx)) {
                            Ok(future) => future,
                            Err(error) => {
                                let _ = stream.close(store.as_context_mut());
                                return Err(error);
                            }
                        };
                    let activity = durable_worker_ctx::<Ctx, U>(store.data_mut())
                        .tail_work_tracker()
                        .activity();
                    let task = TcpSocketReceiveTask::<Ctx>::new(
                        source,
                        demand_rx,
                        result_tx,
                        observational_owner,
                        invocation_id,
                        activity,
                    );
                    let captured = durable_worker_ctx::<Ctx, U>(store.data_mut())
                        .attribution
                        .capture();
                    store.spawn(async move |store| captured.run(task.run(store)).await)?;
                    Ok((stream, future))
                })?;
                wasmtime::error::Ok(handles)
            })
            .await?;
        delivery.deliver_at_accessor_terminal(accessor).await?;
        Ok(handles)
    }
}

#[cfg(test)]
mod release_tests {
    use super::observe_tcp_receive_driver;
    use crate::metrics::resource_release::{Cause, tests::TestMetrics};
    use test_r::test;

    #[test]
    async fn receive_driver_receipt_observes_return_trap_and_destruction() {
        for outcome in ["returned", "trapped", "dropped"] {
            let metrics = TestMetrics::new();
            let scope = metrics.scope();
            let (send, receive) = tokio::sync::oneshot::channel();
            let mut driver = Box::pin(observe_tcp_receive_driver(scope.clone(), async {
                receive.await.unwrap()
            }));
            assert!(futures::poll!(driver.as_mut()).is_pending());
            let joined = scope.accepted_driver();
            scope.freeze_cause(Cause::MonthlyMemory);
            scope.seal();
            metrics.advance(2500);
            assert_eq!(metrics.pending("accepted_stop", "host_operation_exit"), 1.0);
            if outcome == "dropped" {
                drop(driver);
            } else {
                send.send(if outcome == "trapped" {
                    Err(wasmtime::Error::msg("typed driver trap"))
                } else {
                    Ok(())
                })
                .unwrap();
                assert_eq!(driver.await.is_err(), outcome == "trapped");
            }
            assert_eq!(
                metrics.value(
                    "golem_agent_stop_host_operation_exit_seconds",
                    &[("operation", "p3_tcp_receive"), ("outcome", outcome)]
                ),
                1.0
            );
            assert_eq!(metrics.pending("accepted_stop", "end_to_end"), 1.0);
            joined.complete();
            metrics.assert_finished("accepted_stop", "released");
        }
    }

    #[test]
    async fn completed_receive_driver_is_inapplicable_to_later_stop() {
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        observe_tcp_receive_driver(scope.clone(), std::future::ready(Ok(())))
            .await
            .unwrap();
        let joined = scope.accepted_driver();
        scope.freeze_cause(Cause::Interrupt);
        scope.seal();
        joined.complete();
        assert_eq!(
            metrics.value("golem_agent_stop_host_operation_exit_seconds", &[]),
            0.0
        );
        metrics.assert_finished("accepted_stop", "released");
    }
}

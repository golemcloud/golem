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

use crate::durable_host::TcpSocketStreamDirection;
use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable, NotCancellable};
use crate::durable_host::durability::{DurableCallTrapContext, mark_durable_call_trap_context};
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, observe_function_call,
    observe_function_call_store, run_read_access, wasi_sockets_view,
};
use crate::workerctx::WorkerCtx;
use bytes::Bytes;
use golem_common::model::oplog::host_functions::{
    P3SocketsIpNameLookupResolveAddresses, P3SocketsTypesTcpSocketReceive,
    P3SocketsTypesTcpSocketReceiveAcquire, P3SocketsTypesTcpSocketReceiveChunk,
    P3SocketsTypesTcpSocketSend, P3SocketsTypesTcpSocketSendAcquire,
    P3SocketsTypesUdpSocketReceive, P3SocketsTypesUdpSocketSend,
};
use golem_common::model::oplog::types::{
    SerializableP3IpAddresses, SerializableP3SocketErrorCode, SerializableP3TcpChunk,
    SerializableP3UdpDatagram,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestNoInput, HostRequestP3SocketsResolveName,
    HostRequestP3SocketsUdpSend, HostResponseP3SocketsResolveName, HostResponseP3SocketsTcpAcquire,
    HostResponseP3SocketsTcpReceive, HostResponseP3SocketsTcpReceiveChunk,
    HostResponseP3SocketsTcpSend, HostResponseP3SocketsUdpReceive, HostResponseP3SocketsUdpSend,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;
use wasmtime::AsContextMut;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureConsumer, FutureReader, Resource, Source,
    StreamConsumer, StreamProducer, StreamReader, StreamResult,
};
use wasmtime_wasi::p3::bindings::sockets::{ip_name_lookup, types};
use wasmtime_wasi::p3::sockets::{SocketError, SocketResult};
use wasmtime_wasi::sockets::{TcpSocket, UdpSocket, WasiSockets, WasiSocketsView};

fn serialize_socket_error(error: SocketError) -> wasmtime::Result<SerializableP3SocketErrorCode> {
    Ok(SerializableP3SocketErrorCode::from(error.downcast()?))
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

/// Resolves the guest-facing send/receive result `FutureReader`. The durable
/// task sends the final result here; a closed channel means the task was dropped
/// before replying, which surfaces as a trap.
async fn wait_tcp_task_result(
    result_rx: oneshot::Receiver<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    result_rx.await.unwrap_or_else(|_| {
        Err(wasmtime::Error::msg(
            "tcp socket task dropped before replying",
        ))
    })
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

/// Drains the guest's outgoing `send` stream into a buffer so an incomplete
/// replayed `send` call can re-transmit the same bytes. The buffer is delivered
/// once the guest stream is fully drained (on drop of the consumer). The bytes
/// are never persisted to the oplog; only the send result is recorded.
struct TcpSendCaptureConsumer {
    contents: Vec<u8>,
    result_tx: Option<oneshot::Sender<Vec<u8>>>,
}

impl TcpSendCaptureConsumer {
    fn new(result_tx: oneshot::Sender<Vec<u8>>) -> Self {
        Self {
            contents: Vec::new(),
            result_tx: Some(result_tx),
        }
    }

    fn close(&mut self) {
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(std::mem::take(&mut self.contents));
        }
    }
}

impl<D> StreamConsumer<D> for TcpSendCaptureConsumer {
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

        let chunk = bytes.to_vec();
        let len = chunk.len();
        src.mark_read(len);
        self.contents.extend_from_slice(&chunk);

        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for TcpSendCaptureConsumer {
    fn drop(&mut self) {
        self.close();
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

/// Resolution delivered to the guest-facing receive-result future once the
/// receive stream closes (or the durable task fails before recording the
/// terminal).
enum TcpReceiveResolution {
    /// The receive terminal: a clean close or a socket `ErrorCode`.
    Outcome(Result<(), types::ErrorCode>),
    /// A durability failure: the result future traps with this message, tagged
    /// with the failing call scope's trap context.
    Trap {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// Resolves the guest-facing receive-result future. A durability failure traps
/// (carrying the failing call scope's trap context) rather than resolving to a
/// normal error that would mask it.
async fn wait_tcp_receive_result(
    result_rx: oneshot::Receiver<TcpReceiveResolution>,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    match result_rx.await {
        Ok(TcpReceiveResolution::Outcome(result)) => Ok(result),
        Ok(TcpReceiveResolution::Trap {
            message,
            trap_context,
        }) => Err(wasmtime::Error::from_anyhow(
            mark_durable_call_trap_context(anyhow::Error::msg(message), trap_context),
        )),
        Err(_) => Err(wasmtime::Error::msg(
            "tcp receive durable task dropped before resolving result",
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
    demand_tx: mpsc::UnboundedSender<TcpReceiveDemand>,
    pending: Option<oneshot::Receiver<TcpReceiveReply>>,
    finished: bool,
}

impl DurableTcpReceiveProducer {
    fn new(demand_tx: mpsc::UnboundedSender<TcpReceiveDemand>) -> Self {
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
            if self.demand_tx.send(tx).is_err() {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::msg(
                    "tcp receive durable task is gone",
                )));
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
struct TcpReceiveForwardConsumer {
    chunk_tx: PollSender<Vec<u8>>,
    permit_rx: mpsc::UnboundedReceiver<()>,
    has_permit: bool,
}

impl TcpReceiveForwardConsumer {
    fn new(chunk_tx: PollSender<Vec<u8>>, permit_rx: mpsc::UnboundedReceiver<()>) -> Self {
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

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: SocketError) -> wasmtime::Result<types::ErrorCode> {
        observe_function_call(&*self.0, "sockets::types", "convert-error-code");
        types::Host::convert_error_code(&mut WasiSocketsView::sockets(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostTcpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<TcpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "bind");
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostTcpSocket::bind(&mut view, socket, local_address).await
    }

    fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<TcpSocket>> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "create");
        types::HostTcpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)
    }

    fn get_local_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "get-local-address");
        types::HostTcpSocket::get_local_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "get-remote-address");
        types::HostTcpSocket::get_remote_address(&mut WasiSocketsView::sockets(self.0), socket)
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
        types::HostTcpSocket::get_address_family(&mut WasiSocketsView::sockets(self.0), socket)
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

    fn get_keep_alive_enabled(&mut self, socket: Resource<TcpSocket>) -> SocketResult<bool> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-keep-alive-enabled",
        );
        types::HostTcpSocket::get_keep_alive_enabled(&mut WasiSocketsView::sockets(self.0), socket)
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
        types::HostTcpSocket::set_keep_alive_enabled(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_keep_alive_idle_time(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::Duration> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-keep-alive-idle-time",
        );
        types::HostTcpSocket::get_keep_alive_idle_time(
            &mut WasiSocketsView::sockets(self.0),
            socket,
        )
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
        types::HostTcpSocket::set_keep_alive_idle_time(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_keep_alive_interval(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::Duration> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-keep-alive-interval",
        );
        types::HostTcpSocket::get_keep_alive_interval(&mut WasiSocketsView::sockets(self.0), socket)
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
        types::HostTcpSocket::set_keep_alive_interval(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_keep_alive_count(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u32> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-keep-alive-count",
        );
        types::HostTcpSocket::get_keep_alive_count(&mut WasiSocketsView::sockets(self.0), socket)
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
        types::HostTcpSocket::set_keep_alive_count(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_hop_limit(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u8> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "get-hop-limit");
        types::HostTcpSocket::get_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_hop_limit(&mut self, socket: Resource<TcpSocket>, value: u8) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "set-hop-limit");
        types::HostTcpSocket::set_hop_limit(&mut WasiSocketsView::sockets(self.0), socket, value)
    }

    fn get_receive_buffer_size(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-receive-buffer-size",
        );
        types::HostTcpSocket::get_receive_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
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
        types::HostTcpSocket::set_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_send_buffer_size(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::tcp-socket",
            "get-send-buffer-size",
        );
        types::HostTcpSocket::get_send_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
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
        types::HostTcpSocket::set_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn drop(&mut self, sock: Resource<TcpSocket>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "sockets::types::tcp-socket", "drop");
        // Clear the one-shot send/receive taken shadow before the resource (and
        // its rep) can be reused, so a future socket cannot inherit stale flags.
        self.0
            .durable_ctx_mut()
            .forget_tcp_taken_streams(sock.rep());
        types::HostTcpSocket::drop(&mut WasiSocketsView::sockets(self.0), sock)
    }
}

enum TcpSendMode {
    Live {
        socket_result_rx: oneshot::Receiver<Result<(), types::ErrorCode>>,
    },
    Replay {
        input_rx: oneshot::Receiver<Vec<u8>>,
        socket: Resource<TcpSocket>,
    },
}

struct TcpSocketSendTask<Ctx> {
    call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
    mode: TcpSendMode,
    result_tx: oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> TcpSocketSendTask<Ctx> {
    fn live(
        call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
        socket_result_rx: oneshot::Receiver<Result<(), types::ErrorCode>>,
        result_tx: oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            mode: TcpSendMode::Live { socket_result_rx },
            result_tx,
            _phantom: PhantomData,
        }
    }

    fn replay(
        call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
        input_rx: oneshot::Receiver<Vec<u8>>,
        socket: Resource<TcpSocket>,
        result_tx: oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            mode: TcpSendMode::Replay { input_rx, socket },
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for TcpSocketSendTask<Ctx>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let TcpSocketSendTask {
            call,
            mode,
            result_tx,
            ..
        } = self;
        let result = match mode {
            TcpSendMode::Live { socket_result_rx } => {
                complete_tcp_socket_send::<Ctx, U>(accessor, call, socket_result_rx).await
            }
            TcpSendMode::Replay { input_rx, socket } => {
                replay_tcp_socket_send::<Ctx, U>(accessor, call, input_rx, socket).await
            }
        };
        if !result_tx.is_closed() {
            let _ = result_tx.send(result);
        }
        Ok(())
    }
}

/// Live `send`: the guest bytes are already being forwarded to the socket; here
/// we only await the transmission result and persist it (result-only, no bytes
/// in the oplog).
async fn complete_tcp_socket_send<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
    socket_result_rx: oneshot::Receiver<Result<(), types::ErrorCode>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let socket_result = socket_result_rx
        .await
        .unwrap_or(Err(types::ErrorCode::ConnectionBroken));
    let response = call
        .complete_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            HostResponseP3SocketsTcpSend {
                result: serialize_tcp_stream_result(socket_result),
            },
        )
        .await
        .map_err(wasmtime::Error::from)?;
    Ok(deserialize_tcp_stream_result(response.result))
}

/// Replay `send`: drain the (re-produced) guest bytes. If the call already
/// completed, return the recorded result without re-sending. If the call is
/// incomplete (a crash mid-send), re-send the captured bytes and record the
/// result.
async fn replay_tcp_socket_send<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
    input_rx: oneshot::Receiver<Vec<u8>>,
    socket: Resource<TcpSocket>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let contents = input_rx.await.unwrap_or_default();

    match call
        .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
        .await
        .map_err(wasmtime::Error::from)?
    {
        CallReplayOutcome::Replayed(response) => Ok(deserialize_tcp_stream_result(response.result)),
        CallReplayOutcome::Incomplete(call) => {
            let result = send_captured_tcp_bytes::<Ctx, U>(accessor, socket, contents).await?;
            let response = call
                .complete_access(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3SocketsTcpSend {
                        result: serialize_tcp_stream_result(result),
                    },
                )
                .await
                .map_err(wasmtime::Error::from)?;
            Ok(deserialize_tcp_stream_result(response.result))
        }
    }
}

async fn send_captured_tcp_bytes<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    socket: Resource<TcpSocket>,
    contents: Vec<u8>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let stream = accessor.with(|mut store| StreamReader::new(&mut store, contents))?;
    let sockets = accessor.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
    let future =
        <WasiSockets as types::HostTcpSocketWithStore<U>>::send(&sockets, socket, stream).await?;
    let (result_tx, result_rx) = oneshot::channel();
    accessor.with(|mut store| future.pipe(&mut store, TcpSocketResultConsumer::new(result_tx)))?;
    Ok(result_rx
        .await
        .unwrap_or(Err(types::ErrorCode::ConnectionBroken)))
}

struct TcpSocketReceiveTask<Ctx> {
    socket: Resource<TcpSocket>,
    demand_rx: mpsc::UnboundedReceiver<TcpReceiveDemand>,
    result_tx: oneshot::Sender<TcpReceiveResolution>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> TcpSocketReceiveTask<Ctx> {
    fn new(
        socket: Resource<TcpSocket>,
        demand_rx: mpsc::UnboundedReceiver<TcpReceiveDemand>,
        result_tx: oneshot::Sender<TcpReceiveResolution>,
    ) -> Self {
        Self {
            socket,
            demand_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for TcpSocketReceiveTask<Ctx>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        run_tcp_socket_receive::<Ctx, U>(accessor, self.socket, self.demand_rx, self.result_tx)
            .await
    }
}

/// Fail the durable `receive` task loudly on a durability-machinery error (an
/// oplog read/write/socket-setup failure), as opposed to a normal socket error.
///
/// Mirrors the P3 HTTP `consume-body` fail-loud contract: a durability failure
/// must not be turned into a normal terminal (which would commit a completed
/// parent marker after an incomplete child chunk). The caller leaves the parent
/// without a terminal marker (it traps/abandons the parent handle so no
/// `Cancelled` is written), and the guest-facing result future is resolved with
/// a [`TcpReceiveResolution::Trap`] carrying the failing call scope's trap
/// context so it also fails loud with correct retry grouping. When `trap_context`
/// is `None` the sender is dropped, which still traps the result future loudly.
fn fail_tcp_receive_task(
    result_tx: oneshot::Sender<TcpReceiveResolution>,
    error: wasmtime::Error,
    trap_context: Option<DurableCallTrapContext>,
) -> wasmtime::Result<()> {
    match trap_context {
        Some(trap_context) => {
            let _ = result_tx.send(TcpReceiveResolution::Trap {
                message: "tcp receive durable persistence failed".to_string(),
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
/// batched durable scope (mirroring the P3 HTTP incoming-body stream):
///
/// * the parent `P3SocketsTypesTcpSocketReceive` call opens the batched scope and
///   is finalized last with a marker carrying the terminal `Result<(), error>`;
/// * every delivered chunk is persisted as a `P3SocketsTypesTcpSocketReceiveChunk`
///   child (`Data`) before its bytes are handed to the guest;
/// * a final `End` child terminates the recorded stream so replay knows when to
///   stop reading children.
///
/// Each child is produced in response to exactly one producer demand, so on
/// replay the same number of children are read back from the oplog and delivered
/// in the same order — no whole-stream buffering, bounded memory.
async fn run_tcp_socket_receive<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    socket: Resource<TcpSocket>,
    mut demand_rx: mpsc::UnboundedReceiver<TcpReceiveDemand>,
    result_tx: oneshot::Sender<TcpReceiveResolution>,
) -> wasmtime::Result<()>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    // Open the parent batched scope. Children nest under its begin index.
    let mut parent = match CallHandle::<P3SocketsTypesTcpSocketReceive, Cancellable>::start_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        HostRequestNoInput {},
        DurableFunctionType::WriteRemoteBatched(None),
    )
    .await
    {
        Ok(parent) => parent,
        Err(error) => return fail_tcp_receive_task(result_tx, wasmtime::Error::from(error), None),
    };
    let parent_begin = parent.begin_index();

    // Live upstream wiring: only touch the socket when the parent call is live.
    // The upstream stream is piped into a demand-gated consumer over a bounded
    // channel; the consumer forwards a chunk only after the task grants a permit,
    // so nothing is read into the Golem channel before a durable demand.
    let mut bytes_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
    let mut permit_tx: Option<mpsc::UnboundedSender<()>> = None;
    let mut socket_result_rx: Option<oneshot::Receiver<Result<(), types::ErrorCode>>> = None;
    if parent.is_live() {
        let sockets = accessor.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
        match <WasiSockets as types::HostTcpSocketWithStore<U>>::receive(&sockets, socket).await {
            Ok((stream, result_future)) => {
                let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>(1);
                let (demand_permit_tx, demand_permit_rx) = mpsc::unbounded_channel::<()>();
                let (inner_result_tx, inner_result_rx) = oneshot::channel();
                if let Err(error) = accessor.with(|mut store| -> wasmtime::Result<()> {
                    stream.pipe(
                        &mut store,
                        TcpReceiveForwardConsumer::new(PollSender::new(chunk_tx), demand_permit_rx),
                    )?;
                    result_future
                        .pipe(&mut store, TcpSocketResultConsumer::new(inner_result_tx))?;
                    Ok(())
                }) {
                    let trap_context = parent.trap_context();
                    return fail_tcp_receive_task(
                        result_tx,
                        wasmtime::Error::from_anyhow(parent.trap(error)),
                        Some(trap_context),
                    );
                }
                bytes_rx = Some(chunk_rx);
                permit_tx = Some(demand_permit_tx);
                socket_result_rx = Some(inner_result_rx);
            }
            Err(error) => {
                let trap_context = parent.trap_context();
                return fail_tcp_receive_task(
                    result_tx,
                    wasmtime::Error::from_anyhow(parent.trap(error)),
                    Some(trap_context),
                );
            }
        }
    }

    // The terminal, set on the live path; on replay it is taken from the parent
    // marker instead.
    let mut terminal: Result<(), types::ErrorCode> = Ok(());

    loop {
        let demand = demand_rx.recv().await;

        let child =
            match CallHandle::<P3SocketsTypesTcpSocketReceiveChunk, NotCancellable>::start_access(
                accessor,
                durable_worker_ctx::<Ctx, U>,
                HostRequestNoInput {},
                DurableFunctionType::WriteRemoteBatched(Some(parent_begin)),
            )
            .await
            {
                Ok(child) => child,
                Err(error) => {
                    let trap_context = parent.trap_context();
                    if let Some(reply_tx) = demand {
                        let _ = reply_tx.send(TcpReceiveReply::Failed {
                            message: error.to_string(),
                            trap_context,
                        });
                    }
                    return fail_tcp_receive_task(
                        result_tx,
                        wasmtime::Error::from_anyhow(parent.trap(error)),
                        Some(trap_context),
                    );
                }
            };

        // Produce the next item: replay the recorded child (replay) or read the
        // upstream socket and persist it (live).
        let produced = if !child.is_live() {
            match child
                .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
                .await
            {
                Ok(CallReplayOutcome::Replayed(response)) => match response.chunk {
                    SerializableP3TcpChunk::Data(bytes) => {
                        ProducedTcpChunk::Data(Bytes::from(bytes))
                    }
                    SerializableP3TcpChunk::End => ProducedTcpChunk::Terminal,
                },
                Ok(CallReplayOutcome::Incomplete(mut child)) => {
                    // A batched child is not re-executable: `replay_access`
                    // hard-errors on an incomplete `Start` rather than returning
                    // `Incomplete`, so this arm is not reachable in normal
                    // operation. Treat it defensively: abandon the live child
                    // handle (a trap is not a cancellation), then trap the parent.
                    child.abandon_for_trap();
                    let message =
                        "tcp receive chunk replay returned an unexpected incomplete child"
                            .to_string();
                    let trap_context = parent.trap_context();
                    if let Some(reply_tx) = demand {
                        let _ = reply_tx.send(TcpReceiveReply::Failed {
                            message: message.clone(),
                            trap_context,
                        });
                    }
                    return fail_tcp_receive_task(
                        result_tx,
                        wasmtime::Error::from_anyhow(parent.trap(anyhow::Error::msg(message))),
                        Some(trap_context),
                    );
                }
                Err(error) => {
                    let trap_context = parent.trap_context();
                    if let Some(reply_tx) = demand {
                        let _ = reply_tx.send(TcpReceiveReply::Failed {
                            message: error.to_string(),
                            trap_context,
                        });
                    }
                    return fail_tcp_receive_task(
                        result_tx,
                        wasmtime::Error::from_anyhow(parent.trap(error)),
                        Some(trap_context),
                    );
                }
            }
        } else {
            // When the producer is already gone (guest dropped the stream) we
            // terminate the recorded stream with an `End` child instead of reading
            // more of the socket — and we must not start a new socket read whose
            // persisted chunk could never be delivered.
            let producer_gone = demand
                .as_ref()
                .map(|reply_tx| reply_tx.is_closed())
                .unwrap_or(true);
            let frame = if producer_gone {
                TcpReceiveFrame::End(Ok(()))
            } else {
                let bytes_rx = bytes_rx
                    .as_mut()
                    .expect("live tcp receive has an upstream byte channel");
                // Grant the demand-gated consumer permission to forward exactly one
                // chunk. The consumer holds no other read-ahead, so this is the only
                // point at which a socket chunk can reach the Golem channel. A send
                // failure means the consumer is already gone (upstream EOF/teardown),
                // which surfaces below as a closed `bytes_rx`.
                if let Some(permit_tx) = permit_tx.as_ref() {
                    let _ = permit_tx.send(());
                }
                match bytes_rx.recv().await {
                    Some(bytes) => TcpReceiveFrame::Data(Bytes::from(bytes)),
                    None => {
                        let result = match socket_result_rx.take() {
                            Some(rx) => rx.await.unwrap_or(Err(types::ErrorCode::ConnectionBroken)),
                            None => Ok(()),
                        };
                        TcpReceiveFrame::End(result)
                    }
                }
            };

            let chunk = match &frame {
                TcpReceiveFrame::Data(bytes) => SerializableP3TcpChunk::Data(bytes.to_vec()),
                TcpReceiveFrame::End(_) => SerializableP3TcpChunk::End,
            };

            if let Err(error) = child
                .complete_access(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3SocketsTcpReceiveChunk { chunk },
                )
                .await
            {
                // The child `Start` is already persisted but its `End` failed: the
                // recorded chunk history is now incomplete. Fail loud rather than
                // committing a completed parent marker over it. `complete_access`
                // already finished the child handle without recording a `Cancelled`
                // and its error carries the child scope's trap context, so preserve
                // it; we only abandon the still-open parent so it is not dropped
                // unfinished (which would wrongly record a parent `Cancelled`).
                let trap_context = parent.trap_context();
                if let Some(reply_tx) = demand {
                    let _ = reply_tx.send(TcpReceiveReply::Failed {
                        message: error.to_string(),
                        trap_context,
                    });
                }
                parent.abandon_for_trap();
                return fail_tcp_receive_task(
                    result_tx,
                    wasmtime::Error::from(error),
                    Some(trap_context),
                );
            }

            match frame {
                TcpReceiveFrame::Data(bytes) => ProducedTcpChunk::Data(bytes),
                TcpReceiveFrame::End(result) => {
                    terminal = result;
                    ProducedTcpChunk::Terminal
                }
            }
        };

        // Deliver the produced item to the guest-facing stream. This is the single
        // point where chunks reach the guest, identically live and on replay, so
        // the count/order of delivered chunks always matches the count/order of
        // persisted children.
        match produced {
            ProducedTcpChunk::Data(bytes) => match demand {
                Some(reply_tx) => {
                    if reply_tx.send(TcpReceiveReply::Data(bytes)).is_err() {
                        // The chunk was persisted but the producer vanished before
                        // it could be delivered. The recorded stream would diverge
                        // on replay (where the chunk *would* be delivered), so fail
                        // loud instead of finalizing the parent with a clean
                        // terminal over an undelivered chunk.
                        let trap_context = parent.trap_context();
                        parent.abandon_for_trap();
                        return fail_tcp_receive_task(
                            result_tx,
                            wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                                anyhow::Error::msg(
                                    "tcp receive persisted a chunk that could not be delivered to \
                                     the guest stream",
                                ),
                                trap_context,
                            )),
                            Some(trap_context),
                        );
                    }
                }
                None => {
                    // A `Data` item is only ever produced in response to a demand,
                    // so a missing demand here is a protocol invariant violation
                    // rather than a clean stream end.
                    let trap_context = parent.trap_context();
                    parent.abandon_for_trap();
                    return fail_tcp_receive_task(
                        result_tx,
                        wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                            anyhow::Error::msg(
                                "tcp receive produced a chunk without a pending demand",
                            ),
                            trap_context,
                        )),
                        Some(trap_context),
                    );
                }
            },
            ProducedTcpChunk::Terminal => {
                if let Some(reply_tx) = demand {
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if reply_tx.send(TcpReceiveReply::End { ack: ack_tx }).is_ok() {
                        // Wait for the producer to observe the terminal (report EOF
                        // to the guest) before resolving the result / finalizing the
                        // parent.
                        let _ = ack_rx.await;
                    }
                }
                break;
            }
        }
    }

    // Finalize the parent with the terminal marker. Capture the parent scope's
    // trap context first so every finalize failure can tag the guest-facing
    // result trap for correct retry grouping.
    let parent_trap_context = parent.trap_context();
    let outcome = if parent.is_live() {
        match parent
            .complete_access(
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
                return fail_tcp_receive_task(
                    result_tx,
                    wasmtime::Error::from(error),
                    Some(parent_trap_context),
                );
            }
        }
    } else {
        match parent
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
        {
            Ok(CallReplayOutcome::Replayed(response)) => {
                deserialize_tcp_stream_result(response.result)
            }
            Ok(CallReplayOutcome::Incomplete(parent)) => match parent
                .complete_access(
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
                    return fail_tcp_receive_task(
                        result_tx,
                        wasmtime::Error::from(error),
                        Some(parent_trap_context),
                    );
                }
            },
            Err(error) => {
                return fail_tcp_receive_task(
                    result_tx,
                    wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                        anyhow::Error::from(error),
                        parent_trap_context,
                    )),
                    Some(parent_trap_context),
                );
            }
        }
    };

    let _ = result_tx.send(TcpReceiveResolution::Outcome(outcome));
    Ok(())
}

/// Durable one-shot acquisition of a TCP `send`/`receive` stream.
///
/// wasmtime's P3 TCP `send`/`receive` take the socket's send (resp. receive)
/// stream exactly once; a second call returns [`types::ErrorCode::InvalidState`].
/// The durable wrappers replay `send`/`receive` from the oplog and never invoke
/// the native host call on replay, so the native taken flag is not advanced
/// across a replay boundary. This helper records the acquisition outcome durably
/// and maintains a Golem-side shadow of the taken flag, so that a live call made
/// after replay returns the same result uninterrupted execution would.
///
/// On the live path the shadow check, the read-only connectivity probe and the
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
) -> wasmtime::Result<Result<(), types::ErrorCode>>
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
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
    Pair: HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseP3SocketsTcpAcquire>,
{
    // `ReadLocal`: the acquire is a local connectivity probe plus local shadow
    // bookkeeping with no external side effect, so it is safely re-executable on
    // an incomplete replay and must never open a non-idempotent remote-write
    // scope. The actual byte transmission stays a `WriteRemoteBatched` call. The
    // recorded outcome is still persisted and replayed for determinism, exactly
    // like the clock `ReadLocal` calls.
    let response = run_read_access::<_, _, Ctx, Pair, _, _>(
        accessor,
        HostRequestNoInput {},
        DurableFunctionType::ReadLocal,
        || async {
            accessor.with(
                |mut access| -> wasmtime::Result<HostResponseP3SocketsTcpAcquire> {
                    // Read-only connectivity probe: `remote-address` succeeds only when
                    // the socket is connected, mirroring the precondition the native
                    // `take_*_stream` checks (modulo the taken flag, tracked below).
                    let probe = {
                        let mut view = wasi_sockets_view::<Ctx, U>(access.data_mut());
                        types::HostTcpSocket::get_remote_address(
                            &mut view,
                            Resource::new_borrow(socket_rep),
                        )
                    };
                    let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                    let result = if ctx.is_tcp_stream_taken(socket_rep, direction) {
                        Err(SerializableP3SocketErrorCode::from(
                            types::ErrorCode::InvalidState,
                        ))
                    } else {
                        match probe {
                            Ok(_) => {
                                ctx.mark_tcp_stream_taken(socket_rep, direction);
                                Ok(())
                            }
                            Err(error) => Err(serialize_socket_error(error)?),
                        }
                    };
                    Ok(HostResponseP3SocketsTcpAcquire { result })
                },
            )
        },
    )
    .await?;

    // Reapply the recorded outcome to the shadow so replay rehydrates the taken
    // flag before any later live call runs (idempotent on the live path, where
    // the closure above already set it).
    if response.result.is_ok() {
        accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut())
                .mark_tcp_stream_taken(socket_rep, direction);
        });
    }

    Ok(deserialize_tcp_stream_result(response.result))
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostTcpSocketWithStore<U> for DurableP3<Ctx> {
    async fn connect(
        store: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "sockets::types::tcp-socket",
                "connect",
            )
        });
        let store = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostTcpSocketWithStore<U>>::connect(&store, socket, remote_address)
            .await
    }

    fn listen(
        mut store: Access<U, Self>,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<StreamReader<Resource<TcpSocket>>> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "sockets::types::tcp-socket",
            "listen",
        );
        let store =
            Access::<U, WasiSockets>::new(store.as_context_mut(), wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostTcpSocketWithStore<U>>::listen(store, socket)
    }

    async fn send(
        accessor: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
        mut data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        // One-shot acquire of the send stream. On a second call (or when the
        // socket is not connected) this returns an error, which we surface
        // without invoking the native send, mirroring the native `take_send_stream`
        // contract while keeping replay deterministic.
        if let Err(error) =
            tcp_acquire_stream::<Ctx, U>(accessor, socket.rep(), TcpSocketStreamDirection::Send)
                .await?
        {
            return accessor.with(|mut store| {
                data.close(&mut store)?;
                FutureReader::new(&mut store, async move { wasmtime::error::Ok(Err(error)) })
            });
        }

        let call = CallHandle::<P3SocketsTypesTcpSocketSend, Cancellable>::start_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            HostRequestNoInput {},
            DurableFunctionType::WriteRemoteBatched(None),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        let (result_tx, result_rx) = oneshot::channel();

        if call.is_live() {
            // Forward the guest's outgoing bytes straight to the socket; only the
            // transmission result is persisted (no bytes in the oplog).
            let sockets = accessor.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
            let socket_result_future =
                <WasiSockets as types::HostTcpSocketWithStore<U>>::send(&sockets, socket, data)
                    .await?;
            let (socket_result_tx, socket_result_rx) = oneshot::channel();
            accessor.with(|mut store| {
                socket_result_future
                    .pipe(&mut store, TcpSocketResultConsumer::new(socket_result_tx))?;
                store.spawn(TcpSocketSendTask::<Ctx>::live(
                    call,
                    socket_result_rx,
                    result_tx,
                ));
                FutureReader::new(&mut store, wait_tcp_task_result(result_rx))
            })
        } else {
            // Replay: capture the re-produced guest bytes so an incomplete call can
            // re-send them; a completed call returns its recorded result instead.
            let (input_tx, input_rx) = oneshot::channel();
            accessor.with(|mut store| {
                data.pipe(&mut store, TcpSendCaptureConsumer::new(input_tx))?;
                store.spawn(TcpSocketSendTask::<Ctx>::replay(
                    call, input_rx, socket, result_tx,
                ));
                FutureReader::new(&mut store, wait_tcp_task_result(result_rx))
            })
        }
    }

    async fn receive(
        accessor: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        // One-shot acquire of the receive stream. On a second call (or when the
        // socket is not connected) this returns an error, surfaced as an empty
        // stream plus an immediately-failing result future, mirroring the native
        // `take_receive_stream` contract while keeping replay deterministic.
        if let Err(error) =
            tcp_acquire_stream::<Ctx, U>(accessor, socket.rep(), TcpSocketStreamDirection::Receive)
                .await?
        {
            return accessor.with(|mut store| {
                Ok((
                    StreamReader::new(&mut store, std::iter::empty::<u8>())?,
                    FutureReader::new(&mut store, async move { wasmtime::error::Ok(Err(error)) })?,
                ))
            });
        }

        let (demand_tx, demand_rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();

        // Build both guest-facing handles before spawning the durable task. The
        // task appends the `receive` `Start`; the guest cannot poll either handle
        // until this host call returns, so spawning first would risk committing a
        // `Start` with no terminal if a later handle construction fails.
        accessor.with(|mut store| {
            let mut stream =
                StreamReader::new(&mut store, DurableTcpReceiveProducer::new(demand_tx))?;
            let future = match FutureReader::new(&mut store, wait_tcp_receive_result(result_rx)) {
                Ok(future) => future,
                Err(error) => {
                    let _ = stream.close(store.as_context_mut());
                    return Err(error);
                }
            };
            store.spawn(TcpSocketReceiveTask::<Ctx>::new(
                socket, demand_rx, result_tx,
            ));
            Ok((stream, future))
        })
    }
}

impl<Ctx: WorkerCtx> types::HostUdpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<UdpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "bind");
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostUdpSocket::bind(&mut view, socket, local_address).await
    }

    async fn connect(
        &mut self,
        socket: Resource<UdpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "connect");
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostUdpSocket::connect(&mut view, socket, remote_address).await
    }

    fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<UdpSocket>> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "create");
        types::HostUdpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)
    }

    fn disconnect(&mut self, socket: Resource<UdpSocket>) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "disconnect");
        types::HostUdpSocket::disconnect(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_local_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-local-address");
        types::HostUdpSocket::get_local_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-remote-address");
        types::HostUdpSocket::get_remote_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_address_family(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<types::IpAddressFamily> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-address-family");
        types::HostUdpSocket::get_address_family(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_unicast_hop_limit(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u8> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-unicast-hop-limit",
        );
        types::HostUdpSocket::get_unicast_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_unicast_hop_limit(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u8,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "set-unicast-hop-limit",
        );
        types::HostUdpSocket::set_unicast_hop_limit(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_receive_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-receive-buffer-size",
        );
        types::HostUdpSocket::get_receive_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "set-receive-buffer-size",
        );
        types::HostUdpSocket::set_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_send_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-send-buffer-size",
        );
        types::HostUdpSocket::get_send_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "set-send-buffer-size",
        );
        types::HostUdpSocket::set_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn drop(&mut self, sock: Resource<UdpSocket>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "drop");
        types::HostUdpSocket::drop(&mut WasiSocketsView::sockets(self.0), sock)
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostUdpSocketWithStore<U> for DurableP3<Ctx> {
    async fn send(
        store: &Accessor<U, Self>,
        socket: Resource<UdpSocket>,
        data: Vec<u8>,
        remote_address: Option<types::IpSocketAddress>,
    ) -> SocketResult<()> {
        let response = run_read_access::<_, _, Ctx, P3SocketsTypesUdpSocketSend, _, _>(
            store,
            HostRequestP3SocketsUdpSend {
                data: data.clone(),
                remote_address: remote_address.map(Into::into),
            },
            DurableFunctionType::WriteRemoteBatched(None),
            || async {
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let result = <WasiSockets as types::HostUdpSocketWithStore<U>>::send(
                    &sockets,
                    socket,
                    data,
                    remote_address,
                )
                .await;

                Ok(HostResponseP3SocketsUdpSend {
                    result: match result {
                        Ok(()) => Ok(()),
                        Err(error) => Err(serialize_socket_error(error)?),
                    },
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;

        match response.result {
            Ok(()) => Ok(()),
            Err(error) => Err(types::ErrorCode::from(error).into()),
        }
    }

    async fn receive(
        store: &Accessor<U, Self>,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<(Vec<u8>, types::IpSocketAddress)> {
        let response = run_read_access::<_, _, Ctx, P3SocketsTypesUdpSocketReceive, _, _>(
            store,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
            || async {
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let result =
                    <WasiSockets as types::HostUdpSocketWithStore<U>>::receive(&sockets, socket)
                        .await;

                Ok(HostResponseP3SocketsUdpReceive {
                    result: match result {
                        Ok((data, remote_address)) => Ok(SerializableP3UdpDatagram {
                            data,
                            remote_address: remote_address.into(),
                        }),
                        Err(error) => Err(serialize_socket_error(error)?),
                    },
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;

        match response.result {
            Ok(SerializableP3UdpDatagram {
                data,
                remote_address,
            }) => Ok((data, remote_address.into())),
            Err(error) => Err(types::ErrorCode::from(error).into()),
        }
    }
}

impl<Ctx: WorkerCtx> ip_name_lookup::Host for DurableP3View<'_, Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> ip_name_lookup::HostWithStore<U> for DurableP3<Ctx> {
    async fn resolve_addresses(
        store: &Accessor<U, Self>,
        name: String,
    ) -> wasmtime::Result<Result<Vec<types::IpAddress>, ip_name_lookup::ErrorCode>> {
        let response = run_read_access::<_, _, Ctx, P3SocketsIpNameLookupResolveAddresses, _, _>(
            store,
            HostRequestP3SocketsResolveName { name: name.clone() },
            DurableFunctionType::ReadRemote,
            || async {
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let result = <WasiSockets as ip_name_lookup::HostWithStore<U>>::resolve_addresses(
                    &sockets,
                    name.clone(),
                )
                .await?;

                Ok(HostResponseP3SocketsResolveName {
                    result: result
                        .map(SerializableP3IpAddresses::from)
                        .map_err(Into::into),
                })
            },
        )
        .await?;

        Ok(response
            .result
            .map(Vec::<types::IpAddress>::from)
            .map_err(Into::into))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::types::{
        SerializableP3IpAddress, SerializableP3IpNameLookupError, SerializableP3IpSocketAddress,
        SerializableP3SocketErrorCode,
    };
    use golem_common::model::oplog::{HostPayloadPair, HostRequest, HostResponse};
    use test_r::test;
    use test_r::timeout;
    use wasmtime::{Config, Engine, Store};

    #[test]
    fn p3_ip_name_lookup_address_payload_mapping_roundtrips() {
        let ipv4 = types::IpAddress::Ipv4((127, 0, 0, 1));
        let ipv6 = types::IpAddress::Ipv6((0, 0, 0, 0, 0, 0, 0, 1));

        let serialized = SerializableP3IpAddresses::from(vec![ipv4, ipv6]);
        let replayed = Vec::<types::IpAddress>::from(serialized);

        assert_p3_ip_address_eq(replayed[0], ipv4);
        assert_p3_ip_address_eq(replayed[1], ipv6);
    }

    #[test]
    fn p3_ip_name_lookup_error_payload_mapping_roundtrips_named_codes() {
        assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::AccessDenied);
        assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::InvalidArgument);
        assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::NameUnresolvable);
        assert_p3_ip_name_lookup_error_roundtrip(
            ip_name_lookup::ErrorCode::TemporaryResolverFailure,
        );
        assert_p3_ip_name_lookup_error_roundtrip(
            ip_name_lookup::ErrorCode::PermanentResolverFailure,
        );
    }

    #[test]
    fn p3_ip_name_lookup_other_error_payload_mapping_preserves_message() {
        let error = ip_name_lookup::ErrorCode::Other(Some("resolver said no".to_string()));
        let serialized = SerializableP3IpNameLookupError::from(error);
        let replayed = ip_name_lookup::ErrorCode::from(serialized);

        match replayed {
            ip_name_lookup::ErrorCode::Other(Some(message)) => {
                assert_eq!(message, "resolver said no")
            }
            other => panic!("unexpected replayed error: {other:?}"),
        }
    }

    #[test]
    fn p3_socket_error_payload_mapping_roundtrips_named_codes() {
        assert_p3_socket_error_roundtrip(types::ErrorCode::AccessDenied);
        assert_p3_socket_error_roundtrip(types::ErrorCode::NotSupported);
        assert_p3_socket_error_roundtrip(types::ErrorCode::InvalidArgument);
        assert_p3_socket_error_roundtrip(types::ErrorCode::OutOfMemory);
        assert_p3_socket_error_roundtrip(types::ErrorCode::Timeout);
        assert_p3_socket_error_roundtrip(types::ErrorCode::InvalidState);
        assert_p3_socket_error_roundtrip(types::ErrorCode::AddressNotBindable);
        assert_p3_socket_error_roundtrip(types::ErrorCode::AddressInUse);
        assert_p3_socket_error_roundtrip(types::ErrorCode::RemoteUnreachable);
        assert_p3_socket_error_roundtrip(types::ErrorCode::ConnectionRefused);
        assert_p3_socket_error_roundtrip(types::ErrorCode::ConnectionBroken);
        assert_p3_socket_error_roundtrip(types::ErrorCode::ConnectionReset);
        assert_p3_socket_error_roundtrip(types::ErrorCode::ConnectionAborted);
        assert_p3_socket_error_roundtrip(types::ErrorCode::DatagramTooLarge);
    }

    #[test]
    fn p3_socket_other_error_payload_mapping_preserves_message() {
        let error = types::ErrorCode::Other(Some("socket said no".to_string()));
        let serialized = SerializableP3SocketErrorCode::from(error);
        let replayed = types::ErrorCode::from(serialized);

        match replayed {
            types::ErrorCode::Other(Some(message)) => assert_eq!(message, "socket said no"),
            other => panic!("unexpected replayed error: {other:?}"),
        }
    }

    #[test]
    fn p3_udp_socket_address_payload_mapping_roundtrips_ipv4() {
        let address = types::IpSocketAddress::Ipv4(types::Ipv4SocketAddress {
            port: 1234,
            address: (127, 0, 0, 1),
        });

        let serialized = SerializableP3IpSocketAddress::from(address);
        let replayed = types::IpSocketAddress::from(serialized);

        assert_p3_socket_address_eq(replayed, address);
    }

    #[test]
    fn p3_udp_socket_address_payload_mapping_roundtrips_ipv6() {
        let address = types::IpSocketAddress::Ipv6(types::Ipv6SocketAddress {
            port: 1234,
            flow_info: 56,
            address: (0, 1, 2, 3, 4, 5, 6, 7),
            scope_id: 78,
        });

        let serialized = SerializableP3IpSocketAddress::from(address);
        let replayed = types::IpSocketAddress::from(serialized);

        assert_p3_socket_address_eq(replayed, address);
    }

    #[test]
    fn p3_udp_socket_host_payload_pair_roundtrips() {
        let remote_address = SerializableP3IpSocketAddress {
            address: SerializableP3IpAddress::IPv4 {
                address: [127, 0, 0, 1],
            },
            port: 1234,
            flow_info: None,
            scope_id: None,
        };

        assert_host_payload_pair_roundtrip::<P3SocketsTypesUdpSocketSend>(
            HostRequestP3SocketsUdpSend {
                data: b"outgoing udp bytes".to_vec(),
                remote_address: Some(remote_address.clone()),
            },
            HostResponseP3SocketsUdpSend { result: Ok(()) },
        );
        assert_host_payload_pair_roundtrip::<P3SocketsTypesUdpSocketReceive>(
            HostRequestNoInput {},
            HostResponseP3SocketsUdpReceive {
                result: Ok(SerializableP3UdpDatagram {
                    data: b"incoming udp bytes".to_vec(),
                    remote_address,
                }),
            },
        );
    }

    /// A finite upstream stream piped into [`TcpReceiveForwardConsumer`] must
    /// forward its bytes and then close the bounded channel once the upstream
    /// reaches EOF, so the durable receive task observes `bytes_rx.recv() ==
    /// None` and can finalize the terminal instead of hanging.
    ///
    /// EOF closes the channel because the wasmtime host-to-host driver drops the
    /// consumer (and thus its [`PollSender`]) once the upstream producer reports
    /// `StreamResult::Dropped`. That transfer only runs while the store's event
    /// loop is driven, so the channel is drained from inside the driven
    /// `run_concurrent` closure (the event loop polls the closure and the pushed
    /// transfer future together). Draining outside the event loop — e.g. after a
    /// non-draining `run_concurrent` returns — would never run the transfer.
    ///
    /// Because the consumer is demand-gated, the driver grants one permit before
    /// each read (mirroring the durable receive task), so a chunk is forwarded
    /// only when demanded and EOF still closes the channel.
    #[test]
    #[timeout("10s")]
    async fn tcp_receive_forward_consumer_closes_channel_after_source_eof() {
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());

        let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>(1);
        let (permit_tx, permit_rx) = mpsc::unbounded_channel::<()>();

        let chunks = store
            .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<Vec<u8>>> {
                let mut chunk_rx = chunk_rx;
                accessor.with(|mut store| {
                    let stream = StreamReader::new(&mut store, b"abc".to_vec())?;
                    stream.pipe(
                        &mut store,
                        TcpReceiveForwardConsumer::new(PollSender::new(chunk_tx), permit_rx),
                    )
                })?;

                let mut chunks = Vec::new();
                loop {
                    // Grant one permit per demand, exactly like the durable task.
                    let _ = permit_tx.send(());
                    match chunk_rx.recv().await {
                        Some(chunk) => chunks.push(chunk),
                        None => break,
                    }
                }
                Ok(chunks)
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(chunks, vec![b"abc".to_vec()]);
    }

    #[test]
    #[timeout("10s")]
    async fn tcp_receive_forward_consumer_does_not_prefetch_before_durable_demand() {
        let mut config = Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());

        let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>(1);
        let (permit_tx, permit_rx) = mpsc::unbounded_channel::<()>();

        store
            .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
                let mut chunk_rx = chunk_rx;
                // Keep the permit sender alive but never grant a permit: the
                // consumer must not forward (prefetch) any chunk before a demand.
                // (Dropping it would close `permit_rx`, making the consumer tear
                // the channel down and `try_recv` return `Disconnected`.)
                let _permit_tx = permit_tx;
                accessor.with(|mut store| {
                    let stream = StreamReader::new(&mut store, b"prefetched".to_vec())?;
                    stream.pipe(
                        &mut store,
                        TcpReceiveForwardConsumer::new(PollSender::new(chunk_tx), permit_rx),
                    )
                })?;

                tokio::time::sleep(std::time::Duration::from_millis(25)).await;

                assert_eq!(chunk_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty));
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();
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
        let function_name_roundtrip: golem_common::model::oplog::host_functions::HostFunctionName =
            desert_rust::deserialize(&function_name_bytes).unwrap();
        assert_eq!(function_name_roundtrip, Pair::HOST_FUNCTION_NAME);
    }

    fn assert_p3_ip_name_lookup_error_roundtrip(error: ip_name_lookup::ErrorCode) {
        let expected = format!("{error:?}");
        let serialized = SerializableP3IpNameLookupError::from(error);
        let replayed = ip_name_lookup::ErrorCode::from(serialized);
        assert_eq!(format!("{replayed:?}"), expected);
    }

    fn assert_p3_socket_error_roundtrip(error: types::ErrorCode) {
        let expected = format!("{error:?}");
        let serialized = SerializableP3SocketErrorCode::from(error);
        let replayed = types::ErrorCode::from(serialized);
        assert_eq!(format!("{replayed:?}"), expected);
    }

    fn assert_p3_ip_address_eq(actual: types::IpAddress, expected: types::IpAddress) {
        match (actual, expected) {
            (types::IpAddress::Ipv4(actual), types::IpAddress::Ipv4(expected)) => {
                assert_eq!(actual, expected)
            }
            (types::IpAddress::Ipv6(actual), types::IpAddress::Ipv6(expected)) => {
                assert_eq!(actual, expected)
            }
            (actual, expected) => panic!("IP address mismatch: {actual:?} != {expected:?}"),
        }
    }

    fn assert_p3_socket_address_eq(
        actual: types::IpSocketAddress,
        expected: types::IpSocketAddress,
    ) {
        match (actual, expected) {
            (types::IpSocketAddress::Ipv4(actual), types::IpSocketAddress::Ipv4(expected)) => {
                assert_eq!(actual.port, expected.port);
                assert_eq!(actual.address, expected.address);
            }
            (types::IpSocketAddress::Ipv6(actual), types::IpSocketAddress::Ipv6(expected)) => {
                assert_eq!(actual.port, expected.port);
                assert_eq!(actual.flow_info, expected.flow_info);
                assert_eq!(actual.address, expected.address);
                assert_eq!(actual.scope_id, expected.scope_id);
            }
            (actual, expected) => {
                panic!("IP socket address mismatch: {actual:?} != {expected:?}")
            }
        }
    }
}

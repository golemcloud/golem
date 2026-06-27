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
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, run_read_access, wasi_sockets_view,
};
use crate::workerctx::WorkerCtx;
use bytes::BytesMut;
use golem_common::model::oplog::host_functions::{
    P3SocketsIpNameLookupResolveAddresses, P3SocketsTypesTcpSocketReceive,
    P3SocketsTypesTcpSocketSend, P3SocketsTypesUdpSocketReceive, P3SocketsTypesUdpSocketSend,
};
use golem_common::model::oplog::types::{
    SerializableP3IpAddresses, SerializableP3SocketErrorCode, SerializableP3UdpDatagram,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestP3SocketsResolveName,
    HostRequestP3SocketsUdpSend, HostResponseP3SocketsResolveName, HostResponseP3SocketsTcpStream,
    HostResponseP3SocketsUdpReceive, HostResponseP3SocketsUdpSend,
};
use wasmtime::AsContextMut;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureConsumer, FutureReader, Resource, Source,
    StreamConsumer, StreamProducer, StreamReader, StreamResult,
};
use wasmtime_wasi::p3::bindings::sockets::{ip_name_lookup, types};
use wasmtime_wasi::p3::sockets::{SocketError, SocketResult};
use wasmtime_wasi::sockets::{TcpSocket, UdpSocket, WasiSockets, WasiSocketsView};

const TCP_STREAM_BUFFER_CAPACITY: usize = 8192;

fn serialize_udp_socket_error(
    error: SocketError,
) -> wasmtime::Result<SerializableP3SocketErrorCode> {
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

#[derive(Clone)]
struct CapturedTcpStream {
    contents: Vec<u8>,
    result: Result<(), types::ErrorCode>,
}

enum TcpStreamMode {
    Replayed(CapturedTcpStream),
    Live(tokio::sync::mpsc::UnboundedReceiver<TcpStreamEvent>),
    Error(String),
}

enum TcpStreamEvent {
    Bytes(Vec<u8>),
    End(Result<(), types::ErrorCode>),
}

struct TcpSocketInputStreamConsumer {
    chunk_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    result_tx: Option<tokio::sync::oneshot::Sender<CapturedTcpStream>>,
    contents: Vec<u8>,
    result: Result<(), types::ErrorCode>,
}

impl TcpSocketInputStreamConsumer {
    fn new(
        chunk_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
        result_tx: tokio::sync::oneshot::Sender<CapturedTcpStream>,
    ) -> Self {
        Self {
            chunk_tx,
            result_tx: Some(result_tx),
            contents: Vec::new(),
            result: Ok(()),
        }
    }

    fn close(&mut self) {
        self.chunk_tx.take();
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(CapturedTcpStream {
                contents: std::mem::take(&mut self.contents),
                result: self.result.clone(),
            });
        }
    }
}

impl<D> StreamConsumer<D> for TcpSocketInputStreamConsumer {
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

        if let Some(chunk_tx) = &self.chunk_tx
            && chunk_tx.send(chunk).is_err()
        {
            self.result = Err(types::ErrorCode::ConnectionBroken);
            self.close();
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for TcpSocketInputStreamConsumer {
    fn drop(&mut self) {
        self.close();
    }
}

struct TcpSocketForwardStreamProducer {
    chunks_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    current: Cursor<BytesMut>,
}

impl TcpSocketForwardStreamProducer {
    fn new(chunks_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Self {
        Self {
            chunks_rx,
            current: Cursor::new(BytesMut::new()),
        }
    }
}

impl<D> StreamProducer<D> for TcpSocketForwardStreamProducer {
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

        loop {
            let bytes = self.current.get_ref();
            let position = self.current.position() as usize;
            if position < bytes.len() {
                let mut dst = dst.as_direct(store, TCP_STREAM_BUFFER_CAPACITY);
                let remaining = &bytes[position..];
                let n = remaining.len().min(dst.remaining().len());
                dst.remaining()[..n].copy_from_slice(&remaining[..n]);
                dst.mark_written(n);
                self.current.set_position((position + n) as u64);
                return Poll::Ready(Ok(StreamResult::Completed));
            }

            match Pin::new(&mut self.chunks_rx).poll_recv(cx) {
                Poll::Ready(Some(chunk)) => {
                    self.current = Cursor::new(BytesMut::from(chunk.as_slice()));
                }
                Poll::Ready(None) => return Poll::Ready(Ok(StreamResult::Dropped)),
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct TcpSocketOutputStreamProducer {
    state: TcpSocketOutputStreamProducerState,
}

enum TcpSocketOutputStreamProducerState {
    Awaiting {
        mode_rx: tokio::sync::oneshot::Receiver<TcpStreamMode>,
        result_tx: Option<tokio::sync::oneshot::Sender<CapturedTcpStream>>,
        contents: Vec<u8>,
    },
    Streaming {
        events_rx: tokio::sync::mpsc::UnboundedReceiver<TcpStreamEvent>,
        current: Cursor<BytesMut>,
        result_tx: Option<tokio::sync::oneshot::Sender<CapturedTcpStream>>,
        contents: Vec<u8>,
    },
    Replaying {
        current: Cursor<BytesMut>,
        result: Result<(), types::ErrorCode>,
        result_tx: Option<tokio::sync::oneshot::Sender<CapturedTcpStream>>,
        contents: Vec<u8>,
    },
    Done,
}

impl TcpSocketOutputStreamProducer {
    fn live(
        events_rx: tokio::sync::mpsc::UnboundedReceiver<TcpStreamEvent>,
        result_tx: tokio::sync::oneshot::Sender<CapturedTcpStream>,
    ) -> Self {
        Self {
            state: TcpSocketOutputStreamProducerState::Streaming {
                events_rx,
                current: Cursor::new(BytesMut::new()),
                result_tx: Some(result_tx),
                contents: Vec::new(),
            },
        }
    }

    fn deferred(
        mode_rx: tokio::sync::oneshot::Receiver<TcpStreamMode>,
        result_tx: tokio::sync::oneshot::Sender<CapturedTcpStream>,
    ) -> Self {
        Self {
            state: TcpSocketOutputStreamProducerState::Awaiting {
                mode_rx,
                result_tx: Some(result_tx),
                contents: Vec::new(),
            },
        }
    }

    fn close(
        result_tx: &mut Option<tokio::sync::oneshot::Sender<CapturedTcpStream>>,
        contents: &mut Vec<u8>,
        result: Result<(), types::ErrorCode>,
    ) {
        if let Some(result_tx) = result_tx.take() {
            let _ = result_tx.send(CapturedTcpStream {
                contents: std::mem::take(contents),
                result,
            });
        }
    }
}

impl<D> StreamProducer<D> for TcpSocketOutputStreamProducer {
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

        loop {
            match &mut self.state {
                TcpSocketOutputStreamProducerState::Awaiting {
                    mode_rx,
                    result_tx,
                    contents,
                } => match Pin::new(mode_rx).poll(cx) {
                    Poll::Ready(Ok(TcpStreamMode::Replayed(captured))) => {
                        let result_tx = result_tx
                            .take()
                            .expect("tcp stream result sender available for replay");
                        let contents = std::mem::take(contents);
                        self.state = TcpSocketOutputStreamProducerState::Replaying {
                            current: Cursor::new(BytesMut::from(captured.contents.as_slice())),
                            result: captured.result,
                            result_tx: Some(result_tx),
                            contents,
                        };
                    }
                    Poll::Ready(Ok(TcpStreamMode::Live(events_rx))) => {
                        let result_tx = result_tx
                            .take()
                            .expect("tcp stream result sender available for live stream");
                        let contents = std::mem::take(contents);
                        self.state = TcpSocketOutputStreamProducerState::Streaming {
                            events_rx,
                            current: Cursor::new(BytesMut::new()),
                            result_tx: Some(result_tx),
                            contents,
                        };
                    }
                    Poll::Ready(Ok(TcpStreamMode::Error(error))) => {
                        Self::close(result_tx, contents, Err(types::ErrorCode::ConnectionBroken));
                        self.state = TcpSocketOutputStreamProducerState::Done;
                        return Poll::Ready(Err(wasmtime::Error::msg(error)));
                    }
                    Poll::Ready(Err(_)) => {
                        Self::close(result_tx, contents, Err(types::ErrorCode::ConnectionBroken));
                        self.state = TcpSocketOutputStreamProducerState::Done;
                        return Poll::Ready(Err(wasmtime::Error::msg("tcp stream task dropped")));
                    }
                    Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                    Poll::Pending => return Poll::Pending,
                },
                TcpSocketOutputStreamProducerState::Streaming {
                    events_rx,
                    current,
                    result_tx,
                    contents,
                } => {
                    let bytes = current.get_ref();
                    let position = current.position() as usize;
                    if position < bytes.len() {
                        let mut dst = dst.as_direct(store, TCP_STREAM_BUFFER_CAPACITY);
                        let remaining = &bytes[position..];
                        let n = remaining.len().min(dst.remaining().len());
                        dst.remaining()[..n].copy_from_slice(&remaining[..n]);
                        dst.mark_written(n);
                        contents.extend_from_slice(&remaining[..n]);
                        current.set_position((position + n) as u64);
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }

                    match Pin::new(events_rx).poll_recv(cx) {
                        Poll::Ready(Some(TcpStreamEvent::Bytes(chunk))) => {
                            *current = Cursor::new(BytesMut::from(chunk.as_slice()));
                        }
                        Poll::Ready(Some(TcpStreamEvent::End(result))) => {
                            Self::close(result_tx, contents, result);
                            self.state = TcpSocketOutputStreamProducerState::Done;
                            return Poll::Ready(Ok(StreamResult::Dropped));
                        }
                        Poll::Ready(None) => {
                            Self::close(result_tx, contents, Ok(()));
                            self.state = TcpSocketOutputStreamProducerState::Done;
                            return Poll::Ready(Ok(StreamResult::Dropped));
                        }
                        Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                TcpSocketOutputStreamProducerState::Replaying {
                    current,
                    result,
                    result_tx,
                    contents,
                } => {
                    let bytes = current.get_ref();
                    let position = current.position() as usize;
                    if position >= bytes.len() {
                        Self::close(result_tx, contents, result.clone());
                        self.state = TcpSocketOutputStreamProducerState::Done;
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }

                    let mut dst = dst.as_direct(store, TCP_STREAM_BUFFER_CAPACITY);
                    let remaining = &bytes[position..];
                    let n = remaining.len().min(dst.remaining().len());
                    dst.remaining()[..n].copy_from_slice(&remaining[..n]);
                    dst.mark_written(n);
                    contents.extend_from_slice(&remaining[..n]);
                    current.set_position((position + n) as u64);
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                TcpSocketOutputStreamProducerState::Done => {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }
    }
}

impl Drop for TcpSocketOutputStreamProducer {
    fn drop(&mut self) {
        match &mut self.state {
            TcpSocketOutputStreamProducerState::Awaiting {
                result_tx,
                contents,
                ..
            }
            | TcpSocketOutputStreamProducerState::Streaming {
                result_tx,
                contents,
                ..
            }
            | TcpSocketOutputStreamProducerState::Replaying {
                result_tx,
                contents,
                ..
            } => Self::close(result_tx, contents, Ok(())),
            TcpSocketOutputStreamProducerState::Done => {}
        }
    }
}

struct TcpSocketReceiveForwardingConsumer {
    tx: tokio::sync::mpsc::UnboundedSender<TcpStreamEvent>,
}

impl TcpSocketReceiveForwardingConsumer {
    fn new(tx: tokio::sync::mpsc::UnboundedSender<TcpStreamEvent>) -> Self {
        Self { tx }
    }
}

impl<D> StreamConsumer<D> for TcpSocketReceiveForwardingConsumer {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
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
        if self.tx.send(TcpStreamEvent::Bytes(chunk)).is_err() {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        Poll::Ready(Ok(StreamResult::Completed))
    }
}

struct TcpSocketFutureResultConsumer {
    tx: Option<tokio::sync::mpsc::UnboundedSender<TcpStreamEvent>>,
    result_tx: Option<tokio::sync::oneshot::Sender<Result<(), types::ErrorCode>>>,
}

impl TcpSocketFutureResultConsumer {
    fn event(tx: tokio::sync::mpsc::UnboundedSender<TcpStreamEvent>) -> Self {
        Self {
            tx: Some(tx),
            result_tx: None,
        }
    }

    fn result(result_tx: tokio::sync::oneshot::Sender<Result<(), types::ErrorCode>>) -> Self {
        Self {
            tx: None,
            result_tx: Some(result_tx),
        }
    }

    fn send(&mut self, result: Result<(), types::ErrorCode>) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(TcpStreamEvent::End(result));
        } else if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(result);
        }
    }
}

impl<D> FutureConsumer<D> for TcpSocketFutureResultConsumer {
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

async fn wait_tcp_socket_task_result(
    result_rx: tokio::sync::oneshot::Receiver<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    result_rx
        .await
        .unwrap_or_else(|_| Err(wasmtime::Error::msg("tcp socket task dropped")))
}

async fn wait_tcp_socket_future_result(
    result_rx: tokio::sync::oneshot::Receiver<Result<(), types::ErrorCode>>,
) -> Result<(), types::ErrorCode> {
    result_rx
        .await
        .unwrap_or(Err(types::ErrorCode::ConnectionBroken))
}

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: SocketError) -> wasmtime::Result<types::ErrorCode> {
        types::Host::convert_error_code(&mut WasiSocketsView::sockets(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostTcpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<TcpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostTcpSocket::bind(&mut view, socket, local_address).await
    }

    fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<TcpSocket>> {
        types::HostTcpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)
    }

    fn get_local_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        types::HostTcpSocket::get_local_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        types::HostTcpSocket::get_remote_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_is_listening(&mut self, socket: Resource<TcpSocket>) -> wasmtime::Result<bool> {
        types::HostTcpSocket::get_is_listening(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_address_family(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<types::IpAddressFamily> {
        types::HostTcpSocket::get_address_family(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_listen_backlog_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_listen_backlog_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_keep_alive_enabled(&mut self, socket: Resource<TcpSocket>) -> SocketResult<bool> {
        types::HostTcpSocket::get_keep_alive_enabled(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_keep_alive_enabled(
        &mut self,
        socket: Resource<TcpSocket>,
        value: bool,
    ) -> SocketResult<()> {
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
        types::HostTcpSocket::get_keep_alive_interval(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_keep_alive_interval(
        &mut self,
        socket: Resource<TcpSocket>,
        value: types::Duration,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_keep_alive_interval(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_keep_alive_count(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u32> {
        types::HostTcpSocket::get_keep_alive_count(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_keep_alive_count(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u32,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_keep_alive_count(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_hop_limit(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u8> {
        types::HostTcpSocket::get_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_hop_limit(&mut self, socket: Resource<TcpSocket>, value: u8) -> SocketResult<()> {
        types::HostTcpSocket::set_hop_limit(&mut WasiSocketsView::sockets(self.0), socket, value)
    }

    fn get_receive_buffer_size(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u64> {
        types::HostTcpSocket::get_receive_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_send_buffer_size(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u64> {
        types::HostTcpSocket::get_send_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn drop(&mut self, sock: Resource<TcpSocket>) -> wasmtime::Result<()> {
        types::HostTcpSocket::drop(&mut WasiSocketsView::sockets(self.0), sock)
    }
}

struct TcpSocketSendTask<Ctx> {
    call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
    input_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
    socket_result_rx: tokio::sync::oneshot::Receiver<Result<(), types::ErrorCode>>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> TcpSocketSendTask<Ctx> {
    fn new(
        call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
        input_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
        socket_result_rx: tokio::sync::oneshot::Receiver<Result<(), types::ErrorCode>>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            input_rx,
            socket_result_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for TcpSocketSendTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let result = complete_tcp_socket_send::<Ctx, U>(
            accessor,
            self.call,
            self.input_rx,
            self.socket_result_rx,
            &self.result_tx,
        )
        .await;
        if !self.result_tx.is_closed() {
            let _ = self.result_tx.send(result);
        }
        Ok(())
    }
}

struct TcpSocketSendReplayTask<Ctx> {
    call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
    input_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
    socket: Resource<TcpSocket>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> TcpSocketSendReplayTask<Ctx> {
    fn new(
        call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
        input_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
        socket: Resource<TcpSocket>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            input_rx,
            socket,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for TcpSocketSendReplayTask<Ctx>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let result = replay_tcp_socket_send::<Ctx, U>(
            accessor,
            self.call,
            self.input_rx,
            self.socket,
            &self.result_tx,
        )
        .await;
        if !self.result_tx.is_closed() {
            let _ = self.result_tx.send(result);
        }
        Ok(())
    }
}

struct TcpSocketReceiveTask<Ctx> {
    call: CallHandle<P3SocketsTypesTcpSocketReceive, Cancellable>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> TcpSocketReceiveTask<Ctx> {
    fn new(
        call: CallHandle<P3SocketsTypesTcpSocketReceive, Cancellable>,
        stream_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            stream_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for TcpSocketReceiveTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let result = complete_tcp_socket_receive::<Ctx, U>(
            accessor,
            self.call,
            self.stream_rx,
            &self.result_tx,
        )
        .await;
        if !self.result_tx.is_closed() {
            let _ = self.result_tx.send(result);
        }
        Ok(())
    }
}

struct TcpSocketReceiveReplayTask<Ctx> {
    call: CallHandle<P3SocketsTypesTcpSocketReceive, Cancellable>,
    mode_tx: tokio::sync::oneshot::Sender<TcpStreamMode>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
    socket: Resource<TcpSocket>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> TcpSocketReceiveReplayTask<Ctx> {
    fn new(
        call: CallHandle<P3SocketsTypesTcpSocketReceive, Cancellable>,
        mode_tx: tokio::sync::oneshot::Sender<TcpStreamMode>,
        stream_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
        socket: Resource<TcpSocket>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            mode_tx,
            stream_rx,
            socket,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for TcpSocketReceiveReplayTask<Ctx>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let result = replay_tcp_socket_receive::<Ctx, U>(
            accessor,
            self.call,
            self.mode_tx,
            self.stream_rx,
            self.socket,
            &self.result_tx,
        )
        .await;
        if !self.result_tx.is_closed() {
            let _ = self.result_tx.send(result);
        }
        Ok(())
    }
}

async fn start_tcp_socket_send_call<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
) -> wasmtime::Result<CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    CallHandle::<P3SocketsTypesTcpSocketSend, Cancellable>::start_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        HostRequestNoInput {},
        DurableFunctionType::WriteRemoteBatched(None),
    )
    .await
    .map_err(wasmtime::Error::from)
}

async fn start_tcp_socket_receive_call<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
) -> wasmtime::Result<CallHandle<P3SocketsTypesTcpSocketReceive, Cancellable>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    CallHandle::<P3SocketsTypesTcpSocketReceive, Cancellable>::start_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        HostRequestNoInput {},
        DurableFunctionType::ReadRemote,
    )
    .await
    .map_err(wasmtime::Error::from)
}

fn tcp_stream_response(captured: CapturedTcpStream) -> HostResponseP3SocketsTcpStream {
    HostResponseP3SocketsTcpStream {
        contents: captured.contents,
        result: serialize_tcp_stream_result(captured.result),
    }
}

async fn complete_tcp_socket_send<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
    input_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
    socket_result_rx: tokio::sync::oneshot::Receiver<Result<(), types::ErrorCode>>,
    result_tx: &tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let mut captured = input_rx.await.unwrap_or_else(|_| CapturedTcpStream {
        contents: Vec::new(),
        result: Err(types::ErrorCode::ConnectionBroken),
    });
    let socket_result = wait_tcp_socket_future_result(socket_result_rx).await;
    if captured.result.is_ok() {
        captured.result = socket_result;
    }
    let response = tcp_stream_response(captured);

    if result_tx.is_closed() {
        let result = deserialize_tcp_stream_result(response.result.clone());
        call.cancel_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            Some(response.clone()),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        return Ok(result);
    }

    let response = call
        .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)?;

    Ok(deserialize_tcp_stream_result(response.result))
}

async fn replay_tcp_socket_send<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3SocketsTypesTcpSocketSend, Cancellable>,
    input_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
    socket: Resource<TcpSocket>,
    result_tx: &tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let captured = input_rx.await.unwrap_or_else(|_| CapturedTcpStream {
        contents: Vec::new(),
        result: Err(types::ErrorCode::ConnectionBroken),
    });

    match call
        .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
        .await
        .map_err(wasmtime::Error::from)?
    {
        CallReplayOutcome::Replayed(response) => Ok(deserialize_tcp_stream_result(response.result)),
        CallReplayOutcome::Incomplete(call) => {
            let contents = captured.contents;
            let result =
                send_captured_tcp_stream::<Ctx, U>(accessor, socket, contents.clone()).await?;
            let response = HostResponseP3SocketsTcpStream {
                contents,
                result: serialize_tcp_stream_result(result.clone()),
            };

            if result_tx.is_closed() {
                call.cancel_access(accessor, durable_worker_ctx::<Ctx, U>, Some(response))
                    .await
                    .map_err(wasmtime::Error::from)?;
                return Ok(result);
            }

            let response = call
                .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
                .await
                .map_err(wasmtime::Error::from)?;
            Ok(deserialize_tcp_stream_result(response.result))
        }
    }
}

async fn send_captured_tcp_stream<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    socket: Resource<TcpSocket>,
    contents: Vec<u8>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let stream = accessor.with(|mut access| StreamReader::new(&mut access, contents))?;
    let store = accessor.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
    let future =
        <WasiSockets as types::HostTcpSocketWithStore>::send(&store, socket, stream).await?;
    let result_rx = accessor.with(
        |mut access| -> wasmtime::Result<
            tokio::sync::oneshot::Receiver<Result<(), types::ErrorCode>>,
        > {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        future.pipe(
            &mut access,
            TcpSocketFutureResultConsumer::result(result_tx),
        )?;
        wasmtime::Result::Ok(result_rx)
    })?;

    Ok(wait_tcp_socket_future_result(result_rx).await)
}

async fn complete_tcp_socket_receive<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3SocketsTypesTcpSocketReceive, Cancellable>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
    result_tx: &tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let captured = stream_rx.await.unwrap_or_else(|_| CapturedTcpStream {
        contents: Vec::new(),
        result: Err(types::ErrorCode::ConnectionBroken),
    });
    let response = tcp_stream_response(captured);

    if result_tx.is_closed() {
        let result = deserialize_tcp_stream_result(response.result.clone());
        call.cancel_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            Some(response.clone()),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        return Ok(result);
    }

    let response = call
        .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)?;

    Ok(deserialize_tcp_stream_result(response.result))
}

async fn replay_tcp_socket_receive<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3SocketsTypesTcpSocketReceive, Cancellable>,
    mode_tx: tokio::sync::oneshot::Sender<TcpStreamMode>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedTcpStream>,
    socket: Resource<TcpSocket>,
    result_tx: &tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    match call
        .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
        .await
        .map_err(wasmtime::Error::from)
    {
        Ok(CallReplayOutcome::Replayed(response)) => {
            let captured = CapturedTcpStream {
                contents: response.contents,
                result: deserialize_tcp_stream_result(response.result),
            };
            let _ = mode_tx.send(TcpStreamMode::Replayed(captured));
            let captured = stream_rx.await.unwrap_or_else(|_| CapturedTcpStream {
                contents: Vec::new(),
                result: Err(types::ErrorCode::ConnectionBroken),
            });
            Ok(captured.result)
        }
        Ok(CallReplayOutcome::Incomplete(call)) => {
            let events_rx =
                start_live_tcp_socket_receive_access::<Ctx, U>(accessor, socket).await?;
            let _ = mode_tx.send(TcpStreamMode::Live(events_rx));
            complete_tcp_socket_receive::<Ctx, U>(accessor, call, stream_rx, result_tx).await
        }
        Err(error) => {
            let error = error.to_string();
            let _ = mode_tx.send(TcpStreamMode::Error(error.clone()));
            Err(wasmtime::Error::msg(error))
        }
    }
}

async fn start_live_tcp_socket_receive_access<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    socket: Resource<TcpSocket>,
) -> wasmtime::Result<tokio::sync::mpsc::UnboundedReceiver<TcpStreamEvent>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    start_live_tcp_socket_receive::<Ctx, U>(accessor, socket).await
}

async fn start_live_tcp_socket_receive<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    socket: Resource<TcpSocket>,
) -> wasmtime::Result<tokio::sync::mpsc::UnboundedReceiver<TcpStreamEvent>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
    let sockets = accessor.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
    let (stream, future) =
        <WasiSockets as types::HostTcpSocketWithStore>::receive(&sockets, socket).await?;
    accessor.with(|mut store| {
        stream.pipe(
            &mut store,
            TcpSocketReceiveForwardingConsumer::new(event_tx.clone()),
        )?;
        future.pipe(&mut store, TcpSocketFutureResultConsumer::event(event_tx))
    })?;
    Ok(event_rx)
}

impl<Ctx: WorkerCtx> types::HostTcpSocketWithStore for DurableP3<Ctx> {
    async fn connect<U: Send>(
        store: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        let store = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostTcpSocketWithStore>::connect(&store, socket, remote_address)
            .await
    }

    fn listen<U: 'static>(
        mut store: Access<U, Self>,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<StreamReader<Resource<TcpSocket>>> {
        let store =
            Access::<U, WasiSockets>::new(store.as_context_mut(), wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostTcpSocketWithStore>::listen(store, socket)
    }

    async fn send<U: Send + 'static>(
        accessor: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let call = start_tcp_socket_send_call::<Ctx, U>(accessor).await?;
        let (input_tx, input_rx) = tokio::sync::oneshot::channel();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        if call.is_live() {
            let (chunk_tx, chunk_rx) = tokio::sync::mpsc::unbounded_channel();
            accessor.with(|mut store| {
                data.pipe(
                    &mut store,
                    TcpSocketInputStreamConsumer::new(Some(chunk_tx), input_tx),
                )
            })?;
            let forwarded = accessor.with(|mut store| {
                StreamReader::new(&mut store, TcpSocketForwardStreamProducer::new(chunk_rx))
            })?;
            let sockets = accessor.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
            let future =
                <WasiSockets as types::HostTcpSocketWithStore>::send(&sockets, socket, forwarded)
                    .await?;
            let (socket_result_tx, socket_result_rx) = tokio::sync::oneshot::channel();
            accessor.with(|mut store| {
                future.pipe(
                    &mut store,
                    TcpSocketFutureResultConsumer::result(socket_result_tx),
                )?;
                store.spawn(TcpSocketSendTask::<Ctx>::new(
                    call,
                    input_rx,
                    socket_result_rx,
                    result_tx,
                ));
                FutureReader::new(&mut store, wait_tcp_socket_task_result(result_rx))
            })
        } else {
            accessor.with(|mut store| {
                data.pipe(
                    &mut store,
                    TcpSocketInputStreamConsumer::new(None, input_tx),
                )?;
                store.spawn(TcpSocketSendReplayTask::<Ctx>::new(
                    call, input_rx, socket, result_tx,
                ));

                FutureReader::new(&mut store, wait_tcp_socket_task_result(result_rx))
            })
        }
    }

    async fn receive<U: Send + 'static>(
        accessor: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        let call = start_tcp_socket_receive_call::<Ctx, U>(accessor).await?;
        let (stream_tx, stream_rx) = tokio::sync::oneshot::channel();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        if call.is_live() {
            let events_rx = start_live_tcp_socket_receive::<Ctx, U>(accessor, socket).await?;
            accessor.with(|mut store| {
                store.spawn(TcpSocketReceiveTask::<Ctx>::new(call, stream_rx, result_tx));
                let stream = StreamReader::new(
                    &mut store,
                    TcpSocketOutputStreamProducer::live(events_rx, stream_tx),
                )?;
                let future = FutureReader::new(&mut store, wait_tcp_socket_task_result(result_rx))?;
                Ok((stream, future))
            })
        } else {
            let (mode_tx, mode_rx) = tokio::sync::oneshot::channel();
            accessor.with(|mut store| {
                store.spawn(TcpSocketReceiveReplayTask::<Ctx>::new(
                    call, mode_tx, stream_rx, socket, result_tx,
                ));
                let stream = StreamReader::new(
                    &mut store,
                    TcpSocketOutputStreamProducer::deferred(mode_rx, stream_tx),
                )?;
                let future = FutureReader::new(&mut store, wait_tcp_socket_task_result(result_rx))?;
                Ok((stream, future))
            })
        }
    }
}

impl<Ctx: WorkerCtx> types::HostUdpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<UdpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostUdpSocket::bind(&mut view, socket, local_address).await
    }

    async fn connect(
        &mut self,
        socket: Resource<UdpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostUdpSocket::connect(&mut view, socket, remote_address).await
    }

    fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<UdpSocket>> {
        types::HostUdpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)
    }

    fn disconnect(&mut self, socket: Resource<UdpSocket>) -> SocketResult<()> {
        types::HostUdpSocket::disconnect(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_local_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        types::HostUdpSocket::get_local_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        types::HostUdpSocket::get_remote_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_address_family(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<types::IpAddressFamily> {
        types::HostUdpSocket::get_address_family(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_unicast_hop_limit(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u8> {
        types::HostUdpSocket::get_unicast_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_unicast_hop_limit(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u8,
    ) -> SocketResult<()> {
        types::HostUdpSocket::set_unicast_hop_limit(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_receive_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        types::HostUdpSocket::get_receive_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostUdpSocket::set_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_send_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        types::HostUdpSocket::get_send_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostUdpSocket::set_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn drop(&mut self, sock: Resource<UdpSocket>) -> wasmtime::Result<()> {
        types::HostUdpSocket::drop(&mut WasiSocketsView::sockets(self.0), sock)
    }
}

impl<Ctx: WorkerCtx> types::HostUdpSocketWithStore for DurableP3<Ctx> {
    async fn send<U: Send>(
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
                let result = <WasiSockets as types::HostUdpSocketWithStore>::send(
                    &sockets,
                    socket,
                    data,
                    remote_address,
                )
                .await;

                Ok(HostResponseP3SocketsUdpSend {
                    result: match result {
                        Ok(()) => Ok(()),
                        Err(error) => Err(serialize_udp_socket_error(error)?),
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

    async fn receive<U: Send>(
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
                    <WasiSockets as types::HostUdpSocketWithStore>::receive(&sockets, socket).await;

                Ok(HostResponseP3SocketsUdpReceive {
                    result: match result {
                        Ok((data, remote_address)) => Ok(SerializableP3UdpDatagram {
                            data,
                            remote_address: remote_address.into(),
                        }),
                        Err(error) => Err(serialize_udp_socket_error(error)?),
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

impl<Ctx: WorkerCtx> ip_name_lookup::HostWithStore for DurableP3<Ctx> {
    async fn resolve_addresses<U: Send + 'static>(
        store: &Accessor<U, Self>,
        name: String,
    ) -> wasmtime::Result<Result<Vec<types::IpAddress>, ip_name_lookup::ErrorCode>> {
        let response = run_read_access::<_, _, Ctx, P3SocketsIpNameLookupResolveAddresses, _, _>(
            store,
            HostRequestP3SocketsResolveName { name: name.clone() },
            DurableFunctionType::ReadRemote,
            || async {
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let result = <WasiSockets as ip_name_lookup::HostWithStore>::resolve_addresses(
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

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

use super::WebSocketConnectionPool;
use crate::durable_host::authorization::targets::websocket_target;
use crate::durable_host::concurrent::{
    AccessClaimOptions, CallReplayOutcome, DeferredCallReplayOutcome, DurableCallSession,
    LeaveIncompleteOnDrop, NotCancellable, authorize_live_permissions_at_serialized_access,
};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::websocket::client::{
    CloseInfo, Error, Host, HostWebsocketConnection, HostWebsocketConnectionWithStore, Message,
};
#[cfg(feature = "test-utils")]
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use futures::future::Either;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt, pin_mut};
use golem_common::model::OplogIndex;
use golem_common::model::oplog::host_functions;
use golem_common::model::oplog::payload::types::{
    SerializableWebsocketCloseInfo, SerializableWebsocketError, SerializableWebsocketMessage,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestWebsocketClose, HostRequestWebsocketConnect,
    HostRequestWebsocketReceive, HostRequestWebsocketReceiveWithTimeout, HostRequestWebsocketSend,
    HostResponseWebsocketCloseResponse, HostResponseWebsocketConnectResponse,
    HostResponseWebsocketReceiveResponse, HostResponseWebsocketReceiveWithTimeoutResponse,
    HostResponseWebsocketSendResponse,
};
use golem_service_base::error::worker_executor::InterruptKind;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::{MaybeTlsStream, connect_async};
use wasmtime::component::{Accessor, HasSelf, Resource};
use wasmtime_wasi::IoView;

type WsStream = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Live TCP/WebSocket state for a guest connection handle (`WebSocketConnectionEntry::Live`).
/// Fields are private to this module; the type is `pub` only so the resource entry enum remains public.
pub struct LiveWebSocketConnection {
    writer: Mutex<SplitSink<WsStream, tungstenite::Message>>,
    reader: Mutex<SplitStream<WsStream>>,
    /// Held for the lifetime of the connection to limit concurrent WebSocket
    /// connections per executor. Released when the connection is dropped.
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl LiveWebSocketConnection {
    fn new(ws_stream: WsStream, permit: tokio::sync::OwnedSemaphorePermit) -> Self {
        let (writer, reader) = ws_stream.split();
        Self {
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            _permit: permit,
        }
    }
}

#[derive(Clone, Debug)]
pub enum TerminalWebSocketError {
    ConnectionFailure(String),
    Closed(Option<SerializableWebsocketCloseInfo>),
}

impl TerminalWebSocketError {
    fn to_error(&self) -> Error {
        match self {
            Self::ConnectionFailure(reason) => Error::ConnectionFailure(reason.clone()),
            Self::Closed(close_info) => {
                Error::Closed(close_info.as_ref().map(|close_info| CloseInfo {
                    code: close_info.code,
                    reason: close_info.reason.clone(),
                }))
            }
        }
    }
}

pub enum WebSocketConnectionEntry {
    /// `Arc` keeps the other variants small (`clippy::large_enum_variant`) and lets the
    /// accessor-based `receive`/`receive-with-timeout` await on the live connection outside
    /// store windows without holding a table borrow.
    Live(Arc<LiveWebSocketConnection>),
    /// Placeholder for a connection whose live socket did not survive
    /// reconstruction; the `Mutex` is the per-handle reconnect coordination
    /// gate. Concurrent calls seeing this handle serialize on the gate, so at
    /// most one call performs the live reconnect while the others re-read the
    /// entry instead of independently acquiring pool permits.
    Replay(Arc<Mutex<()>>),
    Terminal(TerminalWebSocketError),
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> HostWebsocketConnection for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<WebSocketConnectionEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem:websocket/client", "drop");
        self.unregister_open_websocket(rep.rep());
        self.as_wasi_view().table().delete(rep)?;
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostWebsocketConnectionWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn connect(
        accessor: &Accessor<U, Self>,
        url: String,
        headers: Option<Vec<(String, String)>>,
    ) -> anyhow::Result<Result<Resource<WebSocketConnectionEntry>, Error>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem:websocket/client", "connect")
        });
        let mut denied = false;
        let mut authorization_checked = false;
        let request = HostRequestWebsocketConnect {
            url: url.clone(),
            headers: headers.clone(),
        };
        let mut call = DurableCallSession::<
            host_functions::WebsocketClientConnect,
            NotCancellable,
        >::start_access_with_options(
            accessor,
            accessor.getter(),
            DurableFunctionType::WriteRemote,
            AccessClaimOptions {
                request_identity: Some(request.clone().into()),
                ..Default::default()
            },
            async |start| {
                if start.is_live {
                    authorization_checked = true;
                    denied = match websocket_target(&url) {
                        Ok(normalized) => authorize_live_permissions_at_serialized_access(
                            accessor,
                            accessor.getter(),
                            &[normalized.permission],
                        ).await?.is_err(),
                        Err(_) => true,
                    };
                }
                Ok(request)
            },
        ).await?;
        if !call.is_live() {
            match call
                .replay_access_deferred(accessor, accessor.getter())
                .await?
            {
                DeferredCallReplayOutcome::Replayed(resp, delivery) => {
                    let resp: HostResponseWebsocketConnectResponse = resp;
                    let result = match resp.result {
                        Ok(()) => {
                            let resource = accessor.with(|mut access| {
                                let ctx = access.get();
                                let resource = ctx.as_wasi_view().table().push(
                                    WebSocketConnectionEntry::Replay(Arc::new(Mutex::new(()))),
                                )?;
                                ctx.register_open_websocket(
                                    resource.rep(),
                                    url.clone(),
                                    headers.clone(),
                                );
                                Ok::<_, anyhow::Error>(resource)
                            })?;
                            Ok(Ok(resource))
                        }
                        Err(e) => Ok(Err(serializable_error_to_error(e))),
                    };
                    delivery.deliver_at_accessor_terminal(accessor).await?;
                    return result;
                }
                DeferredCallReplayOutcome::Incomplete(live) => call = live,
            }
        }

        if !authorization_checked {
            denied = match websocket_target(&url) {
                Ok(normalized) => match authorize_live_permissions_at_serialized_access(
                    accessor,
                    accessor.getter(),
                    &[normalized.permission],
                )
                .await
                {
                    Ok(result) => result.is_err(),
                    Err(error) => return Err(call.trap(error)),
                },
                Err(_) => true,
            };
        }
        if denied {
            call.complete_access(
                accessor,
                accessor.getter(),
                HostResponseWebsocketConnectResponse {
                    result: Err(SerializableWebsocketError::Other(
                        "permission denied".into(),
                    )),
                },
            )
            .await?;
            return Ok(Err(Error::Other("permission denied".into())));
        }

        let request = match build_request(&url, headers.as_deref()) {
            Ok(req) => req,
            Err(e) => {
                let resp = HostResponseWebsocketConnectResponse {
                    result: Err(SerializableWebsocketError::ConnectionFailure(e.clone())),
                };
                call.complete_access(accessor, accessor.getter(), resp)
                    .await?;
                return Ok(Err(Error::ConnectionFailure(e)));
            }
        };

        let pool = accessor.with(|mut access| access.get().websocket_connection_pool.clone());
        let interrupt_signal = accessor.with(|mut access| access.get().create_interrupt_signal());
        let permit = match acquire_websocket_connection_or_interrupt(&pool, interrupt_signal).await
        {
            Ok(Ok(permit)) => permit,
            Err(err) => return Err(call.trap(err)),
            Ok(Err(interrupt_kind)) => {
                call.abandon_for_trap();
                return Err(interrupt_kind.into());
            }
        };

        let interrupt_signal = accessor.with(|mut access| access.get().create_interrupt_signal());

        #[cfg(feature = "test-utils")]
        let observer = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.public_state
                .worker()
                .websocket_handshake_observer_for_test(
                    ctx.state.get_current_idempotency_key(),
                    call.start_index(),
                )
        });
        let connect_fut = connect_async(request);
        #[cfg(feature = "test-utils")]
        let connect_fut = async {
            match observer {
                Some(observer) => observer.observe(connect_fut).await,
                None => connect_fut.await,
            }
        };
        pin_mut!(connect_fut);
        let connect_result = match futures::future::select(connect_fut, interrupt_signal).await {
            Either::Left((result, _)) => result,
            Either::Right((interrupt_kind, _)) => {
                tracing::info!("Interrupted while waiting for WebSocket connect");
                call.abandon_for_trap();
                return Err(interrupt_kind.into());
            }
        };

        match connect_result {
            Ok((ws_stream, _response)) => {
                let entry = WebSocketConnectionEntry::Live(Arc::new(LiveWebSocketConnection::new(
                    ws_stream, permit,
                )));
                let pushed = accessor.with(|mut access| {
                    let ctx = access.get();
                    let resource = ctx.as_wasi_view().table().push(entry)?;
                    ctx.register_open_websocket(resource.rep(), url.clone(), headers.clone());
                    Ok::<_, anyhow::Error>(resource)
                });
                let resource = match pushed {
                    Ok(resource) => resource,
                    Err(err) => {
                        return Err(call.trap(err));
                    }
                };
                let resp = HostResponseWebsocketConnectResponse { result: Ok(()) };
                call.complete_access(accessor, accessor.getter(), resp)
                    .await?;
                Ok(Ok(resource))
            }
            Err(e) => {
                let resp = HostResponseWebsocketConnectResponse {
                    result: Err(SerializableWebsocketError::ConnectionFailure(e.to_string())),
                };
                call.complete_access(accessor, accessor.getter(), resp)
                    .await?;
                Ok(Err(Error::ConnectionFailure(e.to_string())))
            }
        }
    }

    async fn send(
        accessor: &Accessor<U, Self>,
        self_: Resource<WebSocketConnectionEntry>,
        message: Message,
    ) -> anyhow::Result<Result<(), Error>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem:websocket/client", "send")
        });

        let request = HostRequestWebsocketSend {
            message: message_to_serializable(&message),
        };
        let mut call =
            DurableCallSession::<host_functions::WebsocketClientSend, NotCancellable>::start_access_with_options(
                accessor,
                accessor.getter(),
                DurableFunctionType::WriteRemote,
                AccessClaimOptions {
                    request_identity: Some(request.clone().into()),
                    ..Default::default()
                },
                async move |_| Ok(request),
            )
            .await?;

        if !call.is_live() {
            accessor
                .with(|mut access| access.get().as_wasi_view().table().get(&self_).map(|_| ()))?;
            match call
                .replay_access_deferred(accessor, accessor.getter())
                .await?
            {
                DeferredCallReplayOutcome::Replayed(resp, delivery) => {
                    let resp: HostResponseWebsocketSendResponse = resp;
                    let result = match resp.result {
                        Ok(()) => Ok(Ok(())),
                        Err(e) => {
                            let error = serializable_error_to_error(e);
                            if let Some(terminal_error) = terminal_websocket_error(&error) {
                                accessor.with(|mut access| {
                                    mark_websocket_terminal(access.get(), &self_, terminal_error)
                                })?;
                            }
                            Ok(Err(error))
                        }
                    };
                    delivery.deliver_at_accessor_terminal(accessor).await?;
                    return result;
                }
                DeferredCallReplayOutcome::Incomplete(live) => call = live,
            }
        }

        match ensure_websocket_connection_live_access(
            accessor,
            &self_,
            call.start_index(),
            #[cfg(feature = "test-utils")]
            crate::worker::WebSocketReconnectPathForTest::Direct,
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let resp = HostResponseWebsocketSendResponse {
                    result: Err(error_to_serializable(&error)),
                };
                call.complete_access(accessor, accessor.getter(), resp)
                    .await?;
                return Ok(Err(error));
            }
            Err(err) => {
                return Err(call.trap(err));
            }
        }

        let tungstenite_msg = to_tungstenite_message(message);
        let live_result =
            match write_websocket_access(accessor, &self_, tungstenite_msg, None).await {
                Ok(result) => result,
                Err(err) => return Err(call.trap(err)),
            };
        let ser_result = match &live_result {
            Ok(()) => Ok(()),
            Err(e) => Err(error_to_serializable(e)),
        };
        let resp = HostResponseWebsocketSendResponse { result: ser_result };
        let (_, delivery) = call
            .complete_access_deferred(accessor, accessor.getter(), resp)
            .await?;
        if let Some(terminal_error) = live_result
            .as_ref()
            .err()
            .and_then(terminal_websocket_error)
        {
            accessor
                .with(|mut access| mark_websocket_terminal(access.get(), &self_, terminal_error))?;
        }
        delivery.deliver_at_accessor_terminal(accessor).await?;
        Ok(live_result)
    }

    async fn close(
        accessor: &Accessor<U, Self>,
        self_: Resource<WebSocketConnectionEntry>,
        code: Option<u16>,
        reason: Option<String>,
    ) -> anyhow::Result<Result<(), Error>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem:websocket/client", "close")
        });

        let request = HostRequestWebsocketClose {
            code,
            reason: reason.clone(),
        };
        let mut call =
            DurableCallSession::<host_functions::WebsocketClientClose, NotCancellable>::start_access_with_options(
                accessor,
                accessor.getter(),
                DurableFunctionType::WriteRemote,
                AccessClaimOptions {
                    request_identity: Some(request.clone().into()),
                    ..Default::default()
                },
                async move |_| Ok(request),
            )
            .await?;

        let terminal_close_error =
            TerminalWebSocketError::Closed(Some(SerializableWebsocketCloseInfo {
                code: code.unwrap_or(1000),
                reason: reason
                    .clone()
                    .unwrap_or_else(|| "Connection closed".to_string()),
            }));

        if !call.is_live() {
            accessor
                .with(|mut access| access.get().as_wasi_view().table().get(&self_).map(|_| ()))?;
            match call
                .replay_access_deferred(accessor, accessor.getter())
                .await?
            {
                DeferredCallReplayOutcome::Replayed(resp, delivery) => {
                    let resp: HostResponseWebsocketCloseResponse = resp;
                    let result = match resp.result {
                        Ok(()) => {
                            accessor.with(|mut access| {
                                mark_websocket_terminal(access.get(), &self_, terminal_close_error)
                            })?;
                            Ok(Ok(()))
                        }
                        Err(e) => {
                            let error = serializable_error_to_error(e);
                            if let Some(terminal_error) = terminal_websocket_error(&error) {
                                accessor.with(|mut access| {
                                    mark_websocket_terminal(access.get(), &self_, terminal_error)
                                })?;
                            }
                            Ok(Err(error))
                        }
                    };
                    delivery.deliver_at_accessor_terminal(accessor).await?;
                    return result;
                }
                DeferredCallReplayOutcome::Incomplete(live) => call = live,
            }
        }

        match ensure_websocket_connection_live_access(
            accessor,
            &self_,
            call.start_index(),
            #[cfg(feature = "test-utils")]
            crate::worker::WebSocketReconnectPathForTest::Direct,
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let resp = HostResponseWebsocketCloseResponse {
                    result: Err(error_to_serializable(&error)),
                };
                call.complete_access(accessor, accessor.getter(), resp)
                    .await?;
                return Ok(Err(error));
            }
            Err(err) => {
                return Err(call.trap(err));
            }
        }

        let close_frame = tungstenite::protocol::CloseFrame {
            code: tungstenite::protocol::frame::coding::CloseCode::from(code.unwrap_or(1000)),
            reason: reason.unwrap_or_default().into(),
        };
        let live_result = match write_websocket_access(
            accessor,
            &self_,
            tungstenite::Message::Close(Some(close_frame)),
            Some(terminal_close_error.clone()),
        )
        .await
        {
            Ok(result) => result,
            Err(err) => return Err(call.trap(err)),
        };
        let ser_result = match &live_result {
            Ok(()) => Ok(()),
            Err(e) => Err(error_to_serializable(e)),
        };
        let resp = HostResponseWebsocketCloseResponse { result: ser_result };
        let (_, delivery) = call
            .complete_access_deferred(accessor, accessor.getter(), resp)
            .await?;
        if live_result.is_ok() {
            accessor.with(|mut access| {
                mark_websocket_terminal(access.get(), &self_, terminal_close_error)
            })?;
        } else if let Some(terminal_error) = live_result
            .as_ref()
            .err()
            .and_then(terminal_websocket_error)
        {
            accessor
                .with(|mut access| mark_websocket_terminal(access.get(), &self_, terminal_error))?;
        }
        delivery.deliver_at_accessor_terminal(accessor).await?;
        Ok(live_result)
    }

    async fn receive(
        accessor: &Accessor<U, Self>,
        self_: Resource<WebSocketConnectionEntry>,
    ) -> anyhow::Result<Result<Message, Error>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem:websocket/client", "receive")
        });

        let mut call = DurableCallSession::<
            host_functions::WebsocketClientReceive,
            LeaveIncompleteOnDrop,
        >::start_access(
            accessor,
            accessor.getter(),
            HostRequestWebsocketReceive {},
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if !call.is_live() {
            accessor.with(|mut access| {
                let ctx = access.get();
                let _ = ctx.as_wasi_view().table().get(&self_)?;
                Ok::<_, anyhow::Error>(())
            })?;
            match call.replay_access(accessor, accessor.getter()).await? {
                CallReplayOutcome::Replayed(resp) => {
                    let resp: HostResponseWebsocketReceiveResponse = resp;
                    return match resp.result {
                        Ok(m) => Ok(Ok(serializable_message_to_message(m))),
                        Err(e) => {
                            let error = serializable_error_to_error(e);
                            if let Some(terminal_error) = terminal_websocket_error(&error) {
                                accessor.with(|mut access| {
                                    mark_websocket_terminal(access.get(), &self_, terminal_error)
                                })?;
                            }
                            Ok(Err(error))
                        }
                    };
                }
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }

        match ensure_websocket_connection_live_access(
            accessor,
            &self_,
            call.start_index(),
            #[cfg(feature = "test-utils")]
            crate::worker::WebSocketReconnectPathForTest::Accessor,
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let resp = HostResponseWebsocketReceiveResponse {
                    result: Err(error_to_serializable(&error)),
                };
                call.complete_access(accessor, accessor.getter(), resp)
                    .await?;
                return Ok(Err(error));
            }
            Err(err) => {
                return Err(call.trap(err));
            }
        }

        let (interrupt_signal, live_lookup) = accessor.with(|mut access| {
            let ctx = access.get();
            let interrupt_signal = ctx.create_interrupt_signal();
            let live_lookup: Result<Result<Arc<LiveWebSocketConnection>, Error>, anyhow::Error> =
                match ctx.as_wasi_view().table().get(&self_) {
                    Ok(WebSocketConnectionEntry::Live(live)) => Ok(Ok(live.clone())),
                    Ok(WebSocketConnectionEntry::Replay(_)) => Err(
                        golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
                            "websocket connection entry kind mismatch during replay (live receive path saw Replay entry)",
                        )
                        .into(),
                    ),
                    Ok(WebSocketConnectionEntry::Terminal(error)) => Ok(Err(error.to_error())),
                    Err(err) => Err(err.into()),
                };
            (interrupt_signal, live_lookup)
        });

        let live_result = match live_lookup {
            Ok(Ok(live)) => {
                let mut reader = live.reader.lock().await;
                let recv_fut = read_next_user_or_close(&mut reader);
                pin_mut!(recv_fut);
                match futures::future::select(recv_fut, interrupt_signal).await {
                    Either::Left((result, _)) => result,
                    Either::Right((interrupt_kind, _)) => {
                        tracing::info!("Interrupted while waiting for WebSocket receive");
                        call.abandon_for_trap();
                        return Err(interrupt_kind.into());
                    }
                }
            }
            Ok(Err(error)) => Err(error),
            Err(err) => return Err(call.trap(err)),
        };

        let ser_result = match &live_result {
            Ok(m) => Ok(message_to_serializable(m)),
            Err(e) => Err(error_to_serializable(e)),
        };
        let resp = HostResponseWebsocketReceiveResponse { result: ser_result };
        call.complete_access(accessor, accessor.getter(), resp)
            .await?;
        if let Some(terminal_error) = live_result
            .as_ref()
            .err()
            .and_then(terminal_websocket_error)
        {
            accessor
                .with(|mut access| mark_websocket_terminal(access.get(), &self_, terminal_error))?;
        }
        Ok(live_result)
    }

    async fn receive_with_timeout(
        accessor: &Accessor<U, Self>,
        self_: Resource<WebSocketConnectionEntry>,
        timeout_ms: u64,
    ) -> anyhow::Result<Result<Option<Message>, Error>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem:websocket/client", "receive-with-timeout")
        });

        let mut call = DurableCallSession::<
            host_functions::WebsocketClientReceiveWithTimeout,
            LeaveIncompleteOnDrop,
        >::start_access(
            accessor,
            accessor.getter(),
            HostRequestWebsocketReceiveWithTimeout { timeout_ms },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if !call.is_live() {
            accessor.with(|mut access| {
                let ctx = access.get();
                let _ = ctx.as_wasi_view().table().get(&self_)?;
                Ok::<_, anyhow::Error>(())
            })?;
            match call.replay_access(accessor, accessor.getter()).await? {
                CallReplayOutcome::Replayed(resp) => {
                    let resp: HostResponseWebsocketReceiveWithTimeoutResponse = resp;
                    return match resp.result {
                        Ok(Some(m)) => Ok(Ok(Some(serializable_message_to_message(m)))),
                        Ok(None) => Ok(Ok(None)),
                        Err(e) => {
                            let error = serializable_error_to_error(e);
                            if let Some(terminal_error) = terminal_websocket_error(&error) {
                                accessor.with(|mut access| {
                                    mark_websocket_terminal(access.get(), &self_, terminal_error)
                                })?;
                            }
                            Ok(Err(error))
                        }
                    };
                }
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }

        match ensure_websocket_connection_live_access(
            accessor,
            &self_,
            call.start_index(),
            #[cfg(feature = "test-utils")]
            crate::worker::WebSocketReconnectPathForTest::Accessor,
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let resp = HostResponseWebsocketReceiveWithTimeoutResponse {
                    result: Err(error_to_serializable(&error)),
                };
                call.complete_access(accessor, accessor.getter(), resp)
                    .await?;
                return Ok(Err(error));
            }
            Err(err) => {
                return Err(call.trap(err));
            }
        }

        let (interrupt_signal, live_lookup) = accessor.with(|mut access| {
            let ctx = access.get();
            let interrupt_signal = ctx.create_interrupt_signal();
            let live_lookup: Result<Result<Arc<LiveWebSocketConnection>, Error>, anyhow::Error> =
                match ctx.as_wasi_view().table().get(&self_) {
                    Ok(WebSocketConnectionEntry::Live(live)) => Ok(Ok(live.clone())),
                    Ok(WebSocketConnectionEntry::Replay(_)) => Err(
                        golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
                            "websocket connection entry kind mismatch during replay (live receive_with_timeout path saw Replay entry)",
                        )
                        .into(),
                    ),
                    Ok(WebSocketConnectionEntry::Terminal(error)) => Ok(Err(error.to_error())),
                    Err(err) => Err(err.into()),
                };
            (interrupt_signal, live_lookup)
        });
        #[cfg(feature = "test-utils")]
        let mut observer = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.public_state
                .worker()
                .websocket_timed_receive_observer_for_test(
                    ctx.state.get_current_idempotency_key(),
                    call.start_index(),
                )
        });

        #[cfg(feature = "test-utils")]
        let lock_observer = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.public_state
                .worker()
                .websocket_reader_lock_observer_for_test(
                    ctx.state.get_current_idempotency_key(),
                    call.start_index(),
                )
        });

        let live_result: Result<Option<Message>, Error> = match live_lookup {
            Ok(Ok(live)) => {
                let reader_lock = live.reader.lock();
                #[cfg(feature = "test-utils")]
                let reader_lock = async {
                    match lock_observer {
                        Some(observer) => observer.observe(reader_lock).await,
                        None => reader_lock.await,
                    }
                };
                let mut reader = reader_lock.await;
                let deadline = Instant::now() + Duration::from_millis(timeout_ms);
                pin_mut!(interrupt_signal);
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break Ok(None);
                    }
                    let next_frame = reader.next();
                    #[cfg(feature = "test-utils")]
                    let next_frame = async {
                        match observer.take() {
                            Some(observer) => observer.observe(next_frame).await,
                            None => next_frame.await,
                        }
                    };
                    let next_frame = tokio::time::timeout(remaining, next_frame);
                    pin_mut!(next_frame);
                    match futures::future::select(next_frame, interrupt_signal.as_mut()).await {
                        Either::Left((Ok(Some(Ok(msg))), _)) => match to_user_message(msg) {
                            Ok(Some(message)) => break Ok(Some(message)),
                            Ok(None) => continue,
                            Err(err) => break Err(err),
                        },
                        Either::Left((Ok(Some(Err(e))), _)) => break Err(to_wit_error(e)),
                        Either::Left((Ok(None), _)) => {
                            break Err(Error::Closed(Some(CloseInfo {
                                code: 1000,
                                reason: "Connection closed".to_string(),
                            })));
                        }
                        Either::Left((Err(_), _)) => break Ok(None),
                        Either::Right((interrupt_kind, _)) => {
                            tracing::info!(
                                "Interrupted while waiting for WebSocket receive with timeout"
                            );
                            call.abandon_for_trap();
                            return Err(interrupt_kind.into());
                        }
                    }
                }
            }
            Ok(Err(error)) => Err(error),
            Err(err) => return Err(call.trap(err)),
        };

        let ser_result = match &live_result {
            Ok(Some(m)) => Ok(Some(message_to_serializable(m))),
            Ok(None) => Ok(None),
            Err(e) => Err(error_to_serializable(e)),
        };
        let resp = HostResponseWebsocketReceiveWithTimeoutResponse { result: ser_result };
        call.complete_access(accessor, accessor.getter(), resp)
            .await?;
        if let Some(terminal_error) = live_result
            .as_ref()
            .err()
            .and_then(terminal_websocket_error)
        {
            accessor
                .with(|mut access| mark_websocket_terminal(access.get(), &self_, terminal_error))?;
        }
        Ok(live_result)
    }
}

/// What a call queued on a replayed websocket handle should do once it re-reads
/// the handle's entry, relative to the per-handle reconnect gate it queued on.
#[derive(Debug, Clone)]
enum ReconnectEntryAction {
    /// The entry is still the same replayed handle this call gated on: this
    /// call performs the live reconnect.
    Reconnect,
    /// The entry is a live connection another call published while this call
    /// queued: use the current entry and take no pool permit.
    UseCurrentEntry,
    /// The entry was terminally closed while this call queued: fail with the
    /// entry's terminal error.
    Fail(TerminalWebSocketError),
    /// The entry carries a different replay incarnation than the gate this
    /// call queued on. The handle's `send`/`receive`/`receive-with-timeout`/
    /// `close` calls borrow the resource, so the guest cannot drop the handle
    /// while one of its calls queues on the gate, and a fresh table after
    /// reconstruction discards the queued call — a call holding the gate can
    /// only ever see its own incarnation in the entry. A different incarnation
    /// is therefore an invariant violation of the gate protocol, not a
    /// reconnect this call may perform.
    InconsistentReplayGate,
}

fn classify_reconnect_entry(
    entry: &WebSocketConnectionEntry,
    gate: &Arc<Mutex<()>>,
) -> ReconnectEntryAction {
    match entry {
        WebSocketConnectionEntry::Replay(entry_gate) if Arc::ptr_eq(gate, entry_gate) => {
            ReconnectEntryAction::Reconnect
        }
        WebSocketConnectionEntry::Live(_) => ReconnectEntryAction::UseCurrentEntry,
        WebSocketConnectionEntry::Terminal(error) => ReconnectEntryAction::Fail(error.clone()),
        WebSocketConnectionEntry::Replay(_) => ReconnectEntryAction::InconsistentReplayGate,
    }
}

fn inconsistent_replay_gate_error() -> anyhow::Error {
    golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
        "websocket connection entry changed replay incarnation while its reconnect gate was held",
    )
    .into()
}

/// Races a websocket wait (coordination, pool permit or I/O) against
/// `interrupt_signal`. `Ok` carries the wait's result; `Err` carries the
/// interruption, which the caller surfaces as a trap with its original typed
/// reason.
async fn wait_or_interrupt<T>(
    wait: impl Future<Output = T>,
    interrupt_signal: impl Future<Output = InterruptKind>,
) -> Result<T, InterruptKind> {
    pin_mut!(wait, interrupt_signal);
    match futures::future::select(wait, interrupt_signal).await {
        Either::Left((result, _)) => Ok(result),
        Either::Right((interrupt_kind, _)) => {
            tracing::info!("Interrupted while waiting for WebSocket reconnect coordination");
            Err(interrupt_kind)
        }
    }
}

async fn write_websocket_access<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    resource: &Resource<WebSocketConnectionEntry>,
    message: tungstenite::Message,
    terminal_on_success: Option<TerminalWebSocketError>,
) -> anyhow::Result<Result<(), Error>> {
    let live = match accessor.with(|mut access| {
        let ctx = access.get();
        match ctx.as_wasi_view().table().get(resource)? {
            WebSocketConnectionEntry::Live(live) => Ok(Ok(live.clone())),
            WebSocketConnectionEntry::Terminal(error) => Ok(Err(error.to_error())),
            WebSocketConnectionEntry::Replay(_) => Err(inconsistent_replay_gate_error()),
        }
    })? {
        Ok(live) => live,
        Err(error) => return Ok(Err(error)),
    };
    let mut writer = wait_or_interrupt(
        live.writer.lock(),
        accessor.with(|mut access| access.get().create_interrupt_signal()),
    )
    .await
    .map_err(anyhow::Error::from)?;
    // A close or receive may have made the table entry terminal while this writer queued.
    let current = accessor.with(|mut access| {
        let ctx = access.get();
        match ctx.as_wasi_view().table().get(resource)? {
            WebSocketConnectionEntry::Live(current) if Arc::ptr_eq(current, &live) => Ok(Ok(())),
            WebSocketConnectionEntry::Terminal(error) => Ok(Err(error.to_error())),
            _ => Err(inconsistent_replay_gate_error()),
        }
    })?;
    if let Err(error) = current {
        return Ok(Err(error));
    }
    let result = wait_or_interrupt(
        writer.send(message),
        accessor.with(|mut access| access.get().create_interrupt_signal()),
    )
    .await
    .map_err(anyhow::Error::from)?
    .map_err(|error| Error::SendFailure(error.to_string()));
    // Publish a successful close before releasing the writer so a queued send cannot use it.
    if result.is_ok()
        && let Some(error) = terminal_on_success
    {
        accessor.with(|mut access| mark_websocket_terminal(access.get(), resource, error))?;
    }
    Ok(result)
}

/// Store windows are only used for table lookups and entry replacement, while reconnect
/// coordination, permit acquisition and the websocket handshake await outside the store.
async fn ensure_websocket_connection_live_access<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    resource: &Resource<WebSocketConnectionEntry>,
    start: OplogIndex,
    #[cfg(feature = "test-utils")] path: crate::worker::WebSocketReconnectPathForTest,
) -> anyhow::Result<Result<(), Error>> {
    #[cfg(not(feature = "test-utils"))]
    let _ = start;
    let rep = resource.rep();

    let (gate, info, pool) = match accessor.with(|mut access| {
        let ctx = access.get();
        let gate = {
            let mut view = ctx.as_wasi_view();
            let entry = view.table().get(resource)?;
            match entry {
                WebSocketConnectionEntry::Replay(gate) => Some(gate.clone()),
                WebSocketConnectionEntry::Live(_) => None,
                WebSocketConnectionEntry::Terminal(error) => return Ok(Err(error.to_error())),
            }
        };
        Ok::<_, anyhow::Error>(Ok((
            gate,
            ctx.websocket_connection_info(rep),
            ctx.websocket_connection_pool.clone(),
        )))
    })? {
        Ok(state) => state,
        Err(error) => return Ok(Err(error)),
    };

    let Some(info) = info else {
        debug_assert!(
            gate.is_none(),
            "Replay entry must have connection info registered via connect()"
        );
        return Ok(Ok(()));
    };

    let Some(gate) = gate else {
        return Ok(Ok(()));
    };

    // At most one call at a time reconnects this handle: the gate is held from
    // here until the reconnect outcome is published, so concurrent calls queue
    // here instead of independently taking a second pool permit.
    let _gate_guard = match wait_or_interrupt(
        gate.lock(),
        accessor.with(|mut access| access.get().create_interrupt_signal()),
    )
    .await
    {
        Ok(guard) => guard,
        Err(interrupt_kind) => return Err(interrupt_kind.into()),
    };

    // Re-read the entry while holding the gate: a concurrent call may have
    // already reconnected this handle or completed a replayed response that
    // terminally closed it while we queued.
    let action = accessor.with(|mut access| {
        let ctx = access.get();
        let mut view = ctx.as_wasi_view();
        let entry = view.table().get(resource)?;
        Ok::<_, anyhow::Error>(classify_reconnect_entry(entry, &gate))
    })?;
    match action {
        ReconnectEntryAction::Reconnect => {}
        ReconnectEntryAction::UseCurrentEntry => return Ok(Ok(())),
        ReconnectEntryAction::Fail(error) => return Ok(Err(error.to_error())),
        ReconnectEntryAction::InconsistentReplayGate => {
            return Err(inconsistent_replay_gate_error());
        }
    }

    let info = {
        let fresh = accessor.with(|mut access| access.get().websocket_connection_info(rep));
        debug_assert!(
            fresh.is_some(),
            "Replay entry must have connection info registered via connect()"
        );
        fresh.unwrap_or(info)
    };

    // The read-only side-effect trap fires earlier: every caller of this helper goes through
    // `DurableCallSession::start_access` with `WriteRemote` first, which routes through
    // `DurabilityHost::begin_durable_function` — the single central read-only guard.
    let request = match build_request(&info.url, info.headers.as_deref()) {
        Ok(request) => request,
        Err(err) => {
            let error = Error::ConnectionFailure(err.clone());
            accessor.with(|mut access| {
                mark_websocket_reconnect_failure_terminal(
                    access.get(),
                    resource,
                    &gate,
                    TerminalWebSocketError::ConnectionFailure(err),
                )
            })?;
            return Ok(Err(error));
        }
    };

    #[cfg(feature = "test-utils")]
    let observer = accessor.with(|mut access| {
        let ctx = access.get();
        ctx.public_state
            .worker()
            .websocket_reconnect_pool_observer_for_test(
                ctx.state.get_current_idempotency_key(),
                start,
                path,
            )
    });
    let acquire = pool.acquire();
    #[cfg(feature = "test-utils")]
    let acquire = async {
        match observer {
            Some(observer) => observer.observe(acquire).await,
            None => acquire.await,
        }
    };
    let interrupt_signal = accessor.with(|mut access| access.get().create_interrupt_signal());
    let permit =
        match acquire_websocket_connection_or_interrupt_future(acquire, interrupt_signal).await? {
            Ok(permit) => permit,
            Err(interrupt_kind) => {
                tracing::info!("Interrupted while waiting for WebSocket reconnect pool");
                return Err(interrupt_kind.into());
            }
        };
    let interrupt_signal = accessor.with(|mut access| access.get().create_interrupt_signal());

    #[cfg(feature = "test-utils")]
    let observer = accessor.with(|mut access| {
        let ctx = access.get();
        ctx.public_state
            .worker()
            .websocket_handshake_observer_with_path_for_test(
                ctx.state.get_current_idempotency_key(),
                start,
                match path {
                    crate::worker::WebSocketReconnectPathForTest::Direct => {
                        crate::worker::WebSocketHandshakePathForTest::DirectReconnect
                    }
                    crate::worker::WebSocketReconnectPathForTest::Accessor => {
                        crate::worker::WebSocketHandshakePathForTest::AccessorReconnect
                    }
                },
            )
    });
    let connect_fut = connect_async(request);
    #[cfg(feature = "test-utils")]
    let connect_fut = async {
        match observer {
            Some(observer) => observer.observe(connect_fut).await,
            None => connect_fut.await,
        }
    };
    pin_mut!(connect_fut);
    let connect_result = match futures::future::select(connect_fut, interrupt_signal).await {
        Either::Left((result, _)) => result,
        Either::Right((interrupt_kind, _)) => {
            tracing::info!("Interrupted while waiting for WebSocket reconnect");
            return Err(interrupt_kind.into());
        }
    };

    let (ws_stream, _) = match connect_result {
        Ok(result) => result,
        Err(err) => {
            let reason = err.to_string();
            let error = Error::ConnectionFailure(reason.clone());
            accessor.with(|mut access| {
                mark_websocket_reconnect_failure_terminal(
                    access.get(),
                    resource,
                    &gate,
                    TerminalWebSocketError::ConnectionFailure(reason),
                )
            })?;
            return Ok(Err(error));
        }
    };

    let new_entry =
        WebSocketConnectionEntry::Live(Arc::new(LiveWebSocketConnection::new(ws_stream, permit)));
    let published = accessor.with(|mut access| {
        let ctx = access.get();
        let mut view = ctx.as_wasi_view();
        let entry = view.table().get_mut(resource)?;
        let action = classify_reconnect_entry(entry, &gate);
        if matches!(action, ReconnectEntryAction::Reconnect) {
            *entry = new_entry;
        }
        Ok::<_, anyhow::Error>(action)
    })?;
    match published {
        ReconnectEntryAction::UseCurrentEntry => return Ok(Ok(())),
        ReconnectEntryAction::Fail(error) => return Ok(Err(error.to_error())),
        ReconnectEntryAction::Reconnect => {}
        ReconnectEntryAction::InconsistentReplayGate => {
            return Err(inconsistent_replay_gate_error());
        }
    }

    Ok(Ok(()))
}

/// Failure-path terminal publication for a reconnect attempt that holds
/// `gate`: only terminally closes the handle if the entry still carries the
/// same replay incarnation this call gated on. A replayed response may have
/// terminally closed the handle, or a concurrent call may have published a
/// live connection, while this attempt awaited its handshake — those
/// publications own the entry's outcome, and this failure must not supersede
/// them. The re-verification and the publication run in one synchronous store
/// window, so no further change can interleave here.
fn mark_websocket_reconnect_failure_terminal<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WebSocketConnectionEntry>,
    gate: &Arc<Mutex<()>>,
    error: TerminalWebSocketError,
) -> anyhow::Result<()> {
    let still_reconnecting = {
        let mut view = ctx.as_wasi_view();
        let entry = view.table().get(resource)?;
        matches!(
            classify_reconnect_entry(entry, gate),
            ReconnectEntryAction::Reconnect
        )
    };
    if still_reconnecting {
        mark_websocket_terminal(ctx, resource, error)?;
    }
    Ok(())
}

fn mark_websocket_terminal<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WebSocketConnectionEntry>,
    error: TerminalWebSocketError,
) -> anyhow::Result<()> {
    ctx.unregister_open_websocket(resource.rep());
    let mut view = ctx.as_wasi_view();
    let entry = view.table().get_mut(resource)?;
    *entry = WebSocketConnectionEntry::Terminal(error);
    Ok(())
}

async fn acquire_websocket_connection_or_interrupt(
    pool: &WebSocketConnectionPool,
    interrupt_signal: impl Future<Output = golem_service_base::error::worker_executor::InterruptKind>,
) -> anyhow::Result<
    Result<
        tokio::sync::OwnedSemaphorePermit,
        golem_service_base::error::worker_executor::InterruptKind,
    >,
> {
    let acquire = pool.acquire();
    pin_mut!(acquire, interrupt_signal);
    match futures::future::select(acquire, interrupt_signal).await {
        Either::Left((result, _)) => result.map(Ok),
        Either::Right((interrupt, _)) => Ok(Err(interrupt)),
    }
}

async fn acquire_websocket_connection_or_interrupt_future(
    acquire: impl Future<Output = anyhow::Result<tokio::sync::OwnedSemaphorePermit>>,
    interrupt_signal: impl Future<Output = golem_service_base::error::worker_executor::InterruptKind>,
) -> anyhow::Result<
    Result<
        tokio::sync::OwnedSemaphorePermit,
        golem_service_base::error::worker_executor::InterruptKind,
    >,
> {
    pin_mut!(acquire, interrupt_signal);
    match futures::future::select(acquire, interrupt_signal).await {
        Either::Left((result, _)) => result.map(Ok),
        Either::Right((interrupt, _)) => Ok(Err(interrupt)),
    }
}

fn build_request(
    url: &str,
    headers: Option<&[(String, String)]>,
) -> Result<tungstenite::http::Request<()>, String> {
    use tungstenite::client::IntoClientRequest;

    let mut request = url.into_client_request().map_err(|e| e.to_string())?;

    if let Some(headers) = headers {
        let req_headers = request.headers_mut();
        for (name, value) in headers {
            let header_name = tungstenite::http::header::HeaderName::try_from(name.as_str())
                .map_err(|e| format!("invalid websocket header name {name:?}: {e}"))?;
            let header_value = tungstenite::http::header::HeaderValue::try_from(value.as_str())
                .map_err(|e| format!("invalid websocket header value for {name:?}: {e}"))?;
            req_headers.insert(header_name, header_value);
        }
    }

    Ok(request)
}

fn to_tungstenite_message(message: Message) -> tungstenite::Message {
    match message {
        Message::Text(text) => tungstenite::Message::Text(text.into()),
        Message::Binary(data) => tungstenite::Message::Binary(data.into()),
    }
}

fn to_user_message(msg: tungstenite::Message) -> Result<Option<Message>, Error> {
    match msg {
        tungstenite::Message::Text(text) => Ok(Some(Message::Text(text.as_str().to_owned()))),
        tungstenite::Message::Binary(data) => Ok(Some(Message::Binary(data.as_ref().to_vec()))),
        tungstenite::Message::Close(frame) => {
            let (code, reason) = match frame {
                Some(frame) => (frame.code.into(), frame.reason.to_string()),
                None => (1000u16, "Connection closed".to_string()),
            };
            Err(Error::Closed(Some(CloseInfo { code, reason })))
        }
        tungstenite::Message::Ping(_)
        | tungstenite::Message::Pong(_)
        | tungstenite::Message::Frame(_) => Ok(None),
    }
}

async fn read_next_user_or_close(stream: &mut SplitStream<WsStream>) -> Result<Message, Error> {
    loop {
        match stream.next().await {
            Some(Ok(msg)) => match to_user_message(msg) {
                Ok(Some(message)) => return Ok(message),
                Ok(None) => continue,
                Err(err) => return Err(err),
            },
            Some(Err(e)) => return Err(to_wit_error(e)),
            None => {
                return Err(Error::Closed(Some(CloseInfo {
                    code: 1000,
                    reason: "Connection closed".to_string(),
                })));
            }
        }
    }
}

fn terminal_websocket_error(error: &Error) -> Option<TerminalWebSocketError> {
    match error {
        Error::ConnectionFailure(reason) => {
            Some(TerminalWebSocketError::ConnectionFailure(reason.clone()))
        }
        Error::Closed(close_info) => Some(TerminalWebSocketError::Closed(close_info.as_ref().map(
            |close_info| SerializableWebsocketCloseInfo {
                code: close_info.code,
                reason: close_info.reason.clone(),
            },
        ))),
        _ => None,
    }
}

fn to_wit_error(e: tungstenite::error::Error) -> Error {
    match e {
        tungstenite::error::Error::ConnectionClosed => Error::Closed(Some(CloseInfo {
            code: 1000,
            reason: "Connection closed normally".to_string(),
        })),
        tungstenite::error::Error::AlreadyClosed => Error::Closed(Some(CloseInfo {
            code: 1000,
            reason: "Connection already closed".to_string(),
        })),
        tungstenite::error::Error::Protocol(p) => Error::ProtocolError(p.to_string()),
        other => Error::ReceiveFailure(other.to_string()),
    }
}

fn message_to_serializable(message: &Message) -> SerializableWebsocketMessage {
    match message {
        Message::Text(text) => SerializableWebsocketMessage::Text(text.clone()),
        Message::Binary(data) => SerializableWebsocketMessage::Binary(data.clone()),
    }
}

fn serializable_message_to_message(m: SerializableWebsocketMessage) -> Message {
    match m {
        SerializableWebsocketMessage::Text(text) => Message::Text(text),
        SerializableWebsocketMessage::Binary(data) => Message::Binary(data),
    }
}

fn error_to_serializable(e: &Error) -> SerializableWebsocketError {
    match e {
        Error::ConnectionFailure(s) => SerializableWebsocketError::ConnectionFailure(s.clone()),
        Error::SendFailure(s) => SerializableWebsocketError::SendFailure(s.clone()),
        Error::ReceiveFailure(s) => SerializableWebsocketError::ReceiveFailure(s.clone()),
        Error::ProtocolError(s) => SerializableWebsocketError::ProtocolError(s.clone()),
        Error::Closed(c) => SerializableWebsocketError::Closed(c.as_ref().map(|ci| {
            SerializableWebsocketCloseInfo {
                code: ci.code,
                reason: ci.reason.clone(),
            }
        })),
        Error::Other(s) => SerializableWebsocketError::Other(s.clone()),
    }
}

fn serializable_error_to_error(e: SerializableWebsocketError) -> Error {
    match e {
        SerializableWebsocketError::ConnectionFailure(s) => Error::ConnectionFailure(s),
        SerializableWebsocketError::SendFailure(s) => Error::SendFailure(s),
        SerializableWebsocketError::ReceiveFailure(s) => Error::ReceiveFailure(s),
        SerializableWebsocketError::ProtocolError(s) => Error::ProtocolError(s),
        SerializableWebsocketError::Closed(c) => Error::Closed(c.map(|ci| CloseInfo {
            code: ci.code,
            reason: ci.reason,
        })),
        SerializableWebsocketError::Other(s) => Error::Other(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::{test, timeout};

    #[test]
    #[timeout("10s")]
    async fn occupied_pool_observes_preexisting_deadline_or_entity_cancellation() {
        use golem_common::model::Timestamp;
        use golem_service_base::error::worker_executor::InterruptKind;
        use tokio_util::sync::CancellationToken;

        let pool = WebSocketConnectionPool::new(1);
        let held = pool.acquire().await.unwrap();
        let deadline = std::future::ready(InterruptKind::Interrupt(Timestamp::now_utc()));
        assert!(matches!(
            acquire_websocket_connection_or_interrupt(&pool, deadline)
                .await
                .unwrap(),
            Err(InterruptKind::Interrupt(_))
        ));

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            acquire_websocket_connection_or_interrupt(&pool, async move {
                cancellation.cancelled().await;
                InterruptKind::Interrupt(Timestamp::now_utc())
            })
            .await
            .unwrap(),
            Err(InterruptKind::Interrupt(_))
        ));
        drop(held);
        assert!(
            acquire_websocket_connection_or_interrupt(&pool, std::future::pending())
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[test]
    fn permission_denial_uses_the_existing_other_error_case() {
        let denied = Error::Other("permission denied".to_string());

        assert!(matches!(
            serializable_error_to_error(error_to_serializable(&denied)),
            Error::Other(message) if message == "permission denied"
        ));
    }

    #[test]
    fn replay_entry_with_the_same_gate_reconnects() {
        let gate = Arc::new(Mutex::new(()));
        let entry = WebSocketConnectionEntry::Replay(gate.clone());

        assert!(matches!(
            classify_reconnect_entry(&entry, &gate),
            ReconnectEntryAction::Reconnect
        ));
    }

    #[test]
    fn replay_entry_with_a_different_gate_is_an_inconsistent_replay_gate() {
        // The guest cannot drop a handle while one of its calls queues on the
        // gate (`send`/`receive`/`receive-with-timeout`/`close` borrow the
        // resource), and reconstruction discards queued calls, so a call
        // holding the gate can only ever see its own replay incarnation in the
        // entry. A different incarnation is an invariant violation of the gate
        // protocol, not a reconnect this call may perform.
        let queued_gate = Arc::new(Mutex::new(()));
        let entry = WebSocketConnectionEntry::Replay(Arc::new(Mutex::new(())));

        assert!(matches!(
            classify_reconnect_entry(&entry, &queued_gate),
            ReconnectEntryAction::InconsistentReplayGate
        ));
    }

    #[test]
    fn terminal_entry_fails_with_the_terminal_error() {
        // A call queued on the gate fails with the terminal error another
        // call published while it queued. (`Live` is exercised by the
        // integration follower path: constructing a `LiveWebSocketConnection`
        // requires a real websocket stream.)
        let gate = Arc::new(Mutex::new(()));
        let entry = WebSocketConnectionEntry::Terminal(TerminalWebSocketError::ConnectionFailure(
            "peer refused".to_string(),
        ));

        match classify_reconnect_entry(&entry, &gate) {
            ReconnectEntryAction::Fail(TerminalWebSocketError::ConnectionFailure(reason)) => {
                assert_eq!(reason, "peer refused");
            }
            other => panic!("expected Fail(ConnectionFailure), got {other:?}"),
        }
    }
}

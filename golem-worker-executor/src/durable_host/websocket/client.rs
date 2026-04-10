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

use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::websocket::client::{
    CloseInfo, Error, Host, HostWebsocketConnection, Message,
};
use crate::workerctx::WorkerCtx;
use futures::future::Either;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt, pin_mut};
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
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::{MaybeTlsStream, connect_async};
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

type WsStream = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct ReaderState {
    stream: SplitStream<WsStream>,
    /// Populated by `Pollable::ready()` as a read-ahead buffer so that
    /// after the guest observes readiness, `receive()`/`receive_with_timeout()`
    /// can return without consuming an additional websocket frame.
    pending: Option<Result<Message, Error>>,
}

/// Live TCP/WebSocket state for a guest connection handle (`WebSocketConnectionEntry::Live`).
/// Fields are private to this module; the type is `pub` only so the resource entry enum remains public.
pub struct LiveWebSocketConnection {
    writer: Mutex<SplitSink<WsStream, tungstenite::Message>>,
    reader: Mutex<ReaderState>,
    /// Held for the lifetime of the connection to limit concurrent WebSocket
    /// connections per executor. Released when the connection is dropped.
    _permit: tokio::sync::OwnedSemaphorePermit,
}

#[derive(Clone)]
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
    /// Boxed so `Replay` stays small (`clippy::large_enum_variant`).
    Live(Box<LiveWebSocketConnection>),
    Replay,
    Terminal(TerminalWebSocketError),
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for WebSocketConnectionEntry {
    async fn ready(&mut self) {
        match self {
            WebSocketConnectionEntry::Live(live) => {
                let mut reader = live.reader.lock().await;
                if reader.pending.is_some() {
                    return;
                }
                let next = read_next_user_or_close(&mut reader.stream).await;
                reader.pending = Some(next);
            }
            WebSocketConnectionEntry::Replay | WebSocketConnectionEntry::Terminal(_) => {}
        }
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> HostWebsocketConnection for DurableWorkerCtx<Ctx> {
    async fn connect(
        &mut self,
        url: String,
        headers: Option<Vec<(String, String)>>,
    ) -> anyhow::Result<Result<Resource<WebSocketConnectionEntry>, Error>> {
        self.observe_function_call("golem:websocket/client", "connect");

        let durability = Durability::<host_functions::WebsocketClientConnect>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let req = HostRequestWebsocketConnect {
            url: url.clone(),
            headers: headers.clone(),
        };

        if durability.is_live() {
            let request = match build_request(&url, headers.as_deref()) {
                Ok(req) => req,
                Err(e) => {
                    let resp = HostResponseWebsocketConnectResponse {
                        result: Err(SerializableWebsocketError::ConnectionFailure(e.clone())),
                    };
                    durability
                        .persist_raw(self, req.into(), resp.into())
                        .await?;
                    return Ok(Err(Error::ConnectionFailure(e)));
                }
            };

            let permit = self.websocket_connection_pool.acquire().await?;

            let interrupt_signal = self
                .execution_status
                .read()
                .unwrap()
                .create_await_interrupt_signal();

            let connect_fut = connect_async(request);
            pin_mut!(connect_fut);
            let connect_result = match futures::future::select(connect_fut, interrupt_signal).await
            {
                Either::Left((result, _)) => result,
                Either::Right((interrupt_kind, _)) => {
                    tracing::info!("Interrupted while waiting for WebSocket connect");
                    return Err(interrupt_kind.into());
                }
            };

            match connect_result {
                Ok((ws_stream, _response)) => {
                    let (writer, reader) = ws_stream.split();
                    let entry = WebSocketConnectionEntry::Live(Box::new(LiveWebSocketConnection {
                        writer: Mutex::new(writer),
                        reader: Mutex::new(ReaderState {
                            stream: reader,
                            pending: None,
                        }),
                        _permit: permit,
                    }));
                    let resource = self.as_wasi_view().table().push(entry)?;
                    self.register_open_websocket(resource.rep(), url.clone(), headers.clone());
                    let resp = HostResponseWebsocketConnectResponse { result: Ok(()) };
                    durability
                        .persist_raw(self, req.into(), resp.into())
                        .await?;
                    Ok(Ok(resource))
                }
                Err(e) => {
                    let resp = HostResponseWebsocketConnectResponse {
                        result: Err(SerializableWebsocketError::ConnectionFailure(e.to_string())),
                    };
                    durability
                        .persist_raw(self, req.into(), resp.into())
                        .await?;
                    Ok(Err(Error::ConnectionFailure(e.to_string())))
                }
            }
        } else {
            let resp: HostResponseWebsocketConnectResponse = durability.replay(self).await?;
            match resp.result {
                Ok(()) => {
                    let resource = self
                        .as_wasi_view()
                        .table()
                        .push(WebSocketConnectionEntry::Replay)?;
                    self.register_open_websocket(resource.rep(), url.clone(), headers.clone());
                    Ok(Ok(resource))
                }
                Err(e) => Ok(Err(serializable_error_to_error(e))),
            }
        }
    }

    async fn send(
        &mut self,
        self_: Resource<WebSocketConnectionEntry>,
        message: Message,
    ) -> anyhow::Result<Result<(), Error>> {
        self.observe_function_call("golem:websocket/client", "send");

        let durability = Durability::<host_functions::WebsocketClientSend>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let req = HostRequestWebsocketSend {
            message: message_to_serializable(&message),
        };

        if durability.is_live() {
            if let Err(error) = ensure_websocket_connection_live(self, &self_).await? {
                let resp = HostResponseWebsocketSendResponse {
                    result: Err(error_to_serializable(&error)),
                };
                durability
                    .persist_raw(self, req.into(), resp.into())
                    .await?;
                return Ok(Err(error));
            }

            let interrupt_signal = self
                .execution_status
                .read()
                .unwrap()
                .create_await_interrupt_signal();

            let mut view = self.as_wasi_view();
            let entry = view.table().get(&self_)?;
            let tungstenite_msg = to_tungstenite_message(message);
            let live_result = match entry {
                WebSocketConnectionEntry::Live(live) => {
                    let mut writer = live.writer.lock().await;
                    let send_fut = writer.send(tungstenite_msg);
                    pin_mut!(send_fut);
                    match futures::future::select(send_fut, interrupt_signal).await {
                        Either::Left((Ok(()), _)) => Ok(()),
                        Either::Left((Err(e), _)) => Err(Error::SendFailure(e.to_string())),
                        Either::Right((interrupt_kind, _)) => {
                            tracing::info!("Interrupted while waiting for WebSocket send");
                            return Err(interrupt_kind.into());
                        }
                    }
                }
                WebSocketConnectionEntry::Replay => {
                    unreachable!("live send path must not use Replay connection entry")
                }
                WebSocketConnectionEntry::Terminal(error) => Err(error.to_error()),
            };
            let ser_result = match &live_result {
                Ok(()) => Ok(()),
                Err(e) => Err(error_to_serializable(e)),
            };
            let resp = HostResponseWebsocketSendResponse { result: ser_result };
            durability
                .persist_raw(self, req.into(), resp.into())
                .await?;
            if let Some(terminal_error) = live_result
                .as_ref()
                .err()
                .and_then(terminal_websocket_error)
            {
                mark_websocket_terminal(self, &self_, terminal_error)?;
            }
            Ok(live_result)
        } else {
            let _ = self.as_wasi_view().table().get(&self_)?;
            let resp: HostResponseWebsocketSendResponse = durability.replay(self).await?;
            match resp.result {
                Ok(()) => Ok(Ok(())),
                Err(e) => {
                    let error = serializable_error_to_error(e);
                    if let Some(terminal_error) = terminal_websocket_error(&error) {
                        mark_websocket_terminal(self, &self_, terminal_error)?;
                    }
                    Ok(Err(error))
                }
            }
        }
    }

    async fn receive(
        &mut self,
        self_: Resource<WebSocketConnectionEntry>,
    ) -> anyhow::Result<Result<Message, Error>> {
        self.observe_function_call("golem:websocket/client", "receive");

        let durability = Durability::<host_functions::WebsocketClientReceive>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let req = HostRequestWebsocketReceive {};

        if durability.is_live() {
            if let Err(error) = ensure_websocket_connection_live(self, &self_).await? {
                let resp = HostResponseWebsocketReceiveResponse {
                    result: Err(error_to_serializable(&error)),
                };
                durability
                    .persist_raw(self, req.into(), resp.into())
                    .await?;
                return Ok(Err(error));
            }

            let interrupt_signal = self
                .execution_status
                .read()
                .unwrap()
                .create_await_interrupt_signal();

            let mut view = self.as_wasi_view();
            let entry = view.table().get(&self_)?;
            let live_result = match entry {
                WebSocketConnectionEntry::Live(live) => {
                    let mut reader = live.reader.lock().await;
                    if let Some(pending) = reader.pending.take() {
                        pending
                    } else {
                        let recv_fut = read_next_user_or_close(&mut reader.stream);
                        pin_mut!(recv_fut);
                        match futures::future::select(recv_fut, interrupt_signal).await {
                            Either::Left((result, _)) => result,
                            Either::Right((interrupt_kind, _)) => {
                                tracing::info!("Interrupted while waiting for WebSocket receive");
                                return Err(interrupt_kind.into());
                            }
                        }
                    }
                }
                WebSocketConnectionEntry::Replay => {
                    unreachable!("live receive path must not use Replay connection entry")
                }
                WebSocketConnectionEntry::Terminal(error) => Err(error.to_error()),
            };
            let ser_result = match &live_result {
                Ok(m) => Ok(message_to_serializable(m)),
                Err(e) => Err(error_to_serializable(e)),
            };
            let resp = HostResponseWebsocketReceiveResponse { result: ser_result };
            durability
                .persist_raw(self, req.into(), resp.into())
                .await?;
            if let Some(terminal_error) = live_result
                .as_ref()
                .err()
                .and_then(terminal_websocket_error)
            {
                mark_websocket_terminal(self, &self_, terminal_error)?;
            }
            Ok(live_result)
        } else {
            let _ = self.as_wasi_view().table().get(&self_)?;
            let resp: HostResponseWebsocketReceiveResponse = durability.replay(self).await?;
            match resp.result {
                Ok(m) => Ok(Ok(serializable_message_to_message(m))),
                Err(e) => {
                    let error = serializable_error_to_error(e);
                    if let Some(terminal_error) = terminal_websocket_error(&error) {
                        mark_websocket_terminal(self, &self_, terminal_error)?;
                    }
                    Ok(Err(error))
                }
            }
        }
    }

    async fn receive_with_timeout(
        &mut self,
        self_: Resource<WebSocketConnectionEntry>,
        timeout_ms: u64,
    ) -> anyhow::Result<Result<Option<Message>, Error>> {
        self.observe_function_call("golem:websocket/client", "receive-with-timeout");

        let durability = Durability::<host_functions::WebsocketClientReceiveWithTimeout>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let req = HostRequestWebsocketReceiveWithTimeout { timeout_ms };

        if durability.is_live() {
            if let Err(error) = ensure_websocket_connection_live(self, &self_).await? {
                let resp = HostResponseWebsocketReceiveWithTimeoutResponse {
                    result: Err(error_to_serializable(&error)),
                };
                durability
                    .persist_raw(self, req.into(), resp.into())
                    .await?;
                return Ok(Err(error));
            }

            let interrupt_signal = self
                .execution_status
                .read()
                .unwrap()
                .create_await_interrupt_signal();

            let mut view = self.as_wasi_view();
            let entry = view.table().get(&self_)?;
            let live_result: Result<Option<Message>, Error> = match entry {
                WebSocketConnectionEntry::Live(live) => {
                    let mut reader = live.reader.lock().await;
                    if let Some(pending) = reader.pending.take() {
                        match pending {
                            Ok(message) => Ok(Some(message)),
                            Err(err) => Err(err),
                        }
                    } else {
                        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
                        pin_mut!(interrupt_signal);
                        loop {
                            let remaining = deadline.saturating_duration_since(Instant::now());
                            if remaining.is_zero() {
                                break Ok(None);
                            }
                            let next_frame = tokio::time::timeout(remaining, reader.stream.next());
                            pin_mut!(next_frame);
                            match futures::future::select(next_frame, interrupt_signal.as_mut())
                                .await
                            {
                                Either::Left((Ok(Some(Ok(msg))), _)) => {
                                    match to_user_message(msg) {
                                        Ok(Some(message)) => break Ok(Some(message)),
                                        Ok(None) => continue,
                                        Err(err) => break Err(err),
                                    }
                                }
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
                                    return Err(interrupt_kind.into());
                                }
                            }
                        }
                    }
                }
                WebSocketConnectionEntry::Replay => {
                    unreachable!(
                        "live receive_with_timeout path must not use Replay connection entry"
                    )
                }
                WebSocketConnectionEntry::Terminal(error) => Err(error.to_error()),
            };
            let ser_result = match &live_result {
                Ok(Some(m)) => Ok(Some(message_to_serializable(m))),
                Ok(None) => Ok(None),
                Err(e) => Err(error_to_serializable(e)),
            };
            let resp = HostResponseWebsocketReceiveWithTimeoutResponse { result: ser_result };
            durability
                .persist_raw(self, req.into(), resp.into())
                .await?;
            if let Some(terminal_error) = live_result
                .as_ref()
                .err()
                .and_then(terminal_websocket_error)
            {
                mark_websocket_terminal(self, &self_, terminal_error)?;
            }
            Ok(live_result)
        } else {
            let _ = self.as_wasi_view().table().get(&self_)?;
            let resp: HostResponseWebsocketReceiveWithTimeoutResponse =
                durability.replay(self).await?;
            match resp.result {
                Ok(Some(m)) => Ok(Ok(Some(serializable_message_to_message(m)))),
                Ok(None) => Ok(Ok(None)),
                Err(e) => {
                    let error = serializable_error_to_error(e);
                    if let Some(terminal_error) = terminal_websocket_error(&error) {
                        mark_websocket_terminal(self, &self_, terminal_error)?;
                    }
                    Ok(Err(error))
                }
            }
        }
    }

    async fn close(
        &mut self,
        self_: Resource<WebSocketConnectionEntry>,
        code: Option<u16>,
        reason: Option<String>,
    ) -> anyhow::Result<Result<(), Error>> {
        self.observe_function_call("golem:websocket/client", "close");

        let durability = Durability::<host_functions::WebsocketClientClose>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let req = HostRequestWebsocketClose {
            code,
            reason: reason.clone(),
        };

        let terminal_close_error =
            TerminalWebSocketError::Closed(Some(SerializableWebsocketCloseInfo {
                code: code.unwrap_or(1000),
                reason: reason
                    .clone()
                    .unwrap_or_else(|| "Connection closed".to_string()),
            }));

        if durability.is_live() {
            if let Err(error) = ensure_websocket_connection_live(self, &self_).await? {
                let resp = HostResponseWebsocketCloseResponse {
                    result: Err(error_to_serializable(&error)),
                };
                durability
                    .persist_raw(self, req.into(), resp.into())
                    .await?;
                return Ok(Err(error));
            }

            let interrupt_signal = self
                .execution_status
                .read()
                .unwrap()
                .create_await_interrupt_signal();

            let mut view = self.as_wasi_view();
            let entry = view.table().get(&self_)?;
            let live_result = match entry {
                WebSocketConnectionEntry::Live(live) => {
                    let close_frame = tungstenite::protocol::CloseFrame {
                        code: tungstenite::protocol::frame::coding::CloseCode::from(
                            code.unwrap_or(1000),
                        ),
                        reason: reason.unwrap_or_default().into(),
                    };
                    let mut writer = live.writer.lock().await;
                    let close_fut = writer.send(tungstenite::Message::Close(Some(close_frame)));
                    pin_mut!(close_fut);
                    match futures::future::select(close_fut, interrupt_signal).await {
                        Either::Left((Ok(()), _)) => Ok(()),
                        Either::Left((Err(e), _)) => Err(Error::SendFailure(e.to_string())),
                        Either::Right((interrupt_kind, _)) => {
                            tracing::info!("Interrupted while waiting for WebSocket close");
                            return Err(interrupt_kind.into());
                        }
                    }
                }
                WebSocketConnectionEntry::Replay => {
                    unreachable!("live close path must not use Replay connection entry")
                }
                WebSocketConnectionEntry::Terminal(error) => Err(error.to_error()),
            };
            let ser_result = match &live_result {
                Ok(()) => Ok(()),
                Err(e) => Err(error_to_serializable(e)),
            };
            let resp = HostResponseWebsocketCloseResponse { result: ser_result };
            durability
                .persist_raw(self, req.into(), resp.into())
                .await?;
            if live_result.is_ok() {
                mark_websocket_terminal(self, &self_, terminal_close_error.clone())?;
            } else if let Some(terminal_error) = live_result
                .as_ref()
                .err()
                .and_then(terminal_websocket_error)
            {
                mark_websocket_terminal(self, &self_, terminal_error)?;
            }
            Ok(live_result)
        } else {
            let _ = self.as_wasi_view().table().get(&self_)?;
            let resp: HostResponseWebsocketCloseResponse = durability.replay(self).await?;
            match resp.result {
                Ok(()) => {
                    mark_websocket_terminal(self, &self_, terminal_close_error)?;
                    Ok(Ok(()))
                }
                Err(e) => {
                    let error = serializable_error_to_error(e);
                    if let Some(terminal_error) = terminal_websocket_error(&error) {
                        mark_websocket_terminal(self, &self_, terminal_error)?;
                    }
                    Ok(Err(error))
                }
            }
        }
    }

    async fn subscribe(
        &mut self,
        self_: Resource<WebSocketConnectionEntry>,
    ) -> anyhow::Result<Resource<wasmtime_wasi::p2::bindings::io::poll::Pollable>> {
        self.observe_function_call("golem:websocket/client", "subscribe");
        if self.state.is_live() {
            self.process_pending_replay_events().await?;
        }
        Ok(wasmtime_wasi::subscribe(self.table(), self_, None)?)
    }

    async fn drop(&mut self, rep: Resource<WebSocketConnectionEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem:websocket/client", "drop");
        self.unregister_open_websocket(rep.rep());
        self.as_wasi_view().table().delete(rep)?;
        Ok(())
    }
}

async fn ensure_websocket_connection_live<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WebSocketConnectionEntry>,
) -> anyhow::Result<Result<(), Error>> {
    let rep = resource.rep();
    let is_replay_entry = {
        let mut view = ctx.as_wasi_view();
        let entry = view.table().get(resource)?;
        match entry {
            WebSocketConnectionEntry::Replay => true,
            WebSocketConnectionEntry::Live(_) => false,
            WebSocketConnectionEntry::Terminal(error) => return Ok(Err(error.to_error())),
        }
    };

    let Some(info) = ctx.websocket_connection_info(rep) else {
        debug_assert!(
            !is_replay_entry,
            "Replay entry must have connection info registered via connect()"
        );
        return Ok(Ok(()));
    };

    if !is_replay_entry {
        return Ok(Ok(()));
    }

    let request = match build_request(&info.url, info.headers.as_deref()) {
        Ok(request) => request,
        Err(err) => {
            let error = Error::ConnectionFailure(err);
            mark_websocket_terminal(
                ctx,
                resource,
                terminal_websocket_error(&error)
                    .expect("connection failures must be terminal websocket errors"),
            )?;
            return Ok(Err(error));
        }
    };

    let permit = ctx.websocket_connection_pool.acquire().await?;
    let interrupt_signal = ctx
        .execution_status
        .read()
        .unwrap()
        .create_await_interrupt_signal();

    let connect_fut = connect_async(request);
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
            let error = Error::ConnectionFailure(err.to_string());
            mark_websocket_terminal(
                ctx,
                resource,
                terminal_websocket_error(&error)
                    .expect("connection failures must be terminal websocket errors"),
            )?;
            return Ok(Err(error));
        }
    };

    let (writer, reader) = ws_stream.split();
    let new_entry = WebSocketConnectionEntry::Live(Box::new(LiveWebSocketConnection {
        writer: Mutex::new(writer),
        reader: Mutex::new(ReaderState {
            stream: reader,
            pending: None,
        }),
        _permit: permit,
    }));

    {
        let mut view = ctx.as_wasi_view();
        let entry = view.table().get_mut(resource)?;
        *entry = new_entry;
    }

    Ok(Ok(()))
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
        tungstenite::Message::Binary(data) => Ok(Some(Message::Binary(data.as_slice().to_vec()))),
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

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
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
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

pub enum WebSocketConnectionEntry {
    /// Boxed so `Replay` stays small (`clippy::large_enum_variant`).
    Live(Box<LiveWebSocketConnection>),
    Replay,
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
            WebSocketConnectionEntry::Replay => {}
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

            match connect_async(request).await {
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
                    self.register_open_websocket(resource.rep());
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
                    self.register_open_websocket(resource.rep());
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
            let mut view = self.as_wasi_view();
            let entry = view.table().get(&self_)?;
            let tungstenite_msg = to_tungstenite_message(message);
            let live_result = match entry {
                WebSocketConnectionEntry::Live(live) => {
                    let mut writer = live.writer.lock().await;
                    match writer.send(tungstenite_msg).await {
                        Ok(()) => Ok(()),
                        Err(e) => Err(Error::SendFailure(e.to_string())),
                    }
                }
                WebSocketConnectionEntry::Replay => {
                    unreachable!("live send path must not use Replay connection entry")
                }
            };
            let ser_result = match &live_result {
                Ok(()) => Ok(()),
                Err(e) => Err(error_to_serializable(e)),
            };
            let resp = HostResponseWebsocketSendResponse { result: ser_result };
            durability
                .persist_raw(self, req.into(), resp.into())
                .await?;
            Ok(live_result)
        } else {
            let _ = self.as_wasi_view().table().get(&self_)?;
            let resp: HostResponseWebsocketSendResponse = durability.replay(self).await?;
            match resp.result {
                Ok(()) => Ok(Ok(())),
                Err(e) => Ok(Err(serializable_error_to_error(e))),
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
            let mut view = self.as_wasi_view();
            let entry = view.table().get(&self_)?;
            let live_result = match entry {
                WebSocketConnectionEntry::Live(live) => {
                    let mut reader = live.reader.lock().await;
                    if let Some(pending) = reader.pending.take() {
                        pending
                    } else {
                        match read_next_user_or_close(&mut reader.stream).await {
                            Ok(message) => Ok(message),
                            Err(err) => Err(err),
                        }
                    }
                }
                WebSocketConnectionEntry::Replay => {
                    unreachable!("live receive path must not use Replay connection entry")
                }
            };
            let ser_result = match &live_result {
                Ok(m) => Ok(message_to_serializable(m)),
                Err(e) => Err(error_to_serializable(e)),
            };
            let resp = HostResponseWebsocketReceiveResponse { result: ser_result };
            durability
                .persist_raw(self, req.into(), resp.into())
                .await?;
            Ok(live_result)
        } else {
            let _ = self.as_wasi_view().table().get(&self_)?;
            let resp: HostResponseWebsocketReceiveResponse = durability.replay(self).await?;
            match resp.result {
                Ok(m) => Ok(Ok(serializable_message_to_message(m))),
                Err(e) => Ok(Err(serializable_error_to_error(e))),
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
                        loop {
                            let remaining = deadline.saturating_duration_since(Instant::now());
                            if remaining.is_zero() {
                                break Ok(None);
                            }
                            match tokio::time::timeout(remaining, reader.stream.next()).await {
                                Ok(Some(Ok(msg))) => match to_user_message(msg) {
                                    Ok(Some(message)) => break Ok(Some(message)),
                                    Ok(None) => continue,
                                    Err(err) => break Err(err),
                                },
                                Ok(Some(Err(e))) => break Err(to_wit_error(e)),
                                Ok(None) => {
                                    break Err(Error::Closed(Some(CloseInfo {
                                        code: 1000,
                                        reason: "Connection closed".to_string(),
                                    })));
                                }
                                Err(_) => break Ok(None),
                            }
                        }
                    }
                }
                WebSocketConnectionEntry::Replay => {
                    unreachable!(
                        "live receive_with_timeout path must not use Replay connection entry"
                    )
                }
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
            Ok(live_result)
        } else {
            let _ = self.as_wasi_view().table().get(&self_)?;
            let resp: HostResponseWebsocketReceiveWithTimeoutResponse =
                durability.replay(self).await?;
            match resp.result {
                Ok(Some(m)) => Ok(Ok(Some(serializable_message_to_message(m)))),
                Ok(None) => Ok(Ok(None)),
                Err(e) => Ok(Err(serializable_error_to_error(e))),
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

        let rep_id = self_.rep();

        if durability.is_live() {
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
                    match writer
                        .send(tungstenite::Message::Close(Some(close_frame)))
                        .await
                    {
                        Ok(()) => Ok(()),
                        Err(e) => Err(Error::SendFailure(e.to_string())),
                    }
                }
                WebSocketConnectionEntry::Replay => {
                    unreachable!("live close path must not use Replay connection entry")
                }
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
                self.unregister_open_websocket(rep_id);
            }
            Ok(live_result)
        } else {
            let _ = self.as_wasi_view().table().get(&self_)?;
            let resp: HostResponseWebsocketCloseResponse = durability.replay(self).await?;
            match resp.result {
                Ok(()) => {
                    self.unregister_open_websocket(rep_id);
                    Ok(Ok(()))
                }
                Err(e) => Ok(Err(serializable_error_to_error(e))),
            }
        }
    }

    async fn subscribe(
        &mut self,
        self_: Resource<WebSocketConnectionEntry>,
    ) -> anyhow::Result<Resource<wasmtime_wasi::p2::bindings::io::poll::Pollable>> {
        self.observe_function_call("golem:websocket/client", "subscribe");
        Ok(wasmtime_wasi::subscribe(self.table(), self_, None)?)
    }

    async fn drop(&mut self, rep: Resource<WebSocketConnectionEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem:websocket/client", "drop");
        self.unregister_open_websocket(rep.rep());
        self.as_wasi_view().table().delete(rep)?;
        Ok(())
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

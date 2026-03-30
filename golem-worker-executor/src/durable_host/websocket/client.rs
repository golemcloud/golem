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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::websocket::client::{
    CloseInfo, Error, Host, HostWebsocketConnection, Message,
};
use crate::workerctx::WorkerCtx;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::{connect_async, MaybeTlsStream};
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

pub struct WebSocketConnectionEntry {
    writer: Mutex<SplitSink<WsStream, tungstenite::Message>>,
    reader: Mutex<ReaderState>,
    /// Held for the lifetime of the connection to limit concurrent WebSocket
    /// connections per executor. Released when the connection is dropped.
    _permit: tokio::sync::OwnedSemaphorePermit,
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for WebSocketConnectionEntry {
    async fn ready(&mut self) {
        // If we already have a buffered message/error, report readiness immediately.
        let mut reader = self.reader.lock().await;
        if reader.pending.is_some() {
            return;
        }

        // Read ahead until we can either:
        // - return a user-visible websocket message (Text/Binary)
        // - or report a websocket close/error
        let next = read_next_user_or_close(&mut reader.stream).await;
        reader.pending = Some(next);
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

        let request = match build_request(&url, headers.as_deref()) {
            Ok(req) => req,
            Err(e) => return Ok(Err(Error::ConnectionFailure(e.to_string()))),
        };

        // Acquire a permit from the per-executor connection pool to limit
        // concurrent WebSocket connections and protect against socket exhaustion.
        let permit = self.websocket_connection_pool.acquire().await?;

        match connect_async(request).await {
            Ok((ws_stream, _response)) => {
                let (writer, reader) = ws_stream.split();
                let entry = WebSocketConnectionEntry {
                    writer: Mutex::new(writer),
                    reader: Mutex::new(ReaderState {
                        stream: reader,
                        pending: None,
                    }),
                    _permit: permit,
                };
                let resource = self.as_wasi_view().table().push(entry)?;
                Ok(Ok(resource))
            }
            Err(e) => {
                // permit is dropped here, releasing the slot
                Ok(Err(Error::ConnectionFailure(e.to_string())))
            }
        }
    }

    async fn send(
        &mut self,
        self_: Resource<WebSocketConnectionEntry>,
        message: Message,
    ) -> anyhow::Result<Result<(), Error>> {
        self.observe_function_call("golem:websocket/client", "send");

        let tungstenite_msg = to_tungstenite_message(message);
        let mut view = self.as_wasi_view();
        let entry = view.table().get(&self_)?;

        let mut writer = entry.writer.lock().await;
        match writer.send(tungstenite_msg).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(Error::SendFailure(e.to_string()))),
        }
    }

    async fn receive(
        &mut self,
        self_: Resource<WebSocketConnectionEntry>,
    ) -> anyhow::Result<Result<Message, Error>> {
        self.observe_function_call("golem:websocket/client", "receive");

        let mut view = self.as_wasi_view();
        let entry = view.table().get(&self_)?;
        let mut reader = entry.reader.lock().await;

        if let Some(pending) = reader.pending.take() {
            return Ok(pending);
        }

        match read_next_user_or_close(&mut reader.stream).await {
            Ok(message) => Ok(Ok(message)),
            Err(err) => Ok(Err(err)),
        }
    }

    async fn receive_with_timeout(
        &mut self,
        self_: Resource<WebSocketConnectionEntry>,
        timeout_ms: u64,
    ) -> anyhow::Result<Result<Option<Message>, Error>> {
        self.observe_function_call("golem:websocket/client", "receive-with-timeout");

        let mut view = self.as_wasi_view();
        let entry = view.table().get(&self_)?;
        let mut reader = entry.reader.lock().await;

        if let Some(pending) = reader.pending.take() {
            return Ok(match pending {
                Ok(message) => Ok(Some(message)),
                Err(err) => Err(err),
            });
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(Ok(None)); // overall timeout expired
            }

            match tokio::time::timeout(remaining, reader.stream.next()).await {
                Ok(Some(Ok(msg))) => match to_user_message(msg) {
                    Ok(Some(message)) => return Ok(Ok(Some(message))),
                    Ok(None) => continue, // ignore ping/pong/frames and wait further
                    Err(err) => return Ok(Err(err)),
                },
                Ok(Some(Err(e))) => return Ok(Err(to_wit_error(e))),
                Ok(None) => {
                    return Ok(Err(Error::Closed(Some(CloseInfo {
                        code: 1000,
                        reason: "Connection closed".to_string(),
                    }))))
                }
                Err(_) => return Ok(Ok(None)), // overall timeout expired
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

        let mut view = self.as_wasi_view();
        let entry = view.table().get(&self_)?;
        let close_frame = tungstenite::protocol::CloseFrame {
            code: tungstenite::protocol::frame::coding::CloseCode::from(code.unwrap_or(1000)),
            reason: reason.unwrap_or_default().into(),
        };

        let mut writer = entry.writer.lock().await;
        match writer
            .send(tungstenite::Message::Close(Some(close_frame)))
            .await
        {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(Error::SendFailure(e.to_string()))),
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
                Ok(None) => continue, // ignore ping/pong/frames
                Err(err) => return Err(err),
            },
            Some(Err(e)) => return Err(to_wit_error(e)),
            None => {
                return Err(Error::Closed(Some(CloseInfo {
                    code: 1000,
                    reason: "Connection closed".to_string(),
                })))
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

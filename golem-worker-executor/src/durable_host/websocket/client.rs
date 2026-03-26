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
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::{connect_async, MaybeTlsStream};
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

type WsStream = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub struct WebSocketConnectionEntry {
    writer: Mutex<SplitSink<WsStream, tungstenite::Message>>,
    reader: Mutex<SplitStream<WsStream>>,
    /// Held for the lifetime of the connection to limit concurrent WebSocket
    /// connections per executor. Released when the connection is dropped.
    _permit: tokio::sync::OwnedSemaphorePermit,
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for WebSocketConnectionEntry {
    async fn ready(&mut self) {}
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
                    reader: Mutex::new(reader),
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

        loop {
            match reader.next().await {
                Some(Ok(msg)) => match from_tungstenite_message(msg) {
                    Some(message) => return Ok(Ok(message)),
                    None => continue,
                },
                Some(Err(e)) => return Ok(Err(to_wit_error(e))),
                None => {
                    return Ok(Err(Error::Closed(Some(CloseInfo {
                        code: 1000,
                        reason: "Connection closed".to_string(),
                    }))));
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

        let mut view = self.as_wasi_view();
        let entry = view.table().get(&self_)?;
        let mut reader = entry.reader.lock().await;

        let timeout = tokio::time::Duration::from_millis(timeout_ms);

        loop {
            match tokio::time::timeout(timeout, reader.next()).await {
                Ok(Some(Ok(msg))) => match from_tungstenite_message(msg) {
                    Some(message) => return Ok(Ok(Some(message))),
                    None => continue,
                },
                Ok(Some(Err(e))) => return Ok(Err(to_wit_error(e))),
                Ok(None) => {
                    return Ok(Err(Error::Closed(Some(CloseInfo {
                        code: 1000,
                        reason: "Connection closed".to_string(),
                    }))));
                }
                Err(_) => return Ok(Ok(None)), // timeout expired
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
            if let (Ok(header_name), Ok(header_value)) = (
                tungstenite::http::header::HeaderName::try_from(name.as_str()),
                tungstenite::http::header::HeaderValue::try_from(value.as_str()),
            ) {
                req_headers.insert(header_name, header_value);
            }
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

fn from_tungstenite_message(msg: tungstenite::Message) -> Option<Message> {
    match msg {
        tungstenite::Message::Text(text) => Some(Message::Text(text.as_str().to_owned())),
        tungstenite::Message::Binary(data) => Some(Message::Binary(data.as_slice().to_vec())),
        tungstenite::Message::Close(_) => None,
        tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => None,
        tungstenite::Message::Frame(_) => None,
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

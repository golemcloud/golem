// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::command_handler::worker::parse_worker_error;
use crate::command_handler::worker::stream_output::WorkerStreamOutput;
use crate::model::{Format, WorkerConnectOptions};
use anyhow::{anyhow, Context};
use bytes::Bytes;
use futures_util::future::Either;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{future, pin_mut, SinkExt, StreamExt, TryStreamExt};
use golem_common::model::{IdempotencyKey, Timestamp, WorkerEvent};
use native_tls::TlsConnector;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::{task, time};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite, Connector, MaybeTlsStream, WebSocketStream,
};
use tracing::{debug, error, info, trace};
use url::Url;
use uuid::Uuid;

pub struct WorkerConnection {
    request: Request,
    connector: Option<Connector>,
    output: WorkerStreamOutput,
    idempotency_key: Option<IdempotencyKey>,
    last_seen_idempotency_key: Arc<Mutex<Option<IdempotencyKey>>>,
    goal_reached: Arc<AtomicBool>,
}

impl WorkerConnection {
    /// Initializes a worker stream. Use `run_forever` to connect and output worker events.
    pub async fn new(
        worker_service_url: Url,
        auth_token: String,
        component_id: Uuid,
        worker_name: String,
        connect_options: WorkerConnectOptions,
        allow_insecure: bool,
        format: Format,
        idempotency_key: Option<IdempotencyKey>,
    ) -> anyhow::Result<WorkerConnection> {
        let (request, connector) = Self::create_request(
            worker_service_url,
            auth_token,
            component_id,
            worker_name,
            allow_insecure,
        )?;
        let output = WorkerStreamOutput::new(connect_options, format);

        let last_seen_idempotency_key = Arc::new(Mutex::new(None));
        let goal_reached = Arc::new(AtomicBool::new(false));

        Ok(Self {
            request,
            connector,
            output,
            idempotency_key,
            last_seen_idempotency_key,
            goal_reached,
        })
    }

    /// Creates a new worker connection and every time the connection is dropped tries to
    /// reconnect. If there was an idempotency_key goal, and it has been reached, the loop
    /// exits.
    pub async fn run_forever(self) {
        while !self.goal_reached.load(Ordering::Acquire) {
            let _ = self.run().await;
            self.output.flush().await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Connects to the worker event stream and outputs incoming messages until the connection is dropped
    async fn run(&self) -> anyhow::Result<()> {
        let (ws_stream, _) = connect_async_tls_with_config(
            self.request.clone(),
            None,
            false,
            self.connector.clone(),
        )
        .await
        .map_err(|e| match e {
            tungstenite::error::Error::Http(http_error_response) => {
                let status = http_error_response.status().as_u16();
                match http_error_response.body().clone() {
                    Some(body) => anyhow!(parse_worker_error(status, body)),
                    None => anyhow!("Websocket connect failed, HTTP error: {}", status),
                }
            }
            _ => anyhow!("Websocket connect failed, error: {}", e),
        })?;

        let (write, read) = ws_stream.split();

        let pings = task::spawn(async move { Self::ping_loop(write).await });

        let output = self.output.clone();
        let last_seen_idempotency_key = self.last_seen_idempotency_key.clone();
        let idempotency_key = self.idempotency_key.clone();
        let goal_reached = self.goal_reached.clone();
        let read_messages = task::spawn(async move {
            Self::read_loop(
                read,
                output,
                last_seen_idempotency_key,
                idempotency_key,
                goal_reached,
            )
            .await;
        });

        pin_mut!(pings, read_messages);
        match future::select(pings, read_messages).await {
            Either::Left((Ok(result), _)) => Err(result),
            Either::Left((Err(err), _)) => Err(anyhow!(err)),
            Either::Right((Ok(_), _)) => Ok(()),
            Either::Right((Err(err), _)) => Err(anyhow!(err)),
        }
    }

    fn create_request(
        worker_service_url: Url,
        auth_token: String,
        component_id: Uuid,
        worker_name: String,
        allow_insecure: bool,
    ) -> anyhow::Result<(Request, Option<Connector>)> {
        let mut url = worker_service_url;

        let ws_schema = if url.scheme() == "http" { "ws" } else { "wss" };

        url.set_scheme(ws_schema)
            .map_err(|()| anyhow!("Failed to set ws url schema".to_string()))?;
        url.path_segments_mut()
            .map_err(|()| anyhow!("Failed to get url path for ws url".to_string()))?
            .push("v1")
            .push("components")
            .push(&component_id.to_string())
            .push("workers")
            .push(&worker_name)
            .push("connect");

        debug!(url = url.as_str(), "Worker stream connect");

        let mut request = url
            .to_string()
            .into_client_request()
            .context("Failed to create request")?;

        {
            let headers = request.headers_mut();
            headers.insert("Authorization", format!("Bearer {auth_token}").parse()?);
        }

        let connector = if allow_insecure {
            Some(Connector::NativeTls(
                TlsConnector::builder()
                    .danger_accept_invalid_certs(true)
                    .danger_accept_invalid_hostnames(true)
                    .build()?,
            ))
        } else {
            None
        };

        Ok((request, connector))
    }

    async fn ping_loop(
        mut write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    ) -> anyhow::Error {
        let mut interval = time::interval(Duration::from_secs(1)); // TODO configure
        let mut cnt: i64 = 1;

        loop {
            interval.tick().await;

            let ping_result = write
                .send(Message::Ping(
                    Bytes::from(cnt.to_ne_bytes().to_vec()).into(),
                ))
                .await
                .context("Failed to send ping");

            if let Err(err) = ping_result {
                break err;
            }

            cnt += 1;
        }
    }

    async fn read_loop(
        read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        output: WorkerStreamOutput,
        last_seen_idempotency_key: Arc<Mutex<Option<IdempotencyKey>>>,
        idempotency_key_to_look_for: Option<IdempotencyKey>,
        goal_reached: Arc<AtomicBool>,
    ) {
        let result = read
            .try_for_each(|message| {
                let output = output.clone();
                let idempotency_key_to_look_for = idempotency_key_to_look_for.clone();
                let last_seen_idempotency_key = last_seen_idempotency_key.clone();
                let goal_reached = goal_reached.clone();
                async move {
                    let mut last_seen_idempotency_key = last_seen_idempotency_key.lock().await;
                    let matching = match &idempotency_key_to_look_for {
                        Some(idempotency_key_to_look_for) => {
                            if let Some(last_seen_idempotency_key) = &*last_seen_idempotency_key {
                                idempotency_key_to_look_for == last_seen_idempotency_key
                            } else {
                                false
                            }
                        }
                        None => true,
                    };

                    let worker_event = Self::parse_websocket_message(message);
                    match worker_event {
                        None => {}
                        Some(msg) => match msg {
                            WorkerEvent::StdOut { timestamp, bytes } => {
                                if matching {
                                    output
                                        .emit_stdout(
                                            timestamp,
                                            String::from_utf8_lossy(&bytes).to_string(),
                                        )
                                        .await;
                                }
                            }
                            WorkerEvent::StdErr { timestamp, bytes } => {
                                if matching {
                                    output
                                        .emit_stderr(
                                            timestamp,
                                            String::from_utf8_lossy(&bytes).to_string(),
                                        )
                                        .await;
                                }
                            }
                            WorkerEvent::Log {
                                timestamp,
                                level,
                                context,
                                message,
                            } => {
                                if matching {
                                    output.emit_log(timestamp, level, context, message).await;
                                }
                            }
                            WorkerEvent::InvocationStart {
                                timestamp,
                                function,
                                idempotency_key,
                            } => {
                                if matching
                                    || idempotency_key_to_look_for.as_ref()
                                        == Some(&idempotency_key)
                                {
                                    *last_seen_idempotency_key = Some(idempotency_key.clone());
                                    output
                                        .emit_invocation_start(timestamp, function, idempotency_key)
                                        .await;
                                }
                            }
                            WorkerEvent::InvocationFinished {
                                timestamp,
                                function,
                                idempotency_key,
                            } => {
                                if matching {
                                    output
                                        .emit_invocation_finished(
                                            timestamp,
                                            function,
                                            idempotency_key.clone(),
                                        )
                                        .await;

                                    last_seen_idempotency_key.take();
                                    if idempotency_key_to_look_for == Some(idempotency_key) {
                                        goal_reached.store(true, Ordering::Release);
                                    }
                                }
                            }
                            WorkerEvent::ClientLagged {
                                number_of_missed_messages,
                            } => {
                                output
                                    .emit_missed_messages(
                                        Timestamp::now_utc(),
                                        number_of_missed_messages,
                                    )
                                    .await;
                            }
                        },
                    }

                    if goal_reached.load(Ordering::Acquire) {
                        // Early return from the stream as the goal of observing a given idempotency key has been reached
                        Err(tungstenite::error::Error::ConnectionClosed)
                    } else {
                        Ok(())
                    }
                }
            })
            .await;

        match result {
            Ok(()) | Err(tungstenite::error::Error::ConnectionClosed) => {
                // successful exit
                output.emit_stream_closed(Timestamp::now_utc()).await;
            }
            Err(error) => {
                output.emit_stream_error(Timestamp::now_utc(), error).await;
            }
        }
    }

    fn parse_websocket_message(message: Message) -> Option<WorkerEvent> {
        match message {
            Message::Text(str) => {
                let parsed: serde_json::Result<WorkerEvent> = serde_json::from_str(str.as_str());

                match parsed {
                    Ok(parsed) => Some(parsed),
                    Err(err) => {
                        error!("Failed to parse worker connect message: {err}");
                        None
                    }
                }
            }
            Message::Binary(data) => {
                let parsed: serde_json::Result<WorkerEvent> =
                    serde_json::from_slice(data.as_slice());
                match parsed {
                    Ok(parsed) => Some(parsed),
                    Err(err) => {
                        error!("Failed to parse worker connect message: {err}");
                        None
                    }
                }
            }
            Message::Ping(_) => {
                trace!("Ignore ping");
                None
            }
            Message::Pong(_) => {
                trace!("Ignore pong");
                None
            }
            Message::Close(details) => {
                match details {
                    Some(closed_frame) => {
                        info!("Connection Closed: {}", closed_frame);
                    }
                    None => {
                        info!("Connection Closed");
                    }
                }
                None
            }
            Message::Frame(f) => {
                debug!("Ignored unexpected frame {f:?}");
                None
            }
        }
    }
}

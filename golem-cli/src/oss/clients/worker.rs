// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::time::Duration;

use crate::clients::worker::WorkerClient;
use async_trait::async_trait;
use futures_util::{future, pin_mut, SinkExt, StreamExt};
use golem_client::model::{
    InvokeParameters, InvokeResult, ScanCursor, UpdateWorkerRequest, WorkerCreationRequest,
    WorkerFilter, WorkerId, WorkersMetadataRequest,
};
use golem_client::Context;
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
use native_tls::TlsConnector;
use serde::Deserialize;
use tokio::{task, time};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tracing::{debug, info};

use crate::model::{
    GolemError, IdempotencyKey, WorkerMetadata, WorkerName, WorkerUpdateMode,
    WorkersMetadataResponse,
};

#[derive(Clone)]
pub struct WorkerClientLive<C: golem_client::api::WorkerClient + Sync + Send> {
    pub client: C,
    pub context: Context,
    pub allow_insecure: bool,
}

#[async_trait]
impl<C: golem_client::api::WorkerClient + Sync + Send> WorkerClient for WorkerClientLive<C> {
    async fn new_worker(
        &self,
        name: WorkerName,
        component_urn: ComponentUrn,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<WorkerId, GolemError> {
        info!("Creating worker {name} of {component_urn}");

        Ok(self
            .client
            .launch_new_worker(
                &component_urn.id.0,
                &WorkerCreationRequest {
                    name: name.0,
                    args,
                    env: env.into_iter().collect(),
                },
            )
            .await?
            .worker_id)
    }

    async fn invoke_and_await(
        &self,
        worker_urn: WorkerUrn,
        function: String,
        parameters: InvokeParameters,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<InvokeResult, GolemError> {
        info!("Invoke and await for function {function} in {worker_urn}");

        Ok(self
            .client
            .invoke_and_await_function(
                &worker_urn.id.component_id.0,
                &worker_urn.id.worker_name,
                idempotency_key.as_ref().map(|k| k.0.as_str()),
                &function,
                &parameters,
            )
            .await?)
    }

    async fn invoke(
        &self,
        worker_urn: WorkerUrn,
        function: String,
        parameters: InvokeParameters,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<(), GolemError> {
        info!("Invoke function {function} in {worker_urn}");

        let _ = self
            .client
            .invoke_function(
                &worker_urn.id.component_id.0,
                &worker_urn.id.worker_name,
                idempotency_key.as_ref().map(|k| k.0.as_str()),
                &function,
                &parameters,
            )
            .await?;
        Ok(())
    }

    async fn interrupt(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Interrupting {worker_urn}");

        let _ = self
            .client
            .interrupt_worker(
                &worker_urn.id.component_id.0,
                &worker_urn.id.worker_name,
                Some(false),
            )
            .await?;
        Ok(())
    }

    async fn simulated_crash(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Simulating crash of {worker_urn}");

        let _ = self
            .client
            .interrupt_worker(
                &worker_urn.id.component_id.0,
                &worker_urn.id.worker_name,
                Some(true),
            )
            .await?;
        Ok(())
    }

    async fn delete(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Deleting worker {worker_urn}");

        let _ = self
            .client
            .delete_worker(&worker_urn.id.component_id.0, &worker_urn.id.worker_name)
            .await?;
        Ok(())
    }

    async fn get_metadata(&self, worker_urn: WorkerUrn) -> Result<WorkerMetadata, GolemError> {
        info!("Getting worker {worker_urn} metadata");

        Ok(self
            .client
            .get_worker_metadata(&worker_urn.id.component_id.0, &worker_urn.id.worker_name)
            .await?
            .into())
    }

    async fn find_metadata(
        &self,
        component_urn: ComponentUrn,
        filter: Option<WorkerFilter>,
        cursor: Option<ScanCursor>,
        count: Option<u64>,
        precise: Option<bool>,
    ) -> Result<WorkersMetadataResponse, GolemError> {
        info!(
            "Getting workers metadata for component: {component_urn}, filter: {}",
            filter.is_some()
        );

        Ok(self
            .client
            .find_workers_metadata(
                &component_urn.id.0,
                &WorkersMetadataRequest {
                    filter,
                    cursor,
                    count,
                    precise,
                },
            )
            .await?
            .into())
    }

    async fn list_metadata(
        &self,
        component_urn: ComponentUrn,
        filter: Option<Vec<String>>,
        cursor: Option<ScanCursor>,
        count: Option<u64>,
        precise: Option<bool>,
    ) -> Result<WorkersMetadataResponse, GolemError> {
        info!(
            "Getting workers metadata for component: {component_urn}, filter: {}",
            filter
                .clone()
                .map(|fs| fs.join(" AND "))
                .unwrap_or("N/A".to_string())
        );

        let filter: Option<&[String]> = filter.as_deref();

        let cursor = cursor.map(|cursor| format!("{}/{}", cursor.layer, cursor.cursor));

        Ok(self
            .client
            .get_workers_metadata(
                &component_urn.id.0,
                filter,
                cursor.as_deref(),
                count,
                precise,
            )
            .await?
            .into())
    }

    async fn connect(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        let mut url = self.context.base_url.clone();

        let ws_schema = if url.scheme() == "http" { "ws" } else { "wss" };

        url.set_scheme(ws_schema)
            .map_err(|_| GolemError("Can't set schema.".to_string()))?;

        url.path_segments_mut()
            .map_err(|_| GolemError("Can't get path.".to_string()))?
            .push("v1")
            .push("components")
            .push(&worker_urn.id.component_id.0.to_string())
            .push("workers")
            .push(&worker_urn.id.worker_name)
            .push("connect");

        let mut request = url
            .into_client_request()
            .map_err(|e| GolemError(format!("Can't create request: {e}")))?;
        let headers = request.headers_mut();

        if let Some(token) = self.context.bearer_token() {
            headers.insert(
                "Authorization",
                format!("Bearer {}", token).parse().unwrap(),
            );
        }

        let connector = if self.allow_insecure {
            Some(Connector::NativeTls(
                TlsConnector::builder()
                    .danger_accept_invalid_certs(true)
                    .danger_accept_invalid_hostnames(true)
                    .build()
                    .unwrap(),
            ))
        } else {
            None
        };

        let (ws_stream, _) = connect_async_tls_with_config(request, None, false, connector)
            .await
            .map_err(|e| match e {
                tungstenite::error::Error::Http(http_error_response) => {
                    match http_error_response.body().clone() {
                        Some(body) => GolemError(format!(
                            "Failed Websocket. Http error: {}, {}",
                            http_error_response.status(),
                            String::from_utf8_lossy(&body)
                        )),
                        None => GolemError(format!(
                            "Failed Websocket. Http error: {}",
                            http_error_response.status()
                        )),
                    }
                }
                _ => GolemError(format!("Failed Websocket. Error: {}", e)),
            })?;

        let (mut write, read) = ws_stream.split();

        let pings = task::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(5)); // TODO configure

            let mut cnt: i32 = 1;

            loop {
                interval.tick().await;

                write
                    .send(Message::Ping(cnt.to_ne_bytes().to_vec()))
                    .await
                    .unwrap(); // TODO: handle errors: map_err(|e| GolemError(format!("Ping failure: {e}")))?;

                cnt += 1;
            }
        });

        let read_res = read.for_each(|message_or_error| async {
            match message_or_error {
                Err(error) => {
                    print!("Error reading message: {}", error);
                }
                Ok(message) => {
                    let instance_connect_msg = match message {
                        Message::Text(str) => {
                            let parsed: serde_json::Result<InstanceConnectMessage> =
                                serde_json::from_str(&str);
                            Some(parsed.unwrap()) // TODO: error handling
                        }
                        Message::Binary(data) => {
                            let parsed: serde_json::Result<InstanceConnectMessage> =
                                serde_json::from_slice(&data);
                            Some(parsed.unwrap()) // TODO: error handling
                        }
                        Message::Ping(_) => {
                            debug!("Ignore ping");
                            None
                        }
                        Message::Pong(_) => {
                            debug!("Ignore pong");
                            None
                        }
                        Message::Close(details) => {
                            match details {
                                Some(closed_frame) => {
                                    print!("Connection Closed: {}", closed_frame);
                                }
                                None => {
                                    print!("Connection Closed");
                                }
                            }
                            None
                        }
                        Message::Frame(_) => {
                            info!("Ignore unexpected frame");
                            None
                        }
                    };

                    match instance_connect_msg {
                        None => {}
                        Some(msg) => match msg.event {
                            WorkerEvent::Stdout(StdOutLog { message }) => {
                                print!("{message}")
                            }
                            WorkerEvent::Stderr(StdErrLog { message }) => {
                                print!("{message}")
                            }
                            WorkerEvent::Log(Log {
                                level,
                                context,
                                message,
                            }) => match level {
                                0 => tracing::trace!(message, context = context),
                                1 => tracing::debug!(message, context = context),
                                2 => tracing::info!(message, context = context),
                                3 => tracing::warn!(message, context = context),
                                _ => tracing::error!(message, context = context),
                            },
                        },
                    }
                }
            }
        });

        pin_mut!(read_res, pings);

        future::select(pings, read_res).await;

        Ok(())
    }

    async fn update(
        &self,
        worker_urn: WorkerUrn,
        mode: WorkerUpdateMode,
        target_version: u64,
    ) -> Result<(), GolemError> {
        info!("Updating worker {worker_urn}");
        let update_mode = match mode {
            WorkerUpdateMode::Automatic => golem_client::model::WorkerUpdateMode::Automatic,
            WorkerUpdateMode::Manual => golem_client::model::WorkerUpdateMode::Manual,
        };

        let _ = self
            .client
            .update_worker(
                &worker_urn.id.component_id.0,
                &worker_urn.id.worker_name,
                &UpdateWorkerRequest {
                    mode: update_mode,
                    target_version,
                },
            )
            .await?;
        Ok(())
    }
}

#[derive(Deserialize, Debug)]
struct InstanceConnectMessage {
    pub event: WorkerEvent,
}

#[derive(Deserialize, Debug)]
enum WorkerEvent {
    Stdout(StdOutLog),
    Stderr(StdErrLog),
    Log(Log),
}

#[derive(Deserialize, Debug)]
struct StdOutLog {
    message: String,
}

#[derive(Deserialize, Debug)]
struct StdErrLog {
    message: String,
}

#[derive(Deserialize, Debug)]
struct Log {
    pub level: i32,
    pub context: String,
    pub message: String,
}

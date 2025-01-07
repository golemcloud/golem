// Copyright 2024-2025 Golem Cloud
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

use crate::clients::worker::{worker_name_required, WorkerClient};
use crate::command::worker::WorkerConnectOptions;
use crate::connect_output::ConnectOutput;
use crate::model::{
    Format, GolemError, IdempotencyKey, WorkerMetadata, WorkerName, WorkerUpdateMode,
    WorkersMetadataResponse,
};
use async_trait::async_trait;
use futures_util::{future, pin_mut, SinkExt, StreamExt};
use golem_client::api::WorkerError;
use golem_client::model::{
    InvokeParameters, InvokeResult, ScanCursor, UpdateWorkerRequest, WorkerCreationRequest,
    WorkerFilter, WorkerId, WorkersMetadataRequest,
};
use golem_client::{Context, Error};
use golem_common::model::public_oplog::{OplogCursor, PublicOplogEntry};
use golem_common::model::WorkerEvent;
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
use native_tls::TlsConnector;
use tokio::{task, time};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tracing::{debug, error, info, trace};

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

        if let Some(worker_name) = &worker_urn.id.worker_name {
            Ok(self
                .client
                .invoke_and_await_function(
                    &worker_urn.id.component_id.0,
                    worker_name,
                    idempotency_key.as_ref().map(|k| k.0.as_str()),
                    &function,
                    &parameters,
                )
                .await?)
        } else {
            Ok(self
                .client
                .invoke_and_await_function_without_name(
                    &worker_urn.id.component_id.0,
                    idempotency_key.as_ref().map(|k| k.0.as_str()),
                    &function,
                    &parameters,
                )
                .await?)
        }
    }

    async fn invoke(
        &self,
        worker_urn: WorkerUrn,
        function: String,
        parameters: InvokeParameters,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<(), GolemError> {
        info!("Invoke function {function} in {worker_urn}");

        if let Some(worker_name) = &worker_urn.id.worker_name {
            let _ = self
                .client
                .invoke_function(
                    &worker_urn.id.component_id.0,
                    worker_name,
                    idempotency_key.as_ref().map(|k| k.0.as_str()),
                    &function,
                    &parameters,
                )
                .await?;
        } else {
            let _ = self
                .client
                .invoke_function_without_name(
                    &worker_urn.id.component_id.0,
                    idempotency_key.as_ref().map(|k| k.0.as_str()),
                    &function,
                    &parameters,
                )
                .await?;
        }
        Ok(())
    }

    async fn interrupt(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Interrupting {worker_urn}");

        let _ = self
            .client
            .interrupt_worker(
                &worker_urn.id.component_id.0,
                &worker_name_required(&worker_urn)?,
                Some(false),
            )
            .await?;
        Ok(())
    }

    async fn resume(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Resuming {worker_urn}");

        let _ = self
            .client
            .resume_worker(
                &worker_urn.id.component_id.0,
                &worker_name_required(&worker_urn)?,
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
                &worker_name_required(&worker_urn)?,
                Some(true),
            )
            .await?;
        Ok(())
    }

    async fn delete(&self, worker_urn: WorkerUrn) -> Result<(), GolemError> {
        info!("Deleting worker {worker_urn}");

        let _ = self
            .client
            .delete_worker(
                &worker_urn.id.component_id.0,
                &worker_name_required(&worker_urn)?,
            )
            .await?;
        Ok(())
    }

    async fn get_metadata(&self, worker_urn: WorkerUrn) -> Result<WorkerMetadata, GolemError> {
        info!("Getting worker {worker_urn} metadata");

        Ok(self
            .client
            .get_worker_metadata(
                &worker_urn.id.component_id.0,
                &worker_name_required(&worker_urn)?,
            )
            .await?
            .into())
    }

    async fn get_metadata_opt(
        &self,
        worker_urn: WorkerUrn,
    ) -> Result<Option<WorkerMetadata>, GolemError> {
        info!("Getting worker {worker_urn} metadata");

        match self
            .client
            .get_worker_metadata(
                &worker_urn.id.component_id.0,
                &worker_name_required(&worker_urn)?,
            )
            .await
        {
            Ok(metadata) => Ok(Some(metadata.into())),
            Err(Error::Item(WorkerError::Error404(_))) => Ok(None),
            Err(err) => Err(err.into()),
        }
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

    async fn connect(
        &self,
        worker_urn: WorkerUrn,
        connect_options: WorkerConnectOptions,
        format: Format,
    ) -> Result<(), GolemError> {
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
            .push(&worker_name_required(&worker_urn)?)
            .push("connect");

        let mut request = url
            .to_string()
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

        info!("Connecting to {worker_urn}");

        let (ws_stream, _) = connect_async_tls_with_config(request, None, false, connector)
            .await
            .map_err(|e| match e {
                tungstenite::error::Error::Http(http_error_response) => {
                    let status = http_error_response.status().as_u16();

                    match http_error_response.body().clone() {
                        Some(body) => get_worker_golem_error(status, body),
                        None => GolemError(format!("Failed Websocket. Http error: {}", status)),
                    }
                }
                _ => GolemError(format!("Failed Websocket. Error: {}", e)),
            })?;

        let (mut write, read) = ws_stream.split();

        let pings = task::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(1)); // TODO configure
            let mut cnt: i32 = 1;

            loop {
                interval.tick().await;

                let ping_result = write
                    .send(Message::Ping(cnt.to_ne_bytes().to_vec()))
                    .await
                    .map_err(|err| GolemError(format!("Worker connection ping failure: {err}")));

                if let Err(err) = ping_result {
                    error!("{}", err);
                    break err;
                }

                cnt += 1;
            }
        });

        let output = ConnectOutput::new(connect_options, format);

        let read_res = read.for_each(move |message_or_error| {
            let output = output.clone();
            async move {
                match message_or_error {
                    Err(error) => {
                        error!("Error reading message: {}", error);
                    }
                    Ok(message) => {
                        let instance_connect_msg = match message {
                            Message::Text(str) => {
                                let parsed: serde_json::Result<WorkerEvent> =
                                    serde_json::from_str(&str);

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
                                    serde_json::from_slice(&data);
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
                        };

                        match instance_connect_msg {
                            None => {}
                            Some(msg) => match msg {
                                WorkerEvent::StdOut { timestamp, bytes } => {
                                    output
                                        .emit_stdout(
                                            timestamp,
                                            String::from_utf8_lossy(&bytes).to_string(),
                                        )
                                        .await;
                                }
                                WorkerEvent::StdErr { timestamp, bytes } => {
                                    output
                                        .emit_stderr(
                                            timestamp,
                                            String::from_utf8_lossy(&bytes).to_string(),
                                        )
                                        .await;
                                }
                                WorkerEvent::Log {
                                    timestamp,
                                    level,
                                    context,
                                    message,
                                } => {
                                    output.emit_log(timestamp, level, context, message);
                                }
                                WorkerEvent::Close => {}
                                WorkerEvent::InvocationStart { .. } => {}
                                WorkerEvent::InvocationFinished { .. } => {}
                            },
                        }
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
                &worker_name_required(&worker_urn)?,
                &UpdateWorkerRequest {
                    mode: update_mode,
                    target_version,
                },
            )
            .await?;
        Ok(())
    }

    async fn get_oplog(
        &self,
        worker_urn: WorkerUrn,
        from: u64,
    ) -> Result<Vec<(u64, PublicOplogEntry)>, GolemError> {
        let mut entries = Vec::new();
        let mut cursor: Option<OplogCursor> = None;

        loop {
            let chunk = self
                .client
                .get_oplog(
                    &worker_urn.id.component_id.0,
                    &worker_name_required(&worker_urn)?,
                    Some(from),
                    100,
                    cursor.as_ref(),
                    None,
                )
                .await?;

            trace!(
                "Got {} oplog entries starting from {}, last index is {}",
                chunk.entries.len(),
                chunk.first_index_in_chunk,
                chunk.last_index
            );

            if chunk.entries.is_empty() {
                break;
            } else {
                entries.extend(chunk.entries.into_iter().map(|e| (e.oplog_index, e.entry)));
                cursor = chunk.next;
            }
        }

        Ok(entries)
    }

    async fn search_oplog(
        &self,
        worker_urn: WorkerUrn,
        query: String,
    ) -> Result<Vec<(u64, PublicOplogEntry)>, GolemError> {
        let mut entries = Vec::new();
        let mut cursor: Option<OplogCursor> = None;

        loop {
            let chunk = self
                .client
                .get_oplog(
                    &worker_urn.id.component_id.0,
                    &worker_name_required(&worker_urn)?,
                    None,
                    100,
                    cursor.as_ref(),
                    Some(&query),
                )
                .await?;

            trace!(
                "Got {} oplog entries starting from {}, last index is {}",
                chunk.entries.len(),
                chunk.first_index_in_chunk,
                chunk.last_index
            );

            if chunk.entries.is_empty() {
                break;
            } else {
                entries.extend(chunk.entries.into_iter().map(|e| (e.oplog_index, e.entry)));
                cursor = chunk.next;
            }
        }

        Ok(entries)
    }
}

fn get_worker_golem_error(status: u16, body: Vec<u8>) -> GolemError {
    let error: Result<Error<WorkerError>, serde_json::Error> = match status {
        400 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error400(body))),
        401 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error401(body))),
        403 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error403(body))),
        404 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error404(body))),
        409 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error409(body))),
        500 => serde_json::from_slice(&body).map(|body| Error::Item(WorkerError::Error500(body))),
        _ => Ok(Error::unexpected(status, body.into())),
    };
    error.unwrap_or_else(Error::from).into()
}

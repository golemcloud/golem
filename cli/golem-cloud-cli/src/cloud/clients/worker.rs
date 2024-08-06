use std::time::Duration;

use async_trait::async_trait;
use futures_util::{future, pin_mut, SinkExt, StreamExt};
use golem_cli::clients::worker::WorkerClient;
use golem_client::model::ScanCursor;
use golem_cloud_client::model::{
    CallingConvention, FilterComparator, InvokeParameters, InvokeResult, StringFilterComparator,
    WorkerAndFilter, WorkerCreatedAtFilter, WorkerCreationRequest, WorkerEnvFilter, WorkerFilter,
    WorkerNameFilter, WorkerNotFilter, WorkerOrFilter, WorkerStatus, WorkerStatusFilter,
    WorkerVersionFilter, WorkersMetadataRequest,
};
use golem_cloud_client::Context;
use native_tls::TlsConnector;
use serde::Deserialize;
use tokio::{task, time};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tracing::{debug, error, info};

use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::{ToCli, ToCloud, ToOss};
use golem_cli::model::{
    ComponentId, GolemError, IdempotencyKey, WorkerMetadata, WorkerName, WorkerUpdateMode,
};

#[derive(Clone)]
pub struct WorkerClientLive<C: golem_cloud_client::api::WorkerClient + Sync + Send> {
    pub client: C,
    pub context: Context,
    pub allow_insecure: bool,
}

fn to_cloud_invoke_parameters(ps: golem_client::model::InvokeParameters) -> InvokeParameters {
    InvokeParameters { params: ps.params }
}

fn to_oss_invoke_result(r: InvokeResult) -> golem_client::model::InvokeResult {
    golem_client::model::InvokeResult { result: r.result }
}

fn to_cloud_worker_filter(f: golem_client::model::WorkerFilter) -> WorkerFilter {
    fn to_cloud_string_filter_comparator(
        c: &golem_client::model::StringFilterComparator,
    ) -> StringFilterComparator {
        match c {
            golem_client::model::StringFilterComparator::Equal => StringFilterComparator::Equal,
            golem_client::model::StringFilterComparator::NotEqual => {
                StringFilterComparator::NotEqual
            }
            golem_client::model::StringFilterComparator::Like => StringFilterComparator::Like,
            golem_client::model::StringFilterComparator::NotLike => StringFilterComparator::NotLike,
        }
    }

    fn to_cloud_filter_name(f: golem_client::model::WorkerNameFilter) -> WorkerNameFilter {
        WorkerNameFilter {
            comparator: to_cloud_string_filter_comparator(&f.comparator),
            value: f.value,
        }
    }

    fn to_cloud_filter_comparator(f: &golem_client::model::FilterComparator) -> FilterComparator {
        match f {
            golem_client::model::FilterComparator::Equal => FilterComparator::Equal,
            golem_client::model::FilterComparator::NotEqual => FilterComparator::NotEqual,
            golem_client::model::FilterComparator::GreaterEqual => FilterComparator::GreaterEqual,
            golem_client::model::FilterComparator::Greater => FilterComparator::Greater,
            golem_client::model::FilterComparator::LessEqual => FilterComparator::LessEqual,
            golem_client::model::FilterComparator::Less => FilterComparator::Less,
        }
    }

    fn to_cloud_worker_status(s: &golem_client::model::WorkerStatus) -> WorkerStatus {
        match s {
            golem_client::model::WorkerStatus::Running => WorkerStatus::Running,
            golem_client::model::WorkerStatus::Idle => WorkerStatus::Idle,
            golem_client::model::WorkerStatus::Suspended => WorkerStatus::Suspended,
            golem_client::model::WorkerStatus::Interrupted => WorkerStatus::Interrupted,
            golem_client::model::WorkerStatus::Retrying => WorkerStatus::Retrying,
            golem_client::model::WorkerStatus::Failed => WorkerStatus::Failed,
            golem_client::model::WorkerStatus::Exited => WorkerStatus::Exited,
        }
    }

    fn to_cloud_filter_status(f: golem_client::model::WorkerStatusFilter) -> WorkerStatusFilter {
        WorkerStatusFilter {
            comparator: to_cloud_filter_comparator(&f.comparator),
            value: to_cloud_worker_status(&f.value),
        }
    }
    fn to_cloud_filter_version(f: golem_client::model::WorkerVersionFilter) -> WorkerVersionFilter {
        WorkerVersionFilter {
            comparator: to_cloud_filter_comparator(&f.comparator),
            value: f.value,
        }
    }
    fn to_cloud_filter_created_at(
        f: golem_client::model::WorkerCreatedAtFilter,
    ) -> WorkerCreatedAtFilter {
        WorkerCreatedAtFilter {
            comparator: to_cloud_filter_comparator(&f.comparator),
            value: f.value,
        }
    }
    fn to_cloud_filter_env(f: golem_client::model::WorkerEnvFilter) -> WorkerEnvFilter {
        let golem_client::model::WorkerEnvFilter {
            name,
            comparator,
            value,
        } = f;

        WorkerEnvFilter {
            name,
            comparator: to_cloud_string_filter_comparator(&comparator),
            value,
        }
    }
    fn to_cloud_filter_and(f: golem_client::model::WorkerAndFilter) -> WorkerAndFilter {
        WorkerAndFilter {
            filters: f.filters.into_iter().map(to_cloud_worker_filter).collect(),
        }
    }
    fn to_cloud_filter_or(f: golem_client::model::WorkerOrFilter) -> WorkerOrFilter {
        WorkerOrFilter {
            filters: f.filters.into_iter().map(to_cloud_worker_filter).collect(),
        }
    }
    fn to_cloud_filter_not(f: golem_client::model::WorkerNotFilter) -> WorkerNotFilter {
        WorkerNotFilter {
            filter: to_cloud_worker_filter(f.filter),
        }
    }

    match f {
        golem_client::model::WorkerFilter::Name(f) => WorkerFilter::Name(to_cloud_filter_name(f)),
        golem_client::model::WorkerFilter::Status(f) => {
            WorkerFilter::Status(to_cloud_filter_status(f))
        }
        golem_client::model::WorkerFilter::Version(f) => {
            WorkerFilter::Version(to_cloud_filter_version(f))
        }
        golem_client::model::WorkerFilter::CreatedAt(f) => {
            WorkerFilter::CreatedAt(to_cloud_filter_created_at(f))
        }
        golem_client::model::WorkerFilter::Env(f) => WorkerFilter::Env(to_cloud_filter_env(f)),
        golem_client::model::WorkerFilter::And(f) => WorkerFilter::And(to_cloud_filter_and(f)),
        golem_client::model::WorkerFilter::Or(f) => WorkerFilter::Or(to_cloud_filter_or(f)),
        golem_client::model::WorkerFilter::Not(f) => {
            WorkerFilter::Not(Box::new(to_cloud_filter_not(*f)))
        }
    }
}

#[async_trait]
impl<C: golem_cloud_client::api::WorkerClient + Sync + Send> WorkerClient for WorkerClientLive<C> {
    async fn new_worker(
        &self,
        name: WorkerName,
        component_id: ComponentId,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<golem_client::model::WorkerId, GolemError> {
        info!("Creating worker {name} of {}", component_id.0);

        Ok(self
            .client
            .launch_new_worker(
                &component_id.0,
                &WorkerCreationRequest {
                    name: name.0,
                    args,
                    env: env.into_iter().collect(),
                },
            )
            .await
            .map_err(CloudGolemError::from)?
            .worker_id
            .to_oss())
    }

    async fn invoke_and_await(
        &self,
        name: WorkerName,
        component_id: ComponentId,
        function: String,
        parameters: golem_client::model::InvokeParameters,
        idempotency_key: Option<IdempotencyKey>,
        use_stdio: bool,
    ) -> Result<golem_client::model::InvokeResult, GolemError> {
        info!(
            "Invoke and await for function {function} in {}/{}",
            component_id.0, name.0
        );

        let calling_convention = if use_stdio {
            CallingConvention::Stdio
        } else {
            CallingConvention::Component
        };

        Ok(to_oss_invoke_result(
            self.client
                .invoke_and_await_function(
                    &component_id.0,
                    &name.0,
                    idempotency_key.as_ref().map(|k| k.0.as_str()),
                    &function,
                    Some(&calling_convention),
                    &to_cloud_invoke_parameters(parameters),
                )
                .await
                .map_err(CloudGolemError::from)?,
        ))
    }

    async fn invoke(
        &self,
        name: WorkerName,
        component_id: ComponentId,
        function: String,
        parameters: golem_client::model::InvokeParameters,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<(), GolemError> {
        info!(
            "Invoke function {function} in {}/{}",
            component_id.0, name.0
        );

        let _ = self
            .client
            .invoke_function(
                &component_id.0,
                &name.0,
                idempotency_key.as_ref().map(|k| k.0.as_str()),
                &function,
                &to_cloud_invoke_parameters(parameters),
            )
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }

    async fn interrupt(
        &self,
        name: WorkerName,
        component_id: ComponentId,
    ) -> Result<(), GolemError> {
        info!("Interrupting {}/{}", component_id.0, name.0);

        let _ = self
            .client
            .interrupt_worker(&component_id.0, &name.0, Some(false))
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }

    async fn simulated_crash(
        &self,
        name: WorkerName,
        component_id: ComponentId,
    ) -> Result<(), GolemError> {
        info!("Simulating crash of {}/{}", component_id.0, name.0);

        let _ = self
            .client
            .interrupt_worker(&component_id.0, &name.0, Some(true))
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }

    async fn delete(&self, name: WorkerName, component_id: ComponentId) -> Result<(), GolemError> {
        info!("Deleting worker {}/{}", component_id.0, name.0);

        let _ = self
            .client
            .delete_worker(&component_id.0, &name.0)
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }

    async fn get_metadata(
        &self,
        name: WorkerName,
        component_id: ComponentId,
    ) -> Result<WorkerMetadata, GolemError> {
        info!("Getting worker {}/{} metadata", component_id.0, name.0);

        Ok(self
            .client
            .get_worker_metadata(&component_id.0, &name.0)
            .await
            .map_err(CloudGolemError::from)?
            .to_cli())
    }

    async fn find_metadata(
        &self,
        component_id: ComponentId,
        filter: Option<golem_client::model::WorkerFilter>,
        cursor: Option<ScanCursor>,
        count: Option<u64>,
        precise: Option<bool>,
    ) -> Result<golem_cli::model::WorkersMetadataResponse, GolemError> {
        info!(
            "Getting workers metadata for component: {}, filter: {}",
            component_id.0,
            filter.is_some()
        );

        Ok(self
            .client
            .find_workers_metadata(
                &component_id.0,
                &WorkersMetadataRequest {
                    filter: filter.map(to_cloud_worker_filter),
                    cursor: cursor.map(|c| c.to_cloud()),
                    count,
                    precise,
                },
            )
            .await
            .map_err(CloudGolemError::from)?
            .to_cli())
    }

    async fn list_metadata(
        &self,
        component_id: ComponentId,
        filter: Option<Vec<String>>,
        cursor: Option<ScanCursor>,
        count: Option<u64>,
        precise: Option<bool>,
    ) -> Result<golem_cli::model::WorkersMetadataResponse, GolemError> {
        info!(
            "Getting workers metadata for component: {}, filter: {}",
            component_id.0,
            filter
                .clone()
                .map(|fs| fs.join(" AND "))
                .unwrap_or("N/A".to_string())
        );

        let filter: Option<&[String]> = filter.as_deref();

        let cursor = cursor.map(|cursor| format!("{}/{}", cursor.layer, cursor.cursor));

        Ok(self
            .client
            .get_workers_metadata(&component_id.0, filter, cursor.as_deref(), count, precise)
            .await
            .map_err(CloudGolemError::from)?
            .to_cli())
    }

    async fn connect(&self, name: WorkerName, component_id: ComponentId) -> Result<(), GolemError> {
        let mut url = self.context.base_url.clone();

        let ws_schema = if url.scheme() == "http" { "ws" } else { "wss" };

        url.set_scheme(ws_schema)
            .map_err(|_| GolemError("Can't set schema.".to_string()))?;

        url.path_segments_mut()
            .map_err(|_| GolemError("Can't get path.".to_string()))?
            .push("v1")
            .push("components")
            .push(&component_id.0.to_string())
            .push("workers")
            .push(&name.0)
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
                    debug!("Error reading message: {}", error);
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
                            debug!("Ping received from server");
                            None
                        }
                        Message::Pong(_) => {
                            debug!("Pong received from server");
                            None
                        }
                        Message::Close(details) => {
                            match details {
                                Some(closed_frame) => {
                                    error!("Connection Closed: {}", closed_frame);
                                }
                                None => {
                                    info!("Connection Closed");
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
        name: WorkerName,
        component_id: ComponentId,
        mode: WorkerUpdateMode,
        target_version: u64,
    ) -> Result<(), GolemError> {
        info!("Updating worker {name} of {}", component_id.0);
        let update_mode = match mode {
            WorkerUpdateMode::Automatic => golem_cloud_client::model::WorkerUpdateMode::Automatic,
            WorkerUpdateMode::Manual => golem_cloud_client::model::WorkerUpdateMode::Manual,
        };

        let _ = self
            .client
            .update_worker(
                &component_id.0,
                &name.0,
                &golem_cloud_client::model::UpdateWorkerRequest {
                    mode: update_mode,
                    target_version,
                },
            )
            .await
            .map_err(CloudGolemError::from)?;
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

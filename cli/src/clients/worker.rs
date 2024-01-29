use std::time::Duration;

use async_trait::async_trait;
use futures_util::{future, pin_mut, SinkExt, StreamExt};
use golem_client::model::{
    CallingConvention, InvokeParameters, InvokeResult, VersionedWorkerId, WorkerCreationRequest,
    WorkerMetadata,
};
use golem_client::Context;
use native_tls::TlsConnector;
use serde::Deserialize;
use tokio::{task, time};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tracing::{debug, info};

use crate::model::{GolemError, InvocationKey, RawTemplateId};
use crate::WorkerName;

#[async_trait]
pub trait WorkerClient {
    async fn new_worker(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<VersionedWorkerId, GolemError>;
    async fn get_invocation_key(
        &self,
        name: &WorkerName,
        template_id: &RawTemplateId,
    ) -> Result<InvocationKey, GolemError>;

    async fn invoke_and_await(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
        function: String,
        parameters: InvokeParameters,
        invocation_key: InvocationKey,
        use_stdio: bool,
    ) -> Result<InvokeResult, GolemError>;

    async fn invoke(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
        function: String,
        parameters: InvokeParameters,
    ) -> Result<(), GolemError>;

    async fn interrupt(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<(), GolemError>;
    async fn simulated_crash(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<(), GolemError>;
    async fn delete(&self, name: WorkerName, template_id: RawTemplateId) -> Result<(), GolemError>;
    async fn get_metadata(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<WorkerMetadata, GolemError>;
    async fn connect(&self, name: WorkerName, template_id: RawTemplateId)
        -> Result<(), GolemError>;
}

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
        template_id: RawTemplateId,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<VersionedWorkerId, GolemError> {
        info!("Creating worker {name} of {}", template_id.0);

        Ok(self
            .client
            .template_id_workers_post(
                &template_id.0,
                &WorkerCreationRequest {
                    name: name.0,
                    args,
                    env: env.into_iter().collect(),
                },
            )
            .await?)
    }

    async fn get_invocation_key(
        &self,
        name: &WorkerName,
        template_id: &RawTemplateId,
    ) -> Result<InvocationKey, GolemError> {
        info!("Getting invocation key for {}/{}", template_id.0, name.0);

        let key = self
            .client
            .template_id_workers_worker_name_key_post(&template_id.0, &name.0)
            .await?;

        Ok(key_api_to_cli(key))
    }

    async fn invoke_and_await(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
        function: String,
        parameters: InvokeParameters,
        invocation_key: InvocationKey,
        use_stdio: bool,
    ) -> Result<InvokeResult, GolemError> {
        info!(
            "Invoke and await for function {function} in {}/{}",
            template_id.0, name.0
        );

        let calling_convention = if use_stdio {
            CallingConvention::Stdio
        } else {
            CallingConvention::Component
        };

        Ok(self
            .client
            .template_id_workers_worker_name_invoke_and_await_post(
                &template_id.0,
                &name.0,
                &invocation_key.0,
                &function,
                Some(&calling_convention),
                &parameters,
            )
            .await?)
    }

    async fn invoke(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
        function: String,
        parameters: InvokeParameters,
    ) -> Result<(), GolemError> {
        info!("Invoke function {function} in {}/{}", template_id.0, name.0);

        let _ = self
            .client
            .template_id_workers_worker_name_invoke_post(
                &template_id.0,
                &name.0,
                &function,
                &parameters,
            )
            .await?;
        Ok(())
    }

    async fn interrupt(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<(), GolemError> {
        info!("Interrupting {}/{}", template_id.0, name.0);

        let _ = self
            .client
            .template_id_workers_worker_name_interrupt_post(&template_id.0, &name.0, Some(false))
            .await?;
        Ok(())
    }

    async fn simulated_crash(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<(), GolemError> {
        info!("Simulating crash of {}/{}", template_id.0, name.0);

        let _ = self
            .client
            .template_id_workers_worker_name_interrupt_post(&template_id.0, &name.0, Some(true))
            .await?;
        Ok(())
    }

    async fn delete(&self, name: WorkerName, template_id: RawTemplateId) -> Result<(), GolemError> {
        info!("Deleting worker {}/{}", template_id.0, name.0);

        let _ = self
            .client
            .template_id_workers_worker_name_delete(&template_id.0, &name.0)
            .await?;
        Ok(())
    }

    async fn get_metadata(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<WorkerMetadata, GolemError> {
        info!("Getting worker {}/{} metadata", template_id.0, name.0);

        Ok(self
            .client
            .template_id_workers_worker_name_get(&template_id.0, &name.0)
            .await?)
    }

    async fn connect(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<(), GolemError> {
        let mut url = self.context.base_url.clone();

        let ws_schema = if url.scheme() == "http" { "ws" } else { "wss" };

        url.set_scheme(ws_schema)
            .map_err(|_| GolemError("Can't set schema.".to_string()))?;

        url.path_segments_mut()
            .map_err(|_| GolemError("Can't get path.".to_string()))?
            .push("v2")
            .push("templates")
            .push(&template_id.0.to_string())
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

fn key_api_to_cli(key: golem_client::model::InvocationKey) -> InvocationKey {
    InvocationKey(key.value)
}

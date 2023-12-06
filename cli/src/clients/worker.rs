use std::fmt::{Display, Formatter};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{future, pin_mut, SinkExt, StreamExt};
use golem_client::apis::configuration::Configuration;
use golem_client::apis::worker_api::{
    v2_templates_template_id_workers_post, v2_templates_template_id_workers_worker_name_delete,
    v2_templates_template_id_workers_worker_name_get,
    v2_templates_template_id_workers_worker_name_interrupt_post,
    v2_templates_template_id_workers_worker_name_invoke_and_await_post,
    v2_templates_template_id_workers_worker_name_invoke_post,
    v2_templates_template_id_workers_worker_name_key_post,
};
use golem_client::models::{
    CallingConvention, InvokeParameters, InvokeResult, VersionedWorkerId, WorkerCreationRequest,
    WorkerMetadata,
};
use native_tls::TlsConnector;
use reqwest::Url;
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
pub struct WorkerClientLive {
    pub configuration: Configuration,
    pub allow_insecure: bool,
}

#[async_trait]
impl WorkerClient for WorkerClientLive {
    async fn new_worker(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<VersionedWorkerId, GolemError> {
        info!("Creating worker {name} of {}", template_id.0);

        Ok(v2_templates_template_id_workers_post(
            &self.configuration,
            &template_id.0.to_string(),
            WorkerCreationRequest {
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

        let key = v2_templates_template_id_workers_worker_name_key_post(
            &self.configuration,
            &template_id.0.to_string(),
            &name.0,
        )
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

        Ok(
            v2_templates_template_id_workers_worker_name_invoke_and_await_post(
                &self.configuration,
                &template_id.0.to_string(),
                &name.0,
                &invocation_key.0,
                &function,
                parameters,
                Some(calling_convention),
            )
            .await?,
        )
    }

    async fn invoke(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
        function: String,
        parameters: InvokeParameters,
    ) -> Result<(), GolemError> {
        info!("Invoke function {function} in {}/{}", template_id.0, name.0);

        let _ = v2_templates_template_id_workers_worker_name_invoke_post(
            &self.configuration,
            &template_id.0.to_string(),
            &name.0,
            &function,
            parameters,
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

        let _ = v2_templates_template_id_workers_worker_name_interrupt_post(
            &self.configuration,
            &template_id.0.to_string(),
            &name.0,
            Some(false),
        )
        .await?;
        Ok(())
    }

    async fn simulated_crash(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<(), GolemError> {
        info!("Simulating crash of {}/{}", template_id.0, name.0);

        let _ = v2_templates_template_id_workers_worker_name_interrupt_post(
            &self.configuration,
            &template_id.0.to_string(),
            &name.0,
            Some(true),
        )
        .await?;
        Ok(())
    }

    async fn delete(&self, name: WorkerName, template_id: RawTemplateId) -> Result<(), GolemError> {
        info!("Deleting worker {}/{}", template_id.0, name.0);

        let _ = v2_templates_template_id_workers_worker_name_delete(
            &self.configuration,
            &template_id.0.to_string(),
            &name.0,
        )
        .await?;
        Ok(())
    }

    async fn get_metadata(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<WorkerMetadata, GolemError> {
        info!("Getting worker {}/{} metadata", template_id.0, name.0);

        Ok(v2_templates_template_id_workers_worker_name_get(
            &self.configuration,
            &template_id.0.to_string(),
            &name.0,
        )
        .await?)
    }

    async fn connect(
        &self,
        name: WorkerName,
        template_id: RawTemplateId,
    ) -> Result<(), GolemError> {
        let mut url = Url::parse(&self.configuration.base_path).unwrap();

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
        headers.insert(
            "Authorization",
            format!(
                "Bearer {}",
                self.configuration.bearer_access_token.as_ref().unwrap()
            )
            .parse()
            .unwrap(),
        );

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
            .map_err(|e| GolemError(format!("Failed websocket: {e}")))?;

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

        let read_res = read.for_each(|message| async {
            let message: Message = message.unwrap();

            let msg = match message {
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
                Message::Close(_) => {
                    info!("Ignore unexpected close");
                    None
                }
                Message::Frame(_) => {
                    info!("Ignore unexpected frame");
                    None
                }
            };

            match msg {
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
                    }) => {
                        println!("[{level}] [{context}] {message}")
                    }
                },
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

#[derive(Deserialize, Debug)]
pub enum InstanceEndpointError {
    BadRequest {
        errors: Vec<String>,
    },
    Unauthorized {
        error: String,
    },
    LimitExceeded {
        error: String,
    },
    Golem {
        #[serde(rename = "golemError")]
        golem_error: golem_client::models::GolemError,
    },
    GatewayTimeout {},
    NotFound {
        error: String,
    },
    AlreadyExists {
        error: String,
    },
}

impl Display for InstanceEndpointError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceEndpointError::BadRequest { errors } => {
                write!(f, "BadRequest: {errors:?}")
            }
            InstanceEndpointError::Unauthorized { error } => {
                write!(f, "Unauthorized: {error}")
            }
            InstanceEndpointError::LimitExceeded { error } => {
                write!(f, "LimitExceeded: {error}")
            }
            InstanceEndpointError::Golem { golem_error } => {
                write!(f, "Golem: {golem_error:?}")
            }
            InstanceEndpointError::GatewayTimeout {} => {
                write!(f, "GatewayTimeout")
            }
            InstanceEndpointError::NotFound { error } => {
                write!(f, "NotFound: {error}")
            }
            InstanceEndpointError::AlreadyExists { error } => {
                write!(f, "AlreadyExists: {error}")
            }
        }
    }
}

fn key_api_to_cli(key: golem_client::models::InvocationKey) -> InvocationKey {
    InvocationKey(key.value)
}

use std::fmt::{Display, Formatter};
use std::time::Duration;
use async_trait::async_trait;
use futures_util::{future, pin_mut, SinkExt, StreamExt};
use golem_client::model::{ComponentInstance, InstanceMetadata, InvokeParameters, InvokeResult};
use reqwest::Url;
use tracing::{info, debug};
use crate::clients::CloudAuthentication;
use crate::{InstanceName};
use crate::model::{GolemError, InvocationKey, RawComponentId};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message, tungstenite::client::IntoClientRequest};
use serde::Deserialize;
use tokio::{task, time};

#[async_trait]
pub trait InstanceClient {
    async fn new_instance(
        &self,
        name: InstanceName,
        component_id: RawComponentId,
        args: Vec<String>,
        env: Vec<(String, String)>,
        auth: &CloudAuthentication,
    ) -> Result<ComponentInstance, GolemError>;
    async fn get_invocation_key(&self, name: &InstanceName, component_id: &RawComponentId, auth: &CloudAuthentication) -> Result<InvocationKey, GolemError>;

    async fn invoke_and_await(
        &self,
        name: InstanceName,
        component_id: RawComponentId,
        function: String,
        parameters: InvokeParameters,
        invocation_key: InvocationKey,
        use_stdio: bool,
        auth: &CloudAuthentication,
    ) -> Result<InvokeResult, GolemError>;

    async fn invoke(
        &self,
        name: InstanceName,
        component_id: RawComponentId,
        function: String,
        parameters: InvokeParameters,
        auth: &CloudAuthentication,
    ) -> Result<(), GolemError>;

    async fn interrupt(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError>;
    async fn simulated_crash(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError>;
    async fn delete(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError>;
    async fn get_metadata(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<InstanceMetadata, GolemError>;
    async fn connect(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError>;
}

#[derive(Clone)]
pub struct InstantClientLive<C: golem_client::instance::Instance + Send + Sync> {
    pub client: C,
    pub base_url: Url,
}

#[async_trait]
impl<C: golem_client::instance::Instance + Send + Sync> InstanceClient for InstantClientLive<C> {
    async fn new_instance(&self, name: InstanceName, component_id: RawComponentId, args: Vec<String>, env: Vec<(String, String)>, auth: &CloudAuthentication) -> Result<ComponentInstance, GolemError> {
        info!("Creating instance {name} of {}", component_id.0);

        let args = if args.is_empty() {
            None
        } else {
            Some(args.join(",")) // TODO: use json
        };

        let env = if env.is_empty() {
            None
        } else {
            Some(env.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<String>>().join(",")) //  TODO use json
        };

        Ok(self.client.launch_new_instance(&component_id.0.to_string(), &name.0, args.as_deref(), env.as_deref(), &auth.header()).await?)
    }

    async fn get_invocation_key(&self, name: &InstanceName, component_id: &RawComponentId, auth: &CloudAuthentication) -> Result<InvocationKey, GolemError> {
        info!("Getting invocation key for {}/{}", component_id.0, name.0);

        let key = self.client.get_invocation_key(&component_id.0.to_string(), &name.0, &auth.header()).await?;

        Ok(key_api_to_cli(key))
    }

    async fn invoke_and_await(&self, name: InstanceName, component_id: RawComponentId, function: String, parameters: InvokeParameters, invocation_key: InvocationKey, use_stdio: bool, auth: &CloudAuthentication) -> Result<InvokeResult, GolemError> {
        info!("Invoke and await for function {function} in {}/{}", component_id.0, name.0);

        let calling_convention = if use_stdio { "stdio" } else { "component" };

        Ok(self.client.invoke_and_await_function(
            &component_id.0.to_string(),
            &name.0,
            &invocation_key.0,
            &function,
            Some(&calling_convention),
            parameters,
            &auth.header(),
        ).await?)
    }

    async fn invoke(&self, name: InstanceName, component_id: RawComponentId, function: String, parameters: InvokeParameters, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Invoke function {function} in {}/{}", component_id.0, name.0);

        Ok(self.client.invoke_function(
            &component_id.0.to_string(),
            &name.0,
            &function,
            parameters,
            &auth.header(),
        ).await?)
    }

    async fn interrupt(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Interrupting {}/{}", component_id.0, name.0);

        Ok(self.client.interrupt_instance(
            &component_id.0.to_string(),
            &name.0,
            Some(false),
            &auth.header(),
        ).await?)
    }

    async fn simulated_crash(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Simulating crash of {}/{}", component_id.0, name.0);

        Ok(self.client.interrupt_instance(
            &component_id.0.to_string(),
            &name.0,
            Some(true),
            &auth.header(),
        ).await?)
    }

    async fn delete(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Deleting instance {}/{}", component_id.0, name.0);

        Ok(self.client.delete_instance(
            &component_id.0.to_string(),
            &name.0,
            &auth.header(),
        ).await?)
    }

    async fn get_metadata(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<InstanceMetadata, GolemError> {
        info!("Getting instance {}/{} metadata", component_id.0, name.0);

        Ok(self.client.get_instance_metadata(
            &component_id.0.to_string(),
            &name.0,
            &auth.header(),
        ).await?)
    }

    async fn connect(&self, name: InstanceName, component_id: RawComponentId, auth: &CloudAuthentication) -> Result<(), GolemError> {
        let mut base_url = self.base_url.clone();
        base_url.set_scheme("wss").map_err(|_| GolemError("Can't set schema.".to_string()))?;
        let url = base_url.join(&format!("/v1/templates/{}/workers/{}/connect", component_id.0, name.0))
            .map_err(|e| GolemError(format!("Failed to join url: {e:>}")))?;

        let mut request = url.into_client_request().map_err(|e| GolemError(format!("Can't create request: {e}")))?;
        let headers = request.headers_mut();
        headers.insert("Authorization", auth.header().parse().unwrap());

        let (ws_stream, _) = connect_async(request).await.map_err(|e| GolemError(format!("Failed websocket: {e}")))?;

        let (mut write, read) = ws_stream.split();

        let pings = task::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(5)); // TODO configure

            let cnt:std::cell::Cell<i32> = std::cell::Cell::new(1);

            loop {
                interval.tick().await;

                let i = cnt.get();
                write.send(Message::Ping(i.to_ne_bytes().to_vec())).await.unwrap(); // TODO: handle errors: map_err(|e| GolemError(format!("Ping failure: {e}")))?;

                cnt.set(i + 1);
            }
        });

        let read_res = read.for_each(|message| async {
            let message: Message = message.unwrap();

            let msg = match message {
                Message::Text(str) => {
                    let parsed: serde_json::Result<InstanceConnectMessage> = serde_json::from_str(&str);
                    Some(parsed.unwrap()) // TODO: error handling
                }
                Message::Binary(data) => {
                    let parsed: serde_json::Result<InstanceConnectMessage> = serde_json::from_slice(&data);
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
                Some(msg) => {
                    match msg {
                        InstanceConnectMessage::Message { message } => {
                            print!("{message}")
                        }
                        InstanceConnectMessage::Error { error } => {
                            eprintln!("Connection error: {error}")
                        }
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
enum InstanceConnectMessage {
    Message {
        message: String
    },
    Error {
        error: InstanceEndpointError
    },
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
enum InstanceEndpointError {
    BadRequest {
        errors: Vec<String>
    },
    Unauthorized {
        error: String
    },
    LimitExceeded { error: String },
    Golem { golem_error: golem_client::model::GolemError },
    GatewayTimeout {},
    NotFound { error: String },
    AlreadyExists { error: String },
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
            InstanceEndpointError::GatewayTimeout {  } => {
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

fn key_api_to_cli(key: golem_client::model::InvocationKey) -> InvocationKey {
    InvocationKey(key.value)
}
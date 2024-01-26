use std::fmt::{Display, Formatter};
use std::sync::Arc;

use crate::service::worker::WorkerService;
use futures_util::{SinkExt, StreamExt};
use golem_cloud_server_base::model::WorkerId;
use golem_common::model::TemplateId;
use poem::web::websocket::{Message, WebSocket, WebSocketStream};
use poem::web::Data;
use poem::*;
use tonic::Status;

#[derive(Clone)]
pub struct ConnectService {
    worker_service: Arc<dyn WorkerService + Send + Sync>,
}

impl ConnectService {
    pub fn new(worker_service: Arc<dyn WorkerService + Send + Sync>) -> Self {
        Self { worker_service }
    }
}

#[handler]
pub async fn ws(
    req: &Request,
    web_socket: WebSocket,
    Data(service): Data<&ConnectService>,
) -> Response {
    tracing::info!("Connect request: {:?} {:?}", req.uri(), req);

    let worker_metadata = match validate_worker_id(req, Data(service)).await {
        Ok(worker_id) => worker_id,
        Err(err) => return (http::StatusCode::BAD_REQUEST, err.0).into_response(),
    };

    dbg!("Connecting to the worker_id {}", worker_metadata.clone());

    let service = service.clone();

    web_socket
        .on_upgrade(move |mut socket| async move {
            tokio::spawn(async move {
                match try_proxy_worker_connection(&service, worker_metadata.worker_id, &mut socket)
                    .await
                {
                    Ok(()) => {
                        tracing::info!("Worker connection closed");
                    }
                    Err(err) => {
                        tracing::error!("Error connecting to worker: {}", err);
                    }
                }
            })
        })
        .into_response()
}

async fn try_proxy_worker_connection(
    service: &ConnectService,
    worker_id: WorkerId,
    socket: &mut WebSocketStream,
) -> Result<(), ConnectError> {
    let mut stream = service.worker_service.connect(&worker_id).await?;

    while let Some(msg) = stream.next().await {
        let message = msg?;
        let msg_json = serde_json::to_string(&message)?;
        socket.send(Message::Text(msg_json)).await?;
    }

    Ok(())
}

#[derive(Debug)]
struct ConnectError(String);

impl Display for ConnectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<tonic::transport::Error> for ConnectError {
    fn from(value: tonic::transport::Error) -> Self {
        ConnectError(value.to_string())
    }
}

impl From<Status> for ConnectError {
    fn from(value: Status) -> Self {
        ConnectError(value.to_string())
    }
}

impl From<String> for ConnectError {
    fn from(value: String) -> Self {
        ConnectError(value)
    }
}

impl From<std::io::Error> for ConnectError {
    fn from(value: std::io::Error) -> Self {
        ConnectError(value.to_string())
    }
}

impl From<serde_json::Error> for ConnectError {
    fn from(value: serde_json::Error) -> Self {
        ConnectError(value.to_string())
    }
}

impl From<crate::service::worker::WorkerError> for ConnectError {
    fn from(error: crate::service::worker::WorkerError) -> Self {
        ConnectError(error.to_string())
    }
}

fn make_worker_id(
    template_id: TemplateId,
    worker_name: String,
) -> std::result::Result<golem_cloud_server_base::model::WorkerId, ConnectError> {
    golem_cloud_server_base::model::WorkerId::new(template_id, worker_name)
        .map_err(|error| ConnectError(format!("Invalid worker name: {error}")))
}

async fn validate_worker_id(
    req: &Request,
    Data(service): Data<&ConnectService>,
) -> Result<golem_cloud_server_base::model::WorkerMetadata, ConnectError> {
    let (template_id, worker_name) = req.path_params::<(String, String)>().map_err(|_| {
        ConnectError(
            "Valid path parameters (template_id and worker_name) are required ".to_string(),
        )
    })?;

    let template_id = TemplateId::try_from(template_id.as_str())
        .map_err(|err| ConnectError(format!("Invalid template id: {}", err)))?;

    let worker_id = make_worker_id(template_id, worker_name)?;

    service
        .worker_service
        .get_metadata(&worker_id)
        .await
        .map_err(|err| ConnectError(format!("Invalid worker {}, {}", worker_id, err)))
}

use std::fmt::{Display, Formatter};
use std::sync::Arc;

use crate::service::worker::WorkerService;
use futures_util::{SinkExt, StreamExt};
use golem_cloud_server_base::model::WorkerId;
use golem_common::model::TemplateId;
use poem::web::websocket::{CloseCode, Message, WebSocket, WebSocketStream};
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

    let worker_id = match get_worker_id(req) {
        Ok(worker_id) => worker_id,
        Err(err) => return (http::StatusCode::BAD_REQUEST, err.0).into_response(),
    };

    let service = service.clone();

    web_socket
        .on_upgrade(move |mut socket| async move {
            tokio::spawn(async move {
                let result = try_proxy_worker_connection(&service, worker_id, &mut socket).await;
                match result {
                    Ok(()) => {
                        tracing::info!("Worker connection closed");
                    }
                    Err(err) => {
                        tracing::error!("Error connecting to worker: {}", err);
                        let close_message = format!("Error connecting to worker: {}", err);
                        let message = Message::Close(Some((CloseCode::Error, close_message)));

                        if let Err(e) = socket.send(message).await {
                            tracing::error!("Failed to send closing frame: {}", e);
                        }

                        //socket.close().await;
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
) -> std::result::Result<WorkerId, ConnectError> {
    WorkerId::new(template_id, worker_name)
        .map_err(|error| ConnectError(format!("Invalid worker name: {error}")))
}

fn make_template_id(template_id: String) -> std::result::Result<TemplateId, ConnectError> {
    TemplateId::try_from(template_id.as_str())
        .map_err(|error| ConnectError(format!("Invalid template id: {error}")))
}

fn get_worker_id(req: &Request) -> Result<golem_cloud_server_base::model::WorkerId, ConnectError> {
    let (template_id, worker_name) = req.path_params::<(String, String)>().map_err(|_| {
        ConnectError(
            "Valid path parameters (template_id and worker_name) are required ".to_string(),
        )
    })?;

    let template_id = make_template_id(template_id)?;

    let worker_id = make_worker_id(template_id, worker_name)?;

    Ok(worker_id)
}

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

use std::fmt::{Display, Formatter};
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::TemplateId;
use golem_service_base::model::WorkerId;
use golem_worker_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::service::worker::{
    ConnectWorkerStream, WorkerService, WorkerServiceBaseError,
};
use poem::web::websocket::{Message, WebSocket, WebSocketStream};
use poem::web::Data;
use poem::*;
use tonic::Status;

#[derive(Clone)]
pub struct ConnectService {
    worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Send + Sync>,
}

impl ConnectService {
    pub fn new(worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Send + Sync>) -> Self {
        Self { worker_service }
    }
}

#[handler]
pub async fn ws(
    req: &Request,
    websocket: WebSocket,
    Data(service): Data<&ConnectService>,
) -> Response {
    tracing::info!("Connect request: {:?} {:?}", req.uri(), req);

    get_worker_stream(service, req)
        .await
        .map(|(worker_id, worker_stream)| {
            websocket
                .on_upgrade(move |socket| {
                    tokio::spawn(async move {
                        proxy_worker_connection(worker_id, worker_stream, socket).await;
                    })
                })
                .into_response()
        })
        .unwrap_or_else(|err| err)
}

async fn get_worker_stream(
    service: &ConnectService,
    req: &Request,
) -> Result<(WorkerId, ConnectWorkerStream), Response> {
    let worker_id = match get_worker_id(req) {
        Ok(worker_id) => worker_id,
        Err(err) => return Err((http::StatusCode::BAD_REQUEST, err.0).into_response()),
    };

    let worker_stream = service
        .worker_service
        .connect(&worker_id, &EmptyAuthCtx {})
        .await
        .map_err(|e| (http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((worker_id, worker_stream))
}

#[tracing::instrument(skip_all, fields(worker_id = worker_id.to_string()))]
async fn proxy_worker_connection(
    worker_id: golem_service_base::model::WorkerId,
    mut worker_stream: ConnectWorkerStream,
    mut websocket: WebSocketStream,
) {
    tracing::info!("Connecting to worker: {worker_id}");

    let result = loop {
        tokio::select! {
            // WebSocket is cancellation safe.
            // https://github.com/snapview/tokio-tungstenite/issues/167
            websocket_message = websocket.next() => {
                match websocket_message {
                    Some(Ok(Message::Close(payload))) => {
                        tracing::info!("Client closed WebSocket connection: {payload:?}");
                        break Ok(());
                    }

                    Some(Ok(Message::Ping(_))) => {
                        tracing::info!("Received PING");
                        if let Err(error) = websocket.send(Message::Pong(Vec::new())).await{
                            tracing::info!("Error sending PONG: {error}");
                            break Err(error.into());
                        }
                    }
                    Some(Err(error)) => {
                        let error: ConnectError = error.into();
                        tracing::info!("Received WebSocket Error: {error}");
                        break Err(error);
                    },
                    Some(Ok(_)) => {}
                    None => {
                        tracing::info!("WebSocket connection closed");
                        break Ok(());
                    }
                }
            },

            worker_message = worker_stream.next() => {
                if let Some(message) = worker_message {
                    if let Err(error) = forward_worker_message(message, &mut websocket).await {
                        tracing::info!("Error forwarding message to WebSocket client: {error}");
                        break(Err(error))
                    }
                } else {
                    tracing::info!("Worker Stream ended");
                    break Ok(());
                }
            },
        }
    };

    if let Err(ref error) = result {
        tracing::info!("Error proxying worker: {worker_id} {error}");
    }

    if let Err(error) = websocket.close().await {
        tracing::info!("Error closing WebSocket connection: {error}");
    } else {
        tracing::info!("WebSocket connection successfully closed");
    }
}

async fn forward_worker_message<S, E>(
    message: Result<LogEvent, tonic::Status>,
    socket: &mut S,
) -> Result<(), ConnectError>
where
    S: futures_util::Sink<Message, Error = E> + Unpin,
    ConnectError: From<E>,
{
    let message = message?;
    let msg_json = serde_json::to_string(&message)?;
    socket.send(Message::Text(msg_json)).await?;
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

impl From<WorkerServiceBaseError> for ConnectError {
    fn from(error: WorkerServiceBaseError) -> Self {
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

fn get_worker_id(req: &Request) -> Result<golem_service_base::model::WorkerId, ConnectError> {
    let (template_id, worker_name) = req.path_params::<(String, String)>().map_err(|_| {
        ConnectError(
            "Valid path parameters (template_id and worker_name) are required ".to_string(),
        )
    })?;

    let template_id = make_template_id(template_id)?;

    let worker_id = make_worker_id(template_id, worker_name)?;

    Ok(worker_id)
}

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

use crate::debug_session::ActiveSession;
use crate::jrpc::jrpc_handler;
use crate::services::debug_service::DebugService;
use axum_jrpc::error::{JsonRpcError, JsonRpcErrorReason};
use axum_jrpc::{Id, JsonRpcRequest, JsonRpcResponse};
use futures_util::stream::SplitSink;
use futures_util::SinkExt;
use futures_util::StreamExt;
use golem_common::model::auth::AuthCtx;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::model::OwnedWorkerId;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::WrappedGolemSecuritySchema;
use log::debug;
use poem::web::websocket::{BoxWebSocketUpgraded, CloseCode, Message, WebSocket, WebSocketStream};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::error;
use tracing::warn;

#[derive(ApiResponse, Debug, Clone)]
pub enum DebuggingApiError {
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 403)]
    Forbidden(Json<ErrorBody>),
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

type Result<T> = std::result::Result<T, DebuggingApiError>;

pub struct DebuggingApi {
    debug_service: Arc<dyn DebugService>,
}

#[OpenApi(prefix_path = "/v1/debugger", tag = ApiTags::Debugging)]
impl DebuggingApi {
    pub fn new(debug_service: Arc<dyn DebugService>) -> Self {
        Self { debug_service }
    }

    /// Start a new debugging sessions
    #[oai(path = "/", method = "get", operation_id = "debugger_start")]
    pub async fn get_debugger(
        &self,
        websocket: WebSocket,
        token: WrappedGolemSecuritySchema,
    ) -> Result<BoxWebSocketUpgraded> {
        let debug_service = self.debug_service.clone();
        let auth_ctx = AuthCtx::new(token.0.secret());
        let upgraded: BoxWebSocketUpgraded = websocket.on_upgrade(Box::new(|socket_stream| {
            Box::pin(async move {
                let active_session = Arc::new(ActiveSession::default());
                let (mut sink, mut stream) = socket_stream.split();

                while let Some(Ok(msg)) = stream.next().await {
                    match msg {
                        Message::Text(text) => {
                            let rpc_request: JsonRpcRequest = match serde_json::from_str(&text) {
                                Ok(request) => request,
                                Err(_) => {
                                    let response = JsonRpcResponse::error(
                                        Id::None(()),
                                        JsonRpcError::new(
                                            JsonRpcErrorReason::ParseError,
                                            "Invalid JSON-RPC".to_string(),
                                            axum_jrpc::Value::Null,
                                        ),
                                    );

                                    send_error_response(&mut sink, response).await;
                                    continue;
                                }
                            };

                            let response = jrpc_handler(
                                debug_service.clone(),
                                rpc_request,
                                Arc::clone(&active_session),
                                auth_ctx.clone(),
                            )
                            .await;

                            match response {
                                Ok(json_rpc_success) => {
                                    send_success_response(&mut sink, json_rpc_success).await
                                }

                                Err(handler_error) => {
                                    error!("Received error from jrpc handler: {}", handler_error);
                                    if handler_error.should_terminate_session() {
                                        close_on_error(
                                            active_session.clone(),
                                            handler_error.to_string(),
                                            &mut sink,
                                            debug_service.clone(),
                                        )
                                        .await;
                                        break;
                                    } else {
                                        send_error_response(
                                            &mut sink,
                                            handler_error.to_jrpc_response(),
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                        Message::Close(_) => {
                            close(active_session.clone(), &mut sink, debug_service.clone()).await;
                            break;
                        }
                        _ => {}
                    }
                }
            })
        }));

        Ok(upgraded)
    }
}

async fn close_on_error(
    active_session: Arc<ActiveSession>,
    error_message: String,
    sink: &mut SplitSink<WebSocketStream, Message>,
    debug_service: Arc<dyn DebugService>,
) {
    if let Some(active_session_data) = active_session.get_active_session().await {
        let worker_id = active_session_data.worker_id;
        let namespace = active_session_data.cloud_namespace;
        let owned_worker_id = OwnedWorkerId::new(&namespace.project_id, &worker_id);
        debug_service.terminate_session(&owned_worker_id).await.ok();
    }

    let result = sink
        .send(Message::Close(Some((CloseCode::Error, error_message))))
        .await;

    if let Err(e) = result {
        debug!("Error sending response: {e}");
    }
}

async fn close(
    active_session: Arc<ActiveSession>,
    sink: &mut SplitSink<WebSocketStream, Message>,
    debug_service: Arc<dyn DebugService>,
) {
    debug!("Closing connection");

    let active_session_data = active_session.get_active_session().await;

    if let Some(active_session_data) = active_session_data {
        let worker_id = active_session_data.worker_id;
        let namespace = active_session_data.cloud_namespace;
        let owned_worker_id = OwnedWorkerId::new(&namespace.project_id, &worker_id);

        debug_service.terminate_session(&owned_worker_id).await.ok();
    }

    sink.send(Message::Close(Some((
        CloseCode::Normal,
        "Connection closed".to_string(),
    ))))
    .await
    .ok();
}

async fn send_error_response(
    sink: &mut SplitSink<WebSocketStream, Message>,
    rpc_error: JsonRpcResponse,
) {
    let result = sink
        .send(Message::Close(Some((
            CloseCode::Error,
            serde_json::to_string(&rpc_error).unwrap(),
        ))))
        .await;

    if let Err(e) = result {
        debug!("Error sending response: {e}");
    }
}

async fn send_success_response(
    sink: &mut SplitSink<WebSocketStream, Message>,
    rpc_success: JsonRpcResponse,
) {
    let result = sink
        .send(Message::Text(serde_json::to_string(&rpc_success).unwrap()))
        .await;

    if let Err(e) = result {
        warn!("Error sending response: {}", e);
    }
}

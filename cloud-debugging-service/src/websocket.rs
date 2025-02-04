use crate::debug_session::ActiveSession;
use crate::jrpc::jrpc_handler;
use crate::services::debug_service::DebugService;
use axum::extract::ws::close_code;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum_jrpc::error::{JsonRpcError, JsonRpcErrorReason};
use axum_jrpc::{Id, JsonRpcRequest, JsonRpcResponse};
use golem_common::model::OwnedWorkerId;
use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;
use tracing::error;
use tracing::log::debug;

pub async fn handle_ws(
    ws: WebSocketUpgrade,
    debug_service: Arc<dyn DebugService + Sync + Send>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, debug_service))
}

async fn handle_socket(mut socket: WebSocket, debug_service: Arc<dyn DebugService + Sync + Send>) {
    let active_session = Arc::new(ActiveSession::default());

    while let Some(Ok(msg)) = socket.recv().await {
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
                                Value::Null,
                            ),
                        );

                        send_error_response(&mut socket, response).await;
                        continue;
                    }
                };

                let response = jrpc_handler(
                    debug_service.clone(),
                    rpc_request,
                    Arc::clone(&active_session),
                )
                .await;

                match response {
                    Ok(json_rpc_success) => {
                        send_success_response(&mut socket, json_rpc_success).await
                    }

                    Err(handler_error) => {
                        error!("Error: {}", handler_error);
                        if handler_error.should_terminate_session() {
                            close_on_error(
                                active_session.clone(),
                                handler_error.to_string(),
                                &mut socket,
                                debug_service.clone(),
                            )
                            .await;
                            break;
                        } else {
                            send_error_response(&mut socket, handler_error.to_jrpc_response())
                                .await;
                        }
                    }
                }
            }
            Message::Close(_) => {
                close(active_session.clone(), &mut socket, debug_service.clone()).await;

                break;
            }
            _ => {}
        }
    }
}

async fn close_on_error(
    active_session: Arc<ActiveSession>,
    error_message: String,
    socket: &mut WebSocket,
    debug_service: Arc<dyn DebugService + Sync + Send>,
) {
    if let Some(active_session_data) = active_session.get_active_session().await {
        let worker_id = active_session_data.worker_id;
        let namespace = active_session_data.cloud_namespace;
        let owned_worker_id = OwnedWorkerId::new(&namespace.account_id, &worker_id);

        debug_service.terminate_session(owned_worker_id).await.ok();
    }

    let close_frame = CloseFrame {
        code: close_code::ERROR,
        reason: Cow::from(error_message),
    };

    let result = socket.send(Message::Close(Some(close_frame))).await;

    if let Err(e) = result {
        debug!("Error sending response: {}", e);
    }
}

async fn close(
    active_session: Arc<ActiveSession>,
    socket: &mut WebSocket,
    debug_service: Arc<dyn DebugService + Sync + Send>,
) {
    debug!("Closing connection");

    let active_session_data = active_session.get_active_session().await;

    if let Some(active_session_data) = active_session_data {
        let worker_id = active_session_data.worker_id;
        let namespace = active_session_data.cloud_namespace;
        let owned_worker_id = OwnedWorkerId::new(&namespace.account_id, &worker_id);

        debug_service.terminate_session(owned_worker_id).await.ok();
    }

    let close_frame = CloseFrame {
        code: close_code::NORMAL,
        reason: Cow::from("Connection closed"),
    };

    socket.send(Message::Close(Some(close_frame))).await.ok();
}

async fn send_error_response(socket: &mut WebSocket, rpc_error: JsonRpcResponse) {
    let close_frame = CloseFrame {
        code: close_code::ERROR,
        reason: Cow::from(serde_json::to_string(&rpc_error).unwrap()),
    };

    let result = socket.send(Message::Close(Some(close_frame))).await;

    if let Err(e) = result {
        debug!("Error sending response: {}", e);
    }
}

async fn send_success_response(socket: &mut WebSocket, rpc_success: JsonRpcResponse) {
    let result = socket
        .send(Message::Text(serde_json::to_string(&rpc_success).unwrap()))
        .await;

    if let Err(e) = result {
        error!("Error sending response: {}", e);
    }
}

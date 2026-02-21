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

use crate::model::params::*;
use crate::services::debug_service::{DebugService, DebugServiceError};
use axum_jrpc::error::{JsonRpcError, JsonRpcErrorReason};
use axum_jrpc::{Id, JsonRpcRequest, JsonRpcResponse};
use futures::{SinkExt, StreamExt};
use golem_common::model::account::AccountId;
use golem_common::model::OwnedWorkerId;
use golem_common::SafeDisplay;
use golem_service_base::model::auth::AuthCtx;
use golem_worker_executor::services::worker_event::WorkerEventReceiver;
use poem::web::websocket::{CloseCode, Message, WebSocketStream};
use serde::Serialize;
use serde_json::Value;
use std::fmt::Display;
use std::sync::Arc;
use tokio::select;
use tokio::sync::mpsc::{self, Sender, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::{debug, warn};

pub async fn run_jrpc_debug_websocket_session(
    socket_stream: WebSocketStream,
    debug_service: Arc<dyn DebugService>,
    auth_ctx: AuthCtx,
) {
    let (mut sink, mut stream) = socket_stream.split();
    let (sender, mut receiver) = mpsc::channel(64);

    // dedicated spawned future for sending outgoing messages to the client.
    // The sink cannot be shared between threads and we need to emit notifications via side channels while driving the session.
    let sender_handle = tokio::spawn(async move {
        let mut closed = false;
        while !closed {
            let message = receiver.recv().await;
            debug!("Sending message on jrpc debugging websocket: {message:?}");
            match message {
                Some(OutgoingJsonRpcMessage::Close) => {
                    debug!("Closing connection");

                    let _ = sink
                        .send(Message::Close(Some((
                            CloseCode::Normal,
                            "Connection closed".to_string(),
                        ))))
                        .await;

                    closed = true;
                }
                Some(OutgoingJsonRpcMessage::Response(response)) => {
                    let result = sink
                        .send(Message::Text(serde_json::to_string(&response).unwrap()))
                        .await;

                    if let Err(e) = result {
                        warn!("Error sending response: {}", e);
                    }
                }
                Some(OutgoingJsonRpcMessage::Notification(notification)) => {
                    let result = sink
                        .send(Message::Text(serde_json::to_string(&notification).unwrap()))
                        .await;

                    if let Err(e) = result {
                        warn!("Error sending notification: {}", e);
                    };
                }
                Some(OutgoingJsonRpcMessage::Error(error)) => {
                    if error.should_terminate_session() {
                        let result = sink
                            .send(Message::Close(Some((CloseCode::Error, error.to_string()))))
                            .await;

                        if let Err(e) = result {
                            debug!("Error sending close with error: {e}");
                        }

                        closed = true;
                    } else {
                        let result = sink
                            .send(Message::Text(
                                serde_json::to_string(&error.to_jrpc_response()).unwrap(),
                            ))
                            .await;

                        if let Err(e) = result {
                            warn!("Error sending error: {}", e);
                        };
                    }
                }
                None => {
                    closed = true;
                }
            }
        }
    });

    let mut session = JrpcSession::new(debug_service.clone(), auth_ctx, sender.clone());

    // drive the session using the incoming websocket messages
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

                        let _ = sender
                            .send(OutgoingJsonRpcMessage::Response(response))
                            .await;
                        continue;
                    }
                };

                debug!("Received request on jrpc debugging websocket: {rpc_request:?}");

                let response = session.handle_request(rpc_request).await;

                match response {
                    Ok(json_rpc_success) => {
                        let _ = sender
                            .send(OutgoingJsonRpcMessage::Response(json_rpc_success))
                            .await;
                    }
                    Err(handler_error) => {
                        let _ = sender
                            .send(OutgoingJsonRpcMessage::Error(handler_error))
                            .await;
                    }
                }
            }
            Message::Close(_) => {
                // ack the close
                let _ = sender.send(OutgoingJsonRpcMessage::Close).await;
            }
            _ => {}
        }
    }

    let _ = sender_handle.await;

    // clean up after ourselves
    session.terminate().await;
}

struct JrpcSessionData {
    pub account_id: AccountId,
    pub connected_worker: OwnedWorkerId,
}

struct JrpcSession {
    debug_service: Arc<dyn DebugService>,
    auth_ctx: AuthCtx,
    active_session: Option<JrpcSessionData>,

    // will be taken by a spawned future when succesfully connect to a worker for the first time.
    // used to send notifications via a second channel
    notifications_sidechannel: Sender<OutgoingJsonRpcMessage>,
    worker_events_processor_dropguard: Option<DropGuard>,

    // used to coordinate a final poll of notifications before returning to the calles
    events_poll_sender: UnboundedSender<oneshot::Sender<()>>,
    events_poll_receiver: Option<UnboundedReceiver<oneshot::Sender<()>>>,
}

impl JrpcSession {
    fn new(
        debug_service: Arc<dyn DebugService>,
        auth_ctx: AuthCtx,
        notifications_sidechannel: Sender<OutgoingJsonRpcMessage>,
    ) -> Self {
        let (events_poll_sender, events_poll_receiver) = mpsc::unbounded_channel();

        Self {
            debug_service,
            auth_ctx,
            active_session: None,
            notifications_sidechannel,
            worker_events_processor_dropguard: None,
            events_poll_sender,
            events_poll_receiver: Some(events_poll_receiver),
        }
    }

    async fn terminate(self) {
        if let Some(active_session) = self.active_session {
            let result = self
                .debug_service
                .terminate_session(&active_session.connected_worker)
                .await;
            if let Err(e) = result {
                warn!("Failed to terminate debugging session: {e}");
            }
        }
    }

    async fn handle_request(
        &mut self,
        request: JsonRpcRequest,
    ) -> Result<JsonRpcResponse, JrpcHandlerError> {
        let jrpc_id: Id = request.id;

        match request.method.as_str() {
            "current_oplog_index" => {
                if let Some(active_session_data) = &self.active_session {
                    let owned_worker_id = active_session_data.connected_worker.clone();

                    let result = self
                        .debug_service
                        .current_oplog_index(&owned_worker_id)
                        .await;
                    to_json_rpc_result(&jrpc_id, result)
                } else {
                    Err(inactive_session_error(&jrpc_id))
                }
            }
            "connect" => {
                if self.active_session.is_some() {
                    Err(JrpcHandlerError::session_already_connected(
                        jrpc_id.clone(),
                        "Session is already connected to a worker".to_string(),
                    ))?
                }

                let params: ConnectParams = parse_params(&jrpc_id, request.params)?;

                let result = self
                    .debug_service
                    .connect(&self.auth_ctx, &params.worker_id)
                    .await;

                match result {
                    Ok((result, account_id, connected_worker, worker_event_receiver)) => {
                        self.active_session = Some(JrpcSessionData {
                            account_id,
                            connected_worker,
                        });

                        self.start_worker_event_processor(worker_event_receiver);

                        self.ensure_pending_notifications_are_emitted().await;

                        to_json_rpc_result(&jrpc_id, Ok(result))
                    }
                    Err(err) => to_json_rpc_result::<ConnectResult>(&jrpc_id, Err(err)),
                }
            }
            "playback" => {
                if let Some(active_session_data) = &self.active_session {
                    let params: PlaybackParams = parse_params(&jrpc_id, request.params)?;

                    let owned_worker_id = active_session_data.connected_worker.clone();

                    let result = self
                        .debug_service
                        .playback(
                            &owned_worker_id,
                            active_session_data.account_id,
                            params.target_index,
                            params.overrides,
                            params.ensure_invocation_boundary.unwrap_or(true),
                        )
                        .await;

                    self.ensure_pending_notifications_are_emitted().await;

                    to_json_rpc_result(&jrpc_id, result)
                } else {
                    Err(inactive_session_error(&jrpc_id))
                }
            }
            "rewind" => {
                if let Some(active_session_data) = &self.active_session {
                    let params: RewindParams = parse_params(&jrpc_id, request.params)?;

                    let owned_worker_id = active_session_data.connected_worker.clone();

                    let result = self
                        .debug_service
                        .rewind(
                            &owned_worker_id,
                            active_session_data.account_id,
                            params.target_index,
                            params.ensure_invocation_boundary.unwrap_or(true),
                        )
                        .await;

                    self.ensure_pending_notifications_are_emitted().await;

                    to_json_rpc_result(&jrpc_id, result)
                } else {
                    Err(inactive_session_error(&jrpc_id))
                }
            }
            "fork" => {
                if let Some(active_session_data) = &self.active_session {
                    let owned_worker_id = active_session_data.connected_worker.clone();

                    let params: ForkParams = parse_params(&jrpc_id, request.params)?;
                    let result = self
                        .debug_service
                        .fork(
                            active_session_data.account_id,
                            &owned_worker_id,
                            &params.target_worker_id,
                            params.oplog_index_cut_off,
                        )
                        .await;
                    to_json_rpc_result(&jrpc_id, result)
                } else {
                    Err(inactive_session_error(&jrpc_id))
                }
            }

            method => Err(method_not_found_error(&jrpc_id, method)),
        }
    }

    /// start forwarding notifications. May only be called once
    fn start_worker_event_processor(&mut self, worker_event_receiver: WorkerEventReceiver) {
        let notifications_sidechannel = self.notifications_sidechannel.clone();
        let mut events_poll_receiver = self
            .events_poll_receiver
            .take()
            .expect("events_poll_receiver was already taken");

        let token = CancellationToken::new();
        let cloned_token = token.clone();

        let mut worker_event_stream = worker_event_receiver.to_stream();

        // use a biased select to ensure the stream is empty before
        tokio::spawn(async move {
            loop {
                select! {
                    biased;
                    _ = cloned_token.cancelled() => { break; }
                    Some(event) = worker_event_stream.next() => {
                        match event {
                            Ok(event) => {
                                if let Some(log_notifiation) = LogNotification::from_internal_worker_event(event) {
                                    let params = serde_json::to_value(vec![log_notifiation]).expect("serializing message failed");

                                    let notification = JsonRpcNotification { method: "emit-logs".to_string(), params };

                                    let _ = notifications_sidechannel.send(OutgoingJsonRpcMessage::Notification(notification)).await;
                                }
                            }
                            Err(BroadcastStreamRecvError::Lagged(number_of_missed_messages)) => {
                                let value = LogsLaggedNotification { number_of_missed_messages };

                                let params = serde_json::to_value(value).expect("serializing message failed");

                                let notification = JsonRpcNotification { method: "notify-logs-lagged".to_string(), params };

                                let _ = notifications_sidechannel.send(OutgoingJsonRpcMessage::Notification(notification)).await;
                            },
                        }
                    }
                    Some(sender) = events_poll_receiver.recv() => {
                        sender.send(()).expect("Failed to send event poll response");
                    }
                }
            }
        });

        // cancel spawned forwarding future when we are dropped
        self.worker_events_processor_dropguard = Some(token.drop_guard());
    }

    // Send a signal to the background worker that will only completed once the event stream contains no more messages.
    // may only be called after start_worker_event_processor;
    async fn ensure_pending_notifications_are_emitted(&self) {
        let (sender, receiver) = oneshot::channel();
        self.events_poll_sender.send(sender).unwrap();
        receiver.await.unwrap();
    }
}

#[derive(Debug)]
struct JrpcHandlerError {
    jrpc_id: Id,
    error_type: JrpcHandlerErrorType,
}

impl JrpcHandlerError {
    fn debug_service_error(jrpc_id: Id, error: DebugServiceError) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::DebugServiceError(error),
        }
    }

    fn inactive_session(jrpc_id: Id) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::InactiveSession {
                error: "Inactive session".to_string(),
            },
        }
    }

    fn invalid_params(jrpc_id: Id, error: String) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::InvalidParams { error },
        }
    }

    fn method_not_found(jrpc_id: Id, method: &str) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::MethodNotFound {
                method: method.to_string(),
            },
        }
    }

    fn session_already_connected(jrpc_id: Id, error: String) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::SessionAlreadyConnected { error },
        }
    }
}

#[derive(Debug)]
enum JrpcHandlerErrorType {
    DebugServiceError(DebugServiceError),
    InactiveSession { error: String },
    InvalidParams { error: String },
    MethodNotFound { method: String },
    SessionAlreadyConnected { error: String },
}

impl JrpcHandlerError {
    fn should_terminate_session(&self) -> bool {
        matches!(
            self.error_type,
            JrpcHandlerErrorType::DebugServiceError(DebugServiceError::Conflict { .. })
                | JrpcHandlerErrorType::DebugServiceError(DebugServiceError::Unauthorized { .. })
        )
    }

    fn to_jrpc_response(&self) -> JsonRpcResponse {
        match &self.error_type {
            JrpcHandlerErrorType::DebugServiceError(e) => JsonRpcResponse::error(
                self.jrpc_id.clone(),
                JsonRpcError::new(
                    JsonRpcErrorReason::ApplicationError(-1),
                    e.to_safe_string(),
                    Value::Null,
                ),
            ),
            JrpcHandlerErrorType::InactiveSession { error } => JsonRpcResponse::error(
                self.jrpc_id.clone(),
                JsonRpcError::new(
                    JsonRpcErrorReason::ApplicationError(1000),
                    error.to_string(),
                    Value::Null,
                ),
            ),
            JrpcHandlerErrorType::InvalidParams { error } => JsonRpcResponse::error(
                self.jrpc_id.clone(),
                JsonRpcError::new(
                    JsonRpcErrorReason::InvalidParams,
                    error.to_string(),
                    Value::Null,
                ),
            ),
            JrpcHandlerErrorType::MethodNotFound { method } => JsonRpcResponse::error(
                self.jrpc_id.clone(),
                JsonRpcError::new(
                    JsonRpcErrorReason::MethodNotFound,
                    format!("Method not found: {method}"),
                    Value::Null,
                ),
            ),
            JrpcHandlerErrorType::SessionAlreadyConnected { error } => JsonRpcResponse::error(
                self.jrpc_id.clone(),
                JsonRpcError::new(
                    JsonRpcErrorReason::ApplicationError(1001),
                    error.to_string(),
                    Value::Null,
                ),
            ),
        }
    }
}

impl Display for JrpcHandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.error_type {
            JrpcHandlerErrorType::DebugServiceError(error) => {
                write!(f, "DebugServiceError: {error}")
            }
            JrpcHandlerErrorType::InactiveSession { error } => {
                write!(f, "InactiveSessionError: {error}")
            }
            JrpcHandlerErrorType::InvalidParams { error } => write!(f, "JsonRpcError: {error}"),
            JrpcHandlerErrorType::SessionAlreadyConnected { error } => {
                write!(f, "SessionAlreadyConnected: {error}")
            }
            JrpcHandlerErrorType::MethodNotFound { method } => {
                write!(f, "MethodNotFound: {method}")
            }
        }
    }
}

fn to_json_rpc_result<T: serde::Serialize>(
    jrpc_id: &Id,
    result: Result<T, DebugServiceError>,
) -> Result<JsonRpcResponse, JrpcHandlerError> {
    result
        .map(|result| JsonRpcResponse::success(jrpc_id.clone(), result))
        .map_err(|e| JrpcHandlerError::debug_service_error(jrpc_id.clone(), e))
}

fn parse_params<T: serde::de::DeserializeOwned>(
    jrpc_id: &Id,
    value: Value,
) -> Result<T, JrpcHandlerError> {
    serde_json::from_value(value)
        .map_err(|e| JrpcHandlerError::invalid_params(jrpc_id.clone(), e.to_string()))
}

fn inactive_session_error(jrpc_id: &Id) -> JrpcHandlerError {
    JrpcHandlerError::inactive_session(jrpc_id.clone())
}

fn method_not_found_error(id: &Id, method: &str) -> JrpcHandlerError {
    JrpcHandlerError::method_not_found(id.clone(), method)
}

#[derive(Clone, Debug)]
struct JsonRpcNotification {
    pub method: String,
    pub params: Value,
}

impl Serialize for JsonRpcNotification {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Helper<'a> {
            jsonrpc: &'static str,
            method: &'a str,
            params: &'a Value,
        }

        Helper {
            jsonrpc: "2.0",
            method: &self.method,
            params: &self.params,
        }
        .serialize(serializer)
    }
}

#[derive(Debug)]
enum OutgoingJsonRpcMessage {
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
    Error(JrpcHandlerError),
    Close,
}

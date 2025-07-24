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
use crate::model::params::*;
use crate::services::debug_service::{DebugService, DebugServiceError};
use axum_jrpc::error::{JsonRpcError, JsonRpcErrorReason};
use axum_jrpc::{Id, JsonRpcAnswer, JsonRpcRequest, JsonRpcResponse};
use golem_common::model::auth::{AuthCtx, Namespace};
use golem_common::model::{LogLevel, OwnedWorkerId, Timestamp, WorkerId};
use serde_json::Value;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;
use futures_util::{Sink, StreamExt};
use tokio::sync::mpsc::{self, Sender, Receiver};
use tokio_util::sync::{CancellationToken, DropGuard};
use tokio::select;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use serde::{Deserialize, Serialize};
use golem_worker_executor::services::worker_event::WorkerEventReceiver;

pub struct JrpcSessionData {
    pub namespace: Namespace,
    pub connected_worker: OwnedWorkerId,
}

pub struct JrpcSession {
    debug_service: Arc<dyn DebugService>,
    auth_ctx: AuthCtx,
    active_session: Option<JrpcSessionData>,

    // will be taken by a spawned future when succesfully connect to a worker for the first time
    notifications_sender: Option<Sender<JsonRpcNotification>>,
    // cancellation token dropguard for the future forwarding the notifications
    notifications_future_dropguard: Option<DropGuard>
}

impl JrpcSession {
    pub fn new(
        debug_service: Arc<dyn DebugService>,
        auth_ctx: AuthCtx,
        notifications_sender: Sender<JsonRpcNotification>,
    ) -> Self {
        Self {
            debug_service,
            auth_ctx,
            active_session: None,
            notifications_sender: Some(notifications_sender),
            notifications_future_dropguard: None
        }
    }

    pub async fn handle_request(&mut self, request: JsonRpcRequest) -> JsonRpcResult {
        let jrpc_id: Id = request.id;

        match request.method.as_str() {
            "current_oplog_index" => {
                if let Some(active_session_data) = &self.active_session {
                    let owned_worker_id = active_session_data.connected_worker.clone();

                    let result = self.debug_service.current_oplog_index(&owned_worker_id).await;
                    to_json_rpc_result(&jrpc_id, result)
                } else {
                    Err(inactive_session_error(&jrpc_id))
                }
            }
            "connect" => {
                if let Some(_) = &self.active_session {
                    Err(JrpcHandlerError::session_already_connected(jrpc_id.clone(), "Session is already connected to a worker".to_string()))?
                }

                let params: ConnectParams = parse_params(&jrpc_id, request.params)?;

                let result = self.debug_service
                    .connect(&self.auth_ctx, &params.worker_id)
                    .await;

                match result {
                    Ok((result, connected_worker, namespace, worker_event_receiver)) => {
                        self.active_session = Some(JrpcSessionData {
                            connected_worker,
                            namespace
                         });

                         self.register_worker_event_receiver(worker_event_receiver);

                        to_json_rpc_result(&jrpc_id, Ok(result))
                    },
                    Err(err) => {
                        to_json_rpc_result::<ConnectResult>(&jrpc_id, Err(err))
                    }
                }
            }
            "playback" => {
                if let Some(active_session_data) = &self.active_session {
                    let params: PlaybackParams = parse_params(&jrpc_id, request.params)?;

                    let owned_worker_id = active_session_data.connected_worker.clone();

                    let result = self.debug_service
                        .playback(
                            &owned_worker_id,
                            &active_session_data.namespace.account_id,
                            params.target_index,
                            params.overrides,
                            params.ensure_invocation_boundary.unwrap_or(true),
                            params
                                .time_out_in_seconds
                                .map(Duration::from_secs)
                                .unwrap_or(Duration::from_secs(5)),
                        )
                        .await;

                    to_json_rpc_result(&jrpc_id, result)
                } else {
                    Err(inactive_session_error(&jrpc_id))
                }
            }
            "rewind" => {
                if let Some(active_session_data) = &self.active_session {
                    let params: RewindParams = parse_params(&jrpc_id, request.params)?;

                    let owned_worker_id = active_session_data.connected_worker.clone();

                    let result = self.debug_service
                        .rewind(
                            &owned_worker_id,
                            &active_session_data.namespace.account_id,
                            params.target_index,
                            params.ensure_invocation_boundary.unwrap_or(true),
                            params
                                .time_out_in_seconds
                                .map(Duration::from_secs)
                                .unwrap_or(Duration::from_secs(5)),
                        )
                        .await;
                    to_json_rpc_result(&jrpc_id, result)
                } else {
                    Err(inactive_session_error(&jrpc_id))
                }
            }
            "fork" => {
                if let Some(active_session_data) = &self.active_session {
                    let owned_worker_id = active_session_data.connected_worker.clone();

                    let params: ForkParams = parse_params(&jrpc_id, request.params)?;
                    let result = self.debug_service
                        .fork(
                            &active_session_data.namespace.account_id,
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
    fn register_worker_event_receiver(&mut self, worker_event_receiver: WorkerEventReceiver) {
        // unwrap is safe as only one worker can be connected to the session
        let notifications_sender = self.notifications_sender.take().unwrap();

        let token = CancellationToken::new();
        let cloned_token = token.clone();

        let mut worker_event_stream = worker_event_receiver.to_stream();

        tokio::spawn(async move {
            select! {
                _ = cloned_token.cancelled() => {}
                event = worker_event_stream.next() => {
                    match event {
                        Some(Ok(event)) => {
                            if let Some(log_notifiation) = LogNotification::from_internal_worker_event(event) {
                                let params = serde_json::to_value(vec![log_notifiation]).expect("serializing message failed");

                                let _ = notifications_sender.send(
                                    JsonRpcNotification { method: "emit-logs".to_string(), params }
                                ).await;
                            }
                        }
                        Some(Err(BroadcastStreamRecvError::Lagged(number_of_missed_messages))) => {
                            let value = LogsLaggedNotification { number_of_missed_messages };

                            let params = serde_json::to_value(value).expect("serializing message failed");

                            let _ = notifications_sender.send(
                                JsonRpcNotification { method: "notify-logs-lagged".to_string(), params }
                            ).await;
                        },
                        None => {}
                    }
                }
            }
        });

        // cancel spawned forwarding future when we are dropped
        self.notifications_future_dropguard = Some(token.drop_guard());
    }
}


pub async fn jrpc_handler(
    debug_service: Arc<dyn DebugService>,
    json_rpc_request: JsonRpcRequest,
    active_session: Arc<ActiveSession>,
    auth_ctx: AuthCtx,
) -> JsonRpcResult {
    let jrpc_id: Id = json_rpc_request.id;

    match json_rpc_request.method.as_str() {
        "current_oplog_index" => {
            if let Some(active_session_data) = active_session.get_active_session().await {
                let owned_worker_id = OwnedWorkerId::new(
                    &active_session_data.cloud_namespace.project_id,
                    &active_session_data.worker_id,
                );

                let result = debug_service.current_oplog_index(&owned_worker_id).await;
                to_json_rpc_result(&jrpc_id, result)
            } else {
                Err(inactive_session_error(&jrpc_id))
            }
        }
        "connect" => {
            let params: ConnectParams = parse_params(&jrpc_id, json_rpc_request.params)?;

            let result = debug_service
                .connect(&auth_ctx, &params.worker_id, active_session)
                .await;

            to_json_rpc_result(&jrpc_id, result)
        }
        "playback" => {
            if let Some(active_session_data) = active_session.get_active_session().await {
                let params: PlaybackParams = parse_params(&jrpc_id, json_rpc_request.params)?;

                let owned_worker_id = OwnedWorkerId::new(
                    &active_session_data.cloud_namespace.project_id,
                    &active_session_data.worker_id,
                );

                let result = debug_service
                    .playback(
                        &owned_worker_id,
                        &active_session_data.cloud_namespace.account_id,
                        params.target_index,
                        params.overrides,
                        params.ensure_invocation_boundary.unwrap_or(true),
                        params
                            .time_out_in_seconds
                            .map(Duration::from_secs)
                            .unwrap_or(Duration::from_secs(5)),
                    )
                    .await;

                to_json_rpc_result(&jrpc_id, result)
            } else {
                Err(inactive_session_error(&jrpc_id))
            }
        }
        "rewind" => {
            if let Some(active_session_data) = active_session.get_active_session().await {
                let params: RewindParams = parse_params(&jrpc_id, json_rpc_request.params)?;

                let owned_worker_id = OwnedWorkerId::new(
                    &active_session_data.cloud_namespace.project_id,
                    &active_session_data.worker_id,
                );

                let result = debug_service
                    .rewind(
                        &owned_worker_id,
                        &active_session_data.cloud_namespace.account_id,
                        params.target_index,
                        params.ensure_invocation_boundary.unwrap_or(true),
                        params
                            .time_out_in_seconds
                            .map(Duration::from_secs)
                            .unwrap_or(Duration::from_secs(5)),
                    )
                    .await;
                to_json_rpc_result(&jrpc_id, result)
            } else {
                Err(inactive_session_error(&jrpc_id))
            }
        }
        "fork" => {
            if let Some(active_session_data) = active_session.get_active_session().await {
                let owned_worker_id = OwnedWorkerId::new(
                    &active_session_data.cloud_namespace.project_id,
                    &active_session_data.worker_id,
                );

                let params: ForkParams = parse_params(&jrpc_id, json_rpc_request.params)?;
                let result = debug_service
                    .fork(
                        &active_session_data.cloud_namespace.account_id,
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

pub struct JrpcHandlerError {
    pub jrpc_id: Id,
    pub error_type: JrpcHandlerErrorType,
}

impl JrpcHandlerError {
    pub fn debug_service_error(jrpc_id: Id, error: DebugServiceError) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::DebugServiceError(error),
        }
    }

    pub fn inactive_session(jrpc_id: Id) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::InactiveSession {
                error: "Inactive session".to_string(),
            },
        }
    }

    pub fn invalid_params(jrpc_id: Id, error: String) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::InvalidParams { error },
        }
    }

    pub fn method_not_found(jrpc_id: Id, method: &str) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::MethodNotFound {
                method: method.to_string(),
            },
        }
    }

    pub fn session_already_connected(jrpc_id: Id, error: String) -> Self {
        JrpcHandlerError {
            jrpc_id,
            error_type: JrpcHandlerErrorType::SessionAlreadyConnected { error },
        }
    }
}

pub enum JrpcHandlerErrorType {
    DebugServiceError(DebugServiceError),
    InactiveSession { error: String },
    InvalidParams { error: String },
    MethodNotFound { method: String },
    SessionAlreadyConnected { error: String }
}

impl JrpcHandlerError {
    pub fn should_terminate_session(&self) -> bool {
        matches!(
            self.error_type,
            JrpcHandlerErrorType::DebugServiceError(DebugServiceError::Conflict { .. })
                | JrpcHandlerErrorType::DebugServiceError(DebugServiceError::Unauthorized { .. })
        )
    }

    pub fn to_jrpc_response(&self) -> JsonRpcResponse {
        match &self.error_type {
            JrpcHandlerErrorType::DebugServiceError(e) => JsonRpcResponse::error(
                self.jrpc_id.clone(),
                JsonRpcError::new(
                    JsonRpcErrorReason::ApplicationError(-1),
                    e.to_string(),
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
            JrpcHandlerErrorType::SessionAlreadyConnected { error } => write!(f, "SessionAlreadyConnected: {error}"),
            JrpcHandlerErrorType::MethodNotFound { method } => {
                write!(f, "MethodNotFound: {method}")
            }
        }
    }
}

pub type JsonRpcResult = Result<JsonRpcResponse, JrpcHandlerError>;

pub fn to_json_rpc_result<T: serde::Serialize>(
    jrpc_id: &Id,
    result: Result<T, DebugServiceError>,
) -> JsonRpcResult {
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

pub fn method_not_found_error(id: &Id, method: &str) -> JrpcHandlerError {
    JrpcHandlerError::method_not_found(id.clone(), method)
}

#[derive(Clone)]
pub struct JsonRpcNotification {
    pub method: String,
    pub params: Value,
}

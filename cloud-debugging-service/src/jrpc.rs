use crate::debug_session::ActiveSession;
use crate::model::params::*;
use crate::services::debug_service::{DebugService, DebugServiceError};
use axum_jrpc::error::{JsonRpcError, JsonRpcErrorReason};
use axum_jrpc::{Id, JsonRpcRequest, JsonRpcResponse};
use cloud_common::auth::CloudAuthCtx;
use golem_common::model::OwnedWorkerId;
use serde_json::Value;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

pub async fn jrpc_handler(
    debug_service: Arc<dyn DebugService + Sync + Send>,
    json_rpc_request: JsonRpcRequest,
    active_session: Arc<ActiveSession>,
) -> JsonRpcResult {
    let jrpc_id: Id = json_rpc_request.id;

    match json_rpc_request.method.as_str() {
        "current_oplog_index" => {
            if let Some(active_session_data) = active_session.get_active_session().await {
                let owned_worker_id = OwnedWorkerId::new(
                    &active_session_data.cloud_namespace.account_id,
                    &active_session_data.worker_id,
                );

                let result = debug_service.current_oplog_index(owned_worker_id).await;
                to_json_rpc_result(&jrpc_id, result)
            } else {
                Err(inactive_session_error(&jrpc_id))
            }
        }
        "connect" => {
            let params: ConnectParams = parse_params(&jrpc_id, json_rpc_request.params)?;

            let auth_ctx = CloudAuthCtx::new(params.token);

            let result = debug_service
                .connect(&auth_ctx, params.worker_id, active_session)
                .await;

            to_json_rpc_result(&jrpc_id, result)
        }
        "playback" => {
            if let Some(active_session_data) = active_session.get_active_session().await {
                let params: PlaybackParams = parse_params(&jrpc_id, json_rpc_request.params)?;

                let owned_worker_id = OwnedWorkerId::new(
                    &active_session_data.cloud_namespace.account_id,
                    &active_session_data.worker_id,
                );

                let result = debug_service
                    .playback(
                        owned_worker_id,
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
                    &active_session_data.cloud_namespace.account_id,
                    &active_session_data.worker_id,
                );

                let result = debug_service
                    .rewind(
                        owned_worker_id,
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
                    &active_session_data.cloud_namespace.account_id,
                    &active_session_data.worker_id,
                );

                let params: ForkParams = parse_params(&jrpc_id, json_rpc_request.params)?;
                let result = debug_service
                    .fork(
                        owned_worker_id,
                        params.target_worker_id,
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
}

pub enum JrpcHandlerErrorType {
    DebugServiceError(DebugServiceError),
    InactiveSession { error: String },
    InvalidParams { error: String },
    MethodNotFound { method: String },
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
                    JsonRpcErrorReason::ApplicationError(-32000),
                    e.to_string(),
                    Value::Null,
                ),
            ),
            JrpcHandlerErrorType::InactiveSession { error } => JsonRpcResponse::error(
                self.jrpc_id.clone(),
                JsonRpcError::new(
                    JsonRpcErrorReason::ApplicationError(-32003),
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
                    format!("Method not found: {}", method),
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
                write!(f, "DebugServiceError: {}", error)
            }
            JrpcHandlerErrorType::InactiveSession { error } => {
                write!(f, "InactiveSessionError: {}", error)
            }
            JrpcHandlerErrorType::InvalidParams { error } => write!(f, "JsonRpcError: {}", error),
            JrpcHandlerErrorType::MethodNotFound { method } => {
                write!(f, "MethodNotFound: {}", method)
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

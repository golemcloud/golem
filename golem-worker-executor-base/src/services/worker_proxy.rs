// Copyright 2024-2025 Golem Cloud
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

use crate::error::GolemError;
use crate::grpc::authorised_grpc_request;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::v1::{
    invoke_and_await_typed_response, invoke_response, update_worker_response, worker_error,
    InvokeAndAwaitRequest, InvokeAndAwaitTypedResponse, InvokeRequest, InvokeResponse,
    UpdateWorkerRequest, UpdateWorkerResponse, WorkerError,
};
use golem_api_grpc::proto::golem::worker::{InvocationContext, InvokeParameters, UpdateMode};
use golem_common::client::GrpcClient;
use golem_common::model::{ComponentVersion, IdempotencyKey, OwnedWorkerId, WorkerId};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{Value, WitValue};
use http::Uri;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::debug;
use uuid::Uuid;

#[async_trait]
pub trait WorkerProxy {
    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        caller_worker_id: WorkerId,
        caller_args: Vec<String>,
        caller_env: HashMap<String, String>,
    ) -> Result<TypeAnnotatedValue, WorkerProxyError>;

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        caller_worker_id: WorkerId,
        caller_args: Vec<String>,
        caller_env: HashMap<String, String>,
    ) -> Result<(), WorkerProxyError>;

    async fn update(
        &self,
        owned_worker_id: &OwnedWorkerId,
        target_version: ComponentVersion,
        mode: UpdateMode,
    ) -> Result<(), WorkerProxyError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum WorkerProxyError {
    BadRequest(Vec<String>),
    Unauthorized(String),
    LimitExceeded(String),
    NotFound(String),
    AlreadyExists(String),
    InternalError(GolemError),
}

impl Error for WorkerProxyError {}

impl Display for WorkerProxyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerProxyError::BadRequest(errors) => write!(f, "Bad request: {}", errors.join(", ")),
            WorkerProxyError::Unauthorized(error) => write!(f, "Unauthorized: {error}"),
            WorkerProxyError::LimitExceeded(error) => write!(f, "Limit exceeded: {error}"),
            WorkerProxyError::NotFound(error) => write!(f, "Not found: {error}"),
            WorkerProxyError::AlreadyExists(error) => write!(f, "Already exists: {error}"),
            WorkerProxyError::InternalError(error) => write!(f, "Internal error: {error}"),
        }
    }
}

impl From<tonic::transport::Error> for WorkerProxyError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::InternalError(GolemError::unknown(format!(
            "gRPC Transport error: {}",
            value
        )))
    }
}

impl From<tonic::Status> for WorkerProxyError {
    fn from(value: tonic::Status) -> Self {
        Self::InternalError(GolemError::unknown(format!("gRPC error: {}", value)))
    }
}

impl From<WorkerError> for WorkerProxyError {
    fn from(value: WorkerError) -> Self {
        match value.error {
            Some(worker_error::Error::BadRequest(body)) => {
                WorkerProxyError::BadRequest(body.errors)
            }
            Some(worker_error::Error::Unauthorized(body)) => {
                WorkerProxyError::Unauthorized(body.error)
            }
            Some(worker_error::Error::LimitExceeded(body)) => {
                WorkerProxyError::LimitExceeded(body.error)
            }
            Some(worker_error::Error::NotFound(body)) => WorkerProxyError::NotFound(body.error),
            Some(worker_error::Error::AlreadyExists(body)) => {
                WorkerProxyError::AlreadyExists(body.error)
            }
            Some(worker_error::Error::InternalError(worker_executor_error)) => {
                WorkerProxyError::InternalError(worker_executor_error.try_into().unwrap_or(
                    GolemError::unknown("Unknown error from the worker executor".to_string()),
                ))
            }
            None => WorkerProxyError::InternalError(GolemError::unknown(
                "Empty error response from the worker API".to_string(),
            )),
        }
    }
}

impl From<GolemError> for WorkerProxyError {
    fn from(value: GolemError) -> Self {
        WorkerProxyError::InternalError(value)
    }
}

pub struct RemoteWorkerProxy {
    client: GrpcClient<WorkerServiceClient<Channel>>,
    access_token: Uuid,
}

impl RemoteWorkerProxy {
    pub fn new(endpoint: Uri, access_token: Uuid) -> Self {
        Self {
            client: GrpcClient::new(
                "worker_service",
                |channel| {
                    WorkerServiceClient::new(channel)
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip)
                },
                endpoint,
                Default::default(), // TODO
            ),
            access_token,
        }
    }
}

#[async_trait]
impl WorkerProxy for RemoteWorkerProxy {
    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        caller_worker_id: WorkerId,
        caller_args: Vec<String>,
        caller_env: HashMap<String, String>,
    ) -> Result<TypeAnnotatedValue, WorkerProxyError> {
        debug!(
            "Invoking remote worker function {function_name} with parameters {function_params:?}"
        );

        let proto_params = function_params
            .into_iter()
            .map(|param| {
                let value: Value = param.into();
                value.into()
            })
            .collect();
        let invoke_parameters = Some(InvokeParameters {
            params: proto_params,
        });

        let response: InvokeAndAwaitTypedResponse = self
            .client
            .call("invoke_and_await_typed", move |client| {
                Box::pin(client.invoke_and_await_typed(authorised_grpc_request(
                    InvokeAndAwaitRequest {
                        worker_id: Some(owned_worker_id.worker_id().into_target_worker_id().into()),
                        idempotency_key: idempotency_key.clone().map(|k| k.into()),
                        function: function_name.clone(),
                        invoke_parameters: invoke_parameters.clone(),
                        context: Some(InvocationContext {
                            parent: Some(caller_worker_id.clone().into()),
                            args: caller_args.clone(),
                            env: caller_env.clone(),
                        }),
                    },
                    &self.access_token,
                )))
            })
            .await?
            .into_inner();

        match response.result {
            Some(invoke_and_await_typed_response::Result::Success(result)) => {
                let result =
                    result
                        .result
                        .ok_or(WorkerProxyError::InternalError(GolemError::unknown(
                            "Missing result value in the worker API response".to_string(),
                        )))?;
                let result = result
                    .type_annotated_value
                    .ok_or(WorkerProxyError::InternalError(GolemError::unknown(
                        "Missing type_annotated_value in the worker API response".to_string(),
                    )))?;
                Ok(result)
            }
            Some(invoke_and_await_typed_response::Result::Error(error)) => Err(error.into()),
            None => Err(WorkerProxyError::InternalError(GolemError::unknown(
                "Empty response through the worker API".to_string(),
            ))),
        }
    }

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        caller_worker_id: WorkerId,
        caller_args: Vec<String>,
        caller_env: HashMap<String, String>,
    ) -> Result<(), WorkerProxyError> {
        debug!("Invoking remote worker function {function_name} with parameters {function_params:?} without awaiting for the result");

        let proto_params = function_params
            .into_iter()
            .map(|param| {
                let value: Value = param.into();
                value.into()
            })
            .collect();
        let invoke_parameters = Some(InvokeParameters {
            params: proto_params,
        });

        let response: InvokeResponse = self
            .client
            .call("invoke", move |client| {
                Box::pin(client.invoke(authorised_grpc_request(
                    InvokeRequest {
                        worker_id: Some(owned_worker_id.worker_id().into_target_worker_id().into()),
                        idempotency_key: idempotency_key.clone().map(|k| k.into()),
                        function: function_name.clone(),
                        invoke_parameters: invoke_parameters.clone(),
                        context: Some(InvocationContext {
                            parent: Some(caller_worker_id.clone().into()),
                            args: caller_args.clone(),
                            env: caller_env.clone(),
                        }),
                    },
                    &self.access_token,
                )))
            })
            .await?
            .into_inner();

        match response.result {
            Some(invoke_response::Result::Success(_)) => Ok(()),
            Some(invoke_response::Result::Error(error)) => Err(error.into()),
            None => Err(WorkerProxyError::InternalError(GolemError::unknown(
                "Empty response through the worker API".to_string(),
            ))),
        }
    }

    async fn update(
        &self,
        owned_worker_id: &OwnedWorkerId,
        target_version: ComponentVersion,
        mode: UpdateMode,
    ) -> Result<(), WorkerProxyError> {
        debug!("Updating remote worker to version {target_version} in {mode:?} mode");

        let response: UpdateWorkerResponse = self
            .client
            .call("update_worker", move |client| {
                Box::pin(client.update_worker(authorised_grpc_request(
                    UpdateWorkerRequest {
                        worker_id: Some(owned_worker_id.worker_id().into()),
                        target_version,
                        mode: mode as i32,
                    },
                    &self.access_token,
                )))
            })
            .await?
            .into_inner();

        match response.result {
            Some(update_worker_response::Result::Success(_)) => Ok(()),
            Some(update_worker_response::Result::Error(error)) => Err(error.into()),
            None => Err(WorkerProxyError::InternalError(GolemError::unknown(
                "Empty response through the worker API".to_string(),
            ))),
        }
    }
}

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

use crate::error::GolemError;
use crate::grpc::{authorised_grpc_request, UriBackConversion};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::worker_error::Error;
use golem_api_grpc::proto::golem::worker::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::{
    invoke_and_await_response, invoke_and_await_response_json, CallingConvention,
    InvokeAndAwaitRequest, InvokeAndAwaitRequestJson, InvokeAndAwaitResponse,
    InvokeAndAwaitResponseJson, InvokeParameters, WorkerError,
};
use golem_common::model::WorkerId;
use golem_wasm_rpc::{Value, WitValue};
use http::Uri;
use std::fmt::Write;
use uuid::Uuid;

#[async_trait]
pub trait Rpc {
    async fn create_demand(&self, worker_id: &WorkerId) -> Box<dyn RpcDemand>;

    async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<String>,
    ) -> Result<String, RpcError>;

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> Result<WitValue, RpcError>;
}

pub enum RpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
}

impl From<WorkerError> for RpcError {
    fn from(value: WorkerError) -> Self {
        match &value.error {
            Some(Error::BadRequest(errors)) => Self::ProtocolError {
                details: format!("Bad request: {}", errors.errors.join(", ")),
            },
            Some(Error::Unauthorized(error)) => Self::Denied {
                details: format!("Unauthorized: {}", error.error),
            },
            Some(Error::LimitExceeded(error)) => Self::Denied {
                details: format!("Limit exceeded: {}", error.error),
            },
            Some(Error::NotFound(error)) => Self::NotFound {
                details: error.error.clone(),
            },
            Some(Error::AlreadyExists(error)) => Self::ProtocolError {
                details: format!(
                    "Unexpected response: worker already exists: {}",
                    error.error
                ),
            },
            Some(Error::InternalError(error)) => {
                match TryInto::<GolemError>::try_into(error.clone()) {
                    Ok(golem_error) => Self::RemoteInternalError {
                        details: golem_error.to_string(),
                    },
                    Err(_) => Self::ProtocolError {
                        details: format!("Invalid internal error: {:?}", error.error),
                    },
                }
            }
            None => Self::ProtocolError {
                details: "Error response without any details".to_string(),
            },
        }
    }
}

impl From<tonic::transport::Error> for RpcError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::ProtocolError {
            details: format!("gRPC Transport error: {}", value),
        }
    }
}

impl From<tonic::Status> for RpcError {
    fn from(value: tonic::Status) -> Self {
        Self::ProtocolError {
            details: format!("gRPC error: {}", value),
        }
    }
}

pub trait RpcDemand: Send + Sync {}

pub struct RemoteInvocationRpc {
    endpoint: Uri,
    access_token: Uuid,
}

impl RemoteInvocationRpc {
    pub fn new(endpoint: Uri, access_token: Uuid) -> Self {
        Self {
            endpoint,
            access_token,
        }
    }
}

/// Rpc implementation simply calling the public Golem Worker API for invocation
#[async_trait]
impl Rpc for RemoteInvocationRpc {
    async fn create_demand(&self, _worker_id: &WorkerId) -> Box<dyn RpcDemand> {
        Box::new(())
    }

    async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<String>,
    ) -> Result<String, RpcError> {
        let mut json_parameter_array = String::new();
        let _ = json_parameter_array.write_char('[');
        let _ = json_parameter_array.write_str(&function_params.join(","));
        let _ = json_parameter_array.write_char(']');

        let mut client = WorkerServiceClient::connect(self.endpoint.as_http_02()).await?;
        let response: InvokeAndAwaitResponseJson = client
            .invoke_and_await_json(authorised_grpc_request(
                InvokeAndAwaitRequestJson {
                    worker_id: Some(worker_id.clone().into()),
                    invocation_key: None,
                    function: function_name,
                    invoke_parameters_json: json_parameter_array,
                    calling_convention: CallingConvention::Component as i32,
                },
                &self.access_token,
            ))
            .await?
            .into_inner();

        match response.result {
            Some(invoke_and_await_response_json::Result::Success(result)) => Ok(result.result_json),
            Some(invoke_and_await_response_json::Result::Error(error)) => Err(error.into()),
            None => Err(RpcError::ProtocolError {
                details: "Empty response through the worker API".to_string(),
            }),
        }
    }

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> Result<WitValue, RpcError> {
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

        let mut client = WorkerServiceClient::connect(self.endpoint.as_http_02()).await?;
        let response: InvokeAndAwaitResponse = client
            .invoke_and_await(authorised_grpc_request(
                InvokeAndAwaitRequest {
                    worker_id: Some(worker_id.clone().into()),
                    invocation_key: None,
                    function: function_name,
                    invoke_parameters,
                    calling_convention: CallingConvention::Component as i32,
                },
                &self.access_token,
            ))
            .await?
            .into_inner();

        match response.result {
            Some(invoke_and_await_response::Result::Success(result)) => {
                let mut result_values = Vec::new();
                for proto_value in result.result {
                    let value: Value =
                        proto_value
                            .try_into()
                            .map_err(|err| RpcError::ProtocolError {
                                details: format!("Could not decode result: {err}"),
                            })?;
                    result_values.push(value);
                }
                let result: WitValue = Value::Tuple(result_values).into();
                Ok(result)
            }
            Some(invoke_and_await_response::Result::Error(error)) => Err(error.into()),
            None => Err(RpcError::ProtocolError {
                details: "Empty response through the worker API".to_string(),
            }),
        }
    }
}

impl RpcDemand for () {}

#[cfg(any(feature = "mocks", test))]
pub struct RpcMock;

#[cfg(any(feature = "mocks", test))]
impl Default for RpcMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl RpcMock {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl Rpc for RpcMock {
    async fn create_demand(&self, _worker_id: &WorkerId) -> Box<dyn RpcDemand> {
        Box::new(())
    }

    async fn invoke_and_await_json(
        &self,
        _worker_id: &WorkerId,
        _function_name: String,
        _function_params: Vec<String>,
    ) -> Result<String, RpcError> {
        todo!()
    }

    async fn invoke_and_await(
        &self,
        _worker_id: &WorkerId,
        _function_name: String,
        _function_params: Vec<WitValue>,
    ) -> Result<WitValue, RpcError> {
        todo!()
    }
}

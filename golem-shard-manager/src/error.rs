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

use std::fmt::Debug;

use std::fmt::Formatter;

use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_error;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::retriable_error::IsRetriableError;

#[derive(thiserror::Error, Debug)]
pub enum ShardManagerError {
    #[error("No source IP for pod")]
    NoSourceIpForPod,
    #[error("Failed to resolve address for pod")]
    FailedAddressResolveForPod,
    #[error("Timeout")]
    Timeout,
    #[error("gRPC: error status: {0}")]
    GrpcError(#[from] tonic::Status),
    #[error("No result")]
    NoResult,
    #[error("Worker execution error: {0}")]
    WorkerExecutionError(String),
    #[error("Persistence serialization error {0}")]
    SerializationError(String),
    #[error("Redis error {0}")]
    RedisError(#[from] fred::error::RedisError),
    #[error("IO error {0}")]
    IoError(#[from] std::io::Error),
}

impl IsRetriableError for ShardManagerError {
    fn is_retriable(&self) -> bool {
        match self {
            ShardManagerError::NoSourceIpForPod => false,
            ShardManagerError::FailedAddressResolveForPod => false,
            ShardManagerError::Timeout => true,
            ShardManagerError::GrpcError(status) => status.is_retriable(),
            ShardManagerError::NoResult => true,
            ShardManagerError::WorkerExecutionError(_) => true, // TODO: can we define which ones are retryable?
            ShardManagerError::SerializationError(_) => false,
            ShardManagerError::RedisError(_) => false,
            ShardManagerError::IoError(_) => false,
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl From<ShardManagerError> for golem::shardmanager::v1::ShardManagerError {
    fn from(value: ShardManagerError) -> golem::shardmanager::v1::ShardManagerError {
        let error = |cons: fn(golem::common::ErrorBody) -> shard_manager_error::Error,
                     error: String| {
            golem::shardmanager::v1::ShardManagerError {
                error: Some(cons(golem::common::ErrorBody { error })),
            }
        };

        match value {
            ShardManagerError::NoSourceIpForPod => error(
                shard_manager_error::Error::InvalidRequest,
                "NoSourceIpForPod".to_string(),
            ),
            ShardManagerError::FailedAddressResolveForPod => error(
                shard_manager_error::Error::Unknown,
                "FailedAddressResolveForPod".to_string(),
            ),
            ShardManagerError::Timeout => {
                error(shard_manager_error::Error::Timeout, "Timeout".to_string())
            }
            ShardManagerError::GrpcError(status) => {
                error(shard_manager_error::Error::Unknown, status.to_string())
            }
            ShardManagerError::NoResult => {
                error(shard_manager_error::Error::Unknown, "NoResult".to_string())
            }
            ShardManagerError::WorkerExecutionError(details) => {
                error(shard_manager_error::Error::Unknown, details)
            }
            ShardManagerError::SerializationError(details) => {
                error(shard_manager_error::Error::Unknown, details)
            }
            ShardManagerError::RedisError(err) => {
                error(shard_manager_error::Error::Unknown, err.to_string())
            }
            ShardManagerError::IoError(err) => {
                error(shard_manager_error::Error::Unknown, err.to_string())
            }
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum HealthCheckError {
    #[error("gRPC: error status: {0}")]
    GrpcError(tonic::Status),
    #[error("gRPC: transport error: {0}")]
    GrpcTransportError(#[source] tonic::transport::Error),
    #[error("gRPC: {0}")]
    GrpcOther(&'static str),
    #[error("K8s: connect error: {0}")]
    K8sConnectError(#[source] kube::Error),
    #[error("K8s: {0}")]
    K8sOther(&'static str),
}

impl IsRetriableError for HealthCheckError {
    fn is_retriable(&self) -> bool {
        match self {
            HealthCheckError::GrpcError(status) => status.is_retriable(),
            HealthCheckError::GrpcTransportError(_) => true,
            HealthCheckError::GrpcOther(_) => true,
            HealthCheckError::K8sConnectError(_) => true,
            HealthCheckError::K8sOther(_) => true,
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

pub struct ShardManagerTraceErrorKind<'a>(pub &'a golem::shardmanager::v1::ShardManagerError);

impl Debug for ShardManagerTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TraceErrorKind for ShardManagerTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                shard_manager_error::Error::InvalidRequest(_) => "InvalidRequest",
                shard_manager_error::Error::Timeout(_) => "Timeout",
                shard_manager_error::Error::Unknown(_) => "Unknown",
            },
        }
    }
}

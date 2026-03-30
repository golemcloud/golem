// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use golem_common::retriable_error::IsRetriableError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;

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
    WorkerExecutionError(WorkerExecutorError),
    #[error("Persistence serialization error {0}")]
    SerializationError(String),
    #[error("Postgres error {0}")]
    RepoError(#[from] RepoError),
    #[error("Migration error {0}")]
    MigrationError(#[from] anyhow::Error),
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
            ShardManagerError::RepoError(_) => false,
            ShardManagerError::MigrationError(_) => false,
            ShardManagerError::IoError(_) => false,
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
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

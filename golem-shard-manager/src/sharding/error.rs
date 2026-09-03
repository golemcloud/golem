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

use crate::sharding::leader_election::LeaseLost;
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
    #[error("Concurrent modification: the persisted shard state was changed by another writer")]
    ConcurrentModification,
    #[error(
        "Leadership lost: the election key {leader_key} is no longer held at creation revision \
         {create_revision}"
    )]
    LeadershipLost {
        leader_key: String,
        create_revision: i64,
    },
    #[error("Leadership lease lost while campaigning: {0}")]
    LeaseLostWhileCampaigning(#[source] LeaseLost),
    #[error("Shutdown requested")]
    ShutdownRequested,
    #[error("DB error {0}")]
    RepoError(#[from] RepoError),
    #[error("etcd error {0}")]
    EtcdError(#[from] etcd_client::Error),
    #[error("Migration error {0}")]
    MigrationError(#[from] anyhow::Error),
    #[error("IO error {0}")]
    IoError(#[from] std::io::Error),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl ShardManagerError {
    /// A second copy of this error, for the fail-stop slot: a refused write has to reach both the
    /// caller whose request it was and the loop that must end the process because of it.
    ///
    /// Every variant a caller *matches* on - a lost fence, a revision conflict, a shutdown - is
    /// reproduced exactly. The ones whose payload cannot be duplicated (`anyhow`, `io`, `RepoError`,
    /// `etcd_client`) degrade to [`ShardManagerError::Internal`] carrying the same message, which
    /// is what a log line or a gRPC error body would have shown of them anyway.
    pub(crate) fn duplicate(&self) -> Self {
        match self {
            Self::NoSourceIpForPod => Self::NoSourceIpForPod,
            Self::FailedAddressResolveForPod => Self::FailedAddressResolveForPod,
            Self::Timeout => Self::Timeout,
            Self::GrpcError(status) => Self::GrpcError(status.clone()),
            Self::NoResult => Self::NoResult,
            Self::SerializationError(message) => Self::SerializationError(message.clone()),
            Self::ConcurrentModification => Self::ConcurrentModification,
            Self::LeadershipLost {
                leader_key,
                create_revision,
            } => Self::LeadershipLost {
                leader_key: leader_key.clone(),
                create_revision: *create_revision,
            },
            Self::ShutdownRequested => Self::ShutdownRequested,
            Self::Internal(message) => Self::Internal(message.clone()),
            other => Self::Internal(other.to_string()),
        }
    }
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
            // Retrying a compare-and-swap with the same, now stale, previous revision can never
            // succeed: recovery is a re-read followed by re-deriving the change, which is a
            // different operation. Reporting this as retriable would turn a conflict into a spin.
            ShardManagerError::ConcurrentModification => false,
            // Another replica holds the leadership now; no retry here can take it back.
            ShardManagerError::LeadershipLost { .. } => false,
            // A campaigner holds nothing yet: a fresh lease and a new campaign is full recovery.
            ShardManagerError::LeaseLostWhileCampaigning(_) => true,
            // Retrying would be the process refusing the stop it was just asked for.
            ShardManagerError::ShutdownRequested => false,
            ShardManagerError::RepoError(_) => false,
            ShardManagerError::EtcdError(err) => match err {
                etcd_client::Error::GRpcStatus(status) => status.is_retriable(),
                etcd_client::Error::TransportError(_)
                | etcd_client::Error::IoError(_)
                | etcd_client::Error::EndpointError(_) => true,
                // A catch-all is required regardless: `etcd_client::Error` has a
                // `#[cfg(feature = "tls-openssl")]` variant. Everything else - bad URI, bad
                // arguments, bad metadata - is a configuration bug, not a transient failure.
                _ => false,
            },
            ShardManagerError::MigrationError(_) => false,
            ShardManagerError::IoError(_) => false,
            ShardManagerError::Internal(_) => false,
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
    #[cfg(feature = "kubernetes")]
    #[error("K8s: connect error: {0}")]
    K8sConnectError(#[source] kube::Error),
    #[cfg(feature = "kubernetes")]
    #[error("K8s: pod not found")]
    K8sPodNotFound,
    #[cfg(feature = "kubernetes")]
    #[error("K8s: pod terminated")]
    K8sPodTerminated,
    #[cfg(feature = "kubernetes")]
    #[error("K8s: pod is not ready")]
    K8sPodNotReady,
    #[cfg(feature = "kubernetes")]
    #[error("K8s: no pod status")]
    K8sNoPodStatus,
    #[cfg(feature = "kubernetes")]
    #[error("K8s: no pod name")]
    K8sNoPodName,
}

impl IsRetriableError for HealthCheckError {
    fn is_retriable(&self) -> bool {
        match self {
            HealthCheckError::GrpcError(status) => status.is_retriable(),
            HealthCheckError::GrpcTransportError(_) => true,
            HealthCheckError::GrpcOther(_) => true,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sConnectError(_) => true,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sPodNotFound => false,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sPodTerminated => false,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sPodNotReady => true,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sNoPodStatus => true,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sNoPodName => false,
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::ShardManagerError;
    use golem_common::retriable_error::IsRetriableError;

    #[test]
    // `with_retries` re-invokes with the same arguments, so a retry of any of these can only
    // spin: the revision stays stale, the leadership stays lost, the stop stays requested.
    fn the_fail_stop_errors_are_not_retriable() {
        assert!(!ShardManagerError::ConcurrentModification.is_retriable());
        assert!(
            !ShardManagerError::LeadershipLost {
                leader_key: "/golem/shard-manager/leader/abc".to_string(),
                create_revision: 7,
            }
            .is_retriable()
        );
        assert!(!ShardManagerError::ShutdownRequested.is_retriable());
    }
}

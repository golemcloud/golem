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

use crate::grpc::client::{GrpcClient, GrpcClientConfig};
use crate::model::quota_lease::{PendingReservation, QuotaLease};
use applying::Apply;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_client::ShardManagerServiceClient;
use golem_api_grpc::proto::golem::shardmanager::v1::{
    AcquireQuotaLeaseRequest, BatchRenewQuotaLeasesRequest, DeregisterRequest,
    GetRoutingTableRequest, RegisterRequest, ReleaseQuotaLeaseRequest, RenewQuotaLeaseRequest,
    RenewShardLeaseRequest, acquire_quota_lease_response, deregister_response,
    get_routing_table_response, register_response, release_quota_lease_response,
    renew_quota_lease_response, renew_quota_lease_result, renew_shard_lease_response,
};
use golem_common::config::{ConfigExample, HasConfigExamples};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::protobuf::{shard_epochs_from_proto, shard_epochs_to_proto};
use golem_common::model::quota::{ResourceDefinitionId, ResourceName};
use golem_common::model::{RetryConfig, RoutingTable, ShardEpoch, ShardId};
use golem_common::retriable_error::IsRetriableError;
use golem_common::retries::with_retries;
use golem_common::{IntoAnyhow, SafeDisplay, grpc_uri};
use http::Uri;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::SystemTime;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;
use uuid::Uuid;

/// Client for the shard manager service.
///
/// Mirrors `golem-api-grpc/proto/golem/shardmanager/v1/shard_manager_service.proto`.
#[async_trait]
pub trait ShardManager: Send + Sync {
    /// Fetches the current routing table (shard-to-pod mapping).
    async fn get_routing_table(&self) -> Result<RoutingTable, ShardManagerError>;

    /// Registers this executor pod with the shard manager, identified by the
    /// UUID it generated at startup. Idempotent: the same `executor_id` at the
    /// same address refreshes the existing shard lease rather than creating a
    /// second one.
    async fn register(
        &self,
        port: u16,
        pod_name: Option<String>,
        executor_id: Uuid,
    ) -> Result<ShardRegistration, ShardManagerError>;

    /// Extends this executor's shard lease. `shard_epochs` must be exactly the
    /// set the executor was last told it holds; any mismatch fails the whole
    /// renewal with `StaleEpoch` and renews nothing.
    async fn renew_shard_lease(
        &self,
        executor_id: Uuid,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<ShardLease, ShardLeaseError>;

    /// Releases the shard lease on a graceful shutdown. Lenient by contract: a
    /// stale entry, or an unknown executor, never fails a shutdown.
    async fn deregister(
        &self,
        executor_id: Uuid,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<(), ShardLeaseError>;

    /// Declares interest in a quota and requests an initial lease.
    async fn acquire_quota_lease(
        &self,
        environment_id: EnvironmentId,
        resource_name: ResourceName,
        port: u16,
    ) -> Result<QuotaLease, QuotaError>;

    /// Renews an existing lease before it expires.
    async fn renew_quota_lease(
        &self,
        resource_definition_id: ResourceDefinitionId,
        port: u16,
        epoch: u64,
        unused: u64,
        pending_reservations: Vec<PendingReservation>,
    ) -> Result<QuotaLease, QuotaError>;

    /// Renew multiple leases in one round-trip.  Results are in the same
    /// order as the input entries; each entry is independent.
    async fn batch_renew_quota_leases(
        &self,
        port: u16,
        renewals: Vec<BatchRenewalEntry>,
    ) -> Result<Vec<Result<QuotaLease, QuotaError>>, ShardManagerError>;

    /// Releases a lease, returning any unused allocation.
    async fn release_quota_lease(
        &self,
        resource_definition_id: ResourceDefinitionId,
        port: u16,
        epoch: u64,
        unused: u64,
    ) -> Result<(), QuotaError>;
}

/// What a successful `Register` returns: the cluster's shard count plus this
/// executor's shard lease.
#[derive(Debug, Clone)]
pub struct ShardRegistration {
    pub number_of_shards: u32,
    pub lease: ShardLease,
}

/// An executor's shard lease: the complete set of shards it owns with the epoch
/// of each, and when the lease lapses if it is not renewed.
#[derive(Debug, Clone, Default)]
pub struct ShardLease {
    pub shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    /// `None` means the lease never expires.
    pub expires_at: Option<DateTime<Utc>>,
}

impl TryFrom<golem_api_grpc::proto::golem::shardmanager::v1::ShardLease> for ShardLease {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::shardmanager::v1::ShardLease,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            shard_epochs: shard_epochs_from_proto(value.shard_epochs)?,
            expires_at: expires_at_from_proto(value.expires_at)?,
        })
    }
}

pub(crate) fn expires_at_from_proto(
    expires_at: Option<prost_types::Timestamp>,
) -> Result<Option<DateTime<Utc>>, String> {
    expires_at
        .map(|timestamp| {
            timestamp
                .apply(SystemTime::try_from)
                .map_err(|_| "Failed to convert expires_at".to_string())
                .map(DateTime::<Utc>::from)
        })
        .transpose()
}

/// One entry in a batch renewal request.
#[derive(Debug, Clone)]
pub struct BatchRenewalEntry {
    pub resource_definition_id: ResourceDefinitionId,
    pub epoch: u64,
    pub unused: u64,
    pub pending_reservations: Vec<PendingReservation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrpcShardManagerConfig {
    pub host: String,
    pub port: u16,
    #[serde(flatten)]
    pub client_config: GrpcClientConfig,
    pub retries: RetryConfig,
}

impl GrpcShardManagerConfig {
    pub fn uri(&self) -> Uri {
        grpc_uri(&self.host, self.port, self.client_config.tls_enabled())
    }
}

impl SafeDisplay for GrpcShardManagerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "{}", self.client_config.to_safe_string());
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        result
    }
}

impl Default for GrpcShardManagerConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9002,
            client_config: GrpcClientConfig::default(),
            retries: RetryConfig::default(),
        }
    }
}

impl HasConfigExamples<GrpcShardManagerConfig> for GrpcShardManagerConfig {
    fn examples() -> Vec<ConfigExample<GrpcShardManagerConfig>> {
        vec![]
    }
}

#[derive(Clone)]
pub struct GrpcShardManager {
    client: GrpcClient<ShardManagerServiceClient<OtelGrpcService<Channel>>>,
    retries: RetryConfig,
}

impl GrpcShardManager {
    pub fn new(config: &GrpcShardManagerConfig) -> Self {
        let client = GrpcClient::new(
            "shard_manager",
            |channel, max_message_size| {
                ShardManagerServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
                    .max_decoding_message_size(max_message_size)
                    .max_encoding_message_size(max_message_size)
            },
            config.uri(),
            config.client_config.clone(),
        );
        Self {
            client,
            retries: config.retries.clone(),
        }
    }
}

#[async_trait]
impl ShardManager for GrpcShardManager {
    async fn get_routing_table(&self) -> Result<RoutingTable, ShardManagerError> {
        let response = self
            .client
            .call("get_routing_table", move |client| {
                Box::pin(client.get_routing_table(GetRoutingTableRequest {}))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(ShardManagerError::empty_response()),
            Some(get_routing_table_response::Result::Success(routing_table)) => routing_table
                .try_into()
                .map_err(ShardManagerError::ConversionError),
            Some(get_routing_table_response::Result::Failure(failure)) => Err(failure.into()),
        }
    }

    async fn register(
        &self,
        port: u16,
        pod_name: Option<String>,
        executor_id: Uuid,
    ) -> Result<ShardRegistration, ShardManagerError> {
        with_retries(
            "shard_manager",
            "register",
            Some(format!("{pod_name:?}")),
            &self.retries,
            &(self.client.clone(), port, pod_name, executor_id),
            |(client, port, pod_name, executor_id)| {
                Box::pin(async move {
                    let response = client
                        .call("register", move |client| {
                            let request = RegisterRequest {
                                port: *port as i32,
                                pod_name: pod_name.clone(),
                                executor_id: executor_id.to_string(),
                            };
                            Box::pin(client.register(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err(ShardManagerError::empty_response()),
                        Some(register_response::Result::Success(success)) => {
                            Ok(ShardRegistration {
                                number_of_shards: success.number_of_shards,
                                lease: ShardLease {
                                    shard_epochs: shard_epochs_from_proto(success.shard_epochs)
                                        .map_err(ShardManagerError::ConversionError)?,
                                    expires_at: expires_at_from_proto(success.expires_at)
                                        .map_err(ShardManagerError::ConversionError)?,
                                },
                            })
                        }
                        Some(register_response::Result::Failure(failure)) => Err(failure.into()),
                    }
                })
            },
            |_| true,
        )
        .await
    }

    async fn renew_shard_lease(
        &self,
        executor_id: Uuid,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<ShardLease, ShardLeaseError> {
        let response = self
            .client
            .call("renew_shard_lease", move |client| {
                let request = RenewShardLeaseRequest {
                    executor_id: executor_id.to_string(),
                    shard_epochs: shard_epochs_to_proto(
                        shard_epochs
                            .iter()
                            .map(|(shard_id, epoch)| (*shard_id, *epoch)),
                    ),
                };
                Box::pin(client.renew_shard_lease(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(ShardLeaseError::empty_response()),
            Some(renew_shard_lease_response::Result::Success(lease)) => {
                lease.try_into().map_err(ShardLeaseError::ConversionError)
            }
            Some(renew_shard_lease_response::Result::Failure(failure)) => Err(failure.into()),
        }
    }

    async fn deregister(
        &self,
        executor_id: Uuid,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<(), ShardLeaseError> {
        let response = self
            .client
            .call("deregister", move |client| {
                let request = DeregisterRequest {
                    executor_id: executor_id.to_string(),
                    shard_epochs: shard_epochs_to_proto(
                        shard_epochs
                            .iter()
                            .map(|(shard_id, epoch)| (*shard_id, *epoch)),
                    ),
                };
                Box::pin(client.deregister(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(ShardLeaseError::empty_response()),
            Some(deregister_response::Result::Success(_)) => Ok(()),
            Some(deregister_response::Result::Failure(failure)) => Err(failure.into()),
        }
    }

    async fn acquire_quota_lease(
        &self,
        environment_id: EnvironmentId,
        resource_name: ResourceName,
        port: u16,
    ) -> Result<QuotaLease, QuotaError> {
        let response = self
            .client
            .call("acquire_quota_lease", move |client| {
                let request = AcquireQuotaLeaseRequest {
                    environment_id: Some(environment_id.into()),
                    resource_name: resource_name.0.clone(),
                    port: port as i32,
                };
                Box::pin(client.acquire_quota_lease(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(QuotaError::empty_response()),
            Some(acquire_quota_lease_response::Result::Success(success)) => {
                let lease = success.lease.ok_or_else(QuotaError::empty_response)?;
                lease.try_into().map_err(QuotaError::ConversionError)
            }
            Some(acquire_quota_lease_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn renew_quota_lease(
        &self,
        resource_definition_id: ResourceDefinitionId,
        port: u16,
        epoch: u64,
        unused: u64,
        pending_reservations: Vec<PendingReservation>,
    ) -> Result<QuotaLease, QuotaError> {
        let response = self
            .client
            .call("renew_quota_lease", move |client| {
                let request = RenewQuotaLeaseRequest {
                    resource_definition_id: Some(resource_definition_id.into()),
                    port: port as i32,
                    epoch,
                    unused,
                    pending_reservations: pending_reservations
                        .iter()
                        .cloned()
                        .map(Into::into)
                        .collect(),
                };
                Box::pin(client.renew_quota_lease(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(QuotaError::empty_response()),
            Some(renew_quota_lease_response::Result::Success(success)) => {
                let lease = success.lease.ok_or_else(QuotaError::empty_response)?;
                lease.try_into().map_err(QuotaError::ConversionError)
            }
            Some(renew_quota_lease_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn batch_renew_quota_leases(
        &self,
        port: u16,
        renewals: Vec<BatchRenewalEntry>,
    ) -> Result<Vec<Result<QuotaLease, QuotaError>>, ShardManagerError> {
        let grpc_renewals: Vec<RenewQuotaLeaseRequest> = renewals
            .into_iter()
            .map(|e| RenewQuotaLeaseRequest {
                resource_definition_id: Some(e.resource_definition_id.into()),
                port: port as i32,
                epoch: e.epoch,
                unused: e.unused,
                pending_reservations: e.pending_reservations.into_iter().map(Into::into).collect(),
            })
            .collect();

        let response = self
            .client
            .call("batch_renew_quota_leases", move |client| {
                Box::pin(
                    client.batch_renew_quota_leases(BatchRenewQuotaLeasesRequest {
                        renewals: grpc_renewals.clone(),
                    }),
                )
            })
            .await?
            .into_inner();

        Ok(response
            .results
            .into_iter()
            .map(|r| match r.result {
                None => Err(QuotaError::empty_response()),
                Some(renew_quota_lease_result::Result::Success(s)) => s
                    .lease
                    .ok_or_else(QuotaError::empty_response)?
                    .try_into()
                    .map_err(QuotaError::ConversionError),
                Some(renew_quota_lease_result::Result::Error(e)) => Err(e.into()),
            })
            .collect())
    }

    async fn release_quota_lease(
        &self,
        resource_definition_id: ResourceDefinitionId,
        port: u16,
        epoch: u64,
        unused: u64,
    ) -> Result<(), QuotaError> {
        let response = self
            .client
            .call("release_quota_lease", move |client| {
                let request = ReleaseQuotaLeaseRequest {
                    resource_definition_id: Some(resource_definition_id.into()),
                    port: port as i32,
                    epoch,
                    unused,
                };
                Box::pin(client.release_quota_lease(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(QuotaError::empty_response()),
            Some(release_quota_lease_response::Result::Success(_)) => Ok(()),
            Some(release_quota_lease_response::Result::Error(error)) => Err(error.into()),
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ShardManagerError {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Timeout: {0}")]
    Timeout(String),
    #[error("Conversion error: {0}")]
    ConversionError(String),
    #[error("Internal server error: {0}")]
    InternalServerError(String),
    #[error("Internal client error: {0}")]
    InternalClientError(String),
}

impl ShardManagerError {
    pub fn internal_client_error(error: impl AsRef<str>) -> Self {
        Self::InternalClientError(error.as_ref().to_string())
    }

    pub fn empty_response() -> Self {
        Self::internal_client_error("empty response")
    }
}

impl SafeDisplay for ShardManagerError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InvalidRequest(_) => self.to_string(),
            Self::Timeout(_) => self.to_string(),
            Self::ConversionError(_) => self.to_string(),
            Self::InternalServerError(_) => "Internal error".to_string(),
            Self::InternalClientError(_) => "Internal error".to_string(),
        }
    }
}

impl IsRetriableError for ShardManagerError {
    fn is_retriable(&self) -> bool {
        matches!(
            self,
            ShardManagerError::Timeout(_)
                | ShardManagerError::InternalServerError(_)
                | ShardManagerError::InternalClientError(_)
        )
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl IntoAnyhow for ShardManagerError {
    fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::from(self).context("ShardManagerError")
    }
}

impl From<golem_api_grpc::proto::golem::shardmanager::v1::ShardManagerError> for ShardManagerError {
    fn from(value: golem_api_grpc::proto::golem::shardmanager::v1::ShardManagerError) -> Self {
        use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_error::Error;
        match value.error {
            Some(Error::InvalidRequest(body)) => Self::InvalidRequest(body.error),
            Some(Error::Timeout(body)) => Self::Timeout(body.error),
            Some(Error::Unknown(body)) => Self::InternalServerError(body.error),
            None => Self::internal_client_error("Missing error field"),
        }
    }
}

impl From<tonic::transport::Error> for ShardManagerError {
    fn from(error: tonic::transport::Error) -> Self {
        Self::internal_client_error(format!("Transport error: {error}"))
    }
}

impl From<tonic::Status> for ShardManagerError {
    fn from(status: tonic::Status) -> Self {
        Self::internal_client_error(format!("Connection error: {status}"))
    }
}

impl From<String> for ShardManagerError {
    fn from(value: String) -> Self {
        Self::internal_client_error(value)
    }
}

impl From<&'static str> for ShardManagerError {
    fn from(value: &'static str) -> Self {
        Self::internal_client_error(value)
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum QuotaError {
    #[error("Lease not found: {0}")]
    LeaseNotFound(String),
    #[error("Stale epoch: {0}")]
    StaleEpoch(String),
    #[error("Conversion error: {0}")]
    ConversionError(String),
    #[error("Internal server error: {0}")]
    InternalServerError(String),
    #[error("Internal client error: {0}")]
    InternalClientError(String),
}

impl QuotaError {
    pub fn internal_client_error(error: impl AsRef<str>) -> Self {
        Self::InternalClientError(error.as_ref().to_string())
    }

    pub fn empty_response() -> Self {
        Self::internal_client_error("empty response")
    }
}

impl SafeDisplay for QuotaError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::LeaseNotFound(_) => self.to_string(),
            Self::StaleEpoch(_) => self.to_string(),
            Self::ConversionError(_) => self.to_string(),
            Self::InternalServerError(_) => "Internal error".to_string(),
            Self::InternalClientError(_) => "Internal error".to_string(),
        }
    }
}

impl IntoAnyhow for QuotaError {
    fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::from(self).context("QuotaError")
    }
}

impl From<golem_api_grpc::proto::golem::shardmanager::v1::QuotaError> for QuotaError {
    fn from(value: golem_api_grpc::proto::golem::shardmanager::v1::QuotaError) -> Self {
        use golem_api_grpc::proto::golem::shardmanager::v1::quota_error::Error;
        match value.error {
            Some(Error::LeaseNotFound(body)) => Self::LeaseNotFound(body.error),
            Some(Error::StaleEpoch(body)) => Self::StaleEpoch(body.error),
            Some(Error::Internal(body)) => Self::InternalServerError(body.error),
            None => Self::internal_client_error("Missing error field"),
        }
    }
}

impl From<tonic::transport::Error> for QuotaError {
    fn from(error: tonic::transport::Error) -> Self {
        Self::internal_client_error(format!("Transport error: {error}"))
    }
}

impl From<tonic::Status> for QuotaError {
    fn from(status: tonic::Status) -> Self {
        Self::internal_client_error(format!("Connection error: {status}"))
    }
}

impl From<String> for QuotaError {
    fn from(value: String) -> Self {
        Self::internal_client_error(value)
    }
}

impl From<&'static str> for QuotaError {
    fn from(value: &'static str) -> Self {
        Self::internal_client_error(value)
    }
}

/// The failure arms of `RenewShardLease` and `Deregister`. Mirrors `QuotaError`
/// arm for arm; the executor branches on the arm, never on the message string.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ShardLeaseError {
    #[error("Shard lease not found: {0}")]
    LeaseNotFound(String),
    #[error("Stale shard epoch: {0}")]
    StaleEpoch(String),
    #[error("Conversion error: {0}")]
    ConversionError(String),
    #[error("Internal server error: {0}")]
    InternalServerError(String),
    #[error("Internal client error: {0}")]
    InternalClientError(String),
}

impl ShardLeaseError {
    pub fn internal_client_error(error: impl AsRef<str>) -> Self {
        Self::InternalClientError(error.as_ref().to_string())
    }

    pub fn empty_response() -> Self {
        Self::internal_client_error("empty response")
    }
}

impl SafeDisplay for ShardLeaseError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::LeaseNotFound(_) => self.to_string(),
            Self::StaleEpoch(_) => self.to_string(),
            Self::ConversionError(_) => self.to_string(),
            Self::InternalServerError(_) => "Internal error".to_string(),
            Self::InternalClientError(_) => "Internal error".to_string(),
        }
    }
}

impl IntoAnyhow for ShardLeaseError {
    fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::from(self).context("ShardLeaseError")
    }
}

impl From<golem_api_grpc::proto::golem::shardmanager::v1::ShardLeaseError> for ShardLeaseError {
    fn from(value: golem_api_grpc::proto::golem::shardmanager::v1::ShardLeaseError) -> Self {
        use golem_api_grpc::proto::golem::shardmanager::v1::shard_lease_error::Error;
        match value.error {
            Some(Error::LeaseNotFound(body)) => Self::LeaseNotFound(body.error),
            Some(Error::StaleEpoch(body)) => Self::StaleEpoch(body.error),
            Some(Error::Internal(body)) => Self::InternalServerError(body.error),
            None => Self::internal_client_error("Missing error field"),
        }
    }
}

impl From<tonic::transport::Error> for ShardLeaseError {
    fn from(error: tonic::transport::Error) -> Self {
        Self::internal_client_error(format!("Transport error: {error}"))
    }
}

impl From<tonic::Status> for ShardLeaseError {
    fn from(status: tonic::Status) -> Self {
        Self::internal_client_error(format!("Connection error: {status}"))
    }
}

impl From<String> for ShardLeaseError {
    fn from(value: String) -> Self {
        Self::internal_client_error(value)
    }
}

impl From<&'static str> for ShardLeaseError {
    fn from(value: &'static str) -> Self {
        Self::internal_client_error(value)
    }
}

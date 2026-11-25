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

use super::{
    ExecutorEndpoint, GrpcShardManager, ShardLease, ShardLeaseClient, ShardLeaseError,
    ShardManagerError, ShardRegistration, issued, registration_from_wire, shard_lease_from_wire,
};
use crate::clients::shard_manager_router::Attempts;
use async_trait::async_trait;
use futures::future::BoxFuture;
use golem_api_grpc::proto::golem::shardmanager::v1::{
    DeregisterRequest, DeregisterResponse, RegisterRequest, RegisterResponse,
    RenewShardLeaseRequest, RenewShardLeaseResponse, deregister_response, register_response,
    renew_shard_lease_response,
};
use golem_common::model::protobuf::shard_epochs_to_proto;
use golem_common::model::{RetryConfig, ShardEpoch, ShardId};
use golem_common::retries::with_retries;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

/// Returns final routed outcomes. Replica selection, redirects and connection retries finish
/// before these operations return. Registration retries around decoding are separate.
#[async_trait]
trait RoutedShardLeases: Send + Sync {
    fn retries(&self) -> &RetryConfig;

    async fn register_routed(
        &self,
        request: RegisterRequest,
    ) -> Result<(Instant, RegisterResponse), ShardManagerError>;

    async fn renew_routed(
        &self,
        request: RenewShardLeaseRequest,
    ) -> Result<(Instant, RenewShardLeaseResponse), ShardManagerError>;

    async fn deregister_routed(
        &self,
        request: DeregisterRequest,
    ) -> Result<DeregisterResponse, ShardManagerError>;
}

async fn register(
    routed: &(impl RoutedShardLeases + ?Sized),
    endpoint: ExecutorEndpoint,
    pod_name: Option<String>,
    executor_id: Uuid,
) -> Result<ShardRegistration, ShardManagerError> {
    with_retries(
        "shard_manager",
        "register",
        Some(format!("{pod_name:?}")),
        routed.retries(),
        &(routed, endpoint, pod_name, executor_id),
        |(routed, endpoint, pod_name, executor_id)| {
            Box::pin(async move {
                let (sent_at, response) = routed
                    .register_routed(RegisterRequest {
                        endpoint: Some((*endpoint).into()),
                        pod_name: pod_name.clone(),
                        executor_id: executor_id.to_string(),
                    })
                    .await?;
                decode_registration(response, sent_at)
            })
        },
        |_| true,
    )
    .await
}

fn decode_registration(
    response: RegisterResponse,
    sent_at: Instant,
) -> Result<ShardRegistration, ShardManagerError> {
    match response.result {
        None => Err(ShardManagerError::empty_response()),
        Some(register_response::Result::Success(success)) => {
            registration_from_wire(success, sent_at).map_err(ShardManagerError::ConversionError)
        }
        Some(register_response::Result::Failure(failure)) => Err(failure.into()),
    }
}

async fn renew(
    routed: &(impl RoutedShardLeases + ?Sized),
    executor_id: Uuid,
    shard_epochs: BTreeMap<ShardId, ShardEpoch>,
) -> Result<ShardLease, ShardLeaseError> {
    let (sent_at, response) = routed
        .renew_routed(RenewShardLeaseRequest {
            executor_id: executor_id.to_string(),
            shard_epochs: shard_epochs_to_proto(shard_epochs),
        })
        .await?;
    decode_renewal(response, sent_at)
}

fn decode_renewal(
    response: RenewShardLeaseResponse,
    sent_at: Instant,
) -> Result<ShardLease, ShardLeaseError> {
    match response.result {
        None => Err(ShardLeaseError::empty_response()),
        Some(renew_shard_lease_response::Result::Success(lease)) => {
            shard_lease_from_wire(lease, sent_at).map_err(ShardLeaseError::ConversionError)
        }
        Some(renew_shard_lease_response::Result::Failure(failure)) => Err(failure.into()),
    }
}

async fn deregister(
    routed: &(impl RoutedShardLeases + ?Sized),
    executor_id: Uuid,
    shard_epochs: BTreeMap<ShardId, ShardEpoch>,
) -> Result<(), ShardLeaseError> {
    let response = routed
        .deregister_routed(DeregisterRequest {
            executor_id: executor_id.to_string(),
            shard_epochs: shard_epochs_to_proto(shard_epochs),
        })
        .await?;
    decode_deregistration(response)
}

fn decode_deregistration(response: DeregisterResponse) -> Result<(), ShardLeaseError> {
    match response.result {
        None => Err(ShardLeaseError::empty_response()),
        Some(deregister_response::Result::Success(_)) => Ok(()),
        Some(deregister_response::Result::Failure(failure)) => Err(failure.into()),
    }
}

#[async_trait]
impl<T: RoutedShardLeases + ?Sized> ShardLeaseClient for T {
    async fn register(
        &self,
        endpoint: ExecutorEndpoint,
        pod_name: Option<String>,
        executor_id: Uuid,
    ) -> Result<ShardRegistration, ShardManagerError> {
        register(self, endpoint, pod_name, executor_id).await
    }

    async fn renew_shard_lease(
        &self,
        executor_id: Uuid,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<ShardLease, ShardLeaseError> {
        renew(self, executor_id, shard_epochs).await
    }

    async fn deregister(
        &self,
        executor_id: Uuid,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<(), ShardLeaseError> {
        deregister(self, executor_id, shard_epochs).await
    }
}

#[async_trait]
impl RoutedShardLeases for GrpcShardManager {
    fn retries(&self) -> &RetryConfig {
        &self.retries
    }

    async fn register_routed(
        &self,
        request: RegisterRequest,
    ) -> Result<(Instant, RegisterResponse), ShardManagerError> {
        self.router
            .call("register", Attempts::Retried, move |client| {
                let request = request.clone();
                Box::pin(async move {
                    let (sent_at, response) = issued(client.register(request)).await;
                    response.map(|response| (sent_at, response.into_inner()))
                })
            })
            .await
            .map_err(Into::into)
    }

    async fn renew_routed(
        &self,
        request: RenewShardLeaseRequest,
    ) -> Result<(Instant, RenewShardLeaseResponse), ShardManagerError> {
        self.router
            .call("renew_shard_lease", Attempts::Once, move |client| {
                let request = request.clone();
                Box::pin(async move {
                    let (sent_at, response) = issued(client.renew_shard_lease(request)).await;
                    response.map(|response| (sent_at, response.into_inner()))
                })
            })
            .await
            .map_err(Into::into)
    }

    async fn deregister_routed(
        &self,
        request: DeregisterRequest,
    ) -> Result<DeregisterResponse, ShardManagerError> {
        self.router
            .call("deregister", Attempts::Once, move |client| {
                Box::pin(client.deregister(request.clone()))
            })
            .await
            .map(tonic::Response::into_inner)
            .map_err(Into::into)
    }
}

type Handler<Request, Response> =
    Arc<dyn Fn(Request) -> BoxFuture<'static, Result<Response, ShardManagerError>> + Send + Sync>;

/// An in-memory shard lease client with the same request encoding, registration retry schedule,
/// response validation and error mapping as `GrpcShardManager`.
///
/// Handlers receive owned wire requests and return final routed outcomes, not individual replica
/// responses. They can record generated executor IDs or await a gate. They do not model replica
/// selection, redirects, membership or quota policy.
#[derive(Clone)]
pub struct InMemoryShardLeaseClient {
    retries: RetryConfig,
    register_handler: Handler<RegisterRequest, RegisterResponse>,
    renew_handler: Handler<RenewShardLeaseRequest, RenewShardLeaseResponse>,
    deregister_handler: Handler<DeregisterRequest, DeregisterResponse>,
}

impl InMemoryShardLeaseClient {
    pub fn new<Register, RegisterFuture, Renew, RenewFuture, Deregister, DeregisterFuture>(
        retries: RetryConfig,
        register: Register,
        renew: Renew,
        deregister: Deregister,
    ) -> Self
    where
        Register: Fn(RegisterRequest) -> RegisterFuture + Send + Sync + 'static,
        RegisterFuture:
            Future<Output = Result<RegisterResponse, ShardManagerError>> + Send + 'static,
        Renew: Fn(RenewShardLeaseRequest) -> RenewFuture + Send + Sync + 'static,
        RenewFuture:
            Future<Output = Result<RenewShardLeaseResponse, ShardManagerError>> + Send + 'static,
        Deregister: Fn(DeregisterRequest) -> DeregisterFuture + Send + Sync + 'static,
        DeregisterFuture:
            Future<Output = Result<DeregisterResponse, ShardManagerError>> + Send + 'static,
    {
        Self {
            retries,
            register_handler: Arc::new(move |request| Box::pin(register(request))),
            renew_handler: Arc::new(move |request| Box::pin(renew(request))),
            deregister_handler: Arc::new(move |request| Box::pin(deregister(request))),
        }
    }
}

#[async_trait]
impl RoutedShardLeases for InMemoryShardLeaseClient {
    fn retries(&self) -> &RetryConfig {
        &self.retries
    }

    async fn register_routed(
        &self,
        request: RegisterRequest,
    ) -> Result<(Instant, RegisterResponse), ShardManagerError> {
        let sent_at = Instant::now();
        let response = (self.register_handler)(request).await?;
        Ok((sent_at, response))
    }

    async fn renew_routed(
        &self,
        request: RenewShardLeaseRequest,
    ) -> Result<(Instant, RenewShardLeaseResponse), ShardManagerError> {
        let sent_at = Instant::now();
        let response = (self.renew_handler)(request).await?;
        Ok((sent_at, response))
    }

    async fn deregister_routed(
        &self,
        request: DeregisterRequest,
    ) -> Result<DeregisterResponse, ShardManagerError> {
        (self.deregister_handler)(request).await
    }
}

#[cfg(test)]
mod tests;

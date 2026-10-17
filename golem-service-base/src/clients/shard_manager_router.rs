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

//! Sends shard manager calls to the leader. Every replica accepts the call, but only the leader
//! serves it; a standby answers with the leader's address, and the router connects there and keeps
//! that connection until a call to it fails.

use crate::clients::shard_manager::GrpcShardManagerConfig;
use crate::clients::shard_manager_leader::{LeaderAddress, is_not_leader, leader_of};
use crate::grpc::client::{GrpcClient, GrpcClientConfig, MultiTargetGrpcClient};
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_client::ShardManagerServiceClient;
use golem_common::model::RetryConfig;
use golem_common::retries::RetryState;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::{Code, Status};
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

/// How long the client waits to connect to a leader that a replica named. Shorter than the
/// configured `connect_timeout`: a named leader that does not answer quickly is usually gone, and
/// the configured address can name its successor.
const LEADER_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) type Client = ShardManagerServiceClient<OtelGrpcService<Channel>>;

/// One call on a shard manager client, in the form the gRPC clients take it. Written once here:
/// [`LeaderRouter::call`] spells the bound out, because a closure's higher-ranked signature is
/// inferred only from an `Fn` bound written on the parameter itself.
pub(crate) trait ClientCall<R>:
    for<'a> Fn(&'a mut Client) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
    + Send
    + Sync
{
}

impl<R, F> ClientCall<R> for F where
    F: for<'a> Fn(&'a mut Client) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
        + Send
        + Sync
{
}

fn new_client(channel: OtelGrpcService<Channel>, max_message_size: usize) -> Client {
    ShardManagerServiceClient::new(channel)
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
        .max_decoding_message_size(max_message_size)
        .max_encoding_message_size(max_message_size)
}

/// One attempt with a short connect: on any failure the client goes back to the configured
/// address, whose replicas know the current leader.
fn leader_client_config(config: &GrpcClientConfig) -> GrpcClientConfig {
    GrpcClientConfig {
        connect_timeout: config.connect_timeout.min(LEADER_CONNECT_TIMEOUT),
        retries_on_unavailable: RetryConfig {
            max_attempts: 0,
            ..config.retries_on_unavailable.clone()
        },
        ..config.clone()
    }
}

/// The outcome of one pass of [`LeaderRouter::call`].
enum Routed<R> {
    Done(Result<R, Status>),
    /// The leader that a replica named could not be reached; a later pass may reach its successor.
    LeaderUnreachable(Status),
}

/// A failure of the connection or of the peer's process, not an answer from the shard manager.
fn is_transport_failure(status: &Status) -> bool {
    matches!(
        status.code(),
        Code::Unavailable | Code::Unknown | Code::Cancelled | Code::DeadlineExceeded
    )
}

/// Whether a call may be sent again after a failure that the peer may have seen.
#[derive(Clone, Copy)]
pub(crate) enum Attempts {
    Retried,
    Once,
}

impl Attempts {
    fn replayable(self) -> bool {
        matches!(self, Attempts::Retried)
    }
}

#[derive(Clone)]
pub(crate) struct LeaderRouter {
    /// The configured address, which reaches any replica.
    configured: GrpcClient<Client>,
    /// Direct connections to the leader that a replica named.
    leaders: MultiTargetGrpcClient<Client>,
    /// The leader that the last answer named, until a call to it fails.
    leader: Arc<Mutex<Option<LeaderAddress>>>,
    tls: bool,
    /// The budget for waiting out a leader that the replicas name but nobody can reach.
    retries_on_unavailable: RetryConfig,
}

impl LeaderRouter {
    pub(crate) fn new(config: &GrpcShardManagerConfig) -> Self {
        Self {
            configured: GrpcClient::new(
                "shard_manager",
                new_client,
                config.uri(),
                config.client_config.clone(),
            )
            .with_redirect(is_not_leader),
            leaders: MultiTargetGrpcClient::new(
                "shard_manager_leader",
                new_client,
                leader_client_config(&config.client_config),
            )
            .with_redirect(is_not_leader),
            leader: Arc::new(Mutex::new(None)),
            tls: config.client_config.tls_enabled(),
            retries_on_unavailable: config.client_config.retries_on_unavailable.clone(),
        }
    }

    /// Sends one call to the leader: to the one already known, otherwise to the configured
    /// address, following a replica that names the leader.
    ///
    /// A replica that names the leader has not run the call, so following it is safe even for a
    /// call that must not be repeated. A failed call to a known leader is not repeated for such a
    /// call, because it may have run; the caller's own schedule tries again.
    ///
    /// After an ungraceful stop, the replicas name the dead leader until its etcd lease expires. A
    /// call that may be repeated waits that out with the configured `retries_on_unavailable`, as
    /// it waited out a closed port before the replicas named their leader.
    pub(crate) async fn call<F, R>(
        &self,
        description: &'static str,
        attempts: Attempts,
        f: F,
    ) -> Result<R, Status>
    where
        F: for<'a> Fn(
                &'a mut Client,
            ) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
            + Send
            + Sync,
    {
        let mut unreachable = None;
        let mut retries = RetryState::new(&self.retries_on_unavailable);
        loop {
            retries.start_attempt();
            match self
                .route_once(description, attempts, &f, &mut unreachable)
                .await
            {
                Routed::Done(result) => return result,
                Routed::LeaderUnreachable(status) => {
                    if attempts.replayable() && retries.failed_attempt().await {
                        continue;
                    }
                    return Err(status);
                }
            }
        }
    }

    /// One pass of [`Self::call`]. `unreachable` is the named leader that this call could not
    /// reach; while the replicas still name it, the pass ends without trying it again.
    async fn route_once<F, R>(
        &self,
        description: &'static str,
        attempts: Attempts,
        f: &F,
        unreachable: &mut Option<LeaderAddress>,
    ) -> Routed<R>
    where
        F: ClientCall<R>,
    {
        if let Some(leader) = self.known_leader() {
            match self.send_to_leader(description, &leader, attempts, f).await {
                Ok(result) => return Routed::Done(Ok(result)),
                Err(status) => {
                    if let Some(next) = leader_of(&status) {
                        self.forget_leader(&leader);
                        return self
                            .follow(description, next, attempts, f, unreachable)
                            .await;
                    }
                    // The shard manager answers most refusals inside a successful response, but
                    // some with a status such as `InvalidArgument`: those come from a working
                    // leader, which stays the one to call.
                    if !is_transport_failure(&status) {
                        return Routed::Done(Err(status));
                    }
                    self.forget_leader(&leader);
                    if !attempts.replayable() {
                        return Routed::Done(Err(status));
                    }
                    *unreachable = Some(leader);
                }
            }
        }

        let status = match self.send_to_configured(description, attempts, f).await {
            Ok(result) => return Routed::Done(Ok(result)),
            Err(status) => status,
        };
        match leader_of(&status) {
            // Named until that leader's lease expires; wait, then ask again.
            Some(next) if unreachable.as_ref() == Some(&next) => Routed::LeaderUnreachable(status),
            Some(next) => {
                self.follow(description, next, attempts, f, unreachable)
                    .await
            }
            // The configured address has already spent its own retries.
            None => Routed::Done(Err(status)),
        }
    }

    async fn follow<F, R>(
        &self,
        description: &'static str,
        leader: LeaderAddress,
        attempts: Attempts,
        f: &F,
        unreachable: &mut Option<LeaderAddress>,
    ) -> Routed<R>
    where
        F: ClientCall<R>,
    {
        // Two calls redirected by different replicas can race here; a stale leader that wins is
        // forgotten at its first failure.
        self.remember_leader(leader.clone());
        match self.send_to_leader(description, &leader, attempts, f).await {
            Ok(result) => Routed::Done(Ok(result)),
            Err(status) if leader_of(&status).is_some() || is_transport_failure(&status) => {
                self.forget_leader(&leader);
                if !attempts.replayable() {
                    return Routed::Done(Err(status));
                }
                *unreachable = Some(leader);
                Routed::LeaderUnreachable(status)
            }
            Err(status) => Routed::Done(Err(status)),
        }
    }

    async fn send_to_leader<F, R>(
        &self,
        description: &'static str,
        leader: &LeaderAddress,
        attempts: Attempts,
        f: &F,
    ) -> Result<R, Status>
    where
        F: ClientCall<R>,
    {
        self.leaders
            .call_with_retry(description, leader.uri(self.tls), attempts.replayable(), f)
            .await
    }

    async fn send_to_configured<F, R>(
        &self,
        description: &'static str,
        attempts: Attempts,
        f: &F,
    ) -> Result<R, Status>
    where
        F: ClientCall<R>,
    {
        self.configured
            .call_with_retry(description, attempts.replayable(), f)
            .await
    }

    fn known_leader(&self) -> Option<LeaderAddress> {
        self.leader
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn remember_leader(&self, leader: LeaderAddress) {
        *self.leader.lock().unwrap_or_else(PoisonError::into_inner) = Some(leader);
    }

    /// Only while it is still the one that failed: a concurrent call may have learned a newer one.
    fn forget_leader(&self, leader: &LeaderAddress) {
        let mut known = self.leader.lock().unwrap_or_else(PoisonError::into_inner);
        if known.as_ref() == Some(leader) {
            *known = None;
        }
    }
}

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

//! Decides, call by call, whether this replica serves a shard manager request or sends the caller
//! to the leader.
//!
//! Every replica opens its gRPC port at startup, before the election, so that an orchestrator sees
//! each replica as ready and can roll them like any other service. Only the leader serves: a
//! standby answers each call with `UNAVAILABLE` and, once the leader is ready, the leader's
//! address, which the shard manager client follows.

use crate::ShardManagerError;
use crate::sharding::leader_election::{ElectionValue, LeaderLocator};
use async_trait::async_trait;
use golem_service_base::clients::shard_manager_leader::{LeaderAddress, not_leader_status};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, PoisonError, RwLock};
use std::task::{Context, Poll};
use tonic::Status;
use tonic::body::Body;
use tonic::codegen::Service;
use tonic::server::NamedService;

/// Where a standby reads the current leader. A trait so that the gate is testable without etcd.
#[async_trait]
pub(crate) trait LeaderLookup: Send + Sync {
    async fn leader(&self) -> Result<Option<ElectionValue>, ShardManagerError>;
}

#[async_trait]
impl LeaderLookup for LeaderLocator {
    async fn leader(&self) -> Result<Option<ElectionValue>, ShardManagerError> {
        LeaderLocator::leader(self).await
    }
}

enum GateState<S> {
    /// Before this replica can say anything useful: local mode during its startup, or a
    /// distributed replica that has not yet connected to etcd.
    Starting,
    /// Distributed mode, from the etcd connection until this replica serves as the leader.
    Standby {
        leader: Arc<dyn LeaderLookup>,
        own: LeaderAddress,
    },
    Serving(S),
}

/// Shared by the gRPC server, which asks it about each call, and `run()`, which moves it on.
pub(crate) struct LeaderGate<S> {
    state: Arc<RwLock<GateState<S>>>,
}

impl<S> Clone for LeaderGate<S> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

enum Route<S> {
    Serve(S),
    Refuse(Status),
}

impl<S: Clone> LeaderGate<S> {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(GateState::Starting)),
        }
    }

    pub fn stand_by(&self, leader: Arc<dyn LeaderLookup>, own: LeaderAddress) {
        self.set(GateState::Standby { leader, own });
    }

    pub fn serve(&self, service: S) {
        self.set(GateState::Serving(service));
    }

    fn set(&self, state: GateState<S>) {
        *self.state.write().unwrap_or_else(PoisonError::into_inner) = state;
    }

    async fn route(&self) -> Route<S> {
        // Copied out, so that no lock is held across the etcd read below.
        let (leader, own) = match &*self.state.read().unwrap_or_else(PoisonError::into_inner) {
            GateState::Starting => {
                return Route::Refuse(Status::unavailable("the shard manager is starting; retry"));
            }
            GateState::Serving(service) => return Route::Serve(service.clone()),
            GateState::Standby { leader, own } => (leader.clone(), own.clone()),
        };
        Route::Refuse(standby_answer(leader.as_ref(), &own).await)
    }
}

/// Names the leader only when its value is ready and is not this replica's own: a replica that
/// has just won still finds itself here until its startup completes.
async fn standby_answer(leader: &dyn LeaderLookup, own: &LeaderAddress) -> Status {
    match leader.leader().await {
        Ok(Some(ElectionValue {
            ready: true,
            grpc_address: Some(address),
            ..
        })) if &address != own => not_leader_status(&address),
        Ok(_) => Status::unavailable("no shard manager leader is ready yet; retry"),
        Err(err) => Status::unavailable(format!(
            "cannot read the shard manager leader from etcd: {err}; retry"
        )),
    }
}

/// The tonic service in front of the shard manager service, under the same name.
pub(crate) struct GatedService<S> {
    gate: LeaderGate<S>,
}

impl<S> GatedService<S> {
    pub fn new(gate: LeaderGate<S>) -> Self {
        Self { gate }
    }
}

impl<S> Clone for GatedService<S> {
    fn clone(&self) -> Self {
        Self {
            gate: self.gate.clone(),
        }
    }
}

impl<S: NamedService> NamedService for GatedService<S> {
    const NAME: &'static str = S::NAME;
}

impl<S> Service<http::Request<Body>> for GatedService<S>
where
    S: Service<http::Request<Body>, Response = http::Response<Body>, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
{
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        let gate = self.gate.clone();
        Box::pin(async move {
            match gate.route().await {
                Route::Serve(mut service) => {
                    std::future::poll_fn(|cx| service.poll_ready(cx)).await?;
                    service.call(request).await
                }
                Route::Refuse(status) => Ok(status.into_http()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_service_base::clients::shard_manager_leader::leader_of;
    use test_r::test;

    /// Answers 200 with an `x-served` header: stands in for the shard manager service.
    #[derive(Clone)]
    struct Served;

    impl NamedService for Served {
        const NAME: &'static str = "test.Served";
    }

    impl Service<http::Request<Body>> for Served {
        type Response = http::Response<Body>;
        type Error = Infallible;
        type Future = std::future::Ready<Result<Self::Response, Infallible>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: http::Request<Body>) -> Self::Future {
            let mut response = http::Response::new(Body::default());
            response
                .headers_mut()
                .insert("x-served", http::HeaderValue::from_static("yes"));
            std::future::ready(Ok(response))
        }
    }

    struct FixedLeader(Result<Option<ElectionValue>, String>);

    #[async_trait]
    impl LeaderLookup for FixedLeader {
        async fn leader(&self) -> Result<Option<ElectionValue>, ShardManagerError> {
            self.0.clone().map_err(ShardManagerError::Internal)
        }
    }

    fn address(port: u16) -> LeaderAddress {
        LeaderAddress {
            host: "10.0.3.7".to_string(),
            port,
        }
    }

    fn value(port: u16, ready: bool) -> ElectionValue {
        ElectionValue {
            identity: "sm".to_string(),
            grpc_address: Some(address(port)),
            ready,
        }
    }

    async fn answer(gate: &LeaderGate<Served>) -> http::Response<Body> {
        GatedService::new(gate.clone())
            .call(http::Request::new(Body::default()))
            .await
            .unwrap()
    }

    fn status(response: &http::Response<Body>) -> Status {
        Status::from_header_map(response.headers()).expect("a refusal carries a gRPC status")
    }

    fn standby(leader: Result<Option<ElectionValue>, String>) -> LeaderGate<Served> {
        let gate = LeaderGate::new();
        gate.stand_by(Arc::new(FixedLeader(leader)), address(9001));
        gate
    }

    #[test]
    async fn a_starting_replica_answers_unavailable_without_an_address() {
        let response = answer(&LeaderGate::new()).await;
        assert_eq!(status(&response).code(), tonic::Code::Unavailable);
        assert_eq!(leader_of(&status(&response)), None);
    }

    #[test]
    async fn a_standby_sends_the_caller_to_a_ready_leader() {
        let response = answer(&standby(Ok(Some(value(9002, true))))).await;
        assert_eq!(leader_of(&status(&response)), Some(address(9002)));
    }

    #[test]
    async fn a_standby_does_not_name_a_leader_that_is_not_ready() {
        let response = answer(&standby(Ok(Some(value(9002, false))))).await;
        assert_eq!(status(&response).code(), tonic::Code::Unavailable);
        assert_eq!(leader_of(&status(&response)), None);
    }

    #[test]
    async fn a_standby_never_names_itself() {
        let response = answer(&standby(Ok(Some(value(9001, true))))).await;
        assert_eq!(leader_of(&status(&response)), None);
    }

    #[test]
    async fn a_standby_without_a_leader_names_none() {
        let response = answer(&standby(Ok(None))).await;
        assert_eq!(leader_of(&status(&response)), None);
    }

    #[test]
    async fn a_standby_that_cannot_read_etcd_names_no_leader() {
        let response = answer(&standby(Err("etcd is unreachable".to_string()))).await;
        let status = status(&response);
        assert_eq!(status.code(), tonic::Code::Unavailable);
        assert_eq!(leader_of(&status), None);
        assert!(
            status.message().contains("etcd is unreachable"),
            "{}",
            status.message()
        );
    }

    #[test]
    async fn a_serving_replica_passes_the_call_through() {
        let gate = standby(Ok(Some(value(9002, true))));
        gate.serve(Served);
        let response = answer(&gate).await;
        assert_eq!(response.headers().get("x-served").unwrap(), "yes");
    }
}

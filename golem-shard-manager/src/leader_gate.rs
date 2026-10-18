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

use crate::sharding::election_watch::{ElectionWatch, LeaderView};
use crate::sharding::leader_election::{ElectionValue, LeaseBelief};
use golem_service_base::clients::shard_manager_leader::{LeaderAddress, not_leader_status};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;
use tonic::Status;
use tonic::body::Body;
use tonic::codegen::Service;
use tonic::server::NamedService;

/// Where a standby reads the current leader. A trait so that the gate is testable without etcd.
pub(crate) trait LeaderLookup: Send + Sync {
    /// What this replica currently knows of the leader.
    fn leader(&self) -> LeaderView;
    /// Whether etcd answered this replica within `window`.
    fn answered_within(&self, window: Duration) -> bool;
}

impl LeaderLookup for ElectionWatch {
    fn leader(&self) -> LeaderView {
        self.leader_value()
    }

    fn answered_within(&self, window: Duration) -> bool {
        ElectionWatch::answered_within(self, window)
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
        /// A standby that etcd has not answered for this long is not ready: one lease TTL.
        ready_window: Duration,
    },
    Serving {
        service: S,
        /// The leader's lease belief; `None` in local mode, which has no lease.
        lease: Option<LeaseBelief>,
    },
}

/// Shared by the gRPC server, which asks it about each call, and `run()`, which moves it on.
pub(crate) struct LeaderGate<S> {
    state: Arc<RwLock<GateState<S>>>,
    /// Whether this replica still has to reach its own advertised address before it is ready.
    /// An atomic of its own, so that `/readyz` stays a cheap read.
    own_address_pending: Arc<AtomicBool>,
}

impl<S> Clone for LeaderGate<S> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            own_address_pending: self.own_address_pending.clone(),
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
            own_address_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    /// From now on the replica is not ready until [`Self::own_address_reached`] is called. Only
    /// distributed mode, whose replicas advertise an address to each other, asks for this.
    pub fn require_own_address_check(&self) {
        self.own_address_pending.store(true, Ordering::Release);
    }

    /// The replica's advertised address reaches the replica itself.
    pub fn own_address_reached(&self) {
        self.own_address_pending.store(false, Ordering::Release);
    }

    pub fn stand_by(
        &self,
        leader: Arc<dyn LeaderLookup>,
        own: LeaderAddress,
        ready_window: Duration,
    ) {
        self.set(GateState::Standby {
            leader,
            own,
            ready_window,
        });
    }

    pub fn serve(&self, service: S, lease: Option<LeaseBelief>) {
        self.set(GateState::Serving { service, lease });
    }

    fn set(&self, state: GateState<S>) {
        *self.state.write().unwrap_or_else(PoisonError::into_inner) = state;
    }

    /// What `/readyz` answers. A serving replica is ready while it believes it holds its lease; a
    /// standby while etcd answers it, so that a replica cut off from etcd is taken out of a
    /// rollout's count; a starting replica never. A distributed replica is also not ready while its
    /// advertised address does not reach it, because the clients that are sent there would find
    /// nothing.
    pub fn is_ready(&self) -> bool {
        if self.own_address_pending.load(Ordering::Acquire) {
            return false;
        }
        match &*self.state.read().unwrap_or_else(PoisonError::into_inner) {
            GateState::Starting => false,
            GateState::Standby {
                leader,
                ready_window,
                ..
            } => leader.answered_within(*ready_window),
            GateState::Serving { lease, .. } => lease.as_ref().is_none_or(LeaseBelief::held),
        }
    }

    fn route(&self) -> Route<S> {
        match &*self.state.read().unwrap_or_else(PoisonError::into_inner) {
            GateState::Starting => {
                Route::Refuse(Status::unavailable("the shard manager is starting; retry"))
            }
            GateState::Serving {
                lease: Some(lease), ..
            } if !lease.held() => Route::Refuse(Status::unavailable(
                "this shard manager's leadership lease may have lapsed; retry",
            )),
            GateState::Serving { service, .. } => Route::Serve(service.clone()),
            GateState::Standby { leader, own, .. } => {
                Route::Refuse(standby_answer(leader.leader(), own))
            }
        }
    }
}

/// Names the leader only when its value is ready and is not this replica's own: a replica that
/// has just won still finds itself here until its startup completes.
fn standby_answer(leader: LeaderView, own: &LeaderAddress) -> Status {
    match leader {
        LeaderView::Unread => Status::unavailable(
            "this replica has not read the shard manager leader from etcd yet; retry",
        ),
        LeaderView::Leader(ElectionValue {
            ready: true,
            grpc_address: address,
            ..
        }) if &address != own => not_leader_status(&address),
        LeaderView::Leader(_) | LeaderView::NoLeader => {
            Status::unavailable("no shard manager leader is ready yet; retry")
        }
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
            match gate.route() {
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
    use tokio::time::Instant;

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

    struct FixedLeader {
        leader: LeaderView,
        answered: bool,
    }

    impl LeaderLookup for FixedLeader {
        fn leader(&self) -> LeaderView {
            self.leader.clone()
        }

        fn answered_within(&self, _: Duration) -> bool {
            self.answered
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
            grpc_address: address(port),
            ready,
            version: String::new(),
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

    fn standby(leader: LeaderView) -> LeaderGate<Served> {
        let gate = LeaderGate::new();
        gate.stand_by(
            Arc::new(FixedLeader {
                leader,
                answered: true,
            }),
            address(9001),
            Duration::from_secs(10),
        );
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
        let response = answer(&standby(LeaderView::Leader(value(9002, true)))).await;
        assert_eq!(leader_of(&status(&response)), Some(address(9002)));
    }

    #[test]
    async fn a_standby_does_not_name_a_leader_that_is_not_ready() {
        let response = answer(&standby(LeaderView::Leader(value(9002, false)))).await;
        assert_eq!(status(&response).code(), tonic::Code::Unavailable);
        assert_eq!(leader_of(&status(&response)), None);
    }

    #[test]
    async fn a_standby_never_names_itself() {
        let response = answer(&standby(LeaderView::Leader(value(9001, true)))).await;
        assert_eq!(leader_of(&status(&response)), None);
    }

    #[test]
    async fn a_standby_without_a_leader_names_none() {
        let response = answer(&standby(LeaderView::NoLeader)).await;
        assert_eq!(leader_of(&status(&response)), None);
    }

    #[test]
    async fn a_standby_that_has_not_read_etcd_yet_names_no_leader() {
        let response = answer(&standby(LeaderView::Unread)).await;
        let status = status(&response);
        assert_eq!(status.code(), tonic::Code::Unavailable);
        assert_eq!(leader_of(&status), None);
        assert!(
            status.message().contains("not read"),
            "{}",
            status.message()
        );
    }

    #[test]
    async fn a_leader_whose_lease_belief_has_expired_serves_nothing() {
        let gate = standby(LeaderView::NoLeader);
        gate.serve(
            Served,
            Some(LeaseBelief::until(
                Instant::now() - Duration::from_millis(1),
            )),
        );
        let response = answer(&gate).await;
        assert_eq!(status(&response).code(), tonic::Code::Unavailable);
        assert!(response.headers().get("x-served").is_none());
    }

    #[test]
    async fn a_leader_within_its_lease_belief_serves() {
        let gate = standby(LeaderView::NoLeader);
        gate.serve(
            Served,
            Some(LeaseBelief::until(Instant::now() + Duration::from_secs(10))),
        );
        assert_eq!(
            answer(&gate).await.headers().get("x-served").unwrap(),
            "yes"
        );
    }

    #[test]
    fn a_replica_that_cannot_reach_its_own_address_is_not_ready() {
        let gate = standby(LeaderView::NoLeader);
        gate.require_own_address_check();
        assert!(
            !gate.is_ready(),
            "a standby whose advertised address has not been reached is not ready"
        );

        gate.own_address_reached();
        assert!(gate.is_ready(), "reaching the address makes it ready");

        let leader: LeaderGate<Served> = LeaderGate::new();
        leader.require_own_address_check();
        leader.serve(Served, None);
        assert!(
            !leader.is_ready(),
            "a serving replica is not ready before its address has been reached either"
        );
        leader.own_address_reached();
        assert!(leader.is_ready());
    }

    #[test]
    fn readiness_follows_the_gate() {
        let gate: LeaderGate<Served> = LeaderGate::new();
        assert!(!gate.is_ready(), "a starting replica is not ready");

        let quiet = LeaderGate::<Served>::new();
        quiet.stand_by(
            Arc::new(FixedLeader {
                leader: LeaderView::NoLeader,
                answered: false,
            }),
            address(9001),
            Duration::from_secs(10),
        );
        assert!(
            !quiet.is_ready(),
            "a standby that etcd did not answer is not ready"
        );
        assert!(
            standby(LeaderView::NoLeader).is_ready(),
            "a standby that etcd answered is ready"
        );

        gate.serve(Served, None);
        assert!(
            gate.is_ready(),
            "a local-mode replica is ready once it serves"
        );

        let lapsed: LeaderGate<Served> = LeaderGate::new();
        lapsed.serve(
            Served,
            Some(LeaseBelief::until(
                Instant::now() - Duration::from_millis(1),
            )),
        );
        assert!(
            !lapsed.is_ready(),
            "a serving replica whose lease has lapsed is not ready"
        );
    }

    #[test]
    async fn a_serving_replica_passes_the_call_through() {
        let gate = standby(LeaderView::Leader(value(9002, true)));
        gate.serve(Served, None);
        let response = answer(&gate).await;
        assert_eq!(response.headers().get("x-served").unwrap(), "yes");
    }
}

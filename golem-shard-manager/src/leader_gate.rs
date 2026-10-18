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
//!
//! The gate's own refusals (starting, stopping, a lapsed belief before the call, no ready leader)
//! carry a mark that the call did not run, so that a client may send even a call it must not
//! repeat to another replica. A redirect names a leader instead, and the refusal for a call that
//! lapsed while it ran carries no mark, because that call may have taken effect.

use crate::sharding::election_watch::{ElectionWatch, LeaderView};
use crate::sharding::leader_election::{ElectionValue, LeaseBelief};
use golem_service_base::clients::shard_manager_leader::{
    LeaderAddress, not_leader_status, refusal_status,
};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
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
    /// The replica is on its way out, whatever it was: every call is refused and it is not ready.
    Stopped,
    Serving {
        service: S,
        /// The leader's lease belief; `None` in local mode, which has no lease.
        lease: Option<LeaseBelief>,
    },
}

/// Shared by the gRPC server, which asks it about each call, and `run()`, which moves it on.
pub(crate) struct LeaderGate<S> {
    state: Arc<RwLock<GateState<S>>>,
    /// Whether this replica still has to reach its own advertised address before it is ready or
    /// may campaign. A channel of its own, so that `/readyz` stays a cheap read and the campaign
    /// can wait for the change.
    own_address_pending: Arc<tokio::sync::watch::Sender<bool>>,
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
    /// The service, and the lease belief that must still hold when the call has ended.
    Serve(S, Option<LeaseBelief>),
    Refuse(Status),
}

impl<S: Clone> LeaderGate<S> {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(GateState::Starting)),
            own_address_pending: Arc::new(tokio::sync::watch::channel(false).0),
        }
    }

    /// From now on the replica is not ready until [`Self::own_address_reached`] is called. Only
    /// distributed mode, whose replicas advertise an address to each other, asks for this.
    pub fn require_own_address_check(&self) {
        self.own_address_pending.send_replace(true);
    }

    /// The replica's advertised address reaches the replica itself.
    pub fn own_address_reached(&self) {
        self.own_address_pending.send_replace(false);
    }

    /// Completes once the replica's advertised address has reached the replica itself, at once if
    /// no check was asked for.
    pub async fn wait_for_own_address(&self) {
        let mut pending = self.own_address_pending.subscribe();
        // The sender lives as long as the gate does, which this borrows.
        let _ = pending.wait_for(|pending| !*pending).await;
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

    /// From now on every call gets `UNAVAILABLE` and the replica is not ready. Called at the start
    /// of every way a replica stops, before it gives up the leadership: a connection that was
    /// accepted earlier would otherwise be served by a leader that another replica is replacing.
    pub fn stop(&self) {
        self.set(GateState::Stopped);
    }

    /// Whether [`Self::stop`] has been called.
    pub fn is_stopped(&self) -> bool {
        matches!(
            &*self.state.read().unwrap_or_else(PoisonError::into_inner),
            GateState::Stopped
        )
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
        if *self.own_address_pending.borrow() {
            return false;
        }
        match &*self.state.read().unwrap_or_else(PoisonError::into_inner) {
            GateState::Starting | GateState::Stopped => false,
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
                Route::Refuse(refusal_status("the shard manager is starting; retry"))
            }
            GateState::Stopped => {
                Route::Refuse(refusal_status("the shard manager is stopping; retry"))
            }
            GateState::Serving {
                lease: Some(lease), ..
            } if !lease.held() => Route::Refuse(refusal_status(
                "this shard manager's leadership lease may have lapsed; retry",
            )),
            GateState::Serving { service, lease } => Route::Serve(service.clone(), lease.clone()),
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
        LeaderView::Unread => refusal_status(
            "this replica has not read the shard manager leader from etcd yet; retry",
        ),
        LeaderView::Leader(ElectionValue {
            ready: true,
            grpc_address: address,
            ..
        }) if &address != own => not_leader_status(&address),
        LeaderView::Leader(_) | LeaderView::NoLeader => {
            refusal_status("no shard manager leader is ready yet; retry")
        }
    }
}

/// The answer for a call whose lease belief lapsed while it ran. Not marked as refused before
/// running: the call may have taken effect, and a client must not repeat it blindly.
fn lapsed_while_running() -> Status {
    Status::unavailable("this shard manager's leadership lease lapsed while the call ran; retry")
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
                Route::Serve(mut service, lease) => {
                    std::future::poll_fn(|cx| service.poll_ready(cx)).await?;
                    let response = service.call(request).await?;
                    // The belief can lapse while the call runs, and a new leader may already have
                    // begun. The answer of a call that ended after that is not handed out as a
                    // leader's: the caller is told what it is told about a lapsed lease. The call
                    // may have run, so unlike a refusal before running this is a plain refusal.
                    if lease.is_some_and(|lease| !lease.held()) {
                        return Ok(lapsed_while_running().into_http());
                    }
                    Ok(response)
                }
                Route::Refuse(status) => Ok(status.into_http()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_service_base::clients::shard_manager_leader::{leader_of, refused_before_running};
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

    /// Answers after a pause, so that the lease belief can lapse while the call runs.
    #[derive(Clone)]
    struct SlowServed(Duration);

    impl NamedService for SlowServed {
        const NAME: &'static str = "test.SlowServed";
    }

    impl Service<http::Request<Body>> for SlowServed {
        type Response = http::Response<Body>;
        type Error = Infallible;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: http::Request<Body>) -> Self::Future {
            let pause = self.0;
            Box::pin(async move {
                tokio::time::sleep(pause).await;
                let mut response = http::Response::new(Body::default());
                response
                    .headers_mut()
                    .insert("x-served", http::HeaderValue::from_static("yes"));
                Ok(response)
            })
        }
    }

    #[test]
    async fn a_call_that_outlasts_the_lease_belief_is_not_answered_as_served() {
        let gate: LeaderGate<SlowServed> = LeaderGate::new();
        gate.serve(
            SlowServed(Duration::from_millis(300)),
            Some(LeaseBelief::until(
                Instant::now() + Duration::from_millis(100),
            )),
        );
        let response = GatedService::new(gate.clone())
            .call(http::Request::new(Body::default()))
            .await
            .unwrap();
        assert!(
            response.headers().get("x-served").is_none(),
            "the answer of a call that ended after the belief lapsed must not be handed out"
        );
        let refusal = status(&response);
        assert_eq!(refusal.code(), tonic::Code::Unavailable);
        assert!(
            !refused_before_running(&refusal),
            "the call ran, so the refusal must not say that it did not"
        );
        assert_eq!(leader_of(&refusal), None);
    }

    #[test]
    async fn a_call_that_ends_within_the_lease_belief_is_answered() {
        let gate: LeaderGate<SlowServed> = LeaderGate::new();
        gate.serve(
            SlowServed(Duration::from_millis(50)),
            Some(LeaseBelief::until(Instant::now() + Duration::from_secs(10))),
        );
        let response = GatedService::new(gate)
            .call(http::Request::new(Body::default()))
            .await
            .unwrap();
        assert_eq!(response.headers().get("x-served").unwrap(), "yes");
    }

    #[test]
    async fn the_refusals_of_a_gate_that_did_not_run_the_call_say_so() {
        let starting = answer(&LeaderGate::new()).await;
        assert!(refused_before_running(&status(&starting)), "starting");

        let stopping = standby(LeaderView::NoLeader);
        stopping.stop();
        assert!(
            refused_before_running(&status(&answer(&stopping).await)),
            "stopping"
        );

        let lapsed = standby(LeaderView::NoLeader);
        lapsed.serve(
            Served,
            Some(LeaseBelief::until(
                Instant::now() - Duration::from_millis(1),
            )),
        );
        let lapsed = status(&answer(&lapsed).await);
        assert!(refused_before_running(&lapsed), "lapsed before running");
        assert_eq!(leader_of(&lapsed), None);

        for (name, leader) in [
            ("unread", LeaderView::Unread),
            ("no leader", LeaderView::NoLeader),
            ("leader not ready", LeaderView::Leader(value(9002, false))),
            ("leader is itself", LeaderView::Leader(value(9001, true))),
        ] {
            let refusal = status(&answer(&standby(leader)).await);
            assert!(refused_before_running(&refusal), "{name}");
            assert_eq!(leader_of(&refusal), None, "{name}");
        }
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
    async fn waiting_for_the_own_address_ends_when_it_is_reached() {
        let unchecked: LeaderGate<Served> = LeaderGate::new();
        tokio::time::timeout(Duration::from_secs(1), unchecked.wait_for_own_address())
            .await
            .expect("no check was asked for, so there is nothing to wait for");

        let gate: LeaderGate<Served> = LeaderGate::new();
        gate.require_own_address_check();
        let waiting = tokio::spawn({
            let gate = gate.clone();
            async move { gate.wait_for_own_address().await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiting.is_finished(), "the address has not been reached");
        gate.own_address_reached();
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("reaching the address ends the wait")
            .unwrap();
    }

    #[test]
    async fn a_stopping_replica_refuses_every_call_without_naming_a_leader() {
        let leading = standby(LeaderView::NoLeader);
        leading.serve(
            Served,
            Some(LeaseBelief::until(Instant::now() + Duration::from_secs(10))),
        );
        assert!(leading.is_ready());
        leading.stop();
        let response = answer(&leading).await;
        assert!(response.headers().get("x-served").is_none());
        assert_eq!(status(&response).code(), tonic::Code::Unavailable);
        assert_eq!(leader_of(&status(&response)), None);
        assert!(
            !leading.is_ready(),
            "a leader whose lease belief still holds is not ready once it stops"
        );

        let named = standby(LeaderView::Leader(value(9002, true)));
        named.stop();
        assert_eq!(leader_of(&status(&answer(&named).await)), None);
        assert!(!named.is_ready());
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

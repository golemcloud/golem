//! An agent that drives invocation handles from a guest: it triggers and schedules invocations
//! on other agents, keeps their IDs (in its state and in its snapshot) or one handle, and
//! observes and cancels them through held or rebuilt handles. Every handle operation answers a
//! `HandleAnswer`, so a failed operation reaches the test as a typed value instead of a trap.

use crate::{
    CancelTesterClient, RpcBlockingCounterClient, RpcCallOutcome, ScheduledEphemeralTargetClient,
    ScheduledInvocationServerClient, encode_single_parameter, read_effect,
};
use golem_rust::bindings::golem::agent::host::{Datetime, WasmRpc};
use golem_rust::{
    FromSchema, FromWire, GolemReflectError, IntoSchema, IntoWire, Invocation, OwnedInvocationId,
    PromiseId, WireSchema, agent_definition, agent_implementation, read_only,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::task::Poll;

/// The answer of one handle operation.
#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub enum HandleAnswer {
    /// `status` answered this lifecycle state
    Status { status: String },
    /// `await` answered this `u64` result
    Value { value: u64 },
    /// `await` answered a unit result
    Unit,
    /// `cancel` answered whether it withdrew the invocation
    Cancel { withdrawn: bool },
    /// The operation failed with this error
    Error { error: RpcCallOutcome },
}

#[agent_definition(snapshotting = "enabled")]
pub trait HandleProbe {
    fn new(name: String) -> Self;

    /// Triggers `inc_after_promise(promise_id)` on the blocking counter, keeps its handle for
    /// the `held_*` methods and returns its ID.
    async fn trigger_blocked(&mut self, counter_name: String, promise_id: PromiseId) -> String;
    /// Triggers `inc_by(value)` on the blocking counter and returns its ID.
    async fn trigger_inc_by(&self, counter_name: String, value: u64) -> String;
    /// Schedules `inc_after_promise(promise_id)` on the blocking counter and returns its ID.
    async fn schedule_blocked(
        &self,
        counter_name: String,
        promise_id: PromiseId,
        seconds: u64,
        nanoseconds: u32,
    ) -> String;
    /// Schedules `inc_global_by(value)` on the server and returns its ID.
    async fn schedule_global_inc(
        &self,
        server_name: String,
        value: u64,
        seconds: u64,
        nanoseconds: u32,
    ) -> String;
    /// Schedules `inc(hold_ms)` on a never-started ephemeral target and returns its ID.
    async fn schedule_ephemeral_inc(
        &self,
        target_name: String,
        hold_ms: u64,
        seconds: u64,
        nanoseconds: u32,
    ) -> String;
    /// The runs of the ephemeral target's constructor and of its `inc`.
    fn ephemeral_effects(&self, target_name: String) -> (u64, u64);
    /// Triggers a `CancelTester.inc_after_stdout` with an unknown mode, which fails, and
    /// returns its ID.
    async fn trigger_failing(&self, tester_name: String, counter_name: String) -> String;
    /// Triggers the stream-returning `StreamingRpcTarget.produce` with the raw host client.
    async fn trigger_stream_output(&self, target_name: String) -> HandleAnswer;
    /// Schedules the stream-returning `StreamingRpcTarget.produce` with the raw host client.
    async fn schedule_stream_output(&self, target_name: String) -> HandleAnswer;

    async fn held_status(&self) -> HandleAnswer;
    async fn held_await(&self) -> HandleAnswer;
    async fn held_cancel(&self) -> HandleAnswer;

    async fn status(&self, id: String) -> HandleAnswer;
    async fn await_value(&self, id: String) -> HandleAnswer;
    async fn await_unit(&self, id: String) -> HandleAnswer;
    async fn cancel(&self, id: String) -> HandleAnswer;
    #[read_only]
    async fn read_only_status(&self, id: String) -> HandleAnswer;
    #[read_only]
    async fn read_only_cancel(&self, id: String) -> HandleAnswer;

    /// Keeps an ID in the agent state, which the snapshot also carries.
    fn keep(&mut self, id: String);
    fn kept(&self) -> Option<String>;
    /// Whether the agent was restored from a snapshot.
    fn restored(&self) -> bool;

    /// Awaits the `u64` result of the invocation and keeps the answer for `last`.
    async fn await_and_keep(&mut self, id: String) -> HandleAnswer;
    /// Reads the status of the invocation and keeps the answer for `last`.
    async fn status_and_keep(&mut self, id: String) -> HandleAnswer;
    fn last(&self) -> Option<HandleAnswer>;

    /// Awaits two `u64` results at once and keeps both answers, in call order, for
    /// `last_pair`, whatever order they complete in.
    async fn await_two(&mut self, id_a: String, id_b: String) -> (HandleAnswer, HandleAnswer);
    fn last_pair(&self) -> Option<(HandleAnswer, HandleAnswer)>;

    /// Rebuilds two handles of one unit-returning invocation, polls an await of each once,
    /// drops the first await and its handle, then awaits through the second handle.
    async fn two_observers(&self, id: String) -> HandleAnswer;
}

#[derive(Serialize, Deserialize)]
struct HandleProbeSnapshot {
    name: String,
    kept: Option<String>,
}

struct HandleProbeImpl {
    name: String,
    held: Option<Invocation<u64>>,
    kept: Option<String>,
    restored: bool,
    last: Option<HandleAnswer>,
    last_pair: Option<(HandleAnswer, HandleAnswer)>,
}

fn id_of(id: &str) -> OwnedInvocationId {
    id.parse().expect("invalid invocation ID")
}

fn at(seconds: u64, nanoseconds: u32) -> Datetime {
    Datetime {
        seconds: seconds as i64,
        nanoseconds,
    }
}

fn failure(error: GolemReflectError) -> HandleAnswer {
    let error = match error {
        GolemReflectError::Denied(details) => RpcCallOutcome::Denied { details },
        GolemReflectError::RemoteNotFound(details) => RpcCallOutcome::NotFound { details },
        GolemReflectError::ProtocolError(details) => RpcCallOutcome::ProtocolError { details },
        GolemReflectError::RemoteInternalError(details) => {
            RpcCallOutcome::RemoteInternalError { details }
        }
        GolemReflectError::Conflict(details) => RpcCallOutcome::Conflict { details },
        GolemReflectError::Cancelled(details) => RpcCallOutcome::Cancelled { details },
        GolemReflectError::CannotCancelRunning(details) => {
            RpcCallOutcome::CannotCancelRunning { details }
        }
        other => RpcCallOutcome::RemoteInternalError {
            details: format!("{other:?}"),
        },
    };
    HandleAnswer::Error { error }
}

async fn status_of<T>(invocation: &Invocation<T>) -> HandleAnswer {
    match invocation.status().await {
        Ok(status) => HandleAnswer::Status {
            status: format!("{status:?}"),
        },
        Err(error) => failure(error),
    }
}

async fn value_of(invocation: &Invocation<u64>) -> HandleAnswer {
    match invocation.await_result().await {
        Ok(value) => HandleAnswer::Value { value },
        Err(error) => failure(error),
    }
}

async fn unit_of(invocation: &Invocation<()>) -> HandleAnswer {
    match invocation.await_result().await {
        Ok(()) => HandleAnswer::Unit,
        Err(error) => failure(error),
    }
}

async fn cancel_of<T>(invocation: &Invocation<T>) -> HandleAnswer {
    match invocation.cancel().await {
        Ok(withdrawn) => HandleAnswer::Cancel { withdrawn },
        Err(error) => failure(error),
    }
}

fn rpc_answer<T>(
    result: Result<T, golem_rust::bindings::golem::agent::host::RpcError>,
) -> HandleAnswer {
    match result {
        Ok(_) => HandleAnswer::Unit,
        Err(error) => HandleAnswer::Error {
            error: error.into(),
        },
    }
}

impl HandleProbeImpl {
    fn held(&self) -> &Invocation<u64> {
        self.held.as_ref().expect("no held handle")
    }
}

#[agent_implementation]
impl HandleProbe for HandleProbeImpl {
    fn new(name: String) -> Self {
        Self {
            name,
            held: None,
            kept: None,
            restored: false,
            last: None,
            last_pair: None,
        }
    }

    async fn trigger_blocked(&mut self, counter_name: String, promise_id: PromiseId) -> String {
        let mut counter = RpcBlockingCounterClient::upsert(counter_name);
        let invocation = counter
            .trigger_inc_after_promise(promise_id)
            .await
            .expect("failed to trigger the blocked invocation");
        let id = invocation.id().to_string();
        self.held = Some(invocation);
        id
    }

    async fn trigger_inc_by(&self, counter_name: String, value: u64) -> String {
        let mut counter = RpcBlockingCounterClient::upsert(counter_name);
        counter
            .trigger_inc_by(value)
            .await
            .expect("failed to trigger the increment")
            .id()
            .to_string()
    }

    async fn schedule_blocked(
        &self,
        counter_name: String,
        promise_id: PromiseId,
        seconds: u64,
        nanoseconds: u32,
    ) -> String {
        let mut counter = RpcBlockingCounterClient::upsert(counter_name);
        counter
            .schedule_inc_after_promise(promise_id, at(seconds, nanoseconds))
            .await
            .expect("failed to schedule the blocked invocation")
            .id()
            .to_string()
    }

    async fn schedule_global_inc(
        &self,
        server_name: String,
        value: u64,
        seconds: u64,
        nanoseconds: u32,
    ) -> String {
        let mut server = ScheduledInvocationServerClient::upsert(server_name);
        server
            .schedule_inc_global_by(value, at(seconds, nanoseconds))
            .await
            .expect("failed to schedule the increment")
            .id()
            .to_string()
    }

    async fn schedule_ephemeral_inc(
        &self,
        target_name: String,
        hold_ms: u64,
        seconds: u64,
        nanoseconds: u32,
    ) -> String {
        let target = ScheduledEphemeralTargetClient::upsert(target_name);
        target
            .schedule_inc(hold_ms, at(seconds, nanoseconds))
            .await
            .expect("failed to schedule the ephemeral invocation")
            .id()
            .to_string()
    }

    fn ephemeral_effects(&self, target_name: String) -> (u64, u64) {
        (
            read_effect(&format!("constructed:{target_name}")),
            read_effect(&format!("inc:{target_name}")),
        )
    }

    async fn trigger_failing(&self, tester_name: String, counter_name: String) -> String {
        let tester = CancelTesterClient::upsert(tester_name);
        tester
            .trigger_inc_after_stdout(counter_name, "unknown-mode".to_string())
            .await
            .expect("failed to trigger the failing invocation")
            .id()
            .to_string()
    }

    async fn trigger_stream_output(&self, target_name: String) -> HandleAnswer {
        let rpc = WasmRpc::upsert("StreamingRpcTarget", encode_single_parameter(target_name))
            .expect("failed to create the RPC client");
        rpc_answer(
            rpc.trigger(
                "produce".to_string(),
                encode_single_parameter(vec![1u32, 2]),
            )
            .await,
        )
    }

    async fn schedule_stream_output(&self, target_name: String) -> HandleAnswer {
        let rpc = WasmRpc::upsert("StreamingRpcTarget", encode_single_parameter(target_name))
            .expect("failed to create the RPC client");
        rpc_answer(
            rpc.schedule(
                at(0, 0),
                "produce".to_string(),
                encode_single_parameter(vec![1u32, 2]),
            )
            .await,
        )
    }

    async fn held_status(&self) -> HandleAnswer {
        status_of(self.held()).await
    }

    async fn held_await(&self) -> HandleAnswer {
        value_of(self.held()).await
    }

    async fn held_cancel(&self) -> HandleAnswer {
        cancel_of(self.held()).await
    }

    async fn status(&self, id: String) -> HandleAnswer {
        status_of(&Invocation::<()>::get(&id_of(&id))).await
    }

    async fn await_value(&self, id: String) -> HandleAnswer {
        value_of(&Invocation::<u64>::get(&id_of(&id))).await
    }

    async fn await_unit(&self, id: String) -> HandleAnswer {
        unit_of(&Invocation::<()>::get(&id_of(&id))).await
    }

    async fn cancel(&self, id: String) -> HandleAnswer {
        cancel_of(&Invocation::<()>::get(&id_of(&id))).await
    }

    async fn read_only_status(&self, id: String) -> HandleAnswer {
        status_of(&Invocation::<()>::get(&id_of(&id))).await
    }

    async fn read_only_cancel(&self, id: String) -> HandleAnswer {
        cancel_of(&Invocation::<()>::get(&id_of(&id))).await
    }

    fn keep(&mut self, id: String) {
        self.kept = Some(id);
    }

    fn kept(&self) -> Option<String> {
        self.kept.clone()
    }

    fn restored(&self) -> bool {
        self.restored
    }

    async fn await_and_keep(&mut self, id: String) -> HandleAnswer {
        let answer = value_of(&Invocation::<u64>::get(&id_of(&id))).await;
        self.last = Some(answer.clone());
        answer
    }

    async fn status_and_keep(&mut self, id: String) -> HandleAnswer {
        let answer = status_of(&Invocation::<u64>::get(&id_of(&id))).await;
        self.last = Some(answer.clone());
        answer
    }

    fn last(&self) -> Option<HandleAnswer> {
        self.last.clone()
    }

    async fn await_two(&mut self, id_a: String, id_b: String) -> (HandleAnswer, HandleAnswer) {
        let a = Invocation::<u64>::get(&id_of(&id_a));
        let b = Invocation::<u64>::get(&id_of(&id_b));
        let mut await_a = Box::pin(value_of(&a));
        let mut await_b = Box::pin(value_of(&b));
        let mut answer_a = None;
        let mut answer_b = None;
        let pair = std::future::poll_fn(|cx| {
            if answer_a.is_none()
                && let Poll::Ready(answer) = await_a.as_mut().poll(cx)
            {
                answer_a = Some(answer);
            }
            if answer_b.is_none()
                && let Poll::Ready(answer) = await_b.as_mut().poll(cx)
            {
                answer_b = Some(answer);
            }
            match (&answer_a, &answer_b) {
                (Some(a), Some(b)) => Poll::Ready((a.clone(), b.clone())),
                _ => Poll::Pending,
            }
        })
        .await;
        self.last_pair = Some(pair.clone());
        pair
    }

    fn last_pair(&self) -> Option<(HandleAnswer, HandleAnswer)> {
        self.last_pair.clone()
    }

    async fn two_observers(&self, id: String) -> HandleAnswer {
        let id = id_of(&id);
        let first = Invocation::<()>::get(&id);
        let second = Invocation::<()>::get(&id);
        let mut first_await = Box::pin(unit_of(&first));
        let mut second_await = Box::pin(unit_of(&second));
        let first_answer = std::future::poll_fn(|cx| {
            let first_answer = first_await.as_mut().poll(cx);
            let _ = second_await.as_mut().poll(cx);
            Poll::Ready(first_answer)
        })
        .await;
        assert!(
            first_answer.is_pending(),
            "the observed invocation must still be pending"
        );
        drop(first_await);
        drop(first);
        second_await.await
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&HandleProbeSnapshot {
            name: self.name.clone(),
            kept: self.kept.clone(),
        })
        .map_err(|error| error.to_string())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let snapshot: HandleProbeSnapshot =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        Ok(Self {
            name: snapshot.name,
            held: None,
            kept: snapshot.kept,
            restored: true,
            last: None,
            last_pair: None,
        })
    }
}

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

//! The continuous mixed workload a chaos scenario runs a fault against
//! (GOL-363).
//!
//! Four streams run concurrently at a shared, deliberately modest rate:
//!
//! | Stream | Operation | Read-back |
//! | -- | -- | -- |
//! | durable | `Counter.increment` | `Counter.count` |
//! | ephemeral | `EphemeralCounter.increment` | none — no state survives |
//! | scheduled | `ScheduleEmitter.schedule_poll_at` → `ScheduleCounter.poll` | `ScheduleCounter.polls` |
//! | promise | `PromiseAgent.get_promise` + `complete_promise` | none — one-shot |
//!
//! Two properties here are load-bearing for the whole scenario, and both are
//! easy to get subtly wrong:
//!
//! **Deterministic idempotency keys.** Every operation's key is derived from its
//! stream, agent and sequence number, so a retry is the *same* operation to the
//! platform. Minting a fresh key per attempt — which is what the general-purpose
//! benchmark helper does — would turn every duplicate execution into a clean
//! run, because the platform would be right to execute both.
//!
//! **Honest failure classification.** [`is_definite_rejection`] only lets an
//! operation be called
//! `Rejected` when the platform returned a definite, non-retryable status.
//! Anything else that failed is `Indeterminate`: from the client side, a dropped
//! connection is indistinguishable from a request that arrived and executed. A
//! shard-manager kill produces exactly these, and recording them as failures
//! would understate what the platform did while recording them as successes
//! would overstate it.

use crate::chaos::RetryPolicy;
use crate::chaos::history::{OperationHistory, OperationRecord, Outcome, Phase, Stream};
use chrono::Utc;
use golem_common::base_model::agent::{DataValue, ParsedAgentId};
use golem_common::model::IdempotencyKey;
use golem_common::model::component::ComponentDto;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::FromValue;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

/// Agent type names exported by the counters component.
const COUNTER_AGENT: &str = "Counter";
const EPHEMERAL_COUNTER_AGENT: &str = "EphemeralCounter";
const SCHEDULE_EMITTER_AGENT: &str = "ScheduleEmitter";
const SCHEDULE_COUNTER_AGENT: &str = "ScheduleCounter";
const PROMISE_AGENT: &str = "PromiseAgent";

/// How far ahead scheduled polls are registered. Long enough that registration
/// and firing are distinct events (so a fault can land between them), short
/// enough that every fire has landed well before read-back at the end of the
/// recovery phase.
const SCHEDULE_LEAD: Duration = Duration::from_secs(10);

/// Invocation-context spans added when scheduling. Zero: this scenario is about
/// restart recovery, and extra spans only make the traces harder to read.
const SCHEDULE_CONTEXT_SPANS: u32 = 0;

/// Ceiling on operations in flight at once. The rate limiter sets the pace; this
/// only stops a stalled platform from accumulating unbounded tasks during a
/// fault, which would turn a fault window into an out-of-memory abort.
///
/// It has to stay well above `ratePerSec`, because it doubles as a stall
/// backstop: once the pool is exhausted the streams stop submitting, so a cap
/// too close to the rate would silently clamp the workload the moment the
/// platform slowed — and the fault window is exactly when that happens. At the
/// suite's 100 ops/s this is ~10s of submissions, which absorbs a stall long
/// enough to be interesting without letting a fully-stalled platform accumulate
/// the ~12000 tasks a 120s attempt timeout would otherwise permit.
const MAX_IN_FLIGHT: usize = 1024;

/// Per-attempt timeout. Generous, because a shard-manager restart legitimately
/// stalls in-flight work — but bounded, so an operation cannot outlive the
/// scenario that submitted it.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// Payload written when completing a promise.
const PROMISE_PAYLOAD: &[u8] = b"chaos";

/// The phase the workload is currently submitting into, flipped by the scenario
/// as it advances. Shared rather than passed so the flip takes effect for
/// operations already queued.
#[derive(Debug, Clone)]
pub struct PhaseMarker(Arc<AtomicU8>);

impl PhaseMarker {
    pub fn new(phase: Phase) -> Self {
        let marker = Self(Arc::new(AtomicU8::new(0)));
        marker.set(phase);
        marker
    }

    pub fn set(&self, phase: Phase) {
        let code = match phase {
            Phase::Baseline => 0,
            Phase::Fault => 1,
            Phase::Recovery => 2,
        };
        self.0.store(code, Ordering::Relaxed);
    }

    pub fn get(&self) -> Phase {
        match self.0.load(Ordering::Relaxed) {
            0 => Phase::Baseline,
            1 => Phase::Fault,
            _ => Phase::Recovery,
        }
    }
}

/// Everything a workload operation needs to run and record itself.
#[derive(Clone)]
pub struct WorkloadContext {
    pub user: TestUserContext<BenchmarkTestDependencies>,
    /// Counters component: durable, ephemeral and scheduled streams.
    pub counters: ComponentDto,
    /// Promise component.
    pub promise: ComponentDto,
    pub history: OperationHistory,
    pub retry: RetryPolicy,
    pub phase: PhaseMarker,
    /// Prefix every agent id and idempotency key of this run shares, so a trace
    /// or log search can be narrowed to exactly this run.
    pub key_prefix: String,
}

impl WorkloadContext {
    /// Agent id for the `index`-th agent of a stream, e.g.
    /// `chaos-s12-durable-0007`.
    pub fn agent_name(&self, stream: Stream, index: u32) -> String {
        format!("{}-{stream}-{index:04}", self.key_prefix)
    }

    /// The scheduled stream's target agent, distinct from its emitter: keeping
    /// them apart is what makes the fire observable independently of the
    /// registration.
    pub fn schedule_target_name(&self, index: u32) -> String {
        format!("{}-scheduled-target-{index:04}", self.key_prefix)
    }

    /// The deterministic key for one operation. Same inputs, same key — that is
    /// what makes a retry the same operation rather than a new one.
    pub fn idempotency_key(&self, agent: &str, seq: u64) -> String {
        format!("{agent}-{seq:08}")
    }
}

/// Whether an error means the platform definitely refused the request, as
/// opposed to the driver simply not knowing.
///
/// Only a definite, non-retryable HTTP status counts as a rejection. Everything
/// else — connection reset, timeout, no status at all — leaves genuine doubt
/// about whether the request arrived and executed, and is reported as such.
pub fn is_definite_rejection(error: &anyhow::Error) -> bool {
    for cause in error.chain() {
        if let Some(reqwest_error) = cause.downcast_ref::<reqwest::Error>() {
            return match reqwest_error.status() {
                // A status means the server answered, so it saw the request and
                // decided. 5xx/408/429 are transient and leave doubt; the rest
                // are definite refusals.
                Some(status) => {
                    !(status.is_server_error()
                        || status == reqwest::StatusCode::REQUEST_TIMEOUT
                        || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
                }
                // No status: the exchange never completed. Nobody can say
                // whether the server executed it.
                None => false,
            };
        }
    }
    // An error the driver cannot interpret is treated as doubt rather than as a
    // refusal. Widening the band costs a little precision; narrowing it wrongly
    // would manufacture a duplicate-execution finding out of a timeout.
    false
}

/// Outcome and value of a single invocation attempt.
struct AttemptResult {
    value: Option<u32>,
    error: Option<anyhow::Error>,
    definite_rejection: bool,
}

/// Runs one operation with the configured bounded, same-key retry, and records
/// it in the history.
#[allow(clippy::too_many_arguments)]
async fn run_operation<F, Fut>(
    ctx: &WorkloadContext,
    stream: Stream,
    agent: String,
    method: &str,
    key: String,
    invoke: F,
) where
    F: Fn(IdempotencyKey) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<u32>>>,
{
    let op_id = ctx.history.next_op_id();
    let phase = ctx.phase.get();
    let submitted_at = Utc::now();
    let started = Instant::now();
    let idempotency_key = IdempotencyKey::new(key.clone());

    let mut attempts = 0u32;
    let mut first_attempt_value = None;
    let mut last: AttemptResult;

    loop {
        attempts += 1;
        last = match tokio::time::timeout(ATTEMPT_TIMEOUT, invoke(idempotency_key.clone())).await {
            Ok(Ok(value)) => AttemptResult {
                value,
                error: None,
                definite_rejection: false,
            },
            Ok(Err(error)) => {
                let definite_rejection = is_definite_rejection(&error);
                AttemptResult {
                    value: None,
                    error: Some(error),
                    definite_rejection,
                }
            }
            Err(_) => AttemptResult {
                value: None,
                error: Some(anyhow::anyhow!(
                    "attempt timed out after {ATTEMPT_TIMEOUT:?}"
                )),
                // A timeout is the archetypal case of not knowing: the request
                // may well be executing right now.
                definite_rejection: false,
            },
        };

        let succeeded = last.error.is_none();
        let may_retry = attempts <= ctx.retry.max_retries
            && !succeeded
            && (!ctx.retry.transport_only || !last.definite_rejection);

        if succeeded || !may_retry {
            break;
        }

        // Only meaningful when the first attempt actually answered: comparing a
        // value against nothing says nothing.
        if first_attempt_value.is_none() {
            first_attempt_value = last.value;
        }
        debug!(
            "Chaos {stream} op {key} attempt {attempts} failed, retrying with the same key in {:?}",
            ctx.retry.delay()
        );
        tokio::time::sleep(ctx.retry.delay()).await;
    }

    let outcome = if last.error.is_none() {
        Outcome::Confirmed
    } else if last.definite_rejection {
        Outcome::Rejected
    } else {
        Outcome::Indeterminate
    };

    if outcome == Outcome::Indeterminate {
        warn!(
            "Chaos {stream} op {key} ended indeterminate after {attempts} attempt(s): {}",
            last.error
                .as_ref()
                .map(|e| e.to_string())
                .unwrap_or_default()
        );
    }

    ctx.history.record(OperationRecord {
        op_id,
        stream,
        phase,
        agent,
        method: method.to_string(),
        idempotency_key: key,
        submitted_at,
        completed_at: Some(Utc::now()),
        attempts,
        outcome,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        returned_value: last.value,
        first_attempt_value,
        error: last.error.as_ref().map(|e| format!("{e:#}")),
    });
}

/// Extracts a `u32` return value, if the agent returned one. Absent values are
/// not an error: several workload methods return nothing.
fn as_u32(value: DataValue) -> Option<u32> {
    u32::from_value(value.into_return_value()?).ok()
}

/// A running workload. Dropping the handle does not stop it; call
/// [`WorkloadHandle::stop`] so in-flight operations are recorded rather than
/// cancelled mid-flight, which would lose exactly the operations that matter.
pub struct WorkloadHandle {
    stop: Arc<AtomicU8>,
    tasks: JoinSet<()>,
    submitted: Arc<AtomicU64>,
}

impl WorkloadHandle {
    /// Operations submitted so far.
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }

    /// Signals every stream to stop and waits for in-flight operations to be
    /// recorded.
    pub async fn stop(mut self) {
        self.stop.store(1, Ordering::Relaxed);
        while self.tasks.join_next().await.is_some() {}
        info!(
            "Chaos workload stopped after {} operations",
            self.submitted()
        );
    }
}

/// Starts the mixed workload. Returns immediately; operations continue until
/// [`WorkloadHandle::stop`].
pub fn start(ctx: WorkloadContext, config: &crate::chaos::WorkloadConfig) -> WorkloadHandle {
    let stop = Arc::new(AtomicU8::new(0));
    let submitted = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();

    // One shared permit pool across streams, so a stalled stream cannot starve
    // the others of the ability to submit.
    let in_flight = Arc::new(Semaphore::new(MAX_IN_FLIGHT));

    // The configured rate is split evenly across the active streams. Streams
    // with no agents configured are skipped entirely rather than run empty.
    let active: Vec<(Stream, u32)> = [
        (Stream::Durable, config.durable_agents),
        (Stream::Ephemeral, config.ephemeral_agents),
        (Stream::Scheduled, config.scheduled_agents),
        (Stream::Promise, config.promise_agents),
    ]
    .into_iter()
    .filter(|(_, agents)| *agents > 0)
    .collect();

    if active.is_empty() {
        warn!("Chaos workload configured with no agents in any stream");
        return WorkloadHandle {
            stop,
            tasks,
            submitted,
        };
    }

    let per_stream_rate = (config.rate_per_sec as f64 / active.len() as f64).max(0.1);
    let interval = Duration::from_secs_f64(1.0 / per_stream_rate);
    info!(
        "Chaos workload starting: {} streams, {:.2} ops/s each ({:?} between submissions)",
        active.len(),
        per_stream_rate,
        interval
    );

    for (stream, agent_count) in active {
        let ctx = ctx.clone();
        let stop = stop.clone();
        let submitted = submitted.clone();
        let in_flight = in_flight.clone();

        tasks.spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut seq: u64 = 0;
            let mut in_stream = JoinSet::new();

            while stop.load(Ordering::Relaxed) == 0 {
                ticker.tick().await;

                let Ok(permit) = in_flight.clone().acquire_owned().await else {
                    break;
                };
                let index = (seq % agent_count as u64) as u32;
                let ctx = ctx.clone();
                seq += 1;
                submitted.fetch_add(1, Ordering::Relaxed);

                in_stream.spawn(async move {
                    let _permit = permit;
                    submit_one(&ctx, stream, index, seq).await;
                });

                // Reap finished operations so the set does not grow unbounded.
                while in_stream.try_join_next().is_some() {}
            }

            // Let in-flight operations finish and record themselves: an
            // operation cancelled mid-flight is one the history cannot classify,
            // and those are precisely the interesting ones.
            while in_stream.join_next().await.is_some() {}
        });
    }

    WorkloadHandle {
        stop,
        tasks,
        submitted,
    }
}

/// Submits one operation of `stream` against agent `index`.
async fn submit_one(ctx: &WorkloadContext, stream: Stream, index: u32, seq: u64) {
    match stream {
        Stream::Durable => {
            let agent = ctx.agent_name(Stream::Durable, index);
            let key = ctx.idempotency_key(&agent, seq);
            let parsed: ParsedAgentId = agent_id!(COUNTER_AGENT, agent.clone());
            let ctx2 = ctx.clone();
            let parsed2 = parsed.clone();
            run_operation(ctx, Stream::Durable, agent.clone(), "increment", key, |k| {
                let ctx = ctx2.clone();
                let parsed = parsed2.clone();
                async move {
                    let value = ctx
                        .user
                        .invoke_and_await_agent_with_key(
                            &ctx.counters,
                            &parsed,
                            &k,
                            "increment",
                            data_value!(),
                        )
                        .await?;
                    Ok(as_u32(value))
                }
            })
            .await;
        }
        Stream::Ephemeral => {
            let agent = ctx.agent_name(Stream::Ephemeral, index);
            let key = ctx.idempotency_key(&agent, seq);
            let parsed: ParsedAgentId = agent_id!(EPHEMERAL_COUNTER_AGENT, agent.clone());
            let ctx2 = ctx.clone();
            let parsed2 = parsed.clone();
            run_operation(
                ctx,
                Stream::Ephemeral,
                agent.clone(),
                "increment",
                key,
                |k| {
                    let ctx = ctx2.clone();
                    let parsed = parsed2.clone();
                    async move {
                        let value = ctx
                            .user
                            .invoke_and_await_agent_with_key(
                                &ctx.counters,
                                &parsed,
                                &k,
                                "increment",
                                data_value!(),
                            )
                            .await?;
                        Ok(as_u32(value))
                    }
                },
            )
            .await;
        }
        Stream::Scheduled => {
            // The emitter registers a poll on its own target agent. Read-back
            // compares the target's `polls` count against the registrations, so
            // emitter and target are paired one to one.
            let emitter = ctx.agent_name(Stream::Scheduled, index);
            let target = ctx.schedule_target_name(index);
            let key = ctx.idempotency_key(&emitter, seq);
            let parsed: ParsedAgentId = agent_id!(SCHEDULE_EMITTER_AGENT, emitter.clone());
            let fire_at = std::time::SystemTime::now() + SCHEDULE_LEAD;
            let since_epoch = fire_at
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let (secs, nanos) = (since_epoch.as_secs(), since_epoch.subsec_nanos());

            let ctx2 = ctx.clone();
            let parsed2 = parsed.clone();
            let target2 = target.clone();
            // Recorded against the target, because that is the agent read-back
            // reads.
            run_operation(
                ctx,
                Stream::Scheduled,
                target,
                "schedule_poll_at",
                key,
                |k| {
                    let ctx = ctx2.clone();
                    let parsed = parsed2.clone();
                    let target = target2.clone();
                    async move {
                        ctx.user
                            .invoke_and_await_agent_with_key(
                                &ctx.counters,
                                &parsed,
                                &k,
                                "schedule_poll_at",
                                data_value!(target, secs, nanos, SCHEDULE_CONTEXT_SPANS),
                            )
                            .await?;
                        Ok(None)
                    }
                },
            )
            .await;
        }
        Stream::Promise => {
            let agent = ctx.agent_name(Stream::Promise, index);
            let key = ctx.idempotency_key(&agent, seq);
            let parsed: ParsedAgentId = agent_id!(PROMISE_AGENT, agent.clone());
            let ctx2 = ctx.clone();
            let parsed2 = parsed.clone();
            run_operation(
                ctx,
                Stream::Promise,
                agent.clone(),
                "get_promise+complete",
                key,
                |k| {
                    let ctx = ctx2.clone();
                    let parsed = parsed2.clone();
                    async move {
                        let created = ctx
                            .user
                            .invoke_and_await_agent_with_key(
                                &ctx.promise,
                                &parsed,
                                &k,
                                "get_promise",
                                data_value!(),
                            )
                            .await?;
                        let promise_value = created
                            .into_return_value_and_type()
                            .ok_or_else(|| anyhow::anyhow!("get_promise returned no promise id"))?;
                        let promise_id =
                            golem_common::model::PromiseId::from_value(promise_value.value)
                                .map_err(|e| anyhow::anyhow!("invalid promise id: {e}"))?;
                        // Completing is the half that must survive the fault:
                        // a promise created but never resolved leaves a worker
                        // suspended forever.
                        ctx.user
                            .complete_promise(&promise_id, PROMISE_PAYLOAD.to_vec())
                            .await?;
                        Ok(None)
                    }
                },
            )
            .await;
        }
    }
}

/// Reads back a durable counter agent's value.
pub async fn read_counter(ctx: &WorkloadContext, agent: &str) -> Result<u64, String> {
    let parsed: ParsedAgentId = agent_id!(COUNTER_AGENT, agent.to_string());
    match ctx
        .user
        .invoke_and_await_agent(&ctx.counters, &parsed, "count", data_value!())
        .await
    {
        Ok(value) => as_u32(value)
            .map(u64::from)
            .ok_or_else(|| "count returned no value".to_string()),
        Err(e) => Err(format!("{e:#}")),
    }
}

/// Reads back how many scheduled polls actually fired on a target agent.
pub async fn read_polls(ctx: &WorkloadContext, agent: &str) -> Result<u64, String> {
    let parsed: ParsedAgentId = agent_id!(SCHEDULE_COUNTER_AGENT, agent.to_string());
    match ctx
        .user
        .invoke_and_await_agent(&ctx.counters, &parsed, "polls", data_value!())
        .await
    {
        Ok(value) => as_u32(value)
            .map(u64::from)
            .ok_or_else(|| "polls returned no value".to_string()),
        Err(e) => Err(format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn ctx_names(prefix: &str) -> (String, String, String) {
        // Only the naming helpers are exercised here; they do not need a live
        // platform, and they are what a trace search depends on.
        let key_prefix = prefix.to_string();
        let agent = format!("{key_prefix}-durable-0007");
        let target = format!("{key_prefix}-scheduled-target-0007");
        let key = format!("{agent}-00000042");
        (agent, target, key)
    }

    #[test]
    fn phase_marker_round_trips_every_phase() {
        let marker = PhaseMarker::new(Phase::Baseline);
        assert_eq!(marker.get(), Phase::Baseline);
        marker.set(Phase::Fault);
        assert_eq!(marker.get(), Phase::Fault);
        marker.set(Phase::Recovery);
        assert_eq!(marker.get(), Phase::Recovery);
    }

    /// A flip has to be visible through a clone: the scenario holds one marker
    /// and every stream holds a clone of the same one.
    #[test]
    fn phase_marker_is_shared_across_clones() {
        let marker = PhaseMarker::new(Phase::Baseline);
        let clone = marker.clone();
        marker.set(Phase::Fault);
        assert_eq!(clone.get(), Phase::Fault);
    }

    /// Keys and agent names carry the run prefix so a trace or log search can be
    /// narrowed to one run, and they are zero-padded so they sort.
    #[test]
    fn agent_names_and_keys_are_prefixed_and_sortable() {
        let (agent, target, key) = ctx_names("chaos-s12");
        assert_eq!(agent, "chaos-s12-durable-0007");
        assert_eq!(target, "chaos-s12-scheduled-target-0007");
        assert!(key.starts_with(&agent));
        assert!(
            key > format!("{agent}-00000041"),
            "zero-padded sequences must order lexicographically"
        );
    }

    // ── Failure classification ──────────────────────────────────────────────
    // The three-way split is what keeps the read-back honest, so the boundary
    // between "definitely refused" and "cannot tell" gets direct tests.

    #[test]
    fn an_uninterpretable_error_leaves_doubt_rather_than_claiming_refusal() {
        let error = anyhow::anyhow!("connection reset by peer");
        assert!(
            !is_definite_rejection(&error),
            "an error the driver cannot read must widen the band, not narrow it"
        );
    }

    #[test]
    fn a_wrapped_uninterpretable_error_still_leaves_doubt() {
        let error = anyhow::anyhow!("broken pipe").context("invoking increment");
        assert!(!is_definite_rejection(&error));
    }

    /// The retry rule the policy encodes: retry only what might be transient,
    /// and only under the same key.
    #[test]
    fn retry_policy_permits_one_retry_for_non_definite_failures() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 1);
        assert!(policy.transport_only);

        // attempts=1 (the first) may retry once; attempts=2 may not.
        let may_retry = |attempts: u32, definite: bool| {
            attempts <= policy.max_retries && (!policy.transport_only || !definite)
        };
        assert!(may_retry(1, false), "first transport failure retries");
        assert!(!may_retry(2, false), "the retry budget is one");
        assert!(!may_retry(1, true), "a definite refusal is not retried");
    }
}

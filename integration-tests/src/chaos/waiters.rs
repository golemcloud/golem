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

//! The suspended-waiter workload (GOL-377).
//!
//! Every other stream in this suite keeps an agent *busy*. This one keeps a pool
//! of them asleep. Each waiter repeats one round:
//!
//! 1. `arm(token)` creates a promise and returns it. Fast, and the only part the
//!    driver needs an answer from.
//! 2. `wait(token, promise)` parks the agent on that promise. The invocation
//!    stays open — on the platform as a suspended worker, in the driver as a
//!    task holding a connection.
//! 3. After `dwellMillis`, the driver completes the promise from the outside.
//! 4. The wakeup ends the round, and the waiter starts the next one.
//!
//! ## Why one promise per waiter
//!
//! An agent runs its invocations one at a time, so a waiter parked in `wait`
//! cannot be armed again until it wakes. That is not a limitation to work
//! around; it is what makes the pool size mean something exact. With `waiters`
//! agents and one promise each, the number of agents standing suspended at the
//! instant the pod dies is a known constant rather than a sample — the same
//! property [`crate::chaos::pinned`] gets from one in-flight operation per
//! agent, for the same reason.
//!
//! It also makes a lost wakeup visible while the run is still going. A waiter
//! whose completion never arrives can never start another round, so it simply
//! stops producing. See [`WaiterHandle::stalled`].
//!
//! ## Why the dwell
//!
//! `dwellMillis` decides how the population divides at the moment of the kill.
//! Every waiter is either parked-and-waiting (the dwell) or being-woken (the
//! completion round trip), and the dwell is far the longer of the two — so the
//! kill lands mostly in the first, which is the state this scenario is about,
//! and occasionally in the second, which is the narrower race S8 already covers
//! for ordinary invocations.
//!
//! The dwell also has to comfortably exceed the workflow's inject-and-verify
//! path — signal poll, `kubectl apply`, waiting for `AllInjected` — or every
//! promise armed before the kill would already have been completed by the time
//! the pod died, and the run would measure nothing.
//!
//! ## Why completions are retried like everything else
//!
//! The suite's retry policy — one same-key retry, transport failures only —
//! applies here too, and a completion is the one operation where that could
//! plausibly be accused of hiding the defect: `complete` writes with
//! `set_if_not_exists` and re-triggers the wakeup, so a retry can repair a
//! completion whose wakeup was lost.
//!
//! It cannot hide what this scenario asks, because the question is scoped to
//! completions the platform *confirmed*. A retry only happens when the previous
//! attempt did not return success, so a confirmed completion is always a single
//! accepted call. "The platform said yes and the waiter never woke" is exactly
//! as detectable with the retry as without it, and the retry keeps the workload
//! behaving like a client anyone would actually write.

use crate::chaos::PromiseConfig;
use crate::chaos::history::{Stream, WaiterWakeupLog, WakeupRecord};
use crate::chaos::workload::{self, WorkloadContext};
use chrono::{DateTime, TimeZone, Utc};
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::model::PromiseId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::FromValue;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Agent type exported by the promise component for this workload.
pub const PROMISE_WAITER_AGENT: &str = "PromiseWaiter";

/// Payload a completion carries. Nothing reads it; the token is what identifies
/// a round, and it travels through the agent rather than through the promise.
const COMPLETION_PAYLOAD: &[u8] = b"chaos-s11";

/// How long a waiter's loop waits for its own wakeup before concluding the
/// waiter is parked for good and standing down.
///
/// Deliberately a multiple of the wakeup budget rather than a fixed number: the
/// budget is already the run's statement about what a recovery may cost, and
/// anything that treats "slow" as "lost" would turn a bad p99 into a false
/// finding. Standing down is not a verdict either — the read-back still asks the
/// waiter what happened, and a waiter that woke late says so.
const STALL_MULTIPLE: u32 = 4;

/// Floor under [`STALL_MULTIPLE`], for a configuration with a very short budget.
const MIN_STALL_TIMEOUT: Duration = Duration::from_secs(180);

/// How many waiters are read back at once. Same reasoning as
/// [`crate::chaos::scenarios::read_back_agents`]: reads do not mutate, and
/// walking a few hundred of them one at a time behind a per-read ceiling
/// outlasts the maintenance window.
const READ_CONCURRENCY: usize = 16;

/// Ceiling on one wakeup-log read. Generous, because it happens on a cluster
/// that has just been through a fault, and because the answer carries every
/// wakeup the waiter recorded rather than one number.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// A running waiter workload.
///
/// As elsewhere, dropping the handle does not stop it: call
/// [`WaiterHandle::stop`] so rounds in flight record themselves instead of being
/// cancelled mid-completion.
pub struct WaiterHandle {
    tasks: JoinSet<()>,
    running: Arc<AtomicBool>,
    rounds: Arc<AtomicU64>,
    stalled: Arc<AtomicU64>,
}

impl WaiterHandle {
    /// Rounds started across all waiters.
    pub fn rounds(&self) -> u64 {
        self.rounds.load(Ordering::Relaxed)
    }

    /// Waiters that stood down because a wakeup never arrived.
    ///
    /// The live half of this scenario's oracle. A waiter counted here was parked
    /// on a promise the driver had completed, and stayed parked long enough that
    /// no recovery budget explains it. The read-back then decides whether it
    /// eventually woke.
    pub fn stalled(&self) -> u64 {
        self.stalled.load(Ordering::Relaxed)
    }

    pub async fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        while self.tasks.join_next().await.is_some() {}
    }
}

/// Warms every waiter so the baseline measures resident agents rather than cold
/// starts. Returns how many answered.
pub async fn warm(ctx: &WorkloadContext, waiters: &[String]) -> usize {
    let mut warmed = 0;
    let mut set = JoinSet::new();
    for waiter in waiters {
        let ctx = ctx.clone();
        let waiter = waiter.clone();
        set.spawn(async move {
            let parsed: ParsedAgentId = agent_id!(PROMISE_WAITER_AGENT, waiter.clone());
            ctx.user
                .invoke_and_await_agent(&ctx.promise, &parsed, "wakes", data_value!())
                .await
                .is_ok()
        });
    }
    while let Some(result) = set.join_next().await {
        if matches!(result, Ok(true)) {
            warmed += 1;
        }
    }
    warmed
}

/// Starts one loop per waiter.
pub fn start(ctx: WorkloadContext, waiters: &[String], config: &PromiseConfig) -> WaiterHandle {
    let running = Arc::new(AtomicBool::new(true));
    let rounds = Arc::new(AtomicU64::new(0));
    let stalled = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();

    let stall_timeout = (config.wakeup_budget() * STALL_MULTIPLE).max(MIN_STALL_TIMEOUT);
    info!(
        "S11: starting {} waiters, {:?} dwell, standing a waiter down after {stall_timeout:?} \
         without a wakeup",
        waiters.len(),
        config.dwell()
    );

    for waiter in waiters {
        let ctx = ctx.clone();
        let waiter = waiter.clone();
        let running = running.clone();
        let rounds = rounds.clone();
        let stalled = stalled.clone();
        let dwell = config.dwell();
        tasks.spawn(async move {
            run_waiter(ctx, waiter, dwell, stall_timeout, running, rounds, stalled).await;
        });
    }

    WaiterHandle {
        tasks,
        running,
        rounds,
        stalled,
    }
}

/// One waiter's rounds, until the workload stops or the waiter stands down.
async fn run_waiter(
    ctx: WorkloadContext,
    waiter: String,
    dwell: Duration,
    stall_timeout: Duration,
    running: Arc<AtomicBool>,
    rounds: Arc<AtomicU64>,
    stalled: Arc<AtomicU64>,
) {
    let parsed: ParsedAgentId = agent_id!(PROMISE_WAITER_AGENT, waiter.clone());
    let mut round = 0u64;

    while running.load(Ordering::Relaxed) {
        let token = ctx.idempotency_key(&waiter, round);
        round += 1;
        rounds.fetch_add(1, Ordering::Relaxed);

        let Some(promise_id) = arm(&ctx, &waiter, &parsed, &token).await else {
            // Nothing to complete and nobody parked. The next round tries again
            // after a dwell, which keeps a wholly unreachable platform from
            // spinning.
            tokio::time::sleep(dwell).await;
            continue;
        };

        // Park the waiter. Held open deliberately: the driver's own view of the
        // wakeup is one of the two independent answers this scenario collects,
        // and the fault is expected to take it away.
        let wait_task = {
            let ctx = ctx.clone();
            let waiter = waiter.clone();
            let parsed = parsed.clone();
            let token = token.clone();
            let promise_id = promise_id.clone();
            tokio::spawn(async move { wait(&ctx, &waiter, &parsed, &token, &promise_id).await })
        };

        tokio::time::sleep(dwell).await;
        complete(&ctx, &waiter, &token, &promise_id).await;

        // The handle is dropped rather than aborted on the timeout path, which
        // leaves the invocation running. That is deliberate: aborting it would
        // throw away the one record that says how the round ended, and the agent
        // is parked either way — the driver's task is not what is holding it.
        match tokio::time::timeout(stall_timeout, wait_task).await {
            Ok(_) => {}
            Err(_) => {
                warn!(
                    "S11: waiter {waiter} has not woken {stall_timeout:?} after its completion \
                     on token {token}; standing it down"
                );
                stalled.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }
}

/// Creates the round's promise, recording the invocation.
///
/// The promise id comes back through a cell rather than a return value because
/// [`workload::run_operation`] owns the retry rule and the failure
/// classification for every stream in the suite, and it reports outcomes rather
/// than payloads. Duplicating it here to get one value back would put the two
/// load-bearing rules of every chaos scenario in two places.
async fn arm(
    ctx: &WorkloadContext,
    waiter: &str,
    parsed: &ParsedAgentId,
    token: &str,
) -> Option<PromiseId> {
    let cell: Arc<std::sync::Mutex<Option<PromiseId>>> = Arc::new(std::sync::Mutex::new(None));
    let sink = cell.clone();
    let ctx2 = ctx.clone();
    let parsed2 = parsed.clone();

    workload::run_operation(
        ctx,
        Stream::PromiseWait,
        waiter.to_string(),
        "arm",
        format!("{token}-arm"),
        |key| {
            let ctx = ctx2.clone();
            let parsed = parsed2.clone();
            let sink = sink.clone();
            let token = token.to_string();
            async move {
                let created = ctx
                    .user
                    .invoke_and_await_agent_with_key(
                        &ctx.promise,
                        &parsed,
                        &key,
                        "arm",
                        data_value!(token),
                    )
                    .await?;
                let value = created
                    .into_return_value_and_type()
                    .ok_or_else(|| anyhow::anyhow!("arm returned no promise id"))?;
                let promise_id = PromiseId::from_value(value.value)
                    .map_err(|e| anyhow::anyhow!("invalid promise id: {e}"))?;
                *sink.lock().unwrap() = Some(promise_id);
                Ok(None)
            }
        },
    )
    .await;

    cell.lock().unwrap().clone()
}

/// Parks the waiter until its promise resolves.
async fn wait(
    ctx: &WorkloadContext,
    waiter: &str,
    parsed: &ParsedAgentId,
    token: &str,
    promise_id: &PromiseId,
) {
    let ctx2 = ctx.clone();
    let parsed2 = parsed.clone();
    workload::run_operation(
        ctx,
        Stream::PromiseWait,
        waiter.to_string(),
        "wait",
        format!("{token}-wait"),
        |key| {
            let ctx = ctx2.clone();
            let parsed = parsed2.clone();
            let token = token.to_string();
            let promise_id = promise_id.clone();
            async move {
                ctx.user
                    .invoke_and_await_agent_with_key(
                        &ctx.promise,
                        &parsed,
                        &key,
                        "wait",
                        data_value!(token, promise_id),
                    )
                    .await?;
                Ok(None)
            }
        },
    )
    .await;
}

/// Completes the round's promise from outside the agent.
///
/// Recorded under the round's token rather than under a key of its own, which is
/// what joins this record to the wakeup the waiter logs. The completion API is
/// keyed by promise id and takes no idempotency key of its own — a retry is the
/// same completion because it names the same promise, and `set_if_not_exists` on
/// the platform side is what makes that true.
async fn complete(ctx: &WorkloadContext, waiter: &str, token: &str, promise_id: &PromiseId) {
    let ctx2 = ctx.clone();
    workload::run_operation(
        ctx,
        Stream::PromiseWait,
        waiter.to_string(),
        "complete",
        token.to_string(),
        |_key| {
            let ctx = ctx2.clone();
            let promise_id = promise_id.clone();
            async move {
                ctx.user
                    .complete_promise(&promise_id, COMPLETION_PAYLOAD.to_vec())
                    .await?;
                Ok(None)
            }
        },
    )
    .await;
}

/// Reads every waiter's wakeup log.
pub async fn read_logs(ctx: &WorkloadContext, waiters: &[String]) -> Vec<WaiterWakeupLog> {
    let mut logs = Vec::with_capacity(waiters.len());
    for chunk in waiters.chunks(READ_CONCURRENCY) {
        let mut set = JoinSet::new();
        for waiter in chunk {
            let ctx = ctx.clone();
            let waiter = waiter.clone();
            set.spawn(async move { read_log(&ctx, &waiter).await });
        }
        while let Some(result) = set.join_next().await {
            match result {
                Ok(log) => logs.push(log),
                Err(e) => warn!("S11: a wakeup-log read task failed: {e}"),
            }
        }
    }
    logs.sort_by(|a, b| a.agent.cmp(&b.agent));
    logs
}

async fn read_log(ctx: &WorkloadContext, waiter: &str) -> WaiterWakeupLog {
    let parsed: ParsedAgentId = agent_id!(PROMISE_WAITER_AGENT, waiter.to_string());

    let wakes = match read_within(waiter, "wakes", async {
        ctx.user
            .invoke_and_await_agent(&ctx.promise, &parsed, "wakes", data_value!())
            .await
            .map_err(|e| format!("{e:#}"))
    })
    .await
    {
        Ok(value) => value
            .into_return_value_and_type()
            .and_then(|v| u32::from_value(v.value).ok())
            .map(|v| v as u64),
        Err(e) => {
            return WaiterWakeupLog {
                agent: waiter.to_string(),
                wakes: None,
                wakeups: Vec::new(),
                error: Some(e),
            };
        }
    };

    match read_within(waiter, "wakeups", async {
        ctx.user
            .invoke_and_await_agent(&ctx.promise, &parsed, "wakeups", data_value!())
            .await
            .map_err(|e| format!("{e:#}"))
    })
    .await
    {
        Ok(value) => {
            let wakeups = value
                .into_return_value_and_type()
                .map(|v| parse_wakeups(v.value))
                .unwrap_or_default();
            WaiterWakeupLog {
                agent: waiter.to_string(),
                wakes,
                wakeups,
                error: None,
            }
        }
        Err(e) => WaiterWakeupLog {
            agent: waiter.to_string(),
            wakes,
            wakeups: Vec::new(),
            error: Some(e),
        },
    }
}

/// A read-back invocation under [`READ_TIMEOUT`].
///
/// A timeout is a verdict, not an error to propagate. It is also the loudest
/// thing this scenario can observe: a waiter that will not answer `wakeups` is
/// usually a waiter still parked on a promise that was completed long ago, and
/// [`crate::chaos::wakeups`] treats that case differently from an ordinary
/// unreadable agent.
async fn read_within<T, F>(waiter: &str, what: &str, read: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    match tokio::time::timeout(READ_TIMEOUT, read).await {
        Ok(result) => result,
        Err(_) => {
            warn!("S11: reading {what} on waiter {waiter} timed out after {READ_TIMEOUT:?}");
            Err(format!("{what} timed out after {READ_TIMEOUT:?}"))
        }
    }
}

/// Turns the agent's `(token, armed_millis, woken_millis)` triples into records.
fn parse_wakeups(value: golem_wasm::Value) -> Vec<WakeupRecord> {
    let raw: Vec<(String, u64, u64)> = match Vec::<(String, u64, u64)>::from_value(value) {
        Ok(raw) => raw,
        Err(e) => {
            warn!("S11: could not read a wakeup log: {e}");
            return Vec::new();
        }
    };
    raw.into_iter()
        .map(|(token, armed_millis, woken_millis)| WakeupRecord {
            token,
            armed_at: from_millis(armed_millis),
            woken_at: from_millis(woken_millis),
        })
        .collect()
}

/// Epoch milliseconds as the agent stamped them.
///
/// A zero means the agent had no armed time for the token, which the component
/// only produces if its own arm log rolled over. It is kept as the epoch rather
/// than dropped so the resulting nonsense interval is visible instead of the
/// wakeup silently going missing.
fn from_millis(millis: u64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(millis as i64)
        .single()
        .unwrap_or_else(|| Utc.timestamp_nanos(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn a_stall_timeout_scales_with_the_wakeup_budget() {
        let config = PromiseConfig {
            waiters: 10,
            dwell_millis: 5000,
            wakeup_budget_secs: 120,
        };
        assert_eq!(
            (config.wakeup_budget() * STALL_MULTIPLE).max(MIN_STALL_TIMEOUT),
            Duration::from_secs(480)
        );
    }

    /// A budget short enough that four of it would call an ordinary recovery a
    /// stall still gets the floor.
    #[test]
    fn a_short_wakeup_budget_still_gets_the_floor() {
        let config = PromiseConfig {
            waiters: 10,
            dwell_millis: 5000,
            wakeup_budget_secs: 10,
        };
        assert_eq!(
            (config.wakeup_budget() * STALL_MULTIPLE).max(MIN_STALL_TIMEOUT),
            MIN_STALL_TIMEOUT
        );
    }

    #[test]
    fn an_agent_timestamp_of_zero_stays_visible_as_the_epoch() {
        assert_eq!(from_millis(0).timestamp_millis(), 0);
    }

    /// A promise id has to survive the trip back *into* an agent.
    ///
    /// The driver parses what `arm` returns into a [`PromiseId`] so it can call
    /// the external completion API with it, and passes that same parsed value
    /// into `wait`. Only the first of those directions is exercised anywhere
    /// else in this repository — the density benchmark keeps the raw
    /// `ValueAndType` for its agent calls and never re-encodes one. So the
    /// encoding side is pinned here rather than discovered on a cluster: a
    /// `PromiseId` that does not round-trip would fail every round of S11 after
    /// the run had already spent its baseline.
    #[test]
    fn a_promise_id_survives_the_round_trip_back_into_an_agent() {
        use golem_common::model::{AgentId, OplogIndex};
        use golem_wasm::IntoValue;

        let parsed: ParsedAgentId = agent_id!(PROMISE_WAITER_AGENT, "w-0001".to_string());
        let promise_id = PromiseId {
            agent_id: AgentId {
                component_id: golem_common::model::component::ComponentId(uuid::Uuid::nil()),
                agent_id: parsed.to_string(),
            },
            oplog_idx: OplogIndex::from_u64(42),
        };

        let encoded = promise_id.clone().into_value();
        let decoded = PromiseId::from_value(encoded).expect("promise id should decode");
        assert_eq!(decoded, promise_id);
        assert_eq!(decoded.oplog_idx.as_u64(), 42);
    }
}

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

//! Agents that build state up and then ask the platform to take it back
//! (GOL-371).
//!
//! One emitter per agent, running rounds. A round is `increments_per_round`
//! increments followed by one `revert` of the last `revert_invocations` of
//! them. Both numbers are exact, which is what gives this scenario an oracle
//! with no band of doubt in it: the last increment of a round *returns* the
//! counter's value, so the driver knows exactly what the agent was worth before
//! the revert, and afterwards there are exactly two legitimate answers.
//!
//! ### The revert must never be retried
//!
//! Every other operation in this suite retries once, under the original
//! idempotency key, and that retry is load-bearing: it is what exposes
//! duplicate execution. A revert has no such key and is not idempotent.
//! Reverting "the last two invocations" twice takes back four. So a revert
//! whose response was lost but which actually landed would, on retry, revert
//! again — and the result would be indistinguishable from the platform tearing
//! a truncation, which is precisely the finding this scenario exists to make.
//!
//! The retry is therefore switched off for the revert call alone, and left on
//! for the increments around it. This is the only place in the suite that does
//! that, and it is not an oversight anywhere else.
//!
//! ### How a round is judged
//!
//! Not by reading the counter: a read is an invocation, and it would land in
//! the oplog between the increments and the next revert, shifting what "the
//! last N invocations" means. The **next round's first increment** is the probe
//! instead. It returns the new value, so the value the revert left behind is
//! that minus one, and it costs nothing extra.

use crate::chaos::history::{Outcome, Stream};
use crate::chaos::workload::{self, WorkloadContext};
use crate::chaos::{RetryPolicy, RevertConfig};
use chrono::{DateTime, Utc};
use golem_common::model::worker::{RevertLastInvocations, RevertWorkerTarget};
use golem_test_framework::dsl::TestDsl;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::task::JoinSet;
use tracing::info;

/// One round, as the driver observed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertRound {
    pub agent: String,
    pub round: u32,
    /// The counter value the round's last increment returned, so the value the
    /// agent was worth immediately before the revert. `None` when an increment
    /// in the round did not answer, which leaves the round unjudgeable rather
    /// than failed.
    pub before_revert: Option<u64>,
    /// How many invocations this round's revert asked to take back.
    pub asked_to_revert: u32,
    /// What the revert call itself returned.
    pub outcome: Outcome,
    pub submitted_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// The counter value observed after the revert, taken from the next round's
    /// first increment. `None` for the last round of the run, which the final
    /// read-back answers instead.
    pub observed_after: Option<u64>,
}

/// A running revert workload.
pub struct RevertHandle {
    stop: Arc<AtomicU8>,
    tasks: JoinSet<()>,
    submitted: Arc<AtomicU64>,
    rounds: Arc<Mutex<Vec<RevertRound>>>,
}

impl RevertHandle {
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }

    /// Rounds recorded so far, in no particular order.
    pub fn rounds(&self) -> Vec<RevertRound> {
        self.rounds.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// Signals every emitter to stop and waits for operations in flight to
    /// record themselves.
    pub async fn stop(mut self) -> Vec<RevertRound> {
        self.stop.store(1, Ordering::Relaxed);
        while self.tasks.join_next().await.is_some() {}
        let rounds = self.rounds();
        info!(
            "Chaos revert workload stopped after {} operations across {} rounds",
            self.submitted(),
            rounds.len()
        );
        rounds
    }
}

/// The agents a run of `count` emitters drives, in index order.
pub fn agent_names(ctx: &WorkloadContext, count: u32) -> Vec<String> {
    (0..count)
        .map(|index| ctx.agent_name(Stream::Revert, index))
        .collect()
}

/// Starts one emitter per agent.
pub fn start(ctx: WorkloadContext, config: &RevertConfig) -> RevertHandle {
    let stop = Arc::new(AtomicU8::new(0));
    let submitted = Arc::new(AtomicU64::new(0));
    let rounds: Arc<Mutex<Vec<RevertRound>>> = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = JoinSet::new();

    info!(
        "Chaos revert workload starting: {} emitters, {} increments then a revert of {} per \
         round, {:?} between rounds",
        config.agents,
        config.increments_per_round,
        config.revert_invocations,
        config.interval()
    );

    for index in 0..config.agents {
        let ctx = ctx.clone();
        let stop = stop.clone();
        let submitted = submitted.clone();
        let rounds = rounds.clone();
        let config = config.clone();

        tasks.spawn(async move {
            let agent = ctx.agent_name(Stream::Revert, index);
            let mut round: u32 = 0;
            // Index into `rounds` of the round still waiting to be judged by
            // the next increment this agent runs.
            let mut pending: Option<usize> = None;

            while stop.load(Ordering::Relaxed) == 0 {
                // ── Increments ──────────────────────────────────────────────
                let mut last_value = None;
                let mut all_answered = true;
                for step in 0..config.increments_per_round {
                    if stop.load(Ordering::Relaxed) != 0 {
                        break;
                    }
                    let seq = round as u64 * config.increments_per_round as u64 + step as u64;
                    submitted.fetch_add(1, Ordering::Relaxed);
                    let value = increment(&ctx, &agent, seq).await;

                    // The first increment of a round is also the probe that
                    // says what the previous round's revert left behind.
                    if let Some(slot) = pending.take()
                        && let Some(observed) = value
                        && let Ok(mut rounds) = rounds.lock()
                        && let Some(entry) = rounds.get_mut(slot)
                    {
                        entry.observed_after = Some(observed.saturating_sub(1));
                    }

                    match value {
                        Some(v) => last_value = Some(v),
                        None => all_answered = false,
                    }
                }
                if stop.load(Ordering::Relaxed) != 0 {
                    break;
                }

                // ── Revert ──────────────────────────────────────────────────
                let submitted_at = Utc::now();
                submitted.fetch_add(1, Ordering::Relaxed);
                let outcome = revert_once(&ctx, &agent, round, &config).await;

                if let Ok(mut rounds) = rounds.lock() {
                    rounds.push(RevertRound {
                        agent: agent.clone(),
                        round,
                        before_revert: all_answered.then_some(last_value).flatten(),
                        asked_to_revert: config.revert_invocations,
                        outcome,
                        submitted_at,
                        completed_at: Some(Utc::now()),
                        observed_after: None,
                    });
                    pending = Some(rounds.len() - 1);
                }

                round += 1;
                if stop.load(Ordering::Relaxed) == 0 {
                    tokio::time::sleep(config.interval()).await;
                }
            }
        });
    }

    RevertHandle {
        stop,
        tasks,
        submitted,
        rounds,
    }
}

/// One increment, returning the counter value it reported.
async fn increment(ctx: &WorkloadContext, agent: &str, seq: u64) -> Option<u64> {
    let key = ctx.idempotency_key(agent, seq);
    workload::increment_counter(ctx, Stream::Revert, agent, key)
        .await
        .value
        .map(u64::from)
}

/// One revert, with retries switched off. See the module docs.
async fn revert_once(
    ctx: &WorkloadContext,
    agent: &str,
    round: u32,
    config: &RevertConfig,
) -> Outcome {
    let mut once = ctx.clone();
    once.retry = RetryPolicy {
        transport_only: true,
        max_retries: 0,
        delay_secs: 0,
    };

    let key = format!("{agent}-revert-{round:08}");
    let agent_id = workload::counter_agent_id(&once, agent);
    let number_of_invocations = config.revert_invocations;
    let ctx2 = once.clone();

    workload::run_operation(
        &once,
        Stream::Revert,
        agent.to_string(),
        "revert",
        key,
        |_| {
            let ctx = ctx2.clone();
            let agent_id = agent_id.clone();
            async move {
                ctx.user
                    .revert(
                        &agent_id,
                        RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                            number_of_invocations: number_of_invocations as u64,
                        }),
                    )
                    .await?;
                Ok(None)
            }
        },
    )
    .await
    .outcome
}

/// The value a completed run of `rounds` rounds should leave on an agent whose
/// every round landed.
pub fn expected_after(config: &RevertConfig, rounds: u32) -> u64 {
    config.net_per_round() as u64 * rounds as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn config() -> RevertConfig {
        RevertConfig {
            agents: 200,
            increments_per_round: 4,
            revert_invocations: 2,
            interval_millis: 500,
            recovery_budget_secs: 60,
        }
    }

    /// The arithmetic the whole oracle rests on: a round that lands is worth
    /// exactly its increments less what the revert took back.
    #[test]
    fn a_completed_round_is_worth_its_increments_less_the_revert() {
        assert_eq!(config().net_per_round(), 2);
        assert_eq!(expected_after(&config(), 0), 0);
        assert_eq!(expected_after(&config(), 10), 20);
    }

    /// A revert that takes back everything it added is legal and leaves the
    /// agent where it started. Nothing in the arithmetic may go negative.
    #[test]
    fn a_round_that_reverts_everything_it_added_is_worth_nothing() {
        let config = RevertConfig {
            increments_per_round: 3,
            revert_invocations: 3,
            ..config()
        };
        assert_eq!(config.net_per_round(), 0);
        assert_eq!(expected_after(&config, 100), 0);
    }
}

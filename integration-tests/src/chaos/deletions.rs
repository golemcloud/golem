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

//! Agents that are built up and then deleted outright (GOL-372).
//!
//! One emitter per agent slot, running rounds. A round is
//! `increments_per_round` increments followed by one `delete`. Then the slot is
//! used again: invoking a deleted agent id creates a **new** agent, so the next
//! round's first increment returns `1` if the deletion took and `V + 1` if the
//! old agent is still there.
//!
//! That is the same probe [`crate::chaos::reverts`] uses and it works for the
//! same reason: the round's last increment reports the value, so there are
//! exactly two legal answers afterwards and nothing between them.
//!
//! ### The delete must never be retried
//!
//! `delete_worker_internal` in the executor starts with `get_latest_metadata`
//! and returns `worker_not_found` when there is nothing there. So deleting an
//! agent twice does **not** return success twice: the second call is an error.
//!
//! A delete whose response was lost but which actually landed would therefore,
//! on retry, come back as a refusal — and the driver would record "the platform
//! refused, and the agent is gone anyway", which is one of the violations this
//! scenario exists to detect. The retry is switched off for the delete alone,
//! exactly as it is for a revert, and left on for the increments around it.
//!
//! ### What the kill is aimed at
//!
//! Deleting is four steps: interrupt the running worker, `start_deleting`,
//! remove it from the worker service, remove it from the active set. Only the
//! third is durable. `start_deleting` exists to stop a background status flush
//! from — in the executor's own words — "resurrecting the cached status" after
//! the removal, so the happy path is already defended against exactly the thing
//! this scenario is named for. The question is whether that defence survives the
//! pod dying between the mark and the removal.

use crate::chaos::history::{Outcome, Stream};
use crate::chaos::workload::{self, WorkloadContext};
use crate::chaos::{DeleteConfig, RetryPolicy};
use chrono::{DateTime, Utc};
use golem_test_framework::dsl::TestDsl;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::task::JoinSet;
use tracing::info;

/// The counter a freshly created agent carries.
///
/// Named rather than written as `0` at the comparison, because it is the whole
/// definition of "the deletion took": an agent id that was deleted and then
/// invoked again is a *new* agent, and a new counter starts from nothing.
pub const COUNTER_OF_A_NEW_AGENT: u64 = 0;

/// One round, as the driver observed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRound {
    pub agent: String,
    pub round: u32,
    /// The value the round's last increment returned, so what the agent was
    /// worth immediately before it was deleted. `None` when an increment did
    /// not answer, which leaves the round unjudgeable rather than failed.
    pub before_delete: Option<u64>,
    /// What the delete call itself returned.
    pub outcome: Outcome,
    pub submitted_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Whether the platform refused this delete by saying the agent was not
    /// there, as opposed to refusing it for any other reason.
    ///
    /// Load-bearing rather than diagnostic. Deleting is not idempotent, and
    /// worker-service retries a call whose executor became unreachable — so a
    /// delete that *succeeded* on a pod that then died comes back to the caller
    /// as not-found. Without this flag that reads as "the platform refused and
    /// the agent is gone anyway", which is one of this scenario's violations.
    #[serde(default)]
    pub rejected_as_not_found: bool,
    /// The counter the deletion left behind: `0` if the id came back as a new
    /// agent, `before_delete` if the old one is still there.
    ///
    /// One meaning, whichever probe produced it, which is the point. A round is
    /// normally probed by the *next* round's first increment, and an increment
    /// reports the counter it just raised — so the value it leaves behind is
    /// that minus one. The last round of a run has no increment after it and is
    /// closed by a plain read instead, which reports the counter directly.
    /// Storing the increment's own return would make those two probes disagree
    /// by one, and only ever on the last round of each slot. That is exactly
    /// the bug the first S6 run reported 125 times.
    pub observed_after: Option<u64>,
}

/// The counter an operation left behind, given what the *next* increment
/// reported.
///
/// One line, and it earns a name because getting it wrong is invisible. The
/// account judges `observed_after` as the counter a delete left behind, and the
/// two probes that produce it disagree by one: an increment reports the counter
/// it just raised, a plain read reports the counter itself. The first S6 run
/// stored the increment's own return and reported 125 partial-state findings —
/// exactly the slots whose last round was closed by a read.
pub fn counter_left_by(increment_returned: u64) -> u64 {
    increment_returned.saturating_sub(1)
}

/// A running deletion workload.
pub struct DeleteHandle {
    stop: Arc<AtomicU8>,
    tasks: JoinSet<()>,
    submitted: Arc<AtomicU64>,
    rounds: Arc<Mutex<Vec<DeleteRound>>>,
}

impl DeleteHandle {
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }

    pub fn rounds(&self) -> Vec<DeleteRound> {
        self.rounds.lock().map(|r| r.clone()).unwrap_or_default()
    }

    pub async fn stop(mut self) -> Vec<DeleteRound> {
        self.stop.store(1, Ordering::Relaxed);
        while self.tasks.join_next().await.is_some() {}
        let rounds = self.rounds();
        info!(
            "Chaos delete workload stopped after {} operations across {} rounds",
            self.submitted(),
            rounds.len()
        );
        rounds
    }
}

/// The agent slots a run of `count` emitters drives, in index order.
pub fn agent_names(ctx: &WorkloadContext, count: u32) -> Vec<String> {
    (0..count)
        .map(|index| ctx.agent_name(Stream::Delete, index))
        .collect()
}

/// Builds one agent up, deletes it, and checks it came back new.
///
/// Run once before the baseline, against a throwaway id. It exists because the
/// first S11 run did not have its equivalent: a scenario whose premise is wrong
/// spends its whole baseline before the numbers say so, and the maintenance
/// window is gone. If deletion does not behave the way this whole account
/// assumes, this fails in seconds with the platform's own error.
pub async fn smoke_round(ctx: &WorkloadContext, config: &DeleteConfig) -> anyhow::Result<()> {
    let agent = format!("{}-delete-smoke", ctx.key_prefix);
    let mut value = 0;
    for step in 0..config.increments_per_round.max(1) {
        value = workload::increment_counter(
            ctx,
            Stream::Delete,
            &agent,
            ctx.idempotency_key(&agent, step as u64),
        )
        .await
        .value
        .map(u64::from)
        .ok_or_else(|| {
            anyhow::anyhow!("smoke round: increment {step} on {agent} did not answer")
        })?;
    }

    let agent_id = workload::counter_agent_id(ctx, &agent);
    ctx.user
        .delete_worker(&agent_id)
        .await
        .map_err(|e| anyhow::anyhow!("smoke round: deleting {agent} failed: {e:#}"))?;

    let after = workload::increment_counter(
        ctx,
        Stream::Delete,
        &agent,
        ctx.idempotency_key(&agent, u64::from(config.increments_per_round) + 1),
    )
    .await
    .value
    .map(u64::from)
    .ok_or_else(|| anyhow::anyhow!("smoke round: {agent} did not answer after being deleted"))?;

    if after != COUNTER_OF_A_NEW_AGENT + 1 {
        anyhow::bail!(
            "smoke round: {agent} was worth {value}, was deleted, and its next increment \
             returned {after} rather than {}. Deleting an agent does not behave the way this \
             scenario's whole account assumes, so the run would report a resurrection on every \
             round.",
            COUNTER_OF_A_NEW_AGENT + 1
        );
    }
    info!("S6: smoke round passed — a deleted agent came back as a new one");
    Ok(())
}

/// Starts one emitter per agent slot.
pub fn start(ctx: WorkloadContext, config: &DeleteConfig) -> DeleteHandle {
    let stop = Arc::new(AtomicU8::new(0));
    let submitted = Arc::new(AtomicU64::new(0));
    let rounds: Arc<Mutex<Vec<DeleteRound>>> = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = JoinSet::new();

    info!(
        "Chaos delete workload starting: {} emitters, {} increments then a delete per round, \
         {:?} between rounds",
        config.agents,
        config.increments_per_round,
        config.interval()
    );

    for index in 0..config.agents {
        let ctx = ctx.clone();
        let stop = stop.clone();
        let submitted = submitted.clone();
        let rounds = rounds.clone();
        let config = config.clone();

        tasks.spawn(async move {
            let agent = ctx.agent_name(Stream::Delete, index);
            let mut round: u32 = 0;
            let mut pending: Option<usize> = None;

            while stop.load(Ordering::Relaxed) == 0 {
                let mut last_value = None;
                let mut all_answered = true;
                for step in 0..config.increments_per_round {
                    if stop.load(Ordering::Relaxed) != 0 {
                        break;
                    }
                    let seq = round as u64 * config.increments_per_round as u64 + step as u64;
                    submitted.fetch_add(1, Ordering::Relaxed);
                    let value = workload::increment_counter(
                        &ctx,
                        Stream::Delete,
                        &agent,
                        ctx.idempotency_key(&agent, seq),
                    )
                    .await
                    .value
                    .map(u64::from);

                    // The first increment of a round says what the previous
                    // round's delete left behind.
                    if let Some(slot) = pending.take()
                        && let Some(observed) = value
                        && let Ok(mut rounds) = rounds.lock()
                        && let Some(entry) = rounds.get_mut(slot)
                    {
                        entry.observed_after = Some(counter_left_by(observed));
                    }

                    match value {
                        Some(v) => last_value = Some(v),
                        None => all_answered = false,
                    }
                }
                if stop.load(Ordering::Relaxed) != 0 {
                    break;
                }

                let submitted_at = Utc::now();
                submitted.fetch_add(1, Ordering::Relaxed);
                let (outcome, rejected_as_not_found) = delete_once(&ctx, &agent, round).await;

                if let Ok(mut rounds) = rounds.lock() {
                    rounds.push(DeleteRound {
                        agent: agent.clone(),
                        round,
                        before_delete: all_answered.then_some(last_value).flatten(),
                        outcome,
                        rejected_as_not_found,
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

    DeleteHandle {
        stop,
        tasks,
        submitted,
        rounds,
    }
}

/// The error code the platform returns when there is nothing to delete.
const AGENT_NOT_FOUND: &str = "AGENT_NOT_FOUND";

/// One delete, with retries switched off. See the module docs.
///
/// Returns the outcome and whether a refusal was specifically "no such agent",
/// which is the signature of a delete that had already taken effect.
async fn delete_once(ctx: &WorkloadContext, agent: &str, round: u32) -> (Outcome, bool) {
    let mut once = ctx.clone();
    once.retry = RetryPolicy {
        transport_only: true,
        max_retries: 0,
        delay_secs: 0,
    };

    let key = format!("{agent}-delete-{round:08}");
    let agent_id = workload::counter_agent_id(&once, agent);
    let ctx2 = once.clone();

    let result = workload::run_operation(
        &once,
        Stream::Delete,
        agent.to_string(),
        "delete",
        key,
        |_| {
            let ctx = ctx2.clone();
            let agent_id = agent_id.clone();
            async move {
                ctx.user.delete_worker(&agent_id).await?;
                Ok(None)
            }
        },
    )
    .await;

    let not_found = result
        .error
        .as_deref()
        .is_some_and(|e| e.contains(AGENT_NOT_FOUND));
    (result.outcome, not_found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    /// The conversion the first S6 run got wrong, and which no test covered
    /// because it lived in a spawned task rather than in the account.
    ///
    /// `observed_after` means the counter the delete left behind. An increment
    /// reports the counter it just raised, so it is one more than that; a plain
    /// read reports the counter directly and needs no conversion. Both probes
    /// have to land on the same scale or every slot's last round — the only one
    /// closed by a read — reads as a partial state.
    #[test]
    fn the_two_probes_agree_on_what_a_delete_left_behind() {
        // A deleted agent: the next increment creates it and reports 1.
        assert_eq!(counter_left_by(1), COUNTER_OF_A_NEW_AGENT);
        // A survivor worth 3: the next increment reports 4.
        assert_eq!(counter_left_by(4), 3);
        // A plain read of the same two cases reports 0 and 3 with no
        // conversion, which is what `close_last_rounds` stores.
        for counter in [COUNTER_OF_A_NEW_AGENT, 3] {
            assert_eq!(counter_left_by(counter + 1), counter);
        }
    }

    /// An increment that somehow reported nothing must not underflow into a
    /// value the account would read as a survivor.
    #[test]
    fn a_zero_increment_does_not_wrap_around() {
        assert_eq!(counter_left_by(0), 0);
    }
}

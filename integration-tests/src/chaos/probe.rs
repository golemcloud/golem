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

//! Asking the platform what it finally did with a key (GOL-366, GOL-364).
//!
//! `Worker::invoke_internal` looks an idempotency key's result up before
//! enqueuing anything, so re-invoking under a key that already ran replays the
//! stored result instead of running again. That turns two questions that are
//! otherwise unanswerable from the client side into direct observations:
//!
//! - *Did accepted work end up with a final result?* The probe returns one, or
//!   it does not.
//! - *Did anything run twice?* A key whose probe returns a **different** value
//!   from the one the driver was given executed a second time.
//!
//! Shared by every scenario with an exactly-once claim to make. S8 uses it on
//! its pinned population; S1 uses it on the durable stream, because "two
//! executors owned one shard" is not directly observable from outside the
//! cluster but *its consequence* — one key executing twice — is.
//!
//! Reading the durable counters immediately before and after the pass bounds
//! the probe's own footprint: the delta is exactly how many keys had never run,
//! which is what makes "replayed a stored result" and "executed fresh work"
//! distinguishable in aggregate.
//!
//! ### The pass is bounded, and says so when it runs into a bound
//!
//! A pass covers every key a run accepted, which is six figures, so it is the
//! one place where a single unresponsive agent can outlast the whole job. It
//! ends within [`PROBE_BUDGET`] either way, and every key it decided not to ask
//! about comes back as a [`SkipReason`] rather than going missing. That keeps a
//! shortened pass legible: the verdict is computed over a population with a
//! named gap in it, instead of over a silently smaller one.

use crate::chaos::errors::ErrorClass;
use crate::chaos::history::{OperationRecord, Stream};
use crate::chaos::workload::{self, WorkloadContext};
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Agent type every probed stream lands on.
const COUNTER_AGENT: &str = "Counter";

/// The method a probe invokes.
///
/// For a key that already ran this is irrelevant — the stored result comes back
/// and the arguments are never looked at. For a key that never ran it executes,
/// and `sleep_and_increment(0)` is exactly `increment`, which is what both the
/// durable and the pinned streams do. One method covers both, and the fresh
/// execution still shows up in the counter delta.
const PROBE_METHOD: &str = "sleep_and_increment";
const PROBE_MILLIS: u32 = 0;

/// How many probes run at once. A probe pass is thousands of replays, so it
/// wants some concurrency — but it runs against a cluster that has just been
/// through a fault, and hammering it would be a second experiment nobody asked
/// for.
///
/// A live ceiling, not a batch size. The pass used to join a whole batch before
/// starting the next one, which let one key that took [`PROBE_TIMEOUT`] hold the
/// other thirty-one slots idle behind it; a run where one agent stopped
/// answering spent hours replaying keys that came back in milliseconds.
const PROBE_CONCURRENCY: usize = 32;

/// Ceiling on a single probe invocation.
///
/// A probe replays a stored result, which is fast — but it runs against agents
/// a fault has just been through, and one of them refusing to answer must cost
/// one key rather than the run. A quota lease lost mid-fault parks the agent's
/// next reservation with no timeout on the platform side, so an unbounded probe
/// against it never returns.
///
/// A timed-out probe is *inconclusive*, never a missing result: the driver could
/// not ask, which says nothing about whether the platform holds the answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// How many timed-out probes an agent gets before the pass stops asking it.
///
/// A wedged agent holds every one of its keys for [`PROBE_TIMEOUT`], twice over
/// once the same-key retry has had its turn, and a scenario hands each agent
/// hundreds of keys. Past the second refusal there is nothing left to learn and
/// the only thing still being spent is the run's remaining time.
///
/// Two rather than one because [`probe_one`] already burns its retry before it
/// reports a timeout, so a strike is an agent that ignored two requests, not one
/// dropped connection.
const AGENT_STRIKE_LIMIT: u32 = 2;

/// Wall-clock ceiling on a whole pass.
///
/// A healthy pass over a hundred thousand keys finishes in a few minutes at
/// [`PROBE_CONCURRENCY`], so this leaves several times the room it needs. What
/// it is really guarding is the job timeout: a pass that runs into that takes
/// the whole run down and writes no result at all, which is strictly worse than
/// a verdict over a population with a named gap in it.
///
/// Soft by up to one [`PROBE_TIMEOUT`] and a retry, because probes already in
/// flight when the budget runs out are waited for rather than cancelled.
const PROBE_BUDGET: Duration = Duration::from_secs(900);

/// How often to report progress through a probe pass.
///
/// A pass covers tens of thousands of keys and logs nothing between its opening
/// line and its verdict, which makes "slow" and "wedged" look identical from a
/// job log. They are not the same and the difference is worth a line every few
/// hundred keys.
const PROBE_PROGRESS_EVERY: usize = 2_000;

/// What the platform said when a key was asked about again after recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyProbe {
    pub idempotency_key: String,
    pub agent: String,
    /// The value the platform returned. `None` means the probe itself failed,
    /// which for an accepted operation is the scenario's hard failure: after
    /// recovery, work the platform took has to have a result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_value: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<ErrorClass>,
    /// Set when the pass declined to ask about this key at all. Distinct from
    /// an ordinary failed probe: the exchange did not fail, it never happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<SkipReason>,
}

/// Why a pass declined to ask about a key.
///
/// Both reasons mean the same thing for the verdict — the driver did not ask,
/// so the key is inconclusive — and different things for whoever reads the run.
/// One names a platform problem, the other names a pass that was too slow to
/// finish; keeping them apart is the difference between chasing an agent and
/// chasing a budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    /// The key's agent had already left [`AGENT_STRIKE_LIMIT`] probes
    /// unanswered.
    AgentUnresponsive,
    /// The pass ran out of [`PROBE_BUDGET`] before it reached this key.
    BudgetExhausted,
}

impl SkipReason {
    /// The phrase a report uses when it names a group of skipped keys.
    pub fn describe(self) -> String {
        match self {
            SkipReason::AgentUnresponsive => format!(
                "their agents each left {AGENT_STRIKE_LIMIT} probes unanswered, so the pass \
                 stopped asking them"
            ),
            SkipReason::BudgetExhausted => {
                format!("the pass ran out of its {PROBE_BUDGET:?} budget before reaching them")
            }
        }
    }

    /// The per-key line stored in the artifact.
    fn detail(self, agent: &str) -> String {
        match self {
            SkipReason::AgentUnresponsive => {
                format!("not asked: agent {agent} left {AGENT_STRIKE_LIMIT} probes unanswered")
            }
            SkipReason::BudgetExhausted => {
                format!("not asked: the probe pass exhausted its {PROBE_BUDGET:?} budget")
            }
        }
    }
}

/// Re-invokes every recorded key under its original idempotency key.
///
/// Read the module documentation before changing anything here: the pass only
/// means what it means because the key is reused verbatim and because the
/// counters are read on either side of it.
///
/// Slots are kept full rather than refilled in batches, and the two ways a pass
/// gives up early — an agent that stopped answering, the budget running out —
/// both produce a [`SkipReason`] against the key instead of dropping it.
pub async fn probe_keys(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    stream: Stream,
) -> Vec<KeyProbe> {
    let keys: Vec<(String, String)> = records
        .iter()
        .filter(|r| r.stream == stream)
        .map(|r| (r.idempotency_key.clone(), r.agent.clone()))
        .collect();

    let total = keys.len();
    info!("Chaos: probing {total} {stream} keys for their final results");

    let deadline = Instant::now() + PROBE_BUDGET;
    let mut queue = keys.into_iter();
    let mut running: JoinSet<ProbeOutcome> = JoinSet::new();
    let mut strikes: BTreeMap<String, u32> = BTreeMap::new();
    let mut skipped: BTreeMap<SkipReason, usize> = BTreeMap::new();
    let mut probes = Vec::with_capacity(total);
    let mut next_report = PROBE_PROGRESS_EVERY;

    loop {
        while running.len() < PROBE_CONCURRENCY {
            let Some((key, agent)) = queue.next() else {
                break;
            };
            match next_step(&strikes, &agent, Instant::now() >= deadline) {
                Step::Ask => {
                    let ctx = ctx.clone();
                    running.spawn(async move { probe_one(&ctx, &agent, &key).await });
                }
                Step::Skip(reason) => {
                    *skipped.entry(reason).or_default() += 1;
                    probes.push(skipped_probe(key, agent, reason));
                }
            }
        }

        // Empty only once the queue is drained, since the fill above runs first.
        let Some(joined) = running.join_next().await else {
            break;
        };
        match joined {
            Ok(outcome) => {
                if outcome.timed_out {
                    let strikes = strikes.entry(outcome.probe.agent.clone()).or_default();
                    *strikes += 1;
                    if *strikes == AGENT_STRIKE_LIMIT {
                        warn!(
                            "Chaos: agent {} left {AGENT_STRIKE_LIMIT} probes unanswered — \
                             leaving its remaining keys inconclusive rather than spending \
                             {PROBE_TIMEOUT:?} apiece on them",
                            outcome.probe.agent
                        );
                    }
                }
                probes.push(outcome.probe);
            }
            Err(e) => warn!("Chaos: a probe task panicked: {e}"),
        }

        if probes.len() >= next_report {
            info!("Chaos: probed {} of {total} {stream} keys", probes.len());
            // Counted from here rather than stepped, so a pass that skips a
            // whole agent's keys at once does not then emit a line per join
            // catching up with the thresholds it jumped over.
            next_report = probes.len() + PROBE_PROGRESS_EVERY;
        }
    }

    for (reason, count) in &skipped {
        warn!(
            "Chaos: left {count} of {total} {stream} keys unasked — {}",
            reason.describe()
        );
    }

    probes.sort_by(|a, b| a.idempotency_key.cmp(&b.idempotency_key));
    probes
}

/// What a pass should do with the next key it takes off the queue.
///
/// Split out and kept pure so the two conditions that shorten a pass can be
/// tested without a cluster to wedge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Ask,
    Skip(SkipReason),
}

/// The budget is checked first: once a pass is out of time it stops asking
/// about everything, healthy agents included.
fn next_step(strikes: &BTreeMap<String, u32>, agent: &str, out_of_budget: bool) -> Step {
    if out_of_budget {
        return Step::Skip(SkipReason::BudgetExhausted);
    }
    if strikes.get(agent).copied().unwrap_or(0) >= AGENT_STRIKE_LIMIT {
        return Step::Skip(SkipReason::AgentUnresponsive);
    }
    Step::Ask
}

/// Records a key the pass never asked about.
///
/// Carries a transport class deliberately. Nothing was refused, so this must not
/// read as a definite answer: it lands in the exactly-once account as
/// *inconclusive*, which is what "the driver could not ask" has always meant
/// there, and [`KeyProbe::skipped`] says why this one was never even attempted.
fn skipped_probe(key: String, agent: String, reason: SkipReason) -> KeyProbe {
    let error = reason.detail(&agent);
    KeyProbe {
        idempotency_key: key,
        agent,
        final_value: None,
        error: Some(error),
        error_class: Some(ErrorClass::Transport),
        skipped: Some(reason),
    }
}

/// A finished probe, plus the one fact the pass needs that the result itself
/// does not carry: whether the last attempt ran out of time rather than
/// answering. A timeout is the only outcome that costs the pass real
/// wall-clock, so it is the only one that counts against an agent.
struct ProbeOutcome {
    probe: KeyProbe,
    timed_out: bool,
}

/// Probes one key, retrying once on a transport failure.
///
/// The retry follows the same rule as the workload itself — one attempt, same
/// idempotency key, transport failures only — and here it is purely about not
/// losing a key to a single dropped connection. A probe that cannot complete
/// leaves the key inconclusive, and an inconclusive key weakens the verdict; a
/// cheap same-key retry buys most of them back. It cannot mask a real problem,
/// because a definite refusal is not retried.
async fn probe_one(ctx: &WorkloadContext, agent: &str, key: &str) -> ProbeOutcome {
    let parsed: ParsedAgentId = agent_id!(COUNTER_AGENT, agent.to_string());
    let idempotency_key = golem_common::model::IdempotencyKey::new(key.to_string());

    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let invoke = ctx.user.invoke_and_await_agent_with_key(
            &ctx.counters,
            &parsed,
            &idempotency_key,
            PROBE_METHOD,
            data_value!(PROBE_MILLIS),
        );
        let mut timed_out = false;
        let outcome = match tokio::time::timeout(PROBE_TIMEOUT, invoke).await {
            Ok(outcome) => outcome,
            // Treated exactly as a transport failure: retried once under the
            // same key, then left inconclusive. An agent that will not answer
            // tells the driver nothing about what the platform holds.
            Err(_) => {
                timed_out = true;
                Err(anyhow::anyhow!("probe timed out after {PROBE_TIMEOUT:?}"))
            }
        };

        match outcome {
            Ok(value) => {
                return ProbeOutcome {
                    probe: KeyProbe {
                        idempotency_key: key.to_string(),
                        agent: agent.to_string(),
                        final_value: workload::as_u32_value(value),
                        error: None,
                        error_class: None,
                        skipped: None,
                    },
                    timed_out: false,
                };
            }
            Err(e) => {
                let class = crate::chaos::errors::classify(&e);
                if attempts <= ctx.retry.max_retries && class.is_retryable_transport_failure() {
                    tokio::time::sleep(ctx.retry.delay()).await;
                    continue;
                }
                if !class.is_definite_rejection() {
                    warn!(
                        "Chaos: probe of {key} could not complete ({class}), leaving the key \
                         inconclusive rather than reporting a lost result: {e:#}"
                    );
                }
                return ProbeOutcome {
                    probe: KeyProbe {
                        idempotency_key: key.to_string(),
                        agent: agent.to_string(),
                        final_value: None,
                        error: Some(format!("{e:#}")),
                        error_class: Some(class),
                        skipped: None,
                    },
                    timed_out,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn strikes(pairs: &[(&str, u32)]) -> BTreeMap<String, u32> {
        pairs
            .iter()
            .map(|(agent, count)| ((*agent).to_string(), *count))
            .collect()
    }

    #[test]
    fn an_agent_under_the_strike_limit_is_still_asked() {
        let seen = strikes(&[("a-1", AGENT_STRIKE_LIMIT - 1)]);
        assert_eq!(next_step(&seen, "a-1", false), Step::Ask);
    }

    #[test]
    fn an_agent_at_the_strike_limit_is_not_asked_again() {
        let seen = strikes(&[("a-1", AGENT_STRIKE_LIMIT)]);
        assert_eq!(
            next_step(&seen, "a-1", false),
            Step::Skip(SkipReason::AgentUnresponsive)
        );
    }

    /// The breaker is per agent, which is the whole point of it: one wedged
    /// agent must not cost the other two hundred their keys.
    #[test]
    fn one_agents_strikes_do_not_stop_another_being_asked() {
        let seen = strikes(&[("a-1", AGENT_STRIKE_LIMIT + 3)]);
        assert_eq!(next_step(&seen, "a-2", false), Step::Ask);
    }

    #[test]
    fn running_out_of_budget_stops_the_pass_asking_anyone() {
        assert_eq!(
            next_step(&BTreeMap::new(), "a-1", true),
            Step::Skip(SkipReason::BudgetExhausted)
        );
    }

    /// Both bounds at once report the budget, because that is the one that ends
    /// the pass: an operator chasing a wedged agent when the run simply ran long
    /// would be chasing the wrong thing.
    #[test]
    fn the_budget_is_reported_ahead_of_a_wedged_agent() {
        let seen = strikes(&[("a-1", AGENT_STRIKE_LIMIT)]);
        assert_eq!(
            next_step(&seen, "a-1", true),
            Step::Skip(SkipReason::BudgetExhausted)
        );
    }

    /// A skipped key has to reach the account as *inconclusive*, never as a
    /// refusal — the platform was never asked, so it cannot have said no.
    #[test]
    fn a_skipped_key_is_inconclusive_rather_than_a_definite_answer() {
        let probe = skipped_probe(
            "k-1".to_string(),
            "a-1".to_string(),
            SkipReason::AgentUnresponsive,
        );
        assert!(probe.final_value.is_none());
        assert_eq!(probe.skipped, Some(SkipReason::AgentUnresponsive));
        assert!(!probe.error_class.unwrap().is_definite_rejection());
        assert!(probe.error.unwrap().contains("a-1"));
    }

    /// A pass that gives up must still hand back one entry per key. Anything it
    /// drops instead becomes an unexplained hole in the population the verdict
    /// is computed over.
    #[test]
    fn every_key_the_pass_gives_up_on_still_comes_back() {
        let keys = ["k-1", "k-2", "k-3"];
        let probes: Vec<KeyProbe> = keys
            .iter()
            .map(|key| {
                skipped_probe(
                    (*key).to_string(),
                    "a-1".to_string(),
                    SkipReason::BudgetExhausted,
                )
            })
            .collect();
        assert_eq!(probes.len(), keys.len());
        assert!(
            probes
                .iter()
                .all(|p| p.skipped == Some(SkipReason::BudgetExhausted))
        );
    }
}

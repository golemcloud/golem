// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! S888 — quota agent cold start. **Prototype, not part of the suite's claims.**
//!
//! This is a measurement, not a chaos scenario. Nothing is injected, nothing is
//! scaled, no pod is touched: two executors, a steady cluster, and a ramp of
//! cold agent populations. The only question it asks is how long the *first*
//! invocation on an agent holding a quota token takes to return.
//!
//! ## Why it exists
//!
//! S1 found this by accident. On a healthy cluster, five minutes before any
//! fault, 29 of 30 quota agents blocked for ~85s on their first reservation
//! while agent 0000 answered in ~1.2s. Both clocks agreed: the driver measured
//! 84.9s of wall time around a single `invoke_and_await`, and the executor-side
//! `invocation` span measured 83s of which ~10ms was accounted work — including
//! a `BatchRenewQuotaLease` that succeeded in 4.55ms. So the lease was live and
//! the shard-manager was responsive while the caller waited over a minute.
//!
//! Buried inside S1 that finding is hard to argue about: there is a partition,
//! a scale schedule and four other streams in the way. Here there is nothing
//! else, so a number is a number.
//!
//! ## What it does
//!
//! For each population in [`ROUNDS`], create that many **fresh** agents and
//! invoke `reserve_and_increment` exactly once on each, all at the same time.
//! Every agent is new in every round — index ranges do not overlap — so every
//! measurement is a genuine cold start rather than a warm re-entry.
//!
//! Percentiles are reported per round, and the raw per-agent records are in the
//! operation history. Nothing here fails a run: it reports.
//!
//! ## Reading the result
//!
//! The interesting shape is not the median, it is the spread. One fast agent
//! and a long flat tail at the same value is what the S1 data looked like, and
//! it points at a queue released in a single pass rather than at load. A ramp
//! that degrades smoothly with population would point somewhere else entirely.

use crate::chaos::history::{OperationHistory, Outcome, Stream};
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{OutputPaths, ScenarioOutcome, build_result, write_outputs};
use crate::chaos::signal::{BaselineReady, FaultSignals};
use crate::chaos::summary::{ChaosSummary, TerminationReason};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::collections::BTreeMap;
use tokio::task::JoinSet;
use tracing::info;

/// Agent populations to cold-start, in order.
///
/// Each round is a separate population of brand-new agents, so the ramp
/// measures how cold start behaves as the number of agents arriving at once
/// grows — not how one population behaves under repeated load.
const ROUNDS: [u32; 4] = [50, 100, 200, 500];

/// Index space reserved per round, so no two rounds can ever name the same
/// agent. Must exceed the largest entry in [`ROUNDS`].
const ROUND_INDEX_STRIDE: u32 = 1000;

/// Pause between rounds.
///
/// Long enough for the previous round's arrival burst to finish settling, so a
/// round measures its own cold start rather than the tail of the one before.
/// Note the previous round's agents keep holding their tokens — that is
/// deliberate, since a real deployment accumulates quota holders too.
const ROUND_SETTLE_SECS: u64 = 60;

pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let history = OperationHistory::new(ScenarioCode::S888.as_str());

    let user = manifest.user_context(deps);
    let counters = user
        .get_latest_component_revision(&manifest.counters_component_id)
        .await?;
    let promise = user
        .get_latest_component_revision(&manifest.promise_component_id)
        .await?;

    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S888);

    let ctx = WorkloadContext {
        user,
        counters,
        promise,
        history: history.clone(),
        retry: config.retry_policy.clone(),
        phase: PhaseMarker::new(crate::chaos::history::Phase::Baseline),
        key_prefix: key_prefix.clone(),
    };

    let scope = RunScope {
        environment_id: manifest.environment_id.0.to_string(),
        component_ids: vec![manifest.counters_component_id.0.to_string()],
        agent_id_prefix: key_prefix.clone(),
        idempotency_key_prefix: key_prefix.clone(),
    };

    // Nothing is injected, but the workflow still waits for this before it
    // moves on. Say ready immediately so a prototype run does not spend its
    // signal timeout doing nothing.
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S888.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations: 0,
        fault_target: None,
    })?;

    let mut attention: Vec<String> = vec![
        "S888 is a prototype measurement, not an assertion. Nothing is injected \
         and nothing fails the run."
            .to_string(),
    ];

    for (round, &population) in ROUNDS.iter().enumerate() {
        if round > 0 {
            info!("S888: settling {ROUND_SETTLE_SECS}s before the next round");
            tokio::time::sleep(std::time::Duration::from_secs(ROUND_SETTLE_SECS)).await;
        }

        let base_index = round as u32 * ROUND_INDEX_STRIDE;
        info!(
            "S888: round {} — cold-starting {population} quota agents at indices {}..{}",
            round + 1,
            base_index,
            base_index + population
        );

        let before = history.len();
        let round_started = Utc::now();
        cold_start(&ctx, base_index, population).await;
        let elapsed = (Utc::now() - round_started).num_milliseconds().max(0);

        let latencies = round_latencies(&history, before);
        let line = report_round(round + 1, population, elapsed, &latencies);
        info!("{line}");
        attention.push(line);
    }

    let summary = ChaosSummary {
        total_operations: history.len() as u64,
        phases: Vec::new(),
        recovery: Vec::new(),
        readback: Vec::new(),
        streams_without_readback: Vec::new(),
        routing_snapshots: Vec::new(),
        exactly_once: None,
        ownership: Vec::new(),
        attention,
    };

    let result = build_result(
        config,
        ScenarioOutcome {
            started_at,
            phases: Phases {
                baseline: Some(PhaseWindow {
                    started_at,
                    ended_at: Some(Utc::now()),
                }),
                fault: None,
                recovery: None,
            },
            fault_injected_at: None,
            fault_recovered_at: None,
            fault_id: None,
            fault_target_observed: None,
            scope,
            summary,
            termination_reason: TerminationReason::Completed,
            pinned_selection: None,
        },
    );

    write_outputs(&result, &history, outputs)?;
    Ok(result)
}

/// Invokes `reserve_and_increment` once on `population` fresh agents, all at
/// the same time.
///
/// Simultaneous on purpose: agents arriving together is what a deploy or a
/// restart looks like, and it is the condition under which S1 saw one agent
/// answer and the rest queue.
async fn cold_start(ctx: &WorkloadContext, base_index: u32, population: u32) {
    let mut batch = JoinSet::new();
    for offset in 0..population {
        let ctx = ctx.clone();
        batch.spawn(async move {
            workload::submit_one(&ctx, Stream::Quota, base_index + offset, 1).await;
        });
    }
    while batch.join_next().await.is_some() {}
}

/// Durations of the operations this round added to the history, sorted.
fn round_latencies(history: &OperationHistory, before: usize) -> Vec<u64> {
    let mut out: Vec<u64> = history
        .snapshot()
        .into_iter()
        .skip(before)
        .filter(|r| r.outcome == Outcome::Confirmed)
        .map(|r| r.duration_ms)
        .collect();
    out.sort_unstable();
    out
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// One line per round, in the form an operator can scan without opening the
/// history. Buckets matter more than the mean here: the S1 shape was a handful
/// of fast agents and a long flat tail, which a mean would hide entirely.
fn report_round(round: usize, population: u32, elapsed_ms: i64, sorted: &[u64]) -> String {
    if sorted.is_empty() {
        return format!("S888 round {round}: {population} agents — no confirmed operations");
    }

    let mut buckets: BTreeMap<&str, usize> = BTreeMap::new();
    for &ms in sorted {
        let bucket = match ms {
            0..=999 => "<1s",
            1000..=4999 => "1-5s",
            5000..=29_999 => "5-30s",
            30_000..=59_999 => "30-60s",
            _ => ">60s",
        };
        *buckets.entry(bucket).or_default() += 1;
    }
    let spread = buckets
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        "S888 round {round}: {population} agents cold-started in {elapsed_ms}ms — \
         confirmed={} p50={}ms p90={}ms p99={}ms min={}ms max={}ms [{spread}]",
        sorted.len(),
        percentile(sorted, 0.50),
        percentile(sorted, 0.90),
        percentile(sorted, 0.99),
        sorted.first().copied().unwrap_or(0),
        sorted.last().copied().unwrap_or(0),
    )
}

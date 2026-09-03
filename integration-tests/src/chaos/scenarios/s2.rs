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

//! S2 — executor-to-executor partition, cross-pod RPC control (GOL-368).
//!
//! The suite's control. Every other scenario injects a fault it expects to hurt
//! and measures the damage; this one injects a fault it expects to be inert and
//! measures that nothing happened.
//!
//! The claim under test is architectural. `WorkerExecutorClient` appears
//! nowhere in the executor: when an agent invokes an agent its own executor does
//! not own, `DirectWorkerInvocationRpc::invoke_and_await` finds
//! `shard_service().check_worker()` says no and hands the call to
//! `worker_proxy`, which is a client of **worker-service**. So executor A
//! reaches executor B's agents through a third party, and a partition between A
//! and B cuts a link carrying no traffic.
//!
//! ### The choreography, and where it differs from S12
//!
//! 1. **Pair** — before anything runs, ask the shard-manager who owns each
//!    caller and each callee, and split the callers into the ones whose call
//!    crosses executors and the ones whose call does not. Neither the driver nor
//!    the platform chooses this; both halves are placed by hashing their agent
//!    ids, so the split is whatever the hash gives.
//! 2. **Baseline** — run the mixed workload, RPC stream included.
//! 3. **Gate** — refuse to spend the fault window unless enough callers came out
//!    cross-pod. A run whose pairs all landed together would report exactly the
//!    clean numbers a good run reports, and mean nothing. Same instinct as S9's
//!    forward-leg gate.
//! 4. **Re-pair** — check the split again immediately before injection, because
//!    an ownership change between the gate and the fault would leave the report
//!    comparing populations that no longer exist.
//! 5. **Fault, recovery, read-back** — as in S12.
//!
//! The gate is the step that makes this scenario worth running. Without it a
//! green S2 is indistinguishable from a broken S2.

use crate::chaos::history::{OperationHistory, OperationRecord, Phase, Stream};
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::probe;
use crate::chaos::relay;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ReadKind, ScenarioOutcome, build_result, read_back_agents, sample_ownership,
    signal_termination, snapshot_routing, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals};
use crate::chaos::split::FaultWindow;
use crate::chaos::summary::{AgentReadback, ChaosSummary, ExactlyOnceReport, TerminationReason};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::time::Duration;
use tracing::{info, warn};

/// How long to wait after stopping the workload before reading durable state.
/// Same reasoning as S12: an in-flight RPC still has to land on its callee, and
/// reading early would report a mismatch that says nothing about the platform.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(30);

/// Runs S2 end to end.
pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let workload_config = config.require_workload()?;
    let relay_config = config.require_relay()?;
    let history = OperationHistory::new(ScenarioCode::S2.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S2);

    let user = manifest.user_context(deps);
    let counters = user
        .get_latest_component_revision(&manifest.counters_component_id)
        .await?;
    let promise = user
        .get_latest_component_revision(&manifest.promise_component_id)
        .await?;

    let ctx = WorkloadContext {
        user,
        counters,
        promise,
        history: history.clone(),
        retry: config.retry_policy.clone(),
        phase: PhaseMarker::new(Phase::Baseline),
        key_prefix: key_prefix.clone(),
    };

    let scope = RunScope {
        environment_id: manifest.environment_id.0.to_string(),
        component_ids: vec![
            manifest.counters_component_id.0.to_string(),
            manifest.promise_component_id.0.to_string(),
        ],
        agent_id_prefix: key_prefix.clone(),
        idempotency_key_prefix: format!("{key_prefix}-"),
    };

    let callers: Vec<String> = (0..workload_config.rpc_agents)
        .map(|index| ctx.agent_name(Stream::Rpc, index))
        .collect();

    // Paired before anything runs, so a run that cannot be paired costs no
    // cluster time. Not optional: every `finish!` below attaches the pairing,
    // including an abort, because on an abort the pairing is usually the reason.
    info!(
        "S2: placing {} RPC pairs against the routing table",
        callers.len()
    );
    let mut pairing = relay::select_pairing(&ctx, deps, &callers).await?;

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut ownership_samples = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $exactly_once:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            );
            // Attached whenever the pairing exists, including on an abort. A run
            // that stopped at the gate is exactly the run whose pairing a reader
            // needs to see, since the pairing is why it stopped.
            if !ownership_samples.is_empty() {
                summary = summary.with_ownership(ownership_samples.clone());
            }
            if let Some(report) = $exactly_once {
                summary = summary.with_exactly_once(report);
            }
            {
                summary = summary.with_relay(relay::build(
                    $records,
                    pairing.clone(),
                    fault_injected_at.map(|injected_at| FaultWindow {
                        injected_at,
                        recovered_at: fault_recovered_at,
                    }),
                    relay_config.cross_pod_floor_percent,
                    relay_config.cross_pod_floor_throughput_percent,
                    relay_config.co_located_floor_throughput_percent,
                ));
            }
            let result = build_result(
                config,
                ScenarioOutcome {
                    started_at,
                    phases: phases.clone(),
                    fault_injected_at,
                    fault_recovered_at,
                    fault_id: fault_id.clone(),
                    fault_target_observed: fault_target_observed.clone(),
                    scope: scope.clone(),
                    summary,
                    termination_reason: $reason,
                    pinned_selection: None,
                    scheduled_selection: None,
                    promise_selection: None,
                    isolation_selection: None,
                    revert_selection: None,
                    delete_selection: None,
                },
            );
            write_outputs(&result, &history, outputs)?;
            return Ok(result);
        }};
    }

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S2: baseline phase, running mixed workload for {:?}",
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = workload::start(ctx.clone(), workload_config);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    ownership_samples.push(sample_ownership(deps, "before-fault", None, true).await);
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    // ── Gate ────────────────────────────────────────────────────────────────
    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S2: baseline produced no confirmed operations, aborting before injection");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Vec::new(),
            None
        );
    }

    // Re-read the split rather than trusting the one taken before the workload.
    // Ownership can move between the two, and the report is a comparison
    // between populations — one built against a routing table that has since
    // changed would compare the wrong agents.
    let confirmed = match relay::select_pairing(&ctx, deps, &callers).await {
        Ok(confirmed) => confirmed,
        Err(e) => {
            warn!("S2: could not re-read the pairing before injection: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(
                TerminationReason::PlatformUnreachable {
                    detail: format!("the routing table could not be re-read before the fault: {e}"),
                },
                &records,
                Vec::new(),
                None
            );
        }
    };
    let cross_pod_percent = confirmed.cross_pod_percent().unwrap_or(0.0);
    pairing = confirmed;

    if cross_pod_percent < relay_config.cross_pod_floor_percent {
        // The fault would have nothing to cut. Stop before spending the window:
        // the clean numbers this run would produce are indistinguishable from
        // the clean numbers a good run produces, which makes them worse than no
        // numbers at all.
        warn!(
            "S2: only {cross_pod_percent}% of callers are cross-pod, below the {}% floor",
            relay_config.cross_pod_floor_percent
        );
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!(
                    "only {cross_pod_percent}% of RPC callers had their callee on the other \
                     executor, below the {}% floor — a partition between executors would have \
                     had almost nothing to cut",
                    relay_config.cross_pod_floor_percent
                ),
            },
            &records,
            Vec::new(),
            None
        );
    }

    info!(
        "S2: baseline complete ({baseline_operations} confirmed ops, {cross_pod_percent}% \
         cross-pod), signalling readiness"
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S2.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        // Both executors are partitioned from each other, selected by label. On
        // a two-replica deployment that is every executor there is, so there is
        // no pod for the driver to name.
        fault_target: None,
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S2: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S2: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S2: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S2: fault cleared at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    info!(
        "S2: recovery phase, running for a further {:?}",
        config.phases.recovery()
    );
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    tokio::time::sleep(config.phases.recovery()).await;
    handle.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);
    ownership_samples
        .push(sample_ownership(deps, "after-recovery", ownership_samples.first(), true).await);

    // Shards must not have moved. Nothing in this fault touches the
    // shard-manager, so a reassignment means the run disturbed something it did
    // not intend to, and the two populations are no longer the ones that were
    // paired. The samples are attached to the summary rather than judged here,
    // which is how every other scenario reports ownership.

    // ── Read-back ───────────────────────────────────────────────────────────
    info!("S2: letting the platform settle for {SETTLE_BEFORE_READBACK:?} before read-back");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    let records = history.snapshot();
    let readback = read_back(&ctx, &records, workload_config).await;

    // ── Exactly-once ────────────────────────────────────────────────────────
    //
    // Accounted on the RPC stream rather than the durable one, which is the
    // only interesting choice here. The durable stream is the same population
    // every other scenario probes and this fault cannot reach it; the RPC
    // stream is the one whose work crosses the partitioned link.
    //
    // It matters most in exactly the run that finds something. A control that
    // stays green has almost no indeterminate operations to resolve, so the
    // probe finds nothing. A run where cross-pod calls stalled would be full of
    // them, and the probe is what says whether a stalled call executed anyway.
    // The read-back above is a weaker form of the same question: it compares
    // sums per agent, where this attributes to a key.
    let before_probe = read_callee_counters(&ctx, workload_config).await;
    let probes = probe::probe_keys(&ctx, &records, Stream::Rpc).await;
    let after_probe = read_callee_counters(&ctx, workload_config).await;

    let exactly_once =
        ExactlyOnceReport::build(&records, &probes, Stream::Rpc, &before_probe, &after_probe);
    info!(
        "S2: exactly-once account — {} keys checked, {} recovered by the probe, {} findings",
        exactly_once.keys_checked,
        exactly_once.keys_recovered_by_probe,
        exactly_once.findings.len()
    );

    finish!(
        TerminationReason::Completed,
        &records,
        readback,
        Some(exactly_once)
    );
}

/// The current counter of every RPC callee, keyed by the **callee**.
///
/// Keyed by the agent whose number it is rather than by the caller the probe
/// addresses, because the only thing this map feeds is
/// `ExactlyOnceReport::probe_executed_per_agent` — a list of who moved during
/// the probe pass. Naming the caller there would point an investigation at an
/// agent whose own counter never changes.
async fn read_callee_counters(
    ctx: &WorkloadContext,
    config: &crate::chaos::WorkloadConfig,
) -> std::collections::BTreeMap<String, u64> {
    let mut values = std::collections::BTreeMap::new();
    for index in 0..config.rpc_agents {
        let callee = workload::rpc_callee_name(&ctx.agent_name(Stream::Rpc, index));
        match workload::read_counter(ctx, &callee).await {
            Ok(value) => {
                values.insert(callee, value);
            }
            // Recorded as absent rather than as zero: a callee that could not be
            // read says nothing about whether the probe executed against it, and
            // a zero here would be read as "it moved backwards".
            Err(e) => warn!("S2: could not read RPC callee {callee}: {e}"),
        }
    }
    values
}

/// Reads durable state back for every stream that keeps a count.
///
/// The RPC stream is the one with an indirection: the operation is filed under
/// the caller, but the counter it advanced belongs to the callee. See
/// [`ReadKind::RpcInner`].
async fn read_back(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    config: &crate::chaos::WorkloadConfig,
) -> Vec<AgentReadback> {
    let mut agents = Vec::new();
    for index in 0..config.rpc_agents {
        agents.push((
            Stream::Rpc,
            ctx.agent_name(Stream::Rpc, index),
            ReadKind::RpcInner,
        ));
    }
    for index in 0..config.durable_agents {
        agents.push((
            Stream::Durable,
            ctx.agent_name(Stream::Durable, index),
            ReadKind::Counter,
        ));
    }
    for index in 0..config.scheduled_agents {
        agents.push((
            Stream::Scheduled,
            ctx.schedule_target_name(index),
            ReadKind::Polls,
        ));
    }
    read_back_agents(ctx, records, agents).await
}

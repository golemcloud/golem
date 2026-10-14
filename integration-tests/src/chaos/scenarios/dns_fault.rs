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

//! The shard manager's name stops resolving on one executor: S4 (GOL-373) and
//! MF2 (GOL-537).
//!
//! [`crate::chaos::resolution`] carries the argument: why the comparison is
//! across executors rather than across time, and why the same table is read two
//! opposite ways. This module is the choreography that produces it.
//!
//! ### Why the two are one module
//!
//! MF2 is S4 with the thing that makes S4 null taken away. Same fault, same
//! population, same instrument; it adds a **shard-manager restart inside the
//! DNS window**, which drops the executor's cached connection and forces it to
//! resolve a name that no longer resolves.
//!
//! That also makes the control group sharper than it is anywhere else in the
//! suite. Both executors lose the connection to the same restart — it is a
//! shared shock, not a targeted one — and the only thing that differs between
//! them is whether they can resolve the name to rebuild it. So the gap between
//! the two groups is the DNS failure with everything else held constant.
//!
//! ### What the fault can reach, which is less than the ticket assumed
//!
//! Two pieces of executor-to-shard-manager traffic exist. `register`, once at
//! startup (`golem-worker-executor/src/grpc/mod.rs`), and **quota lease renewal
//! every ten seconds** (`services/quota.rs`). That is all of it.
//!
//! The executor never calls `get_routing_table`; only worker-service does
//! (`service/worker/routing_logic.rs`). Shard ownership is pushed shard-manager
//! to executor and the health checks run in that same direction
//! (`golem-shard-manager/src/sharding/worker_executor.rs`), so the executor's
//! own resolver cannot affect either. The ticket's "without ownership loss" is
//! right, but not because ownership survives a DNS problem — it is because a
//! DNS problem here cannot get near it.
//!
//! So the quota stream is the instrument, and it is the only stream the
//! workload weights. Its doc in [`crate::chaos::history`] already says it is the
//! only stream whose traffic crosses this link.
//!
//! ### Why the control group is load-bearing here in a way it is not elsewhere
//!
//! Every scenario that aims at one executor keeps the other as a control. In S4
//! the control is not a comfort, it is the measurement: the headline number is
//! the target group's quota latency *as a percentage of the control group's, in
//! the same window*. A run whose quota agents all landed on one executor has no
//! comparison to make, so it is refused before the window rather than reported
//! after it.
//!
//! ### The choreography
//!
//! 1. **Warm up** the quota and durable agents, so the fault lands on a live
//!    population.
//! 2. **Select** the executor owning the largest share of the quota agents, and
//!    name it for the workflow.
//! 3. **Baseline** — mixed workload, long enough for several renewal cycles to
//!    have gone round undisturbed.
//! 4. **Fault** — keep driving, and sample the assignment late in the window. A
//!    DNS failure on an executor has no business moving shards, and a run where
//!    it did is a different experiment. MF2 additionally waits for the second
//!    fault and samples the assignment around it, because a shard-manager
//!    restart is the one part of this that plausibly *could* move shards.
//! 5. **Heal**, then keep driving long enough for the quota stream to settle.
//! 6. **Read back and probe** — the same completion and exactly-once oracles
//!    the rest of the suite ends with.

use crate::chaos::composed::ComposedFaultReport;
use crate::chaos::history::{OperationHistory, OperationRecord, Outcome, Phase, Stream};
use crate::chaos::ownership::OwnershipSample;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::resolution::ResolutionInputs;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ReadKind, ScenarioOutcome, WARMUP_SETTLE, build_result, exactly_once_termination,
    read_back_agents, read_counters, sample_ownership, signal_termination, snapshot_routing,
    wait_for_settled_routing, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultInjected, FaultSignals, FaultTarget};
use crate::chaos::split::{self, FaultWindow, PodSplit};
use crate::chaos::summary::{
    AgentReadback, ChaosSummary, ExactlyOnceReport, Note, TerminationReason,
};
use crate::chaos::workload::{PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig, probe, resolution, workload};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::time::Duration;
use tracing::{info, warn};

/// Where in the fault window the assignment is sampled, as a fraction of it.
///
/// Late, and for S3's reason: this sample is checking that nothing moved, and a
/// table that still looks untouched three quarters of the way in says far more
/// than one that looks untouched immediately.
const OWNERSHIP_SAMPLE_FRACTION: f64 = 0.8;

/// How long past the enclosing fault window a composed run keeps waiting for
/// the second fault before giving up on it.
///
/// Generous, and the reason is that a wait which runs out produces a *weaker*
/// report than one that catches a late signal:
/// `secondary-outside-primary` tells a reader the composition missed, where
/// `secondary-never-injected` only says nothing arrived.
const SECONDARY_WAIT_MARGIN: Duration = Duration::from_secs(120);

/// How long to let the cluster react to the shard-manager restart before
/// sampling the assignment again.
///
/// The restart is the one fault here that could plausibly move shards, and a
/// sample taken the instant it lands would describe the cluster before it had a
/// chance to. Capped against what remains of the window by the caller, so it
/// cannot fall past the heal and describe a cluster whose DNS was already back.
const RESTART_SETTLE: Duration = Duration::from_secs(60);

/// Runs S4 or MF2 end to end.
pub async fn run(
    code: ScenarioCode,
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let workload_config = config.require_workload()?;
    let resolution_config = config.require_resolution()?;
    // Only MF2 has one. `require_composed` is the loud version, used once the
    // presence of the block has already said the run intends a composition.
    let composed_config = match config.composed {
        Some(_) => Some(config.require_composed()?),
        None => None,
    };
    let history = OperationHistory::new(code.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(code);

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
        component_ids: vec![manifest.counters_component_id.0.to_string()],
        agent_id_prefix: key_prefix.clone(),
        idempotency_key_prefix: format!("{key_prefix}-"),
    };

    let quota_agents: Vec<String> = (0..workload_config.quota_agents)
        .map(|index| ctx.agent_name(Stream::Quota, index))
        .collect();

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut ownership: Vec<OwnershipSample> = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut selection: Option<PodSplit> = None;
    let mut attention_extra: Vec<Note> = Vec::new();
    let mut secondary: Option<FaultInjected> = None;
    let mut composed_report: Option<ComposedFaultReport> = None;

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $exactly_once:expr, $resolution:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            )
            .with_ownership(ownership.clone());
            summary.absorb(attention_extra.clone());
            if let Some(report) = $exactly_once {
                summary = summary.with_exactly_once(report);
            }
            if let Some(report) = $resolution {
                summary = summary.with_resolution(report);
            }
            if let Some(report) = composed_report.clone() {
                summary = summary.with_composed_fault(report);
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
                    isolation_selection: selection.clone(),
                    revert_selection: None,
                    delete_selection: None,
                },
            );
            write_outputs(&result, &history, outputs)?;
            return Ok(result);
        }};
    }

    // ── Warm-up ─────────────────────────────────────────────────────────────
    //
    // Reads, not increments, for the reason every scenario here warms up the
    // same way: an increment would be invisible to the operation history and
    // would leave every read-back off by one. A cold start also looks exactly
    // like a stall from outside, and this scenario's whole signal is a latency
    // comparison between two populations.
    routing_snapshots.push(snapshot_routing(deps, "before-warmup").await);
    attention_extra.push(wait_for_settled_routing(deps, &mut routing_snapshots).await);

    info!("{code}: warming {} quota agents", quota_agents.len());
    let warm: Vec<(Stream, String, ReadKind)> = quota_agents
        .iter()
        .map(|agent| (Stream::Quota, agent.clone(), ReadKind::QuotaCounter))
        .collect();
    let _ = read_back_agents(&ctx, &[], warm).await;
    info!("{code}: warmed, settling {WARMUP_SETTLE:?}");
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Aim ─────────────────────────────────────────────────────────────────
    //
    // On the quota agents, because a lease renewal is the only executor traffic
    // that crosses the poisoned name. The durable agents are along for the
    // exactly-once oracle and do not decide where the fault lands.
    let subject = split::quota_subject(&ctx);
    let split = match split::select(subject, deps, &quota_agents).await {
        Ok(split) => split,
        Err(e) => {
            warn!("{code}: cannot aim the DNS failure: {e:#}");
            let records = history.snapshot();
            finish!(
                TerminationReason::FaultTargetUnverified {
                    detail: format!("{e:#}"),
                },
                &records,
                Vec::new(),
                None,
                None
            );
        }
    };
    selection = Some(split.clone());

    // Refused rather than reported, because the headline number is a ratio
    // between the two groups and there is no second group to divide by. S19
    // refuses the same shape for a different reason — there, a lone skewed pod
    // rescues its own lease and the fault is inert. Here the fault would be
    // just as real and the run would still have nothing to say about it.
    if split.elsewhere.is_empty() {
        warn!("{code}: every quota agent landed on one executor, so there is no control group");
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!(
                    "all {} quota agents are owned by {}, and the measurement is the target \
                     executor's quota latency as a percentage of the other executor's over the \
                     same window, so a one-sided split leaves nothing to compare against",
                    quota_agents.len(),
                    split.pod_address
                ),
            },
            &records,
            Vec::new(),
            None,
            None
        );
    }

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "{code}: baseline phase, mixed workload for {:?}",
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let mixed = workload::start(ctx.clone(), workload_config);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    ownership.push(sample_ownership(deps, "before-fault", ownership.last(), false).await);

    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("{code}: baseline produced no confirmed operations, aborting before injection");
        mixed.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Vec::new(),
            None,
            None
        );
    }

    // A rebalance between selection and injection would leave the run naming
    // the control group as the affected one and vice versa — a report that is
    // not merely wrong but confidently inverted.
    if let Err(e) = split::verify_ownership(subject, deps, &split).await {
        warn!("{code}: ownership drifted between selection and injection: {e:#}");
        mixed.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!("{e:#}"),
            },
            &records,
            Vec::new(),
            None,
            None
        );
    }

    info!(
        "{code}: baseline complete ({baseline_operations} confirmed ops), naming {} and \
         signalling readiness",
        split.pod_address
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: code.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        fault_target: Some(FaultTarget {
            pod_address: split.pod_address.clone(),
            pod_ip: split.pod_ip.clone(),
            owned_agents: split.on_pod.clone(),
        }),
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("{code}: no fault-injected signal arrived: {e}");
            mixed.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None, None);
        }
    };
    info!(
        "{code}: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    // ── The second fault, for MF2 ───────────────────────────────────────────
    //
    // The workflow applies it and reports it; the driver only learns when. What
    // it does with that is sample the assignment on both sides of it, because a
    // shard-manager restart is the one part of this composition that could
    // plausibly move shards — and if it did, the two quota populations below
    // are no longer the ones the run was aimed with.
    //
    // A wait that runs out is not an abort. The run still has an enclosing
    // fault, a workload and every account after this; what it does not have is
    // the composition, and the report says exactly that.
    if composed_config.is_some() {
        let deadline = config.phases.fault() + SECONDARY_WAIT_MARGIN;
        match signals.await_secondary_fault(deadline).await {
            Ok(signal) => {
                info!(
                    "{code}: second fault {} ({} on {}) reported active at {}",
                    signal.fault_id, signal.kind, signal.target, signal.injected_at
                );
                ownership
                    .push(sample_ownership(deps, "after-restart", ownership.last(), false).await);
                routing_snapshots.push(snapshot_routing(deps, "after-restart").await);
                secondary = Some(signal);

                // Then again once the cluster has had time to react, capped
                // against what is left of the window so the sample cannot land
                // after the heal and describe an executor whose DNS was back.
                if let Some(composed) = composed_config {
                    let remaining = config
                        .phases
                        .fault()
                        .mul_f64((1.0 - composed.after_fraction).max(0.0));
                    let settle = RESTART_SETTLE.min(remaining / 2);
                    info!("{code}: sampling assignment again in {settle:?}");
                    tokio::time::sleep(settle).await;
                    ownership.push(
                        sample_ownership(deps, "after-restart-settled", ownership.last(), false)
                            .await,
                    );
                }
            }
            Err(e) => {
                warn!("{code}: no secondary-fault signal arrived within {deadline:?}: {e}");
            }
        }
    }

    // One stop, late in the window. Unlike S19 there is nothing to probe here:
    // the driver cannot see the executor's resolver, and what it would want to
    // measure — whether a resolution was attempted — is not visible from any
    // agent. The workflow's DNS capability preflight is where that claim is
    // established instead.
    //
    // Timed from here rather than from injection, because on MF2 the block
    // above has already spent part of the window: sleeping the full fraction
    // again would run past the heal and take the "during-fault" sample after
    // the fault.
    let elapsed = Utc::now()
        .signed_duration_since(injected.injected_at)
        .to_std()
        .unwrap_or(Duration::ZERO);
    let sample_at = config.phases.fault().mul_f64(OWNERSHIP_SAMPLE_FRACTION);
    if let Some(wait) = sample_at.checked_sub(elapsed) {
        tokio::time::sleep(wait).await;
    }
    info!("{code}: sampling assignment late in the fault window");
    ownership.push(sample_ownership(deps, "during-fault", ownership.last(), false).await);

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("{code}: no fault-recovered signal arrived: {e}");
            mixed.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None, None);
        }
    };
    info!(
        "{code}: name resolving again at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // Built here rather than at the end, so an abort during recovery or
    // read-back still says whether the two faults ever met. Every account after
    // this point describes a cluster that was under both, and a reader who
    // cannot tell that apart from one under a single fault has been given the
    // wrong document.
    if let Some(composed) = composed_config {
        let report = ComposedFaultReport::build(
            &injected,
            fault_recovered_at,
            secondary.as_ref(),
            composed.min_overlap(),
        );
        for finding in &report.findings {
            warn!("{code}: {}: {}", finding.violation, finding.detail);
        }
        composed_report = Some(report);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    info!(
        "{code}: recovery phase, running for {:?}",
        config.phases.recovery()
    );
    tokio::time::sleep(config.phases.recovery()).await;

    mixed.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);
    ownership.push(sample_ownership(deps, "after-recovery", ownership.last(), true).await);

    // ── Account ─────────────────────────────────────────────────────────────
    let records = history.snapshot();

    let fault = fault_injected_at.map(|injected_at| FaultWindow {
        injected_at,
        recovered_at: fault_recovered_at,
    });

    let resolution_report = resolution::build(
        &records,
        ResolutionInputs {
            scenario: code.as_str(),
            expectation: resolution_config.expectation,
            split: &split,
            fault,
            poisoned_name: resolution_config.poisoned_name.clone(),
            degradation_ceiling_percent: resolution_config.degradation_ceiling_percent,
            recovery_floor_percent: resolution_config.recovery_floor_percent,
        },
    );
    for line in resolution_report.note_lines() {
        info!("{line}");
    }

    let readback = read_back(&ctx, &records, &quota_agents, workload_config).await;
    let before_probe = read_counters(&ctx, &records).await;
    let key_probes = probe::probe_keys(&ctx, &records, Stream::Durable).await;
    let after_probe = read_counters(&ctx, &records).await;

    let exactly_once = ExactlyOnceReport::build(
        &records,
        &key_probes,
        Stream::Durable,
        &before_probe,
        &after_probe,
    );
    info!(
        "{code}: exactly-once account — {} keys checked, {} with a final result, {} \
         recovered by the probe, {} findings",
        exactly_once.keys_checked,
        exactly_once.keys_with_final_result,
        exactly_once.keys_recovered_by_probe,
        exactly_once.findings.len()
    );

    let reason = termination(&exactly_once, &records);
    finish!(
        reason,
        &records,
        readback,
        Some(exactly_once),
        Some(resolution_report)
    );
}

/// The verdict, in the order a reader would want it.
///
/// Shorter than every other scenario's, and deliberately so. Duplicate
/// execution first, because it is the only harm visible from outside the
/// cluster and the ticket's headline guarantee. Then a run where nothing
/// succeeded at all, which is a broken run rather than a finding.
///
/// Neither [`resolution::ResolutionViolation`] appears. Both are comparisons
/// between two executors, and a comparison is a number to read rather than a
/// contract to break: a quota stream that ran slower on the pod that could not
/// resolve the shard manager is the most interesting result either scenario can
/// produce — the point of S4 under one expectation, the point of MF2 under the
/// other — and failing the run on it would bury that under a red cross instead
/// of putting it in front of someone. The assignment check is not here either —
/// [`sample_ownership`] files a movement into the summary's attention list on
/// its own.
fn termination(exactly_once: &ExactlyOnceReport, records: &[OperationRecord]) -> TerminationReason {
    if let Some(reason) = exactly_once_termination(exactly_once) {
        return reason;
    }
    if records.iter().all(|r| r.outcome != Outcome::Confirmed) {
        return TerminationReason::StreamNeverSucceeded {
            stream: Stream::Quota.to_string(),
        };
    }
    TerminationReason::Completed
}

/// Durable state for every stream that keeps a count.
async fn read_back(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    quota_agents: &[String],
    workload_config: &crate::chaos::WorkloadConfig,
) -> Vec<AgentReadback> {
    let mut agents: Vec<(Stream, String, ReadKind)> = quota_agents
        .iter()
        .map(|agent| (Stream::Quota, agent.clone(), ReadKind::QuotaCounter))
        .collect();
    agents.extend(
        (0..workload_config.durable_agents)
            .map(|index| ctx.agent_name(Stream::Durable, index))
            .map(|agent| (Stream::Durable, agent, ReadKind::Counter)),
    );
    read_back_agents(ctx, records, agents).await
}

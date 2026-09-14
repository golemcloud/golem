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

//! S19 — one executor's clock moved half a minute behind the cluster (GOL-383).
//!
//! [`crate::chaos::skew`] carries the argument: why a uniformly wrong clock is
//! invisible from inside, why the quota lease is the only place two clocks are
//! compared, why the offset is negative and why it is thirty seconds. This
//! module is the choreography that produces the numbers that argument needs.
//!
//! ### What is expected to happen
//!
//! Read out of `quota_state.rs` and `services/quota.rs`, and it is worth stating
//! ahead of the run because it decides what the phases have to be long enough to
//! contain.
//!
//! The shard-manager keeps one lease per pod per resource. The executor renews
//! when it believes fewer than `RENEWAL_THRESHOLD` remain of a
//! `LEASE_DURATION`-long lease, and it makes that judgement by subtracting its
//! own clock from an `expires_at` the shard-manager minted. A pod running thirty
//! seconds behind therefore believes it has thirty seconds more headroom than it
//! has, and renews after its lease has already expired by the granting clock.
//!
//! What happens next depends on whether anyone else touched the resource in
//! between, because expiry is lazy: `reclaim_expired` runs only inside
//! `acquire_lease` and `renew_lease`. If the healthy executor renews the same
//! resource inside that window, the skewed pod's lease is reclaimed and its own
//! renewal comes back `LeaseNotFound`. The executor marks the lease `Lost`, and
//! reservations queued against it park until the next loop re-acquires. That
//! park is the observable: a latency spike on the skewed executor's quota agents
//! and nothing at all on the other executor's.
//!
//! This is why the quota population spans **both** executors rather than being
//! pinned to the skewed one, which is where the scenario departs from the
//! ticket. A skewed pod alone renews late, rescues its own lease and nothing
//! ever notices.
//!
//! ### The choreography
//!
//! 1. **Warm up** the quota agents, the durable agents and the schedule
//!    targets, so the skew lands on a live population.
//! 2. **Select** the executor owning the largest share of the *quota* agents,
//!    and name it for the workflow. Quota rather than anything else because that
//!    is the population the fault can actually reach.
//! 3. **Baseline** — mixed workload and registrations together, then a round of
//!    clock probes that should read about zero on both sides. Those are archived
//!    rather than judged, and they are what separates a broken probe from a
//!    clock that never moved.
//! 4. **Fault** — keep driving, and probe both sides several times through the
//!    window. Sample the assignment too: a clock skew has no business moving
//!    shards, and a run where it did is a different experiment.
//! 5. **Heal**, then keep driving long enough for the quota stream to come back
//!    to its baseline and for the scheduled backlog to drain under a corrected
//!    clock.
//! 6. **Read back and probe** — the same completion, exactly-once and
//!    scheduled-fire oracles the rest of the suite ends with.

use crate::chaos::fires::ScheduleFireReport;
use crate::chaos::history::{OperationHistory, OperationRecord, Outcome, Phase, Stream};
use crate::chaos::ownership::OwnershipSample;
use crate::chaos::pinned::owners_by_pod;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ReadKind, ScenarioOutcome, WARMUP_SETTLE, build_result, exactly_once_termination,
    read_back_agents, read_counters, sample_ownership, signal_termination, snapshot_routing,
    wait_for_settled_routing, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals, FaultTarget};
use crate::chaos::skew::{ClockProbe, SkewInputs, SkewReport, SkewViolation};
use crate::chaos::split::{self, FaultWindow, PodSplit};
use crate::chaos::summary::{
    AgentReadback, ChaosSummary, ExactlyOnceReport, Note, TerminationReason,
};
use crate::chaos::workload::{PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig, probe, scheduled, skew, workload};
use chrono::Utc;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use std::collections::BTreeSet;
use std::time::Duration;
use tracing::{info, warn};

/// Where in the fault window the clock probes are taken, as fractions of it.
///
/// Three rather than one, because the reading is a median and one probe round
/// that happened to catch a slow invocation would decide the run. Spread rather
/// than bunched, because the renewal cycle this scenario disturbs is tens of
/// seconds long and a burst of readings inside one cycle is one reading.
///
/// None of them is at the very start: Chaos Mesh reports `AllInjected` when the
/// pod's clock has been stepped, but the executor only acts on it at its next
/// renewal, and a probe before then would read the skew correctly while the
/// quota cells around it described an undisturbed pod.
const PROBE_FRACTIONS: [f64; 3] = [0.25, 0.5, 0.75];

/// Where in the fault window the assignment is sampled, as a fraction of it.
///
/// Late, and for S3's reason: this sample is checking that nothing moved, and a
/// table that still looks untouched three quarters of the way in says far more
/// than one that looks untouched immediately.
const OWNERSHIP_SAMPLE_FRACTION: f64 = 0.8;

/// Runs S19 end to end.
pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let workload_config = config.require_workload()?;
    let scheduled_config = config.require_scheduled()?;
    let skew_config = config.require_skew()?;
    let history = OperationHistory::new(ScenarioCode::S19.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S19);

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
    let targets: Vec<String> = (0..scheduled_config.targets)
        .map(|index| ctx.schedule_target_name(index))
        .collect();

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut ownership: Vec<OwnershipSample> = Vec::new();
    let mut probes: Vec<ClockProbe> = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut selection: Option<PodSplit> = None;
    let mut attention_extra: Vec<Note> = Vec::new();

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $exactly_once:expr, $fires:expr, $skew:expr) => {{
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
            if let Some(report) = $fires {
                summary = summary.with_schedule_fires(report);
            }
            if let Some(report) = $skew {
                summary = summary.with_skew(report);
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

    info!(
        "S19: warming {} quota agents and {} schedule targets",
        quota_agents.len(),
        targets.len()
    );
    let warm: Vec<(Stream, String, ReadKind)> = quota_agents
        .iter()
        .map(|agent| (Stream::Quota, agent.clone(), ReadKind::QuotaCounter))
        .collect();
    let _ = read_back_agents(&ctx, &[], warm).await;
    let warmed = scheduled::warm(&ctx, &targets).await;
    info!("S19: warmed {warmed} scheduled agents, settling {WARMUP_SETTLE:?}");
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Aim ─────────────────────────────────────────────────────────────────
    //
    // On the quota agents, because they are the only population the fault can
    // reach: a lease is the one thing here that one machine mints and another
    // judges. The schedule targets are classified against whichever executor
    // this picks rather than choosing it, since they are along to measure what
    // the skew cost, not to decide where it lands.
    let subject = split::quota_subject(&ctx);
    let split = match split::select(subject, deps, &quota_agents).await {
        Ok(split) => split,
        Err(e) => {
            warn!("S19: cannot aim the clock skew: {e:#}");
            let records = history.snapshot();
            finish!(
                TerminationReason::FaultTargetUnverified {
                    detail: format!("{e:#}"),
                },
                &records,
                Vec::new(),
                None,
                None,
                None
            );
        }
    };
    selection = Some(split.clone());

    // The healthy executor has to hold quota agents too, or the fault is inert
    // by construction: the shard-manager only reclaims an expired lease when
    // some *other* pod touches the same resource, so with every agent on one
    // executor the skewed pod renews late, rescues its own lease and nothing
    // disagrees. Refused before the window rather than reported after it.
    if split.elsewhere.is_empty() {
        warn!("S19: every quota agent landed on one executor, so nothing can contend for a lease");
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!(
                    "all {} quota agents are owned by {}, and a stale lease is only reclaimed \
                     when another pod touches the same resource, so a skew on that pod would \
                     produce no disagreement to measure",
                    quota_agents.len(),
                    split.pod_address
                ),
            },
            &records,
            Vec::new(),
            None,
            None,
            None
        );
    }

    // Which schedule targets sit on the executor about to be skewed. Computed
    // from the same routing table the split came from, because ownership is per
    // agent id and the targets are a different agent type: the executor holding
    // most quota agents need not hold most targets.
    let targets_on_faulted_pod =
        match targets_on_pod(deps, &ctx, &targets, &split.pod_address).await {
            Ok(on_pod) => on_pod,
            Err(e) => {
                warn!("S19: cannot place the schedule targets against the skewed executor: {e:#}");
                let records = history.snapshot();
                finish!(
                    TerminationReason::FaultTargetUnverified {
                        detail: format!("{e:#}"),
                    },
                    &records,
                    Vec::new(),
                    None,
                    None,
                    None
                );
            }
        };
    info!(
        "S19: {} of {} schedule targets sit on {}",
        targets_on_faulted_pod.len(),
        targets.len(),
        split.pod_address
    );
    // The fire account's group is called `on-killed-executor` because every
    // scenario that read it before this one killed a pod. Said out loud in the
    // result rather than left for a reader to trip over, since S19 kills
    // nothing.
    attention_extra.push(Note::context(format!(
        "S19: the scheduled-fire account files the skewed executor's {} targets under \
         `on-killed-executor`. Nothing was killed — the group means the targets on the pod the \
         fault was aimed at, and the name is kept so archived results stay readable",
        targets_on_faulted_pod.len()
    )));

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S19: baseline phase, mixed workload plus {} registration loops, for {:?}",
        targets.len(),
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let mixed = workload::start(ctx.clone(), workload_config);
    let registrations = scheduled::start(ctx.clone(), &targets, scheduled_config);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    ownership.push(sample_ownership(deps, "before-fault", ownership.last(), false).await);

    // Archived rather than judged. If these read cleanly and the fault-window
    // round reads nothing, the probe works and the clock did not move; if both
    // read nothing, the instrument is broken and the run says so instead of
    // blaming Chaos Mesh.
    info!("S19: baseline clock probes, which should read about zero on both executors");
    probes.extend(skew::probe_round(&ctx, &split, skew_config.probes_per_round).await);

    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S19: baseline produced no confirmed operations, aborting before injection");
        mixed.stop().await;
        registrations.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Vec::new(),
            None,
            None,
            None
        );
    }

    let sampled = scheduled::sample_fire_count(&ctx, &targets).await;
    if sampled == 0 {
        warn!("S19: {baseline_operations} operations accepted and no scheduled action has fired");
        mixed.stop().await;
        registrations.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::StreamNeverSucceeded {
                stream: Stream::Scheduled.to_string(),
            },
            &records,
            Vec::new(),
            None,
            None,
            None
        );
    }

    // A rebalance between selection and injection would leave the run naming
    // the control group as the affected one and vice versa — a report that is
    // not merely wrong but confidently inverted.
    if let Err(e) = split::verify_ownership(subject, deps, &split).await {
        warn!("S19: ownership drifted between selection and injection: {e:#}");
        mixed.stop().await;
        registrations.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!("{e:#}"),
            },
            &records,
            Vec::new(),
            None,
            None,
            None
        );
    }

    info!(
        "S19: baseline complete ({baseline_operations} confirmed ops, {sampled} fires across a \
         sample of {} targets), naming {} and signalling readiness",
        scheduled::FIRE_PROOF_SAMPLE.min(targets.len()),
        split.pod_address
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S19.as_str().to_string(),
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
            warn!("S19: no fault-injected signal arrived: {e}");
            mixed.stop().await;
            registrations.stop().await;
            let records = history.snapshot();
            finish!(
                signal_termination(&e),
                &records,
                Vec::new(),
                None,
                None,
                None
            );
        }
    };
    info!(
        "S19: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    // Probe rounds and one assignment sample, on a single schedule so the two
    // never wait on each other. Elapsed is tracked against the fault window
    // rather than slept blindly, because a probe round takes real time and
    // three of them added to a fixed sleep would run past the heal.
    let fault_window = config.phases.fault();
    let mut elapsed = Duration::ZERO;
    let mut stops: Vec<(Duration, Stop)> = PROBE_FRACTIONS
        .iter()
        .map(|f| (fault_window.mul_f64(*f), Stop::Probe))
        .chain(std::iter::once((
            fault_window.mul_f64(OWNERSHIP_SAMPLE_FRACTION),
            Stop::Ownership,
        )))
        .collect();
    stops.sort_by_key(|(at, _)| *at);

    for (at, stop) in stops {
        if let Some(wait) = at.checked_sub(elapsed) {
            tokio::time::sleep(wait).await;
            elapsed += wait;
        }
        match stop {
            Stop::Probe => {
                info!("S19: clock probes {elapsed:?} into the fault window");
                probes.extend(skew::probe_round(&ctx, &split, skew_config.probes_per_round).await);
            }
            Stop::Ownership => {
                info!("S19: sampling assignment {elapsed:?} into the fault window");
                ownership
                    .push(sample_ownership(deps, "during-fault", ownership.last(), false).await);
            }
        }
    }

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S19: no fault-recovered signal arrived: {e}");
            mixed.stop().await;
            registrations.stop().await;
            let records = history.snapshot();
            finish!(
                signal_termination(&e),
                &records,
                Vec::new(),
                None,
                None,
                None
            );
        }
    };
    info!(
        "S19: clock corrected at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    info!(
        "S19: recovery phase, running for {:?}",
        config.phases.recovery()
    );
    tokio::time::sleep(config.phases.recovery()).await;

    let skipped = registrations.skipped();
    mixed.stop().await;
    registrations.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    if skipped > 0 {
        attention_extra.push(Note::attention(format!(
            "S19 skipped {skipped} registration ticks because targets still had their budget of \
             {} in flight — the offered rate was clamped by the platform, so the phase counts \
             understate what the run intended to submit",
            scheduled::MAX_IN_FLIGHT_PER_TARGET
        )));
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);
    ownership.push(sample_ownership(deps, "after-recovery", ownership.last(), true).await);

    // ── Account ─────────────────────────────────────────────────────────────
    let settle = scheduled::settle_before_readback(scheduled_config);
    info!("S19: letting the last actions fall due and fire, {settle:?} before read-back");
    tokio::time::sleep(settle).await;

    let records = history.snapshot();
    let logs = scheduled::read_logs(&ctx, &targets).await;
    history.record_fire_logs(logs.clone());

    let fault = fault_injected_at.map(|injected_at| FaultWindow {
        injected_at,
        recovered_at: fault_recovered_at,
    });

    let skew_report = skew::build(
        &records,
        SkewInputs {
            split: &split,
            fault,
            injected_offset_ms: skew_config.injected_offset_ms,
            tolerance_ms: skew_config.tolerance_ms,
            recovery_floor_percent: skew_config.recovery_floor_percent,
            probes,
        },
    );
    for line in skew_report.note_lines() {
        info!("{line}");
    }

    let fires = ScheduleFireReport::build(
        &records,
        &logs,
        scheduled_config.lead(),
        fault,
        &targets_on_faulted_pod,
        scheduled_config.lease_budget(),
    );
    info!(
        "S19: scheduled-fire account — {} registrations accepted, {} fired once, {} \
         inconclusive, {} unverifiable, {} findings",
        fires.registrations_confirmed,
        fires.fired_once,
        fires.inconclusive,
        fires.unverifiable,
        fires.findings.len()
    );

    let readback = read_back(&ctx, &records, &quota_agents, workload_config, &targets).await;
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
        "S19: exactly-once account — {} keys checked, {} with a final result, {} recovered by \
         the probe, {} findings",
        exactly_once.keys_checked,
        exactly_once.keys_with_final_result,
        exactly_once.keys_recovered_by_probe,
        exactly_once.findings.len()
    );

    let reason = termination(&exactly_once, &fires, &skew_report, &records);
    finish!(
        reason,
        &records,
        readback,
        Some(exactly_once),
        Some(fires),
        Some(skew_report)
    );
}

/// What to do at one point in the fault window.
#[derive(Debug, Clone, Copy)]
enum Stop {
    Probe,
    Ownership,
}

/// The verdict, in the order a reader would want it.
///
/// Duplicate execution first, because it is the only harm visible from outside
/// the cluster and the ticket's headline guarantee. A skew that could not be
/// confirmed comes next: it is not a defect, it is a run that measured nothing,
/// and reporting the numbers underneath it as a pass is the failure this
/// scenario is most exposed to.
///
/// [`SkewViolation::QuotaDidNotRecover`] is deliberately absent. Losing a lease
/// under skew is a legitimate response, and how long getting it back may take
/// is a judgement rather than a constant — so it is reported and left to the
/// operator, the same way the relay account's recovery finding is.
fn termination(
    exactly_once: &ExactlyOnceReport,
    fires: &ScheduleFireReport,
    skew: &SkewReport,
    records: &[OperationRecord],
) -> TerminationReason {
    if let Some(reason) = exactly_once_termination(exactly_once) {
        return reason;
    }
    if fires.has_violations() {
        return TerminationReason::ScheduledFireViolated {
            findings: fires.findings.len() as u64,
            first: fires
                .findings
                .first()
                .map(|f| format!("{} on token {}", f.violation, f.token))
                .unwrap_or_default(),
        };
    }
    if let Some(finding) = skew
        .findings
        .iter()
        .find(|f| f.violation == SkewViolation::ClockNeverMoved)
    {
        return TerminationReason::FaultTargetUnverified {
            detail: finding.detail.clone(),
        };
    }
    if records.iter().all(|r| r.outcome != Outcome::Confirmed) {
        return TerminationReason::StreamNeverSucceeded {
            stream: Stream::Quota.to_string(),
        };
    }
    TerminationReason::Completed
}

/// The schedule targets owned by `pod_address`.
///
/// Read from a fresh routing table rather than reusing the split's, because the
/// split was taken over a different agent type and only carries the agents it
/// was asked about.
async fn targets_on_pod(
    deps: &BenchmarkTestDependencies,
    ctx: &WorkloadContext,
    targets: &[String],
    pod_address: &str,
) -> anyhow::Result<BTreeSet<String>> {
    let table = deps.shard_manager().get_routing_table().await?;
    let by_pod = owners_by_pod(ctx, &table, workload::SCHEDULE_COUNTER_AGENT, targets);
    Ok(by_pod
        .get(pod_address)
        .map(|agents| agents.iter().cloned().collect())
        .unwrap_or_default())
}

/// Durable state for every stream that keeps a count.
async fn read_back(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    quota_agents: &[String],
    workload_config: &crate::chaos::WorkloadConfig,
    targets: &[String],
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
    agents.extend(
        targets
            .iter()
            .map(|target| (Stream::Scheduled, target.clone(), ReadKind::Polls)),
    );
    read_back_agents(ctx, records, agents).await
}

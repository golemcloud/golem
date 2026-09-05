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

//! Storage fault: the shared choreography behind S14, S15, S16, S17, S18, S22
//! and S23, plus the S15A/S15B/S15C eliminations and the composed MF1.
//!
//! All eleven codes run this module. They differ in which store the fault is
//! aimed at, what it does to that store and for how long, all of which are
//! suite settings, and in what a reader should expect of the result:
//!
//! * **S16** (GOL-379) cuts the key-value cluster for the length of an AWS
//!   storage failover, about a minute. The key-value retry budget covers that,
//!   so the claim is that the platform absorbs it: operations stall and then
//!   complete, and no executor is lost.
//! * **S22** (GOL-499) cuts the same cluster for longer than that budget. The
//!   budget is then exhausted, the recovery-index write in `status_flusher`
//!   panics on purpose, and executors are replaced. The claim is not survival
//!   but that the exit is the intended one and nothing is lost across it.
//! * **S14** (GOL-376) cuts the *other* Aurora cluster, the one carrying the
//!   oplog, again for the length of a failover. golem-dev gives the indexed
//!   retry 200 attempts with a 10s cap, so the budget is not the question here
//!   the way it is in the other two, and the claim is again absorption.
//! * **S18** (GOL-384) cuts neither Aurora cluster but the Redis cache in front
//!   of the key-value layer, and holds it past the caller's patience rather
//!   than past any platform budget. There is no budget to exhaust: golem-dev
//!   configures that client to retry forever. The claim is that a stall which
//!   outlasts the caller does not turn one operation into two.
//! * **S17** (GOL-375) leaves that same cache reachable and makes it slow. The
//!   only scenario here that breaks nothing, and a different question rather
//!   than a milder version of S18's: not whether the platform survives losing
//!   its worker-status store, but whether it degrades or breaks when that store
//!   gets slower. Going quiet would be a failure here rather than the expected
//!   result.
//! * **S15** (GOL-374) slows the key-value cluster instead, so it is to S16
//!   what S17 is to S18 and the mirror image of S17 at the same time. The
//!   claim is the same one — degradation rather than breakage — but on the
//!   opposite set of streams, and it carries a risk the Redis delay does not:
//!   this side's connection pools are bounded and its failure path is a panic.
//!   See the suite entry for the arithmetic.
//! * **S15A**, **S15B** and **S15C** are S15 with streams taken away rather
//!   than scenarios of their own: same fault, same endpoint, same phases, one
//!   stream added back per entry, so whichever addition first makes `ephemeral`
//!   suffer names the interaction. S15A drives `ephemeral` alone and so names
//!   no slowed stream at all: its whole claim sits on the other side of the
//!   expectation, that `ephemeral` stays steady. That is the one case where an
//!   empty `slowed` list is the claim rather than an omission. See the suite
//!   entries.
//! * **S23** (GOL-525) slows the indexed cluster instead of cutting it, so it
//!   is to S14 what S15 is to S16, and it fills the last empty cell of the
//!   matrix. It is the only delay here with no control stream, because every
//!   agent commits its oplog to the store being slowed — `ephemeral` included,
//!   one layer down. That costs the run the routing check the other two delays
//!   get for free, and the evidence has to come from the `svc`-labelled storage
//!   series instead. It also meets the one concurrency gate the suite has:
//!   `GOLEM__INDEXED_STORAGE__CONFIG__MAX_CONCURRENT_OPS`, a semaphore tighter
//!   than the pool behind it.
//!
//! * **MF1** (GOL-381) is the odd one out and the only code here that injects
//!   more than one fault. It cuts the key-value cluster exactly as S16 does and
//!   kills a worker-executor half way through that window. Everything below is
//!   the same choreography with three additions, each guarded on the presence
//!   of a `composed` block: the kill is aimed, a second fault signal is waited
//!   for inside the window, and the shard assignment is sampled around it. What
//!   it asks is not in the single-fault matrix at all — a survivor has to take
//!   over shards while the running-workers set it needs to do that is behind
//!   the cut. See [`crate::chaos::composed`] for how a composition that missed
//!   is told apart from one that worked.
//!
//! The driver is the same in all eleven because the difference is one of
//! expectation, not of choreography. Nothing below asserts on which outcome
//! happened: the account it produces answers all ten questions, and the
//! oracles that fail the build — the scheduled-fire account and the
//! exactly-once account — are the ones every one of them shares.
//!
//! The first scenarios in the suite that break something the platform depends
//! on rather than something the platform *is*. Every fault before these removed
//! a golem process or a link between two of them, and in each case some part of
//! the cluster stayed healthy and could be compared against. Here the executors
//! keep running, keep their shards, keep answering the shard-manager, and
//! simply cannot reach a database underneath them.
//!
//! ## What the fault takes away
//!
//! Three different pieces of the platform, depending on the code. The two
//! Aurora cuts are close to mirror images of each other; the Redis cut is a
//! third thing again.
//!
//! Reading the golem-dev executor deployment, the **key-value** cluster that
//! S16 and S22 cut carries four things:
//!
//! * promises,
//! * the running-workers set,
//! * user-defined key-value data,
//! * and the scheduler, in its own schema on the same cluster.
//!
//! The oplog lives on a different Aurora cluster and the worker-status hot
//! cache lives in Redis, and that partition touches neither. So the platform
//! keeps the ability to *record* what it did and loses the ability to know
//! *what it is doing*.
//!
//! The **indexed** cluster that S14 cuts and S23 slows carries the oplog and
//! nothing else. Promises still resolve, the scheduler still claims and
//! acknowledges on time, the running-workers set is still writable. What goes
//! is the ability to commit anything durable at all, which is the opposite
//! arrangement: the platform knows exactly what it is doing and cannot record
//! any of it.
//!
//! The **Redis cache** that S18 and S17 aim at is not a cluster at all but the
//! front half of the key-value layer. `NamespaceRoutedKeyValueStorage` sends the `Worker`,
//! `AgentStatus` and `AgentStatusCheckpoint` namespaces to it and everything
//! else to Postgres, so S18 removes exactly the part S16 leaves standing and
//! S17 slows the same part instead. Both Aurora clusters stay reachable
//! throughout either.
//!
//! S15 aims at the back half of that same layer, which is the key-value cluster
//! again, and the split decides its streams too. `durable` waits on the
//! `RunningWorkers` recovery index, which `AgentStatusFlusher::on_status_changed`
//! updates synchronously whenever an agent crosses between tracked and
//! untracked; `promise` reads and writes promise keys; the scheduler registers
//! into its own schema on the same cluster. `ephemeral` reaches none of it, and
//! is the one stream that should not move.
//!
//! No stream is a control group under the Aurora *cuts*. A durable increment
//! needs the running-workers set before it can start, the worker-status cache
//! to resolve its mode, and the oplog before it can finish, so `durable`
//! degrades under both of those and must not be read as untouched. The three
//! scenarios aimed at one half of the key-value layer are the exception, and
//! deliberately so: there the streams that keep working — or keep their pace —
//! are evidence rather than noise, which is why they are judged by their own
//! expectations. See [`crate::chaos::OutageExpectation`].
//!
//! S23 is neither, and it is worth being explicit about which of the two it
//! resembles. It aims at one store rather than at half of a split layer, but
//! every stream reaches that store, so it has the *cuts'* problem of having
//! nowhere to look for a control while making the *delays'* claim about
//! degradation. Its `steady` list is therefore empty and the routing check is
//! left to the reader with the storage series to do it against, which the
//! runbook names. See the suite entry for which write each stream makes.
//!
//! ## The control is the baseline, not another pod
//!
//! S1 and S3 keep executors on the healthy side of the cut and read the verdict
//! off the disagreement between the two groups. There is no healthy side here:
//! all three executors share one store. So the comparison runs along time
//! instead, and every stream is measured against its own before-fault rate. See
//! [`crate::chaos::outage`] for what that costs and what it still answers.
//!
//! The first question it has to answer is whether the outage landed at all. A
//! partition that failed to take hold produces a report full of healthy numbers
//! and no error anywhere, which is the worst artifact this suite can produce,
//! so [`OutageViolation::OutageNotObserved`] is a named finding rather than
//! something a reader is left to infer from the cells.
//!
//! ## Why S18 is held longer than the others
//!
//! The first three scenarios are sized against a platform budget: S16 and S14
//! stay inside one, S22 deliberately runs past it. S18 has no budget to run
//! past. golem-dev sets
//! `GOLEM__KEY_VALUE_STORAGE__CONFIG__CACHE__CONFIG__RETRIES__MAX_ATTEMPTS` to
//! `0`, which `RedisPool::configured` hands to fred as a `ReconnectPolicy` with
//! unlimited attempts, and it passes no performance or connection config, so
//! fred's defaults apply: no command timeout and an unbounded command buffer.
//! A cache write during the cut therefore never fails. It waits, for as long as
//! the cut lasts.
//!
//! That makes the `unwrap_or_else(|err| panic!(...))` on the cache path in
//! `WorkerService` unreachable through this fault, and it makes a short cut
//! uninformative: at 60s every caller is still inside its 120s attempt timeout,
//! so the run would only show operations taking a minute longer and completing.
//! The window is set past that timeout instead, so callers give up and retry
//! under the same idempotency key while the original write is still sitting in
//! fred's buffer. Whether those two land as one operation or two is the
//! question the scenario exists to answer, and the exactly-once account is
//! where it shows.
//!
//! ## What is expected to stall, and what is not
//!
//! Less than the whole platform, and this is the part worth reading the result
//! carefully for. `AgentStatusFlusher` took the status blob off the commit path
//! deliberately: a status change only marks the agent dirty, and a background
//! sweeper coalesces the writes. So a durable agent that is already resident
//! commits to the oplog without touching Redis synchronously, and the blob it
//! cannot flush is derivable from the oplog anyway, which is what makes the
//! staleness safe rather than merely tolerated.
//!
//! What does cross Redis synchronously is a lifecycle boundary — suspend, evict,
//! reattach — and a `get_agent_mode` miss. Run 33130077355 settled which
//! streams that amounts to: `ephemeral` alone. Its agents are created and torn
//! down per operation, so every one crosses a boundary, and it was silent for
//! 99.997% of the window while `durable`, `scheduled` and `promise` held
//! 99.94–100.06% of their baselines.
//!
//! `promise` was expected to be the second and is not. The mixed workload's
//! promise stream is `get_promise+complete` in one round trip against a durable
//! agent and never suspends; the suspending variant is `promise-wait`, which
//! S11 drives and these scenarios do not enable.
//!
//! That partial shape is why S18 does not share the other three scenarios'
//! verdict. `shareOfBaselinePercent` sits far higher here — 77.83% against
//! S14's 22.39% — without the fault being weaker, and the run-wide quiet figure
//! the other three are judged on reads 0.05%, because `durable` never stopped.
//! Judged by that rule the run reported a partition that had plainly landed as
//! one that never happened. See [`crate::chaos::OutageExpectation`] for the
//! split, and for why the streams that keep working are asserted on rather than
//! merely exempted.
//!
//! ## Why the scheduled stream is driven separately
//!
//! The mixed workload's scheduled stream registers through `schedule_poll_at`,
//! which increments a counter and records nothing else. That is enough to ask
//! "did every registration eventually fire" and useless for asking "how late".
//! Scheduler lag is one of the two things GOL-379 asks these runs to record,
//! and the only place a due time survives is the target's own fire log, so
//! these scenarios drive the token-carrying registration loop from
//! [`crate::chaos::scheduled`] instead and leave `scheduledAgents` at zero in
//! the mixed workload. Setting both is refused at load time — see
//! `ScenarioConfig::require_storage`.
//!
//! An entry may also leave the loop undriven altogether by setting
//! `scheduled.targets` to zero, which is what the S15 eliminations do. The
//! fire-count gate then has nothing to gate, and is skipped rather than failing
//! a run for observing no fires it never asked for.
//!
//! S14 is where that lag reads most directly. Its cut leaves the scheduler on
//! the healthy cluster, so claims and acknowledgements keep their timing and
//! what the delays measure is purely how long the fired invocation could not
//! commit. Under S16 and S22 the scheduler is inside the outage and the two
//! costs are not separable.
//!
//! One consequence worth stating plainly for anyone reading the result: the
//! fire account's `group` axis is degenerate for the ten single-fault codes.
//! None of them names a pod to kill, so every target is reported as `elsewhere`
//! and only the `window` axis carries information. The delays to read are the
//! `during-fault` and `after-fault` cells against `before-fault`.
//!
//! MF1 is the exception and the reason the axis exists here at all. Its kill is
//! aimed at the executor owning the largest share of the targets, so the
//! `on-pod` group is the population whose actions had to be recovered by a
//! survivor that could not read the scheduler's schema, and the `elsewhere`
//! group is the control that was only ever inside the outage.
//!
//! ## What fails the run
//!
//! The same narrow set S3 fails on, and for the same reason: this suite reports
//! rather than judges, and the bar for failing outright is "the run produced
//! nothing worth interpreting".
//!
//! * A token-level scheduled-fire violation. An accepted action that never ran,
//!   ran twice, or ran after being refused is a statement about one named
//!   registration with no band of doubt around it.
//! * A key that the exactly-once probe shows executed twice.
//! * A workload that never confirmed anything at all.
//!
//! Everything the storage account finds is loud without being fatal: it lands
//! in `attention`, which CI annotates. An outage that did not land and a stream
//! that never came back are both things a human has to look at, and neither is
//! improved by turning the job red.
//!
//! Run-by-run findings live in the per-scenario runbooks in golem-cloud, not
//! here.

use crate::chaos::composed::ComposedFaultReport;
use crate::chaos::fires::{FaultWindow, ScheduleFireReport};
use crate::chaos::history::{OperationHistory, OperationRecord, Outcome, Phase, Stream};
use crate::chaos::outage::StorageFaultReport;
use crate::chaos::ownership::OwnershipSample;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::probe;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, WARMUP_SETTLE, build_result, exactly_once_termination,
    read_counters, readback_for, sample_ownership, signal_termination, snapshot_routing,
    wait_for_settled_routing, write_outputs,
};
use crate::chaos::scheduled::{self, ScheduledSelection};
use crate::chaos::signal::{BaselineReady, FaultInjected, FaultSignals, FaultTarget};
use crate::chaos::summary::{
    AgentReadback, ChaosSummary, ExactlyOnceReport, Note, TerminationReason,
};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig, ScheduledConfig};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::collections::BTreeSet;
use std::time::Duration;
use tracing::{info, warn};

/// Extra quiet after the last scheduled action is due, before anything is read
/// back.
///
/// The rest of the settle is derived from the configuration, the same way S10
/// derives it: the final registration falls due one `lead` after the workload
/// stops, and an action the outage delayed can cost up to one lease budget on
/// top. Reading before that has elapsed would report actions as lost that were
/// merely late, which is the one mistake this scenario cannot afford — a
/// storage outage is precisely the thing that makes work late.
const SETTLE_MARGIN: Duration = Duration::from_secs(30);

/// How long past the enclosing fault window a composed run keeps waiting for
/// its second fault.
///
/// The workflow injects the second fault at a fraction of the window and then
/// waits for Chaos Mesh to report it active, which is not instant. Without this
/// margin a slow injection near the end of the window would be recorded as one
/// that never happened, which is a worse mistake than the one it describes: the
/// run would look like a broken harness rather than like a composition that
/// landed too late to mean anything.
///
/// Sized against the workflow's own worst case rather than picked round. It
/// gives Chaos Mesh 120s to confirm the injection, so a signal can appear as
/// late as the injection fraction plus 120s. At MF1's half-way fraction that is
/// the whole window plus 60s, and a margin equal to that would be a tie.
/// Doubling it leaves the driver certain to see a signal the workflow actually
/// wrote, which matters because `secondary-outside-primary` tells a reader far
/// more than `secondary-never-injected` does.
const SECONDARY_WAIT_MARGIN: Duration = Duration::from_secs(120);

/// How long after the second fault lands before the shard assignment is sampled
/// a second time.
///
/// The first sample is taken the instant Chaos Mesh confirms the kill, which is
/// before anything can have reacted to it: the shard-manager has not noticed the
/// pod is gone, so that sample is the assignment as it stood at the kill. Useful
/// as a reference, useless as an observation.
///
/// What MF1 is actually for shows up in the second one — shards revoked from the
/// dead executor, and a survivor that cannot complete the assignment because the
/// running-workers set it has to read is behind the cut. The shard-manager gives
/// `assign_shards` 5s and retries it 5 times, so the interesting state exists for
/// tens of seconds and this has to land inside them.
const REASSIGNMENT_SETTLE: Duration = Duration::from_secs(30);

/// How many targets to sample after the baseline to prove actions are firing at
/// all.
///
/// A smoke test rather than a measurement: if the scheduling path is broken
/// every target is equally broken, and the point is to fail before spending the
/// fault window on a run that would report a clean account of nothing.
const FIRE_PROOF_SAMPLE: usize = 5;

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
    let scheduled_config = config.require_scheduled()?;
    let storage_config = config.require_storage()?;
    // Present only for the `MF` codes. Fetched through the checked accessor so
    // a malformed block fails here rather than half way through the window.
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
        component_ids: vec![
            manifest.counters_component_id.0.to_string(),
            manifest.promise_component_id.0.to_string(),
        ],
        agent_id_prefix: key_prefix.clone(),
        idempotency_key_prefix: format!("{key_prefix}-"),
    };

    let targets: Vec<String> = (0..scheduled_config.targets)
        .map(|index| ctx.schedule_target_name(index))
        .collect();

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut attention_extra: Vec<Note> = Vec::new();
    // All four are empty for the ten single-fault codes and stay out of the
    // result entirely, rather than appearing as empty accounts that would read
    // as "checked, nothing found".
    let mut ownership_samples: Vec<OwnershipSample> = Vec::new();
    let mut selection: Option<ScheduledSelection> = None;
    let mut secondary: Option<FaultInjected> = None;
    let mut composed_report: Option<ComposedFaultReport> = None;

    // Every early return below goes through `finish`, so an abort produces the
    // same artifact shape as a completed run, with fewer phases filled in.
    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $fires:expr, $outage:expr, $exactly:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            );
            summary.absorb(attention_extra.clone());
            if !ownership_samples.is_empty() {
                summary = summary.with_ownership(ownership_samples.clone());
            }
            if let Some(report) = composed_report.clone() {
                summary = summary.with_composed_fault(report);
            }
            if let Some(report) = $fires {
                summary = summary.with_schedule_fires(report);
            }
            if let Some(report) = $outage {
                summary = summary.with_storage_fault(report);
            }
            if let Some(report) = $exactly {
                summary = summary.with_exactly_once(report);
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
                    scheduled_selection: selection.clone(),
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

    // ── Warm-up ─────────────────────────────────────────────────────────────
    routing_snapshots.push(snapshot_routing(deps, "before-warmup").await);
    attention_extra.push(wait_for_settled_routing(deps, &mut routing_snapshots).await);
    info!(
        "{code}: warming {} schedule emitters and targets before the baseline",
        targets.len()
    );
    let warmed = scheduled::warm(&ctx, &targets).await;
    info!("{code}: warmed {warmed} agents, settling {WARMUP_SETTLE:?}");
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Aim the second fault ────────────────────────────────────────────────
    //
    // Only the composed codes have one. The enclosing fault is aimed at every
    // executor and needs nothing chosen, but the kill inside it does, and the
    // choice has to be made before the baseline for the same reason S10 makes
    // it there: a run that cannot be aimed should not spend a maintenance
    // window proving it.
    //
    // Aimed at the executor owning the largest share of the schedule targets,
    // which is exactly what S10 aims at. That is worth more here than anywhere
    // else in this module: the fire account already splits its targets into the
    // ones on the killed executor and the ones elsewhere, and in the ten
    // single-fault codes that axis is degenerate because nothing is killed. A
    // composed run is the first one that fills it in, and the survivors are its
    // control group for a scheduler that cannot reach its own schema.
    if composed_config.is_some() {
        let chosen = match scheduled::select(&ctx, deps, &targets).await {
            Ok(chosen) => chosen,
            Err(e) => {
                warn!("{code}: could not aim the second fault at an executor: {e:#}");
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
            "{code}: second fault aimed at {} ({} of {} targets on it)",
            chosen.pod_address,
            chosen.on_pod.len(),
            targets.len()
        );
        selection = Some(chosen);
    }

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "{code}: baseline phase, mixed workload at {} ops/s plus {} schedule targets, for {:?}",
        workload_config.rate_per_sec,
        targets.len(),
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let mixed = workload::start(ctx.clone(), workload_config);
    let schedules = scheduled::start(ctx.clone(), &targets, scheduled_config);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    // Only for the composed codes. The single-fault ones kill nothing, so every
    // sample would report the same assignment and the findings that only exist
    // between two samples would never fire.
    if composed_config.is_some() {
        ownership_samples.push(sample_ownership(deps, "before-fault", None, true).await);
    }
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    // Both handles have to be stopped on every exit path, and forgetting one
    // leaves its emitters submitting into a history that has already been
    // snapshotted.
    macro_rules! stop_workloads {
        () => {{
            mixed.stop().await;
            schedules.stop().await;
        }};
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        // Taking a database away from a workload that never worked would
        // measure nothing. Stop before touching the cluster.
        warn!("{code}: baseline produced no confirmed operations, aborting before injection");
        stop_workloads!();
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

    // Registering is not firing. The scheduler is one of the three mechanisms
    // this scenario is about, and a platform that accepted every registration
    // and ran none of them would otherwise reach read-back and report a
    // flawless account of a mechanism that never worked.
    //
    // Only when the run drives the stream at all. A scenario that registers
    // nothing has no fire to observe, so holding it to this gate aborts it
    // during the baseline and before the fault is ever injected. That is what
    // happened to S15A's first run, and it cost a cluster run to learn.
    let drives_scheduled = config.drives_stream(Stream::Scheduled);
    let sampled = if drives_scheduled {
        sample_fire_count(code, &ctx, &targets).await
    } else {
        0
    };
    if drives_scheduled && sampled == 0 {
        warn!(
            "{code}: {baseline_operations} operations confirmed and no scheduled action has fired"
        );
        stop_workloads!();
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
    let fires = if drives_scheduled {
        format!(
            "{sampled} fires across a sample of {} targets",
            FIRE_PROOF_SAMPLE.min(targets.len())
        )
    } else {
        "scheduled stream not driven".to_string()
    };
    info!(
        "{code}: baseline complete ({baseline_operations} confirmed ops, {fires}), signalling readiness"
    );

    // ── Signal: ready for the fault ─────────────────────────────────────────
    //
    // A single-fault code has nothing to aim: the partition is between every
    // executor and one endpoint outside the cluster, so there is no pod for the
    // driver to choose and no ownership to verify. A composed code names the
    // executor its kill has to hit, and re-checks the division first — the
    // baseline has just run for five minutes, and a selection made before it is
    // a claim about a cluster that has had time to move underneath it.
    let mut fault_target = None;
    if let Some(chosen) = &selection {
        if let Err(e) = scheduled::verify_ownership(&ctx, deps, chosen).await {
            warn!("{code}: target ownership no longer holds, refusing to inject: {e:#}");
            stop_workloads!();
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
        fault_target = Some(FaultTarget {
            pod_address: chosen.pod_address.clone(),
            pod_ip: chosen.pod_ip.clone(),
            owned_agents: chosen.on_pod.clone(),
        });
    }
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: code.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        fault_target,
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("{code}: no fault-injected signal arrived: {e}");
            stop_workloads!();
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
        "{code}: fault {} ({} on {}) reported active at {}, {} is now unreachable from the executors",
        injected.fault_id,
        injected.kind,
        injected.target,
        injected.injected_at,
        storage_config.endpoint
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    // ── The second fault ────────────────────────────────────────────────────
    //
    // Bounded by the enclosing window rather than by the generous signal
    // timeout every other wait here uses. A second fault that has not landed by
    // the time the first one heals is never going to land inside it, and a
    // driver still blocked on it would sleep through the heal it measures
    // recovery from.
    //
    // A wait that runs out is not an abort. The run still has an enclosing
    // fault, a workload and every account below this one; what it does not have
    // is the composition, and the report says exactly that rather than the run
    // ending with nothing.
    if composed_config.is_some() {
        let deadline = config.phases.fault() + SECONDARY_WAIT_MARGIN;
        match signals.await_secondary_fault(deadline).await {
            Ok(signal) => {
                info!(
                    "{code}: second fault {} ({} on {}) reported active at {}",
                    signal.fault_id, signal.kind, signal.target, signal.injected_at
                );
                ownership_samples.push(
                    sample_ownership(deps, "after-kill", ownership_samples.last(), false).await,
                );
                routing_snapshots.push(snapshot_routing(deps, "after-kill").await);
                secondary = Some(signal);

                // Then again once the shard-manager has had time to react.
                // Capped at half of what is left of the window, so the sample
                // cannot land after the heal and describe a cluster that had
                // its storage back — which would report the reassignment as
                // having worked all along.
                if let Some(composed) = composed_config {
                    let remaining = config
                        .phases
                        .fault()
                        .mul_f64((1.0 - composed.after_fraction).max(0.0));
                    let settle = REASSIGNMENT_SETTLE.min(remaining / 2);
                    info!("{code}: sampling shard assignment again in {settle:?}");
                    tokio::time::sleep(settle).await;
                    ownership_samples.push(
                        sample_ownership(deps, "during-fault", ownership_samples.last(), false)
                            .await,
                    );
                    routing_snapshots.push(snapshot_routing(deps, "during-fault").await);
                }
            }
            Err(e) => {
                warn!("{code}: no secondary-fault signal arrived within {deadline:?}: {e}");
            }
        }
    }

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("{code}: no fault-recovered signal arrived: {e}");
            stop_workloads!();
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
        "{code}: fault cleared at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // Built here rather than at the end, so an abort during recovery or
    // read-back still says whether the two faults ever met. Every account below
    // this point describes a cluster that was under both of them, and a reader
    // who cannot tell that from a reader who cannot tell the difference is the
    // failure this report exists to prevent.
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
    info!(
        "{code}: recovery phase, running for a further {:?}",
        config.phases.recovery()
    );
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    // A composed run takes one more sample early in the recovery phase, and
    // MF1's first run is why. It left the fault window with 564 of 1024 shards
    // assigned to nobody and reached the end of a ten-minute recovery phase with
    // all 1024 assigned again, so the one thing the run most wanted to say — how
    // long after the storage came back the handover completed — was somewhere
    // inside a ten-minute gap between two samples. Waiting the same settle the
    // during-fault sample uses turns that into an answer or a bound.
    if composed_config.is_some() {
        let settle = REASSIGNMENT_SETTLE.min(config.phases.recovery());
        tokio::time::sleep(settle).await;
        ownership_samples
            .push(sample_ownership(deps, "after-heal", ownership_samples.last(), false).await);
        routing_snapshots.push(snapshot_routing(deps, "after-heal").await);
        tokio::time::sleep(config.phases.recovery() - settle).await;
    } else {
        tokio::time::sleep(config.phases.recovery()).await;
    }

    let skipped = schedules.skipped();
    // Stopping waits for in-flight operations to record themselves rather than
    // cancelling them: an operation cancelled mid-flight is one the history
    // cannot classify, and during a storage outage those are exactly the
    // interesting ones.
    stop_workloads!();

    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    if skipped > 0 {
        attention_extra.push(Note::attention(format!(
            "{code} skipped {skipped} registration ticks because targets still had their budget \
             of {} in flight — the offered rate was clamped by the platform, so the phase counts \
             understate what the run intended to submit",
            scheduled::MAX_IN_FLIGHT_PER_TARGET
        )));
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);
    if composed_config.is_some() {
        ownership_samples
            .push(sample_ownership(deps, "after-recovery", ownership_samples.first(), true).await);
    }

    // ── Read-back ───────────────────────────────────────────────────────────
    let settle = settle_before_readback(scheduled_config);
    info!("{code}: letting the last actions fall due and fire, {settle:?} before read-back");
    tokio::time::sleep(settle).await;

    let records = history.snapshot();
    let logs = scheduled::read_logs(&ctx, &targets).await;
    // Archived alongside the operations, not just reduced into the report: the
    // reduction is the part a later ticket is most likely to want to redo.
    history.record_fire_logs(logs.clone());

    let fault_window = fault_injected_at.map(|injected_at| FaultWindow {
        injected_at,
        recovered_at: fault_recovered_at,
    });

    let killed_targets: BTreeSet<String> = selection
        .as_ref()
        .map(|chosen| chosen.on_pod.iter().cloned().collect())
        .unwrap_or_default();
    let fires = ScheduleFireReport::build(
        &records,
        &logs,
        scheduled_config.lead(),
        fault_window,
        // Empty for the ten single-fault codes: nothing is killed, so every
        // target belongs to the report's `elsewhere` group and only its window
        // axis carries meaning. A composed run fills it in with the targets the
        // killed executor owned, which is what turns the survivors into a
        // control group.
        &killed_targets,
        scheduled_config.lease_budget(),
    );
    info!(
        "{code}: scheduled-fire account — {} registrations accepted, {} fired once, {} \
         inconclusive, {} unverifiable, {} findings",
        fires.registrations_confirmed,
        fires.fired_once,
        fires.inconclusive,
        fires.unverifiable,
        fires.findings.len()
    );

    let outage = StorageFaultReport::build(
        &records,
        fault_window,
        &storage_config.endpoint,
        storage_config.expect.clone(),
        storage_config.recovery_budget(),
    );
    info!(
        "{code}: storage account — the streams expected to stop were silent for at least {:?}% of \
         the fault window (floor {}%) and the streams expected to carry on held at least {:?}% of \
         their baseline while {} was unreachable, {} findings",
        outage.quietest_stream_percent,
        outage.expect.quiet_floor_percent(),
        outage.least_serving_stream_percent,
        outage.endpoint,
        outage.findings.len()
    );
    for finding in &outage.findings {
        warn!("{code}: {}: {}", finding.violation, finding.detail);
    }

    let readback = read_back(&ctx, &records, workload_config, &logs).await;

    // The idempotency half of the account. A key that timed out while the
    // database was gone may or may not have executed; re-invoking it under the
    // same key says which, because a platform that stored the result replays it
    // and one that did not runs the work again.
    let before_probe = read_counters(&ctx, &records).await;
    let probes = probe::probe_keys(&ctx, &records, Stream::Durable).await;
    let after_probe = read_counters(&ctx, &records).await;
    let exactly_once = ExactlyOnceReport::build(
        &records,
        &probes,
        Stream::Durable,
        &before_probe,
        &after_probe,
    );
    info!(
        "{code}: exactly-once account — {} keys checked, {} with a final result, {} recovered by \
         the probe, {} findings",
        exactly_once.keys_checked,
        exactly_once.keys_with_final_result,
        exactly_once.keys_recovered_by_probe,
        exactly_once.findings.len()
    );

    let reason = termination(&fires, &exactly_once, &records);

    finish!(
        reason,
        &records,
        readback,
        Some(fires),
        Some(outage),
        Some(exactly_once)
    );
}

/// Why the run stopped.
///
/// Kept apart from [`run`] so the precedence is testable without a cluster. The
/// order matters: a token-level fire violation and a duplicated key are both
/// statements about one named thing, and either is worth more to a reader than
/// "the workload never worked", which is a statement about the run rather than
/// about the platform.
fn termination(
    fires: &ScheduleFireReport,
    exactly_once: &ExactlyOnceReport,
    records: &[OperationRecord],
) -> TerminationReason {
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
    if let Some(reason) = exactly_once_termination(exactly_once) {
        return reason;
    }
    if records.iter().all(|r| r.outcome != Outcome::Confirmed) {
        return TerminationReason::StreamNeverSucceeded {
            stream: Stream::Durable.to_string(),
        };
    }
    TerminationReason::Completed
}

/// How long to wait after the workload stops before reading anything back.
///
/// Same derivation as S10's, and load-bearing for the same reason: the last
/// registration falls due one `lead` after the loop stops, and a delayed action
/// can cost a lease budget on top of that.
fn settle_before_readback(config: &ScheduledConfig) -> Duration {
    config.lead() + config.lease_budget() + SETTLE_MARGIN
}

/// Reads durable state back for the streams that keep a count.
///
/// The durable counters come from the mixed workload's own agents; the
/// scheduled counts come from the fire logs that were already read, because
/// re-reading `polls` would be a second round trip for a number the log already
/// carries.
async fn read_back(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    config: &crate::chaos::WorkloadConfig,
    logs: &[crate::chaos::history::TargetFireLog],
) -> Vec<AgentReadback> {
    let mut readback = Vec::new();

    for index in 0..config.durable_agents {
        let agent = ctx.agent_name(Stream::Durable, index);
        let scoped = records
            .iter()
            .filter(|r| r.stream == Stream::Durable && r.agent == agent);
        if scoped.clone().next().is_none() {
            continue;
        }
        let observed = workload::read_counter(ctx, &agent).await;
        readback.extend(readback_for(Stream::Durable, &agent, scoped, observed));
    }

    for log in logs {
        let scoped = records
            .iter()
            .filter(|r| r.stream == Stream::Scheduled && r.agent == log.agent);
        let observed = match (log.polls, &log.error) {
            (Some(polls), _) => Ok(polls),
            (None, Some(error)) => Err(error.clone()),
            (None, None) => Err(format!("target {} reported no poll count", log.agent)),
        };
        readback.extend(readback_for(
            Stream::Scheduled,
            &log.agent,
            scoped,
            observed,
        ));
    }

    readback
}

/// Reads the fire count of a few targets, to prove actions are firing at all.
async fn sample_fire_count(code: ScenarioCode, ctx: &WorkloadContext, targets: &[String]) -> u64 {
    let mut total = 0u64;
    for target in targets.iter().take(FIRE_PROOF_SAMPLE) {
        match workload::read_polls(ctx, target).await {
            Ok(polls) => total += polls,
            Err(e) => warn!("{code}: could not sample fires on {target}: {e}"),
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::OutageExpectation;
    use crate::chaos::history::{AttemptRecord, FireRecord, TargetFireLog};
    use crate::chaos::outage::{OutageFinding, OutageViolation};
    use chrono::{DateTime, TimeDelta};
    use test_r::test;

    const TARGET: &str = "chaos-s16-schedule-target-0000";
    const LEAD: Duration = Duration::from_secs(10);
    const BUDGET: Duration = Duration::from_secs(240);

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn registration(token: &str, outcome: Outcome) -> OperationRecord {
        OperationRecord {
            op_id: 0,
            stream: Stream::Scheduled,
            phase: Phase::Baseline,
            agent: TARGET.to_string(),
            method: "schedule_fire_at".to_string(),
            idempotency_key: token.to_string(),
            submitted_at: t0(),
            completed_at: Some(t0() + TimeDelta::milliseconds(20)),
            attempts: 1,
            outcome,
            duration_ms: 20,
            returned_value: None,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: vec![AttemptRecord {
                attempt: 1,
                started_at: t0(),
                duration_ms: 20,
                returned_value: None,
                succeeded: outcome == Outcome::Confirmed,
                error_class: None,
                error: None,
            }],
        }
    }

    fn log(fired: &[&str]) -> TargetFireLog {
        TargetFireLog {
            agent: TARGET.to_string(),
            polls: Some(fired.len() as u64),
            fires: fired
                .iter()
                .map(|token| FireRecord {
                    token: (*token).to_string(),
                    scheduled_at: t0() + TimeDelta::seconds(10),
                    observed_at: t0() + TimeDelta::seconds(11),
                })
                .collect(),
            error: None,
        }
    }

    fn fires(records: &[OperationRecord], logs: &[TargetFireLog]) -> ScheduleFireReport {
        ScheduleFireReport::build(records, logs, LEAD, None, &BTreeSet::new(), BUDGET)
    }

    /// The empty account: nothing registered, nothing fired, nothing to say.
    fn clean_exactly_once() -> ExactlyOnceReport {
        ExactlyOnceReport::build(
            &[],
            &[],
            Stream::Durable,
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        )
    }

    /// A registration the platform accepted whose action never ran is a
    /// statement about one named token, and it outranks every aggregate.
    #[test]
    fn a_token_that_never_fired_fails_the_run() {
        let records = vec![registration("token-a", Outcome::Confirmed)];
        let reason = termination(
            &fires(&records, &[log(&[])]),
            &clean_exactly_once(),
            &records,
        );

        assert!(
            matches!(reason, TerminationReason::ScheduledFireViolated { .. }),
            "expected a fire violation, got {reason:?}"
        );
    }

    /// A fire violation outranks "the workload never worked", because it says
    /// something about the platform and the other says something about the run.
    #[test]
    fn a_fire_violation_outranks_a_workload_that_never_confirmed_anything() {
        // Rejected registrations mean nothing confirmed anywhere, and a fire
        // that happened anyway is the sharpest thing this suite can find.
        let records = vec![registration("token-a", Outcome::Rejected)];
        let reason = termination(
            &fires(&records, &[log(&["token-a"])]),
            &clean_exactly_once(),
            &records,
        );

        assert!(
            matches!(reason, TerminationReason::ScheduledFireViolated { .. }),
            "expected a fire violation, got {reason:?}"
        );
    }

    /// A run where nothing confirmed at all says nothing about resilience, so
    /// it fails rather than reporting a clean account of a workload that never
    /// worked.
    #[test]
    fn a_workload_that_never_confirmed_anything_fails_the_run() {
        let records = vec![registration("token-a", Outcome::Indeterminate)];
        let reason = termination(
            &fires(&records, &[log(&["token-a"])]),
            &clean_exactly_once(),
            &records,
        );

        assert!(
            matches!(reason, TerminationReason::StreamNeverSucceeded { .. }),
            "expected a never-succeeded reason, got {reason:?}"
        );
    }

    /// The healthy shape completes, and the storage account's own findings do
    /// not change that: an outage that failed to land is loud in `attention`
    /// and deliberately not fatal.
    #[test]
    fn a_healthy_run_completes() {
        let records = vec![registration("token-a", Outcome::Confirmed)];
        let reason = termination(
            &fires(&records, &[log(&["token-a"])]),
            &clean_exactly_once(),
            &records,
        );

        assert_eq!(reason, TerminationReason::Completed);
    }

    /// The storage account's findings never reach the termination reason. An
    /// outage that failed to land is the loudest thing these scenarios can
    /// report, and it is deliberately not fatal: turning the job red would say
    /// the platform did something wrong, and what actually went wrong is the
    /// experiment.
    #[test]
    fn a_storage_finding_does_not_change_the_termination_reason() {
        let mut outage = StorageFaultReport::build(
            &[],
            None,
            "db.example",
            OutageExpectation::WholeWorkload {
                quiet_floor_percent: 15.0,
            },
            Duration::from_secs(120),
        );
        outage.findings.push(OutageFinding {
            violation: OutageViolation::OutageNotObserved,
            stream: None,
            detail: "kept working".to_string(),
        });
        assert!(outage.has_findings());

        let records = vec![registration("token-a", Outcome::Confirmed)];
        let reason = termination(
            &fires(&records, &[log(&["token-a"])]),
            &clean_exactly_once(),
            &records,
        );
        assert_eq!(reason, TerminationReason::Completed);
    }
}

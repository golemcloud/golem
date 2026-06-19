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

//! Agent-density benchmark driver (golemcloud/golem#3523).
//!
//! Answers: how many agents can a single worker-executor pod hold before
//! something falls over, across realistic workload mixes. Each cell runs in its
//! own freshly-restarted, state-wiped executor (the buildspec drives the
//! between-cell transition), and the benchmark binary runs exactly one cell per
//! invocation via the `density` subcommand.
//!
//! A cell ramps the agent count along [`super::AGENT_RAMP`], feeding the
//! per-attempt latency and out-of-band executor state into the
//! [`super::ceiling::CeilingDetector`], until a catastrophic ceiling fires or
//! the sharing mode's upper bound is hit. The cell records the agent counts at
//! which the soft, hard, and catastrophic ceilings were crossed.
//!
//! # Scenarios
//!
//! 1. create-only: create N agents, each invoked once then left idle (drifts to
//!    `LoadedIdle`, eviction-eligible). Measures the create path.
//! 2. create-with-active-fraction: like (1), but a fraction of the agents are
//!    kept active (one in-flight `busy_for(500ms)` call each, looping).
//! 3. concurrent-active: all N agents kept active concurrently.
//! 4. resume-under-saturation: durable-only. Pre-fill the pod with `prefill_n`
//!    idle agents (forcing eviction), then measure the latency of resuming an
//!    already-evicted agent vs. creating a fresh one.
//!
//! # Operational definitions (from #3523)
//!
//! - Active: the driver maintains exactly one in-flight `busy_for(500ms)` call
//!   per active agent, looping continuously.
//! - Passive (idle): created and invoked once, then left to drift into
//!   `LoadedIdle` and become eviction-eligible.
//! - Soft / hard / catastrophic ceilings: see [`super::ceiling`].

use crate::benchmarks::density::ceiling::{
    CeilingDetector, CeilingEvent, CrossAxisSnapshot, Sample, SampleCoord, TerminatedReason,
};
use crate::benchmarks::density::prep::PrepManifest;
use crate::benchmarks::density::{AgentMode, ComponentSharing};
use futures::StreamExt;
use golem_common::agent_id;
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::data_value;
use golem_common::model::AgentId;
use golem_common::model::component::ComponentDto;
use golem_test_framework::benchmark::{
    BenchmarkRecorder, BenchmarkResult, BenchmarkRunResult, ResultKey, RunConfig,
};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::dsl::TestDsl;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

/// Agent type names exported by the agent-counters component.
const DURABLE_AGENT_TYPE: &str = "Counter";
const EPHEMERAL_AGENT_TYPE: &str = "EphemeralCounter";

/// CPU busy time per `busy_for` call defining an "active" agent.
const BUSY_MILLIS: u32 = 500;

/// Maximum number of agent-create invocations in flight at once while ramping a
/// cell. The cost of a step is dominated by the round-trips for the new agents,
/// so fanning them out cuts wall-clock from hours to minutes. The cap keeps the
/// driver's own connection pool from becoming the bottleneck.
const CREATE_CONCURRENCY: usize = 100;
/// Fraction of a ramp batch that must fail at the transport level (request
/// could not be sent / no round-trip) before the batch is judged as the
/// executor being unreachable. A single transient send failure must not end a
/// cell; a wedged or restarting executor produces a flood of them, which this
/// threshold catches so the catastrophic connection-lost condition fires.
const TRANSPORT_FAILURE_RATIO: f64 = 0.5;

/// Upper bound on per-cell agent-deletion wall time. Cleanup against a healthy
/// executor finishes in seconds even for tens of thousands of agents; the
/// budget only fires if the executor degrades, so the run cannot stall on it.
const CLEANUP_BUDGET: Duration = Duration::from_secs(600);

/// Which density scenario a cell runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Create N idle agents (each invoked once, then left to drift idle).
    CreateOnly,
    /// Create N agents, keep `active_fraction`% of them active.
    CreateWithActiveFraction,
    /// Create N agents, keep all active concurrently.
    ConcurrentActive,
    /// Durable-only: pre-fill with `prefill_n` idle agents, then measure
    /// resume-vs-create latency under eviction pressure.
    ResumeUnderSaturation,
}

impl Scenario {
    pub fn as_str(self) -> &'static str {
        match self {
            Scenario::CreateOnly => "create-only",
            Scenario::CreateWithActiveFraction => "create-with-active",
            Scenario::ConcurrentActive => "concurrent-active",
            Scenario::ResumeUnderSaturation => "resume-under-saturation",
        }
    }
}

/// Fully describes one cell of the agent-density matrix.
#[derive(Debug, Clone)]
pub struct CellConfig {
    pub scenario: Scenario,
    pub mode: AgentMode,
    pub sharing: ComponentSharing,
    /// Percentage of agents kept active (scenario 2 only): 25 / 50 / 75.
    pub active_fraction: Option<u32>,
    /// Number of idle agents to pre-fill before measuring (scenario 4 only).
    pub prefill_n: Option<u32>,
    /// Increasing agent-count ramp the driver walks for this cell. Supplied by
    /// the suite YAML (per-cell `ramp` or the suite `defaultRamp`); falls back
    /// to [`super::DEFAULT_AGENT_RAMP`] when neither is given.
    pub ramp: Vec<u32>,
}

impl CellConfig {
    /// The cell's name, used in S3 paths and result identifiers. Encodes the
    /// full axis set in human-readable terms (no cryptic single letters).
    pub fn cell_name(&self) -> String {
        let mut parts = vec![
            self.scenario.as_str().to_string(),
            self.mode.as_str().to_string(),
            self.sharing.as_str().to_string(),
        ];
        if let Some(f) = self.active_fraction {
            parts.push(format!("active-{f}pct"));
        }
        if let Some(n) = self.prefill_n {
            parts.push(format!("prefill-{n}"));
        }
        parts.join("-")
    }

    /// The agent type name for this cell's durability mode.
    fn agent_type(&self) -> &'static str {
        match self.mode {
            AgentMode::Durable => DURABLE_AGENT_TYPE,
            AgentMode::Ephemeral => EPHEMERAL_AGENT_TYPE,
        }
    }
}

/// Out-of-band executor observations a cell needs that are not derivable from
/// the driver's own invocation results. The only such signal in v1 is the pod
/// restart count (driving the catastrophic pod-restart condition), read via
/// kubectl. Executor `/metrics` is not scraped in v1 — the S3 result carries
/// only what the driver measures itself; executor-side context is read from the
/// Grafana dashboards using the run's start/end timing.
pub struct ExecutorProbe {
    /// Optional executor pod name + namespace for `kubectl` restart-count
    /// polling. When `None`, the pod-restart catastrophic condition relies on
    /// the connection-lost backstop instead.
    pub pod_name: Option<String>,
    pub namespace: String,
    /// The pod's container restart count at cell start. The pod is long-lived
    /// across cells, so its absolute restart count may already be non-zero;
    /// only restarts beyond this baseline indicate a restart during the cell.
    baseline_restart_count: u64,
}

impl ExecutorProbe {
    /// Builds a probe for the given pod, capturing the current restart count as
    /// the baseline so `pod_restart_count` reports restarts during the cell.
    pub async fn new(pod_name: Option<String>, namespace: String) -> Self {
        let mut probe = Self {
            pod_name,
            namespace,
            baseline_restart_count: 0,
        };
        probe.baseline_restart_count = probe.raw_restart_count().await;
        probe
    }

    /// Restarts observed since cell start: the pod's current container restart
    /// count minus the baseline captured at construction. Returns 0 on any
    /// failure (the connection-lost condition backstops a missed restart).
    /// Cheap: called once per ramp step, not per invocation.
    async fn pod_restart_count(&self) -> u64 {
        self.raw_restart_count()
            .await
            .saturating_sub(self.baseline_restart_count)
    }

    /// Reads the executor pod's absolute container restart count via `kubectl`.
    async fn raw_restart_count(&self) -> u64 {
        let Some(pod) = &self.pod_name else {
            return 0;
        };
        let output = tokio::process::Command::new("kubectl")
            .args([
                "get",
                "pod",
                pod,
                "-n",
                &self.namespace,
                "-o",
                "jsonpath={.status.containerStatuses[0].restartCount}",
            ])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse()
                .unwrap_or(0),
            Ok(out) => {
                warn!(
                    "density: kubectl restartCount failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                0
            }
            Err(e) => {
                warn!("density: kubectl restartCount spawn failed: {e:?}");
                0
            }
        }
    }
}

/// The component an agent at `index` uses, plus its agent id, for this cell's
/// sharing mode. For shared sharing every agent uses `components[0]`; for
/// per-agent sharing agent `index` uses `components[index]`.
fn agent_for_index<'a>(
    config: &CellConfig,
    index: u32,
    components: &'a [ComponentDto],
) -> anyhow::Result<(&'a ComponentDto, ParsedAgentId)> {
    let agent_type = config.agent_type();
    let agent_id = agent_id!(agent_type, format!("{}-{index}", config.cell_name()));
    let component = match config.sharing {
        ComponentSharing::Shared => components
            .first()
            .ok_or_else(|| anyhow::anyhow!("no shared component available"))?,
        ComponentSharing::PerAgent => {
            let pos = (index as usize) % components.len();
            &components[pos]
        }
    };
    Ok((component, agent_id))
}

/// Classification of a single invocation attempt.
struct AttemptOutcome {
    latency: Duration,
    /// True if the attempt failed with a connection-level error (as opposed to
    /// a normal application error or a timeout), which the ceiling detector
    /// treats as the catastrophic connection-lost condition.
    connection_lost: bool,
}

/// Per-attempt client timeout. Starts at 30s (the hard-ceiling threshold);
/// escalated to 5 minutes once the hard ceiling is crossed, so the eventual
/// catastrophic 5-minute-timeout condition can fire.
struct AdaptiveTimeout {
    current: Duration,
}

impl AdaptiveTimeout {
    fn new() -> Self {
        Self {
            current: super::ceiling::HARD_CEILING_THRESHOLD,
        }
    }

    fn escalate(&mut self) {
        self.current = super::ceiling::ESCALATED_TIMEOUT;
    }
}

/// Heuristically classifies an invocation error as connection-level (transport
/// failure / reset / refused) vs. an application error. Connection-level errors
/// are the driver-local signal for the catastrophic connection-lost condition,
/// which is the backstop for an OOM-killed pod even when the kubectl
/// restart-count poll misses it.
/// Heuristically classifies an invocation error as transport-level (the request
/// never completed a round-trip) vs. an application error. Transport-level
/// errors are the driver-local signal for the catastrophic connection-lost
/// condition — they spike when the executor wedges and the gateway can no
/// longer reach it. "error sending request" / reqwest-middleware errors mean
/// the request could not even be sent, so they count as transport failures.
fn is_connection_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:?}").to_lowercase();
    msg.contains("connection")
        || msg.contains("connect")
        || msg.contains("reset")
        || msg.contains("broken pipe")
        || msg.contains("eof")
        || msg.contains("refused")
        || msg.contains("unavailable")
        || msg.contains("transport")
        || msg.contains("error sending request")
        || msg.contains("middleware error")
        || msg.contains("dns")
        || msg.contains("timed out")
}

/// Invokes one agent method, measuring wall-clock latency and classifying the
/// outcome. Unlike the cloud-perf `invoke_and_await_agent` helper this does not
/// retry — the ceiling detector, not a retry loop, decides when a cell is done.
async fn timed_invoke(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    method: &str,
    params: golem_common::base_model::agent::DataValue,
    timeout: Duration,
) -> AttemptOutcome {
    let start = Instant::now();
    let result = tokio::time::timeout(
        timeout,
        user.invoke_and_await_agent(component, agent_id, method, params),
    )
    .await;
    let latency = start.elapsed();

    match result {
        Ok(Ok(_)) => AttemptOutcome {
            latency,
            connection_lost: false,
        },
        Ok(Err(e)) => {
            let connection_lost = is_connection_error(&e);
            if !connection_lost {
                warn!("density: invocation error (non-connection): {e:?}");
            }
            AttemptOutcome {
                latency,
                connection_lost,
            }
        }
        Err(_) => {
            // Timed out: report the timeout duration as the latency so the
            // detector's hard/catastrophic thresholds can fire on it.
            AttemptOutcome {
                latency: timeout,
                connection_lost: false,
            }
        }
    }
}

// ── Result schema ─────────────────────────────────────────────────────────

/// Named count keys emitted in a cell's `BenchmarkResult` (the S3 schema from
/// golemcloud/golem#3523).
mod keys {
    pub const SOFT_CEILING_AGENTS: &str = "soft-ceiling-agents";
    pub const USABILITY_CEILING_AGENTS: &str = "usability-ceiling-agents";
    pub const HARD_CEILING_AGENTS: &str = "hard-ceiling-agents";
    pub const CATASTROPHIC_CEILING_AGENTS: &str = "catastrophic-ceiling-agents";
    /// `TerminatedReason` integer code (oom-kill=1 .. upper-bound-hit=4).
    pub const TERMINATED_REASON: &str = "terminated-reason";
    /// Highest agent count reached before the cell stopped.
    pub const MAX_AGENTS_REACHED: &str = "max-agents-reached";

    /// Invoke-latency distribution key (create/invoke round-trip times).
    pub const INVOKE_LATENCY: &str = "invoke-latency";

    // Scenario-4 (resume-under-saturation) latencies, in milliseconds.
    pub const RESUME_EXISTING_P50_MS: &str = "resume-existing-p50-ms";
    pub const RESUME_EXISTING_P99_MS: &str = "resume-existing-p99-ms";
    pub const CREATE_FRESH_P50_MS: &str = "create-fresh-p50-ms";
    pub const CREATE_FRESH_P99_MS: &str = "create-fresh-p99-ms";
}

/// The outcome of running one cell: the agent counts at which each ceiling was
/// crossed (`None` if never crossed) and why the cell stopped.
#[derive(Debug)]
struct CellOutcome {
    soft_ceiling_agents: Option<u32>,
    usability_ceiling_agents: Option<u32>,
    hard_ceiling_agents: Option<u32>,
    catastrophic_ceiling_agents: Option<u32>,
    terminated_reason: TerminatedReason,
    max_agents_reached: u32,
    /// Every create/invoke latency the cell measured, surfaced as an
    /// invoke-latency percentile distribution (avg/min/max/p50/p90/p95/p99).
    invoke_latencies: Vec<Duration>,
    /// Scenario-4 resume/create latency samples (milliseconds).
    resume_existing_ms: Vec<f64>,
    create_fresh_ms: Vec<f64>,
}

impl Default for CellOutcome {
    fn default() -> Self {
        Self {
            soft_ceiling_agents: None,
            usability_ceiling_agents: None,
            hard_ceiling_agents: None,
            catastrophic_ceiling_agents: None,
            // A cell that never reaches catastrophic stops because it hit its
            // sharing-mode upper bound.
            terminated_reason: TerminatedReason::UpperBoundHit,
            max_agents_reached: 0,
            invoke_latencies: Vec::new(),
            resume_existing_ms: Vec::new(),
            create_fresh_ms: Vec::new(),
        }
    }
}

impl CellOutcome {
    /// Builds the cell's `BenchmarkResult` with the named ceiling counts and
    /// (for scenario 4) resume/create latency percentiles.
    fn into_benchmark_result(self, config: &CellConfig) -> BenchmarkResult {
        let recorder = BenchmarkRecorder::new();

        // `null` ceilings (never crossed) are recorded as the max reached so the
        // result distinguishes "crossed at N" from "did not cross within bound".
        // The terminated_reason disambiguates upper-bound-hit from catastrophic.
        if let Some(n) = self.soft_ceiling_agents {
            recorder.count(&ResultKey::primary(keys::SOFT_CEILING_AGENTS), n as u64);
        }
        if let Some(n) = self.usability_ceiling_agents {
            recorder.count(
                &ResultKey::primary(keys::USABILITY_CEILING_AGENTS),
                n as u64,
            );
        }
        if let Some(n) = self.hard_ceiling_agents {
            recorder.count(&ResultKey::primary(keys::HARD_CEILING_AGENTS), n as u64);
        }
        if let Some(n) = self.catastrophic_ceiling_agents {
            recorder.count(
                &ResultKey::primary(keys::CATASTROPHIC_CEILING_AGENTS),
                n as u64,
            );
        }
        recorder.count(
            &ResultKey::primary(keys::TERMINATED_REASON),
            self.terminated_reason.code(),
        );
        recorder.count(
            &ResultKey::primary(keys::MAX_AGENTS_REACHED),
            self.max_agents_reached as u64,
        );

        // Invoke-latency distribution across every create/invoke the cell made,
        // rendered as the same avg/min/max/p50/p90/p95/p99 table as cloud-perf.
        for latency in &self.invoke_latencies {
            recorder.duration(&ResultKey::primary(keys::INVOKE_LATENCY), *latency);
        }

        if !self.resume_existing_ms.is_empty() {
            recorder.count(
                &ResultKey::primary(keys::RESUME_EXISTING_P50_MS),
                percentile_ms(&self.resume_existing_ms, 50.0),
            );
            recorder.count(
                &ResultKey::primary(keys::RESUME_EXISTING_P99_MS),
                percentile_ms(&self.resume_existing_ms, 99.0),
            );
        }
        if !self.create_fresh_ms.is_empty() {
            recorder.count(
                &ResultKey::primary(keys::CREATE_FRESH_P50_MS),
                percentile_ms(&self.create_fresh_ms, 50.0),
            );
            recorder.count(
                &ResultKey::primary(keys::CREATE_FRESH_P99_MS),
                percentile_ms(&self.create_fresh_ms, 99.0),
            );
        }

        let run_config = RunConfig {
            cluster_size: 0,
            size: self.max_agents_reached as usize,
            length: 0,
            disable_compilation_cache: false,
        };
        let mut run_result = BenchmarkRunResult::new(run_config.clone());
        run_result.add(recorder);

        BenchmarkResult {
            name: format!("density-agent-{}", config.cell_name()),
            description: format!(
                "Agent-density cell: scenario={}, mode={}, sharing={}{}{}",
                config.scenario.as_str(),
                config.mode,
                config.sharing,
                config
                    .active_fraction
                    .map(|f| format!(", active-fraction={f}%"))
                    .unwrap_or_default(),
                config
                    .prefill_n
                    .map(|n| format!(", prefill={n}"))
                    .unwrap_or_default(),
            ),
            runs: vec![run_config],
            results: vec![run_result],
            run_id: None,
        }
    }
}

fn percentile_ms(samples: &[f64], k: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    if n == 1 {
        return sorted[0].round() as u64;
    }
    let p = (k / 100.0) * (n as f64 - 1.0);
    let lo = p.floor() as usize;
    let hi = p.ceil() as usize;
    let v = if lo == hi {
        sorted[lo]
    } else {
        let frac = p - lo as f64;
        sorted[lo] + (sorted[hi] - sorted[lo]) * frac
    };
    v.round() as u64
}

// ── Active-load manager ──────────────────────────────────────────────────────

/// Keeps a set of agents "active": one in-flight `busy_for(BUSY_MILLIS)` call
/// per agent, looping continuously, until [`Self::stop`] is called. Spawns one
/// background task per active agent.
struct ActiveLoad {
    tasks: JoinSet<()>,
    stop: Arc<AtomicBool>,
}

impl ActiveLoad {
    fn new() -> Self {
        Self {
            tasks: JoinSet::new(),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Starts looping `busy_for` on `agent` until stopped.
    fn add(
        &mut self,
        user: TestUserContext<BenchmarkTestDependencies>,
        component: ComponentDto,
        agent: ParsedAgentId,
    ) {
        let stop = self.stop.clone();
        self.tasks.spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let _ = timed_invoke(
                    &user,
                    &component,
                    &agent,
                    "busy_for",
                    data_value!(BUSY_MILLIS),
                    super::ceiling::ESCALATED_TIMEOUT,
                )
                .await;
            }
        });
    }

    async fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tasks.shutdown().await;
    }
}

// ── Ramp loop ────────────────────────────────────────────────────────────────

/// Runs one agent-density cell to completion and returns its
/// `BenchmarkResult`. Walks the agent-count ramp, feeding per-attempt latency
/// and out-of-band executor state into the ceiling detector, stopping at the
/// first catastrophic crossing or the sharing-mode upper bound.
pub async fn run_cell(
    config: &CellConfig,
    manifest: &PrepManifest,
    deps: &BenchmarkTestDependencies,
    probe: &ExecutorProbe,
) -> anyhow::Result<BenchmarkResult> {
    info!("Density-agent: running cell {}", config.cell_name());

    let user = manifest.user_context(deps);
    let components = resolve_components(config, manifest, &user).await?;

    let outcome = match config.scenario {
        Scenario::ResumeUnderSaturation => {
            run_resume_cell(config, &user, &components, probe).await?
        }
        _ => run_ramp_cell(config, &user, &components, probe).await?,
    };

    cleanup_cell_agents(config, &user, &components, &outcome).await;

    Ok(outcome.into_benchmark_result(config))
}

/// Deletes every durable agent this cell created so the next cell starts from a
/// clean executor.
///
/// Only durable agents are deleted. Ephemeral agents do not outlive their
/// invocation — they leave nothing in the `running-workers` set and are already
/// gone, so deleting them only yields `AGENT_NOT_FOUND`.
///
/// Cleanup is skipped when the cell ended catastrophically: the executor is no
/// longer healthy, each `delete_worker` loads the agent into memory before
/// removing it, and pouring thousands of those into a collapsed executor stalls
/// indefinitely. State for catastrophic cells is cleared out-of-band by the
/// buildspec (a from-fresh keyvalue/indexed recreate plus Redis flush).
///
/// Deletion goes through the platform's `delete_worker` API, which removes the
/// agent from the executor's `running-workers` set that the startup recovery
/// scan reads. Created agents occupy indices `0..max_agents_reached` (the same
/// indices the ramp and prefill loops use via `agent_for_index`) and are fanned
/// out with the same bounded concurrency as creation. A whole-cleanup time
/// budget and a transport-failure short-circuit bound the wall time so a
/// degraded executor cannot stall the run.
async fn cleanup_cell_agents(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    outcome: &CellOutcome,
) {
    if config.mode == AgentMode::Ephemeral {
        return;
    }
    if outcome.terminated_reason.is_catastrophic() {
        info!(
            "Density-agent[{}]: skipping agent deletion ({:?}); buildspec clears state from fresh",
            config.cell_name(),
            outcome.terminated_reason
        );
        return;
    }
    let count = outcome.max_agents_reached;
    if count == 0 {
        return;
    }
    info!(
        "Density-agent[{}]: cleaning up {count} created agents",
        config.cell_name()
    );

    let agent_ids: Vec<AgentId> = (0..count)
        .filter_map(|index| {
            let (component, parsed) = agent_for_index(config, index, components).ok()?;
            AgentId::from_agent_id(component.id, &parsed).ok()
        })
        .collect();

    // Stop issuing deletes once a request fails to reach the executor: a flood
    // of transport errors means it has gone unhealthy, and continuing only
    // stalls on per-request timeouts.
    let transport_failed = Arc::new(AtomicBool::new(false));
    let delete_all = futures::stream::iter(agent_ids).for_each_concurrent(
        CREATE_CONCURRENCY,
        |agent_id| {
            let transport_failed = transport_failed.clone();
            async move {
                if transport_failed.load(Ordering::Relaxed) {
                    return;
                }
                if let Err(err) = user.delete_worker(&agent_id).await {
                    let text = err.to_string();
                    if text.contains("AGENT_NOT_FOUND") {
                        debug!("Density-agent: agent {agent_id} already gone");
                    } else if is_transport_error(&text) {
                        transport_failed.store(true, Ordering::Relaxed);
                        warn!(
                            "Density-agent: executor unreachable while deleting {agent_id}, abandoning cleanup: {err:?}"
                        );
                    } else {
                        warn!("Density-agent: failed to delete agent {agent_id}: {err:?}");
                    }
                }
            }
        },
    );

    match tokio::time::timeout(CLEANUP_BUDGET, delete_all).await {
        Ok(()) => info!(
            "Density-agent[{}]: cleanup of {count} agents complete",
            config.cell_name()
        ),
        Err(_) => warn!(
            "Density-agent[{}]: cleanup exceeded {}s budget, abandoning",
            config.cell_name(),
            CLEANUP_BUDGET.as_secs()
        ),
    }
}

/// Whether a `delete_worker` error string indicates the request could not reach
/// the executor (as opposed to a per-agent application error).
fn is_transport_error(text: &str) -> bool {
    text.contains("Middleware error")
        || text.contains("error sending request")
        || text.contains("connection")
        || text.contains("timed out")
}

/// Resolves the `ComponentDto`s this cell needs from the manifest's stored ids.
/// For shared sharing, one component; for per-agent sharing, all distinct
/// components (one per agent, used round-robin up to the ramp bound).
async fn resolve_components(
    config: &CellConfig,
    manifest: &PrepManifest,
    user: &TestUserContext<BenchmarkTestDependencies>,
) -> anyhow::Result<Vec<ComponentDto>> {
    match config.sharing {
        ComponentSharing::Shared => {
            let id = manifest
                .uniform_component_id
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("manifest has no shared component id"))?;
            let component = user.get_latest_component_revision(id).await?;
            Ok(vec![component])
        }
        ComponentSharing::PerAgent => {
            if manifest.distinct_component_ids.is_empty() {
                anyhow::bail!("manifest has no per-agent component ids");
            }
            let mut components = Vec::with_capacity(manifest.distinct_component_ids.len());
            for id in &manifest.distinct_component_ids {
                components.push(user.get_latest_component_revision(id).await?);
            }
            Ok(components)
        }
    }
}

/// How many of the first `n` agents are active for scenarios 1-3.
fn active_count(config: &CellConfig, n: u32) -> u32 {
    match config.scenario {
        Scenario::CreateOnly => 0,
        Scenario::ConcurrentActive => n,
        Scenario::CreateWithActiveFraction => {
            let pct = config.active_fraction.unwrap_or(0);
            ((n as u64 * pct as u64) / 100) as u32
        }
        Scenario::ResumeUnderSaturation => 0,
    }
}

/// Scenarios 1-3: ramp the agent count, each step creating the new agents (and
/// activating the configured fraction), measuring the create-path latency of
/// the newly-added batch.
async fn run_ramp_cell(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    probe: &ExecutorProbe,
) -> anyhow::Result<CellOutcome> {
    let ramp = config.ramp.clone();
    let mut detector = CeilingDetector::new();
    let mut outcome = CellOutcome::default();
    let mut active = ActiveLoad::new();
    let mut timeout = AdaptiveTimeout::new();
    let started = Instant::now();
    let mut created = 0u32;

    'ramp: for &target in &ramp {
        info!(
            "Density-agent[{}]: ramping to {target} agents",
            config.cell_name()
        );

        // Create (first-invoke) the new agents from `created` to `target`
        // concurrently with bounded in-flight count, then feed their latency
        // samples to the detector in index order (the detector is sequential).
        // Sequential creation made a 10000-agent step take hours; fanning the
        // round-trips out is the dominant speedup.
        let batch: Vec<u32> = (created..target).collect();
        let timeout_current = timeout.current;
        let attempts: Vec<(u32, AttemptOutcome)> = futures::stream::iter(batch)
            .map(|index| {
                let (component, agent) = agent_for_index(config, index, components)
                    .expect("agent_for_index within ramp");
                let component = component.clone();
                async move {
                    let outcome = timed_invoke(
                        user,
                        &component,
                        &agent,
                        "increment",
                        data_value!(),
                        timeout_current,
                    )
                    .await;
                    (index, outcome)
                }
            })
            .buffer_unordered(CREATE_CONCURRENCY)
            .collect()
            .await;

        // One pod-restart poll for the whole batch (cheap; a mid-batch restart
        // is also caught by the connection-lost signal on the affected calls).
        let pod_restart_count = probe.pod_restart_count().await;
        detector.set_elapsed_secs(started.elapsed().as_secs_f64());

        let mut ordered = attempts;
        ordered.sort_by_key(|(index, _)| *index);

        // Batch-level transport verdict: only declare the connection lost when a
        // large fraction of the batch failed to send, so a single transient
        // send error does not end an otherwise-healthy cell, while a wedged or
        // unreachable executor (a flood of send failures) trips catastrophic.
        let transport_failures = ordered.iter().filter(|(_, a)| a.connection_lost).count();
        let batch_connection_alive = ordered.is_empty()
            || (transport_failures as f64) < (ordered.len() as f64 * TRANSPORT_FAILURE_RATIO);
        if !batch_connection_alive {
            warn!(
                "Density-agent[{}]: {transport_failures}/{} invocations failed at transport level — treating executor as unreachable",
                config.cell_name(),
                ordered.len()
            );
        }

        for (index, attempt) in &ordered {
            outcome.invoke_latencies.push(attempt.latency);
            let sample = Sample {
                latency: attempt.latency,
                coord: SampleCoord::Agents(index + 1),
                pod_restart_count,
                connection_alive: batch_connection_alive,
                snapshot: CrossAxisSnapshot::default(),
                queue_depth: None,
            };
            for event in detector.observe(&sample) {
                handle_event(event, &mut outcome, &mut timeout);
            }
            if detector.is_terminal() {
                outcome.max_agents_reached = index + 1;
                active.stop().await;
                return Ok(outcome);
            }
        }
        created = target;
        outcome.max_agents_reached = target;

        // Bring the active set up to the configured fraction of `target`.
        let want_active = active_count(config, target);
        let already_active = active.tasks.len() as u32;
        for index in already_active..want_active {
            let (component, agent) = agent_for_index(config, index, components)?;
            active.add(user.clone(), component.clone(), agent);
        }

        // With the active set now at full size for this step, take one probe
        // sample so a ceiling that only manifests under sustained active load
        // (latency blow-up, OOM, connection loss) is caught even at the final
        // ramp step where there are no further creates to sample.
        if want_active > 0 {
            let (component, agent) = agent_for_index(config, 0, components)?;
            let attempt = timed_invoke(
                user,
                component,
                &agent,
                "increment",
                data_value!(),
                timeout.current,
            )
            .await;
            detector.set_elapsed_secs(started.elapsed().as_secs_f64());
            let pod_restart_count = probe.pod_restart_count().await;
            let snapshot = CrossAxisSnapshot::default();
            outcome.invoke_latencies.push(attempt.latency);
            let sample = Sample {
                latency: attempt.latency,
                coord: SampleCoord::Agents(target),
                pod_restart_count,
                connection_alive: !attempt.connection_lost,
                snapshot,
                queue_depth: None,
            };
            for event in detector.observe(&sample) {
                handle_event(event, &mut outcome, &mut timeout);
            }
            if detector.is_terminal() {
                break 'ramp;
            }
        }
    }

    active.stop().await;
    Ok(outcome)
}

/// Applies a ceiling event to the cell outcome and the adaptive timeout.
fn handle_event(event: CeilingEvent, outcome: &mut CellOutcome, timeout: &mut AdaptiveTimeout) {
    match event {
        CeilingEvent::EscalateTimeout => {
            timeout.escalate();
        }
        CeilingEvent::SoftCrossed { at, .. } => {
            if outcome.soft_ceiling_agents.is_none() {
                outcome.soft_ceiling_agents = coord_agents(at);
                info!("Density-agent: soft ceiling at {:?} agents", at);
            }
        }
        CeilingEvent::UsabilityCrossed { at, .. } => {
            if outcome.usability_ceiling_agents.is_none() {
                outcome.usability_ceiling_agents = coord_agents(at);
                info!("Density-agent: usability ceiling at {:?} agents", at);
            }
        }
        CeilingEvent::HardCrossed { at } => {
            if outcome.hard_ceiling_agents.is_none() {
                outcome.hard_ceiling_agents = coord_agents(at);
                info!("Density-agent: hard ceiling at {:?} agents", at);
            }
        }
        CeilingEvent::Catastrophic { at, reason, .. } => {
            outcome.catastrophic_ceiling_agents = coord_agents(at);
            outcome.terminated_reason = reason;
            info!(
                "Density-agent: catastrophic ceiling at {:?} agents, reason {:?}",
                at, reason
            );
        }
    }
}

fn coord_agents(coord: SampleCoord) -> Option<u32> {
    match coord {
        SampleCoord::Agents(n) => Some(n),
        SampleCoord::RatePerSec(_) => None,
    }
}

// ── Scenario 4: resume-under-saturation ──────────────────────────────────────

/// Number of resume/create probe pairs taken per ramp step in scenario 4.
const RESUME_PROBES_PER_STEP: u32 = 20;

/// Scenario 4 (durable-only): pre-fill `prefill_n` idle agents to push the pod
/// into eviction, then ramp additional agents while repeatedly measuring the
/// latency of resuming an already-evicted earlier agent versus creating a fresh
/// one. The resume-vs-create gap quantifies the cost of eviction churn under
/// memory pressure.
async fn run_resume_cell(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    probe: &ExecutorProbe,
) -> anyhow::Result<CellOutcome> {
    let prefill = config
        .prefill_n
        .ok_or_else(|| anyhow::anyhow!("resume-under-saturation cell missing prefill_n"))?;
    let mut detector = CeilingDetector::new();
    let mut outcome = CellOutcome::default();
    let mut timeout = AdaptiveTimeout::new();
    let started = Instant::now();

    info!(
        "Density-agent[{}]: pre-filling {prefill} idle agents",
        config.cell_name()
    );
    for index in 0..prefill {
        let (component, agent) = agent_for_index(config, index, components)?;
        let attempt = timed_invoke(
            user,
            component,
            &agent,
            "increment",
            data_value!(),
            timeout.current,
        )
        .await;

        detector.set_elapsed_secs(started.elapsed().as_secs_f64());
        let pod_restart_count = probe.pod_restart_count().await;
        outcome.invoke_latencies.push(attempt.latency);
        let sample = Sample {
            latency: attempt.latency,
            coord: SampleCoord::Agents(index + 1),
            pod_restart_count,
            connection_alive: !attempt.connection_lost,
            snapshot: CrossAxisSnapshot::default(),
            queue_depth: None,
        };
        for event in detector.observe(&sample) {
            handle_event(event, &mut outcome, &mut timeout);
        }
        if detector.is_terminal() {
            outcome.max_agents_reached = index + 1;
            return Ok(outcome);
        }
    }
    outcome.max_agents_reached = prefill;

    // Now ramp additional agents on top of the prefill, taking resume/create
    // probe pairs at each step. Fresh agents use indices above the prefill
    // range; resumes target the earliest (most likely already-evicted) agents.
    let ramp: Vec<u32> = config
        .ramp
        .iter()
        .copied()
        .filter(|&n| n > prefill)
        .collect();
    let mut next_fresh = prefill;

    'ramp: for &target in &ramp {
        info!(
            "Density-agent[{}]: resume probes at {target} total agents",
            config.cell_name()
        );
        for _ in 0..RESUME_PROBES_PER_STEP {
            // Resume an existing (early) agent.
            let resume_index = (next_fresh.wrapping_mul(2_654_435_761)) % prefill;
            let (rc, ragent) = agent_for_index(config, resume_index, components)?;
            let resume = timed_invoke(
                user,
                rc,
                &ragent,
                "increment",
                data_value!(),
                timeout.current,
            )
            .await;
            outcome
                .resume_existing_ms
                .push(resume.latency.as_secs_f64() * 1000.0);

            detector.set_elapsed_secs(started.elapsed().as_secs_f64());
            let pod_restart_count = probe.pod_restart_count().await;
            let snapshot = CrossAxisSnapshot::default();
            outcome.invoke_latencies.push(resume.latency);
            let sample = Sample {
                latency: resume.latency,
                coord: SampleCoord::Agents(target),
                pod_restart_count,
                connection_alive: !resume.connection_lost,
                snapshot: snapshot.clone(),
                queue_depth: None,
            };
            for event in detector.observe(&sample) {
                handle_event(event, &mut outcome, &mut timeout);
            }
            if detector.is_terminal() {
                break 'ramp;
            }

            // Create a fresh agent.
            let (cc, cagent) = agent_for_index(config, next_fresh, components)?;
            let create = timed_invoke(
                user,
                cc,
                &cagent,
                "increment",
                data_value!(),
                timeout.current,
            )
            .await;
            outcome
                .create_fresh_ms
                .push(create.latency.as_secs_f64() * 1000.0);
            next_fresh += 1;
            outcome.max_agents_reached = next_fresh;

            detector.set_elapsed_secs(started.elapsed().as_secs_f64());
            outcome.invoke_latencies.push(create.latency);
            let sample = Sample {
                latency: create.latency,
                coord: SampleCoord::Agents(target),
                pod_restart_count: probe.pod_restart_count().await,
                connection_alive: !create.connection_lost,
                snapshot,
                queue_depth: None,
            };
            for event in detector.observe(&sample) {
                handle_event(event, &mut outcome, &mut timeout);
            }
            if detector.is_terminal() {
                break 'ramp;
            }
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn active_count_respects_fraction() {
        let cell = CellConfig {
            scenario: Scenario::CreateWithActiveFraction,
            mode: AgentMode::Durable,
            sharing: ComponentSharing::Shared,
            active_fraction: Some(50),
            prefill_n: None,
            ramp: vec![100, 250, 500],
        };
        assert_eq!(active_count(&cell, 1000), 500);
    }

    #[test]
    fn concurrent_active_activates_all() {
        let cell = CellConfig {
            scenario: Scenario::ConcurrentActive,
            mode: AgentMode::Durable,
            sharing: ComponentSharing::Shared,
            active_fraction: None,
            prefill_n: None,
            ramp: vec![100, 250, 500],
        };
        assert_eq!(active_count(&cell, 1000), 1000);
    }
}

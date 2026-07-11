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
//! # The usable-floor load model
//!
//! For active scenarios, each ramp step is an independent burst size. First, the
//! driver issues the cold burst and records cold latency. Then it reuses the same
//! agents for [`INVOKES_PER_AGENT_PER_STEP`] warm rounds and records warm
//! latency. Create-only instead measures just the one-shot create/invoke path.
//!
//! # Scenarios
//!
//! 1. create-only: create/invoke N agents once, then leave durable agents idle
//!    and eviction-eligible. Measures the create path.
//! 2. create-with-active-fraction: keep the configured fraction (0/25/50/75%)
//!    busy with `busy_for(250ms)` while measuring `increment` latency on the
//!    rest. At 0%, all agents are measured and no agents are busy in the
//!    background.
//! 3. concurrent-active: invoke all N agents concurrently with `increment`.
//! 4. resume-under-saturation: durable-only. For each independent target, warm
//!    the fixed prefill set with N cheap host calls each to produce oplog,
//!    restart the executor to unload them, then measure concurrent replay/resume
//!    latency.
//!
//! Durable and ephemeral active modes run the same ramp/round driver
//! ([`run_ramp_cell`]). Durable agents are deleted between ramp targets so each
//! target is independent; ephemeral agents have no persistent identity, so a
//! round's concurrent invocations *are* that round's ephemeral agents (created
//! and gone within the round).
//!
//! # Operational definitions (from #3523)
//!
//! - Load round: one concurrent invocation per agent in the load set, all in
//!   flight at once, awaited together. Scenario 2 uses `busy_for(250ms)` only for
//!   the background busy agents; measured calls use `increment`.
//! - Passive (idle): for durable create-only, an agent is created/invoked once,
//!   then left to drift toward `LoadedIdle` and become eviction-eligible.
//! - Soft / hard / catastrophic ceilings: see [`super::ceiling`].

use crate::benchmarks::density::ceiling::{
    CeilingDetector, CeilingEvent, CrossAxisSnapshot, Sample, SampleCoord, TerminatedReason,
};
use crate::benchmarks::density::prep::PrepManifest;
use crate::benchmarks::density::{AgentMode, ComponentSharing};
use futures::StreamExt;
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::agent_id;
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::data_value;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::PublicOplogEntry;
use golem_common::model::{AgentEvent, AgentId};
use golem_test_framework::benchmark::{
    BenchmarkRecorder, BenchmarkResult, BenchmarkRunResult, ResultKey, RunConfig,
};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::dsl::TestDsl;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot::Sender;
use tracing::{debug, info, warn};

/// Agent type names exported by the agent-counters component.
const DURABLE_AGENT_TYPE: &str = "Counter";
const SNAPSHOT_AGENT_TYPE: &str = "SnapshotCounter";
const EPHEMERAL_AGENT_TYPE: &str = "EphemeralCounter";

/// CPU busy time per `busy_for` call defining one unit of load.
const BUSY_MILLIS: u32 = 250;

/// Number of load rounds per ramp step. Each round invokes every agent in the
/// step's load set once, concurrently, so a step applies this many waves of
/// "one simultaneous invocation per loaded agent" — enough samples to measure
/// the latency distribution at that density and surface a latency ceiling.
const INVOKES_PER_AGENT_PER_STEP: u32 = 50;

/// Emit progress every N load rounds so GitHub Actions log lag is distinguishable
/// from a genuinely stuck benchmark.
const LOAD_ROUND_PROGRESS_INTERVAL: u32 = 5;

/// Maximum number of scenario-4 warmup agents in flight at once. Each warmup
/// agent performs many sequential calls, so this can be higher than create
/// concurrency without increasing per-agent request burstiness.
const RESUME_WARMUP_CONCURRENCY: usize = 250;

/// Maximum number of durable-agent deletions in flight during best-effort cell
/// cleanup. Deleting a worker can load it first, so use a lower cap than create
/// to avoid turning cleanup itself into another saturation event.
const DELETE_CONCURRENCY: usize = 25;

/// After a workflow-driven or deliberate executor restart, Kubernetes rollout
/// readiness can become true before the end-to-end invocation path is usable
/// again. Cells wait for this unmeasured canary before starting measurement.
const INVOCATION_PATH_CANARY_BUDGET: Duration = Duration::from_secs(120);
const INVOCATION_PATH_CANARY_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);
const INVOCATION_PATH_CANARY_RETRY_DELAY: Duration = Duration::from_secs(2);
const SNAPSHOT_EVENT_STREAM_BUDGET: Duration = Duration::from_secs(30);
const SNAPSHOT_EVENT_STREAM_RETRY_DELAY: Duration = Duration::from_secs(2);
const SNAPSHOT_EVENT_OBSERVATION_BUDGET: Duration = Duration::from_secs(10);
const SNAPSHOT_EVENT_POST_RESUME_GRACE: Duration = Duration::from_secs(10);
const RESUME_CANARY_INDEX_OFFSET: u32 = 1_000_000;

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
    /// Durable-only: pre-fill `prefill_n` agents, then ramp oplog entries per
    /// agent and measure replay/resume latency under saturation.
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
    /// Whether this cell uses the snapshot-enabled durable counter agent.
    pub snapshotting: bool,
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
        if self.scenario == Scenario::ResumeUnderSaturation {
            parts.push(if self.snapshotting {
                "snapshot-enabled".to_string()
            } else {
                "snapshot-disabled".to_string()
            });
        }
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
        match (self.mode, self.snapshotting) {
            (AgentMode::Durable, true) => SNAPSHOT_AGENT_TYPE,
            (AgentMode::Durable, false) => DURABLE_AGENT_TYPE,
            (AgentMode::Ephemeral, _) => EPHEMERAL_AGENT_TYPE,
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

    /// Scales the worker-executor deployment down and back up to force durable
    /// agents out of memory while preserving registry, keyvalue, indexed, and
    /// oplog state.
    async fn restart_executor(&self) -> anyhow::Result<()> {
        info!(
            "density: scaling worker-executor deployment down in namespace {}",
            self.namespace
        );
        let scale_down = tokio::process::Command::new("kubectl")
            .args([
                "scale",
                "deployment/worker-executor",
                "--replicas=0",
                "-n",
                &self.namespace,
            ])
            .output()
            .await?;
        if !scale_down.status.success() {
            anyhow::bail!(
                "kubectl scale worker-executor down failed: {}",
                String::from_utf8_lossy(&scale_down.stderr)
            );
        }

        let wait_down = tokio::process::Command::new("kubectl")
            .args([
                "wait",
                "--for=delete",
                "pod",
                "-l",
                "app=worker-executor",
                "-n",
                &self.namespace,
                "--timeout=300s",
            ])
            .output()
            .await?;
        if !wait_down.status.success() {
            anyhow::bail!(
                "kubectl wait for worker-executor scale-down failed: {}",
                String::from_utf8_lossy(&wait_down.stderr)
            );
        }

        info!(
            "density: scaling worker-executor deployment up in namespace {}",
            self.namespace
        );
        let scale_up = tokio::process::Command::new("kubectl")
            .args([
                "scale",
                "deployment/worker-executor",
                "--replicas=1",
                "-n",
                &self.namespace,
            ])
            .output()
            .await?;
        if !scale_up.status.success() {
            anyhow::bail!(
                "kubectl scale worker-executor up failed: {}",
                String::from_utf8_lossy(&scale_up.stderr)
            );
        }

        let status = tokio::process::Command::new("kubectl")
            .args([
                "rollout",
                "status",
                "deployment/worker-executor",
                "-n",
                &self.namespace,
                "--timeout=300s",
            ])
            .output()
            .await?;
        if !status.status.success() {
            anyhow::bail!(
                "kubectl rollout status failed after worker-executor scale-up: {}",
                String::from_utf8_lossy(&status.stderr)
            );
        }
        info!("density: worker-executor scale restart complete");
        Ok(())
    }

    async fn restart_worker_service(&self) -> anyhow::Result<()> {
        info!(
            "density: scaling worker-service deployment down in namespace {}",
            self.namespace
        );
        let scale_down = tokio::process::Command::new("kubectl")
            .args([
                "scale",
                "deployment/worker-service",
                "--replicas=0",
                "-n",
                &self.namespace,
            ])
            .output()
            .await?;
        if !scale_down.status.success() {
            anyhow::bail!(
                "kubectl scale worker-service down failed: {}",
                String::from_utf8_lossy(&scale_down.stderr)
            );
        }

        let wait_down = tokio::process::Command::new("kubectl")
            .args([
                "wait",
                "--for=delete",
                "pod",
                "-l",
                "app=worker-service",
                "-n",
                &self.namespace,
                "--timeout=300s",
            ])
            .output()
            .await?;
        if !wait_down.status.success() {
            anyhow::bail!(
                "kubectl wait for worker-service scale-down failed: {}",
                String::from_utf8_lossy(&wait_down.stderr)
            );
        }

        info!(
            "density: scaling worker-service deployment up in namespace {}",
            self.namespace
        );
        let scale_up = tokio::process::Command::new("kubectl")
            .args([
                "scale",
                "deployment/worker-service",
                "--replicas=1",
                "-n",
                &self.namespace,
            ])
            .output()
            .await?;
        if !scale_up.status.success() {
            anyhow::bail!(
                "kubectl scale worker-service up failed: {}",
                String::from_utf8_lossy(&scale_up.stderr)
            );
        }

        let status = tokio::process::Command::new("kubectl")
            .args([
                "rollout",
                "status",
                "deployment/worker-service",
                "-n",
                &self.namespace,
                "--timeout=300s",
            ])
            .output()
            .await?;
        if !status.status.success() {
            anyhow::bail!(
                "kubectl rollout status failed after worker-service restart: {}",
                String::from_utf8_lossy(&status.stderr)
            );
        }
        info!("density: worker-service scale restart complete");
        Ok(())
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
    /// True if the attempt failed with a connection-level error or client-side
    /// timeout, which the ceiling detector treats as the catastrophic
    /// connection-lost condition.
    connection_lost: bool,
    /// True if the attempt was rejected with an overloaded (HTTP 503) response.
    /// A sustained run of these is the catastrophic overloaded condition.
    overloaded: bool,
}

/// Per-attempt client timeout. Starts above the 30s hard-ceiling threshold so a
/// client-side timeout is unambiguously a failed round-trip, not just a sample
/// equal to the hard-ceiling boundary. Escalated to 5 minutes once the hard
/// ceiling is crossed, so the eventual catastrophic 5-minute-timeout condition
/// can fire.
struct AdaptiveTimeout {
    current: Duration,
}

impl AdaptiveTimeout {
    fn new() -> Self {
        Self {
            current: Duration::from_secs(60),
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

/// Classifies an invocation error as an overloaded (HTTP 503) response: the
/// executor accepted the connection but rejected the request because it cannot
/// admit more work. A handful are tolerable; a sustained run is the
/// catastrophic overloaded condition.
fn is_overloaded_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:?}").to_lowercase();
    msg.contains("status 503") || msg.contains("503 service unavailable")
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
            overloaded: false,
        },
        Ok(Err(e)) => {
            let overloaded = is_overloaded_error(&e);
            let connection_lost = !overloaded && is_connection_error(&e);
            if !connection_lost && !overloaded {
                warn!("density: invocation error (non-connection): {e:?}");
            }
            AttemptOutcome {
                latency,
                connection_lost,
                overloaded,
            }
        }
        Err(_) => {
            // Timed out: report the timeout duration as the latency and mark
            // the attempt as a failed round-trip so the detector stops the cell
            // instead of counting the target as successfully resumed.
            AttemptOutcome {
                latency: timeout,
                connection_lost: true,
                overloaded: false,
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

    /// Invoke-latency distribution key prefix (create/invoke round-trip times).
    /// The emitted key is suffixed with `@<zero-padded agent count>` so each
    /// ramp step gets its own distribution.
    pub const INVOKE_LATENCY: &str = "invoke-latency";
    pub const COLD_INVOKE_LATENCY: &str = "cold-invoke-latency";
    pub const WARM_INVOKE_LATENCY: &str = "warm-invoke-latency";

    // Scenario-4 (resume-under-saturation) latencies, in milliseconds.
    pub const RESUME_EXISTING_P50_MS: &str = "resume-existing-p50-ms";
    pub const RESUME_EXISTING_P99_MS: &str = "resume-existing-p99-ms";
    pub const CREATE_FRESH_P50_MS: &str = "create-fresh-p50-ms";
    pub const CREATE_FRESH_P99_MS: &str = "create-fresh-p99-ms";
    pub const SOFT_CEILING_OPLOG_ENTRIES: &str = "soft-ceiling-oplog-entries-per-agent";
    pub const USABILITY_CEILING_OPLOG_ENTRIES: &str = "usability-ceiling-oplog-entries-per-agent";
    pub const HARD_CEILING_OPLOG_ENTRIES: &str = "hard-ceiling-oplog-entries-per-agent";
    pub const CATASTROPHIC_CEILING_OPLOG_ENTRIES: &str =
        "catastrophic-ceiling-oplog-entries-per-agent";
    pub const MAX_OPLOG_ENTRIES_REACHED: &str = "max-oplog-entries-per-agent-reached";
    pub const MAX_REQUESTED_HOST_CALLS_REACHED: &str = "max-requested-host-calls-per-agent-reached";
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
    max_oplog_entries_reached: u64,
    max_requested_host_calls_reached: u32,
    /// Internal bookkeeping: scenarios 1-3 clean durable agents between ramp
    /// targets, so the outer cell cleanup can be skipped when the last target
    /// was already removed successfully.
    cleanup_already_done: bool,
    /// Invoke latencies bucketed by the ramp-step agent count at which they were
    /// measured. Each bucket is surfaced as its own invoke-latency distribution
    /// (avg/min/max/p50/p90/p95/p99) so latency can be read per ramp step rather
    /// than collapsed across the whole cell.
    invoke_latencies: BTreeMap<u32, Vec<Duration>>,
    cold_invoke_latencies: BTreeMap<u32, Vec<Duration>>,
    warm_invoke_latencies: BTreeMap<u32, Vec<Duration>>,
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
            max_oplog_entries_reached: 0,
            max_requested_host_calls_reached: 0,
            cleanup_already_done: false,
            invoke_latencies: BTreeMap::new(),
            cold_invoke_latencies: BTreeMap::new(),
            warm_invoke_latencies: BTreeMap::new(),
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
        let resume_cell = config.scenario == Scenario::ResumeUnderSaturation;
        if let Some(n) = self.soft_ceiling_agents {
            recorder.count(
                &ResultKey::primary(if resume_cell {
                    keys::SOFT_CEILING_OPLOG_ENTRIES
                } else {
                    keys::SOFT_CEILING_AGENTS
                }),
                n as u64,
            );
        }
        if let Some(n) = self.usability_ceiling_agents {
            recorder.count(
                &ResultKey::primary(if resume_cell {
                    keys::USABILITY_CEILING_OPLOG_ENTRIES
                } else {
                    keys::USABILITY_CEILING_AGENTS
                }),
                n as u64,
            );
        }
        if let Some(n) = self.hard_ceiling_agents {
            recorder.count(
                &ResultKey::primary(if resume_cell {
                    keys::HARD_CEILING_OPLOG_ENTRIES
                } else {
                    keys::HARD_CEILING_AGENTS
                }),
                n as u64,
            );
        }
        if let Some(n) = self.catastrophic_ceiling_agents {
            recorder.count(
                &ResultKey::primary(if resume_cell {
                    keys::CATASTROPHIC_CEILING_OPLOG_ENTRIES
                } else {
                    keys::CATASTROPHIC_CEILING_AGENTS
                }),
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
        if resume_cell {
            recorder.count(
                &ResultKey::primary(keys::MAX_OPLOG_ENTRIES_REACHED),
                self.max_oplog_entries_reached,
            );
            recorder.count(
                &ResultKey::primary(keys::MAX_REQUESTED_HOST_CALLS_REACHED),
                self.max_requested_host_calls_reached as u64,
            );
        }

        // Invoke-latency distribution per ramp step, each rendered as the same
        // avg/min/max/p50/p90/p95/p99 table as cloud-perf. Keying per step (the
        // agent count reached) instead of collapsing every step into one
        // distribution makes the per-step latency readable. The count is
        // zero-padded so the keys sort numerically.
        for (agents, latencies) in &self.invoke_latencies {
            let key = format!("{}@{agents:06}", keys::INVOKE_LATENCY);
            for latency in latencies {
                recorder.duration(&ResultKey::primary(&key), *latency);
            }
        }
        for (agents, latencies) in &self.cold_invoke_latencies {
            let key = format!("{}@{agents:06}", keys::COLD_INVOKE_LATENCY);
            for latency in latencies {
                recorder.duration(&ResultKey::primary(&key), *latency);
            }
        }
        for (agents, latencies) in &self.warm_invoke_latencies {
            let key = format!("{}@{agents:06}", keys::WARM_INVOKE_LATENCY);
            for latency in latencies {
                recorder.duration(&ResultKey::primary(&key), *latency);
            }
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
            size: if resume_cell {
                self.max_oplog_entries_reached as usize
            } else {
                self.max_agents_reached as usize
            },
            length: 0,
            disable_compilation_cache: false,
        };
        let mut run_result = BenchmarkRunResult::new(run_config.clone());
        run_result.add(recorder);

        BenchmarkResult {
            name: format!("density-agent-{}", config.cell_name()),
            description: format!(
                "Agent-density cell: scenario={}, mode={}, sharing={}{}{}{}",
                config.scenario.as_str(),
                config.mode,
                config.sharing,
                if config.snapshotting {
                    ", snapshotting=enabled".to_string()
                } else {
                    String::new()
                },
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

    if config.snapshotting && config.mode != AgentMode::Durable {
        anyhow::bail!("snapshotting is only supported for durable agent-density cells");
    }

    let user = manifest.user_context(deps);
    let components = resolve_components(config, manifest, &user).await?;
    wait_for_invocation_path(config, &user, &components, "cell-start").await?;

    let mut outcome = match config.scenario {
        Scenario::ResumeUnderSaturation => {
            run_resume_cell(config, &user, &components, probe).await?
        }
        _ => run_ramp_cell(config, &user, &components, probe).await?,
    };

    if !outcome.cleanup_already_done
        && cleanup_cell_agents(config, &user, &components, &outcome).await == CleanupResult::Failed
    {
        outcome.terminated_reason = TerminatedReason::ConnectionLost;
        outcome.catastrophic_ceiling_agents = Some(outcome.max_agents_reached);
    }

    log_cell_summary(config, &outcome);

    Ok(outcome.into_benchmark_result(config))
}

/// Emits a single human-readable summary line for the cell, so the raw
/// `Results for '<key>'` count tables (which render bare numbers such as
/// `terminated-reason: 2`) are preceded by something interpretable: the stop
/// reason by name, how far the ramp got, and where each ceiling was crossed.
fn log_cell_summary(config: &CellConfig, outcome: &CellOutcome) {
    fn ceiling(label: &str, value: Option<u32>) -> String {
        match value {
            Some(n) => format!("{label}={n}"),
            None => format!("{label}=none"),
        }
    }

    info!(
        "Density-agent[{}]: stopped — reason {} (code {}), max-agents-reached {}, max-oplog-entries-per-agent-reached {}, max-requested-host-calls-per-agent-reached {}, {}, {}, {}, {}",
        config.cell_name(),
        outcome.terminated_reason.as_str(),
        outcome.terminated_reason.code(),
        outcome.max_agents_reached,
        outcome.max_oplog_entries_reached,
        outcome.max_requested_host_calls_reached,
        ceiling("soft-ceiling", outcome.soft_ceiling_agents),
        ceiling("usability-ceiling", outcome.usability_ceiling_agents),
        ceiling("hard-ceiling", outcome.hard_ceiling_agents),
        ceiling("catastrophic-ceiling", outcome.catastrophic_ceiling_agents),
    );
}

/// Deletes every durable agent this cell created so the next cell starts from a
/// clean executor.
///
/// Only durable agents are deleted. Ephemeral agents exist only while an
/// invocation is in flight; an ephemeral cell holds them as concurrent in-flight
/// batches that finish on their own when each ramp step completes, so by the end
/// of the cell they are already gone and there is nothing in the
/// `running-workers` set to delete.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupResult {
    Succeeded,
    Failed,
}

async fn cleanup_cell_agents(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    outcome: &CellOutcome,
) -> CleanupResult {
    if config.mode == AgentMode::Ephemeral {
        return CleanupResult::Succeeded;
    }
    if outcome.terminated_reason.is_catastrophic() {
        info!(
            "Density-agent[{}]: skipping agent deletion ({:?}); buildspec clears state from fresh",
            config.cell_name(),
            outcome.terminated_reason
        );
        return CleanupResult::Succeeded;
    }
    let count = outcome.max_agents_reached;
    if count == 0 {
        return CleanupResult::Succeeded;
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
        DELETE_CONCURRENCY,
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
        Ok(()) if transport_failed.load(Ordering::Relaxed) => {
            warn!(
                "Density-agent[{}]: cleanup of {count} agents abandoned after transport failure",
                config.cell_name()
            );
            CleanupResult::Failed
        }
        Ok(()) => {
            info!(
                "Density-agent[{}]: cleanup of {count} agents complete",
                config.cell_name()
            );
            CleanupResult::Succeeded
        }
        Err(_) => {
            warn!(
                "Density-agent[{}]: cleanup exceeded {}s budget, abandoning",
                config.cell_name(),
                CLEANUP_BUDGET.as_secs()
            );
            CleanupResult::Failed
        }
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

/// Size of the per-round load set at agent count `n`: how many of the `n` live
/// agents receive one concurrent `busy_for` invocation in each load round.
///
/// The load round models the usable floor — N users each invoking their own
/// agent at the same instant — so `ConcurrentActive` invokes every live agent
/// (`n`) and `CreateWithActiveFraction` loads only the configured fraction.
/// `CreateOnly` and `ResumeUnderSaturation` do not run load rounds.
#[cfg(test)]
fn load_count(config: &CellConfig, n: u32) -> u32 {
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

#[cfg(test)]
fn has_create_phase(config: &CellConfig) -> bool {
    config.scenario == Scenario::CreateOnly
}

enum LatencyPhase {
    Invoke,
    Cold,
    Warm,
}

async fn invoke_agent_indices(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    indices: Vec<u32>,
    method: &'static str,
    params: golem_common::base_model::agent::DataValue,
    timeout: Duration,
) -> Vec<(u32, AttemptOutcome)> {
    let concurrency = indices.len().max(1);
    let mut attempts: Vec<(u32, AttemptOutcome)> = futures::stream::iter(indices)
        .map(|index| {
            let (component, agent) =
                agent_for_index(config, index, components).expect("agent_for_index within ramp");
            let component = component.clone();
            let params = params.clone();
            async move {
                let outcome = timed_invoke(user, &component, &agent, method, params, timeout).await;
                (index, outcome)
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;
    attempts.sort_by_key(|(index, _)| *index);
    attempts
}

async fn wait_for_invocation_path(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    label: &str,
) -> anyhow::Result<()> {
    let component = components
        .first()
        .ok_or_else(|| anyhow::anyhow!("no component available for restart canary"))?;
    let agent = agent_id!(
        EPHEMERAL_AGENT_TYPE,
        format!("{}-invocation-path-canary-{label}", config.cell_name())
    );
    let deadline = Instant::now() + INVOCATION_PATH_CANARY_BUDGET;
    let mut attempt = 0u32;

    loop {
        attempt += 1;
        let result = tokio::time::timeout(
            INVOCATION_PATH_CANARY_ATTEMPT_TIMEOUT,
            user.invoke_and_await_agent(component, &agent, "increment", data_value!()),
        )
        .await;

        match result {
            Ok(Ok(_)) => {
                info!(
                    "Density-agent[{}]: invocation path ready for {label} after {attempt} canary attempt(s)",
                    config.cell_name()
                );
                return Ok(());
            }
            Ok(Err(err)) => {
                warn!(
                    "Density-agent[{}]: invocation-path canary for {label} attempt {attempt} failed: {err:?}",
                    config.cell_name()
                );
            }
            Err(_) => {
                warn!(
                    "Density-agent[{}]: invocation-path canary for {label} attempt {attempt} timed out after {:?}",
                    config.cell_name(),
                    INVOCATION_PATH_CANARY_ATTEMPT_TIMEOUT
                );
            }
        }

        if Instant::now() >= deadline {
            anyhow::bail!(
                "invocation path for {label} did not become ready within {:?}",
                INVOCATION_PATH_CANARY_BUDGET
            );
        }
        tokio::time::sleep(INVOCATION_PATH_CANARY_RETRY_DELAY).await;
    }
}

async fn warm_resume_canary_agent(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    target: u32,
    timeout: Duration,
) -> anyhow::Result<()> {
    let canary_index = RESUME_CANARY_INDEX_OFFSET + target;
    let attempts = invoke_agent_indices(
        config,
        user,
        components,
        vec![canary_index],
        "increment",
        data_value!(),
        timeout,
    )
    .await;
    let Some((_, attempt)) = attempts.into_iter().next() else {
        anyhow::bail!("resume canary warmup produced no attempt");
    };
    if attempt.connection_lost || attempt.overloaded {
        anyhow::bail!("resume canary warmup failed before restart");
    }
    Ok(())
}

async fn wait_for_resume_canary_agent(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    target: u32,
) -> anyhow::Result<()> {
    let canary_index = RESUME_CANARY_INDEX_OFFSET + target;
    let deadline = Instant::now() + INVOCATION_PATH_CANARY_BUDGET;
    let mut attempt_no = 0u32;

    loop {
        attempt_no += 1;
        let attempts = invoke_agent_indices(
            config,
            user,
            components,
            vec![canary_index],
            "increment",
            data_value!(),
            INVOCATION_PATH_CANARY_ATTEMPT_TIMEOUT,
        )
        .await;
        let Some((_, attempt)) = attempts.into_iter().next() else {
            anyhow::bail!("resume canary produced no attempt");
        };

        if !attempt.connection_lost && !attempt.overloaded {
            info!(
                "Density-agent[{}]: durable resume canary ready for target {target} after {attempt_no} attempt(s)",
                config.cell_name()
            );
            return Ok(());
        }

        warn!(
            "Density-agent[{}]: durable resume canary for target {target} attempt {attempt_no} failed after {:?}",
            config.cell_name(),
            attempt.latency
        );
        if Instant::now() >= deadline {
            anyhow::bail!(
                "durable resume canary for target {target} did not become ready within {:?}",
                INVOCATION_PATH_CANARY_BUDGET
            );
        }
        tokio::time::sleep(INVOCATION_PATH_CANARY_RETRY_DELAY).await;
    }
}

fn batch_connection_alive(attempts: &[&AttemptOutcome]) -> bool {
    let transport_failures = attempts.iter().filter(|a| a.connection_lost).count();
    attempts.is_empty()
        || (transport_failures as f64) < (attempts.len() as f64 * TRANSPORT_FAILURE_RATIO)
}

fn record_attempts(
    outcome: &mut CellOutcome,
    detector: &mut CeilingDetector,
    timeout: &mut AdaptiveTimeout,
    phase: LatencyPhase,
    target: u32,
    pod_restart_count: u64,
    connection_alive: bool,
    attempts: &[AttemptOutcome],
) -> bool {
    for attempt in attempts {
        match phase {
            LatencyPhase::Invoke => outcome
                .invoke_latencies
                .entry(target)
                .or_default()
                .push(attempt.latency),
            LatencyPhase::Cold => outcome
                .cold_invoke_latencies
                .entry(target)
                .or_default()
                .push(attempt.latency),
            LatencyPhase::Warm => outcome
                .warm_invoke_latencies
                .entry(target)
                .or_default()
                .push(attempt.latency),
        }
        let sample = Sample {
            latency: attempt.latency,
            coord: SampleCoord::Agents(target),
            pod_restart_count,
            connection_alive,
            overloaded: attempt.overloaded,
            snapshot: CrossAxisSnapshot::default(),
            queue_depth: None,
        };
        for event in detector.observe(&sample) {
            handle_event(event, outcome, timeout);
        }
        if detector.is_terminal() {
            return true;
        }
    }
    false
}

fn active_fraction_counts(target: u32, active_fraction: u32) -> (u32, u32) {
    let busy = ((target as u64 * active_fraction as u64) / 100) as u32;
    let measured = target.saturating_sub(busy);
    (busy, measured)
}

/// Scenarios 1-3: each ramp target is an independent burst size. Durable agents
/// are deleted before advancing to the next target, so larger targets do not mix
/// already-warm agents from smaller targets with newly-created agents.
async fn run_ramp_cell(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    probe: &ExecutorProbe,
) -> anyhow::Result<CellOutcome> {
    let ramp = config.ramp.clone();
    let mut detector = CeilingDetector::new();
    let mut outcome = CellOutcome::default();
    let mut timeout = AdaptiveTimeout::new();
    let started = Instant::now();

    'ramp: for &target in &ramp {
        info!(
            "Density-agent[{}]: measuring independent target {target}",
            config.cell_name()
        );
        outcome.max_agents_reached = target;
        outcome.cleanup_already_done = false;

        match config.scenario {
            Scenario::CreateOnly => {
                let creates = invoke_agent_indices(
                    config,
                    user,
                    components,
                    (0..target).collect(),
                    "increment",
                    data_value!(),
                    timeout.current,
                )
                .await;
                let pod_restart_count = probe.pod_restart_count().await;
                detector.set_elapsed_secs(started.elapsed().as_secs_f64());
                let attempt_refs: Vec<&AttemptOutcome> = creates.iter().map(|(_, a)| a).collect();
                let connection_alive = batch_connection_alive(&attempt_refs);
                let attempts: Vec<AttemptOutcome> = creates.into_iter().map(|(_, a)| a).collect();
                if record_attempts(
                    &mut outcome,
                    &mut detector,
                    &mut timeout,
                    LatencyPhase::Invoke,
                    target,
                    pod_restart_count,
                    connection_alive,
                    &attempts,
                ) {
                    break 'ramp;
                }
            }
            Scenario::ConcurrentActive => {
                let cold = invoke_agent_indices(
                    config,
                    user,
                    components,
                    (0..target).collect(),
                    "increment",
                    data_value!(),
                    timeout.current,
                )
                .await;
                let pod_restart_count = probe.pod_restart_count().await;
                detector.set_elapsed_secs(started.elapsed().as_secs_f64());
                let attempt_refs: Vec<&AttemptOutcome> = cold.iter().map(|(_, a)| a).collect();
                let connection_alive = batch_connection_alive(&attempt_refs);
                let attempts: Vec<AttemptOutcome> = cold.into_iter().map(|(_, a)| a).collect();
                if record_attempts(
                    &mut outcome,
                    &mut detector,
                    &mut timeout,
                    LatencyPhase::Cold,
                    target,
                    pod_restart_count,
                    connection_alive,
                    &attempts,
                ) {
                    break 'ramp;
                }

                for round_index in 1..=INVOKES_PER_AGENT_PER_STEP {
                    let warm = invoke_agent_indices(
                        config,
                        user,
                        components,
                        (0..target).collect(),
                        "increment",
                        data_value!(),
                        timeout.current,
                    )
                    .await;
                    let pod_restart_count = probe.pod_restart_count().await;
                    detector.set_elapsed_secs(started.elapsed().as_secs_f64());
                    let attempt_refs: Vec<&AttemptOutcome> = warm.iter().map(|(_, a)| a).collect();
                    let connection_alive = batch_connection_alive(&attempt_refs);
                    let attempts: Vec<AttemptOutcome> = warm.into_iter().map(|(_, a)| a).collect();
                    if record_attempts(
                        &mut outcome,
                        &mut detector,
                        &mut timeout,
                        LatencyPhase::Warm,
                        target,
                        pod_restart_count,
                        connection_alive,
                        &attempts,
                    ) {
                        break 'ramp;
                    }
                    if round_index == 1
                        || round_index % LOAD_ROUND_PROGRESS_INTERVAL == 0
                        || round_index == INVOKES_PER_AGENT_PER_STEP
                    {
                        info!(
                            "Density-agent[{}]: completed warm increment round {round_index}/{} for target {target}",
                            config.cell_name(),
                            INVOKES_PER_AGENT_PER_STEP
                        );
                    }
                }
            }
            Scenario::CreateWithActiveFraction => {
                let active_fraction = config.active_fraction.unwrap_or(0);
                let (busy_count, measured_count) = active_fraction_counts(target, active_fraction);
                info!(
                    "Density-agent[{}]: target {target}, busy {busy_count}, measured {measured_count}",
                    config.cell_name()
                );
                let busy_indices: Vec<u32> = (0..busy_count).collect();
                let measured_indices: Vec<u32> = (busy_count..target).collect();

                let timeout_current = timeout.current;
                let (busy, measured) = futures::join!(
                    invoke_agent_indices(
                        config,
                        user,
                        components,
                        busy_indices.clone(),
                        "busy_for",
                        data_value!(BUSY_MILLIS),
                        timeout_current,
                    ),
                    invoke_agent_indices(
                        config,
                        user,
                        components,
                        measured_indices.clone(),
                        "increment",
                        data_value!(),
                        timeout_current,
                    )
                );
                let pod_restart_count = probe.pod_restart_count().await;
                detector.set_elapsed_secs(started.elapsed().as_secs_f64());
                let attempt_refs: Vec<&AttemptOutcome> = busy
                    .iter()
                    .map(|(_, a)| a)
                    .chain(measured.iter().map(|(_, a)| a))
                    .collect();
                let connection_alive = batch_connection_alive(&attempt_refs);
                let attempts: Vec<AttemptOutcome> = measured.into_iter().map(|(_, a)| a).collect();
                if record_attempts(
                    &mut outcome,
                    &mut detector,
                    &mut timeout,
                    LatencyPhase::Cold,
                    target,
                    pod_restart_count,
                    connection_alive,
                    &attempts,
                ) {
                    break 'ramp;
                }

                for round_index in 1..=INVOKES_PER_AGENT_PER_STEP {
                    let timeout_current = timeout.current;
                    let (busy, measured) = futures::join!(
                        invoke_agent_indices(
                            config,
                            user,
                            components,
                            busy_indices.clone(),
                            "busy_for",
                            data_value!(BUSY_MILLIS),
                            timeout_current,
                        ),
                        invoke_agent_indices(
                            config,
                            user,
                            components,
                            measured_indices.clone(),
                            "increment",
                            data_value!(),
                            timeout_current,
                        )
                    );
                    let pod_restart_count = probe.pod_restart_count().await;
                    detector.set_elapsed_secs(started.elapsed().as_secs_f64());
                    let attempt_refs: Vec<&AttemptOutcome> = busy
                        .iter()
                        .map(|(_, a)| a)
                        .chain(measured.iter().map(|(_, a)| a))
                        .collect();
                    let connection_alive = batch_connection_alive(&attempt_refs);
                    let attempts: Vec<AttemptOutcome> =
                        measured.into_iter().map(|(_, a)| a).collect();
                    if record_attempts(
                        &mut outcome,
                        &mut detector,
                        &mut timeout,
                        LatencyPhase::Warm,
                        target,
                        pod_restart_count,
                        connection_alive,
                        &attempts,
                    ) {
                        break 'ramp;
                    }
                    if round_index == 1
                        || round_index % LOAD_ROUND_PROGRESS_INTERVAL == 0
                        || round_index == INVOKES_PER_AGENT_PER_STEP
                    {
                        info!(
                            "Density-agent[{}]: completed warm measured-increment round {round_index}/{} for target {target}",
                            config.cell_name(),
                            INVOKES_PER_AGENT_PER_STEP
                        );
                    }
                }
            }
            Scenario::ResumeUnderSaturation => unreachable!("scenario 4 uses run_resume_cell"),
        }

        if detector.is_terminal() {
            break 'ramp;
        }

        let step_outcome = CellOutcome {
            max_agents_reached: target,
            ..CellOutcome::default()
        };
        if cleanup_cell_agents(config, user, components, &step_outcome).await
            == CleanupResult::Failed
        {
            outcome.terminated_reason = TerminatedReason::ConnectionLost;
            outcome.catastrophic_ceiling_agents = Some(target);
            break 'ramp;
        }
        outcome.cleanup_already_done = true;
    }

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

/// Number of cheap host calls made by one warmup invocation in scenario 4.
/// Used for snapshot-disabled agents and as the steady-state snapshot interval.
const RESUME_HOST_CALLS_PER_WARMUP_INVOCATION: u32 = 10_000;

const RESUME_HOST_CALLS_PER_SNAPSHOT_INTERVAL: u32 = 100_000;
const RESUME_FIRST_SNAPSHOT_WARMUP_INVOCATIONS: u32 = 9;
const RESUME_STEADY_SNAPSHOT_WARMUP_INVOCATIONS: u32 = 10;

fn split_host_calls(total: u32, invocations: u32) -> Vec<u32> {
    let invocations = invocations.min(total).max(1);
    let base = total / invocations;
    let remainder = total % invocations;

    (0..invocations)
        .map(|i| base + u32::from(i < remainder))
        .collect()
}

fn resume_warmup_chunks(snapshotting: bool, requested_host_calls: u32) -> Vec<u32> {
    if !snapshotting {
        let mut remaining = requested_host_calls;
        let mut chunks = Vec::new();
        while remaining > 0 {
            let chunk = remaining.min(RESUME_HOST_CALLS_PER_WARMUP_INVOCATION);
            chunks.push(chunk);
            remaining -= chunk;
        }
        return chunks;
    }

    let mut remaining = requested_host_calls;
    let mut chunks = Vec::new();
    let mut first_snapshot_interval = true;
    while remaining > 0 {
        let interval = remaining.min(RESUME_HOST_CALLS_PER_SNAPSHOT_INTERVAL);
        let invocations = if first_snapshot_interval {
            // Agent construction counts as the first invocation for every(10).
            RESUME_FIRST_SNAPSHOT_WARMUP_INVOCATIONS
        } else {
            RESUME_STEADY_SNAPSHOT_WARMUP_INVOCATIONS
        };
        chunks.extend(split_host_calls(interval, invocations));
        remaining -= interval;
        first_snapshot_interval = false;
    }
    chunks
}

/// Scenario 4 (durable-only): each ramp target is an independent replay burst.
/// The driver keeps the worker count fixed at `prefill_n`; each ramp target is
/// the number of cheap host calls (and therefore oplog-heavy replay work) made
/// by every prepared agent before the executor restart.
async fn run_resume_cell(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    probe: &ExecutorProbe,
) -> anyhow::Result<CellOutcome> {
    let prefill = config
        .prefill_n
        .ok_or_else(|| anyhow::anyhow!("resume-under-saturation cell missing prefill_n"))?;
    let mut outcome = CellOutcome::default();
    let mut detector = CeilingDetector::new();
    let mut timeout = AdaptiveTimeout::new();
    let started = Instant::now();

    let ramp = config.ramp.clone();

    'ramp: for &requested_host_calls_per_agent in &ramp {
        outcome.max_agents_reached = prefill;
        outcome.max_requested_host_calls_reached = requested_host_calls_per_agent;
        outcome.cleanup_already_done = false;

        info!(
            "Density-agent[{}]: warming {prefill} agents with {requested_host_calls_per_agent} host calls each",
            config.cell_name(),
        );
        let timeout_current = timeout.current;
        let warmups: Vec<(u32, Vec<AttemptOutcome>)> = futures::stream::iter(0..prefill)
            .map(|index| {
                let (component, agent) = agent_for_index(config, index, components)
                    .expect("agent_for_index within warmup");
                let component = component.clone();
                async move {
                    let chunks =
                        resume_warmup_chunks(config.snapshotting, requested_host_calls_per_agent);
                    let mut attempts = Vec::with_capacity(chunks.len());
                    for chunk in chunks {
                        attempts.push(
                            timed_invoke(
                                user,
                                &component,
                                &agent,
                                "oplog_heavy",
                                data_value!(chunk),
                                timeout_current,
                            )
                            .await,
                        );
                    }
                    (index, attempts)
                }
            })
            .buffer_unordered(RESUME_WARMUP_CONCURRENCY)
            .collect()
            .await;

        let warmup_attempts: Vec<&AttemptOutcome> = warmups
            .iter()
            .flat_map(|(_, attempts)| attempts.iter())
            .collect();
        let warmup_connection_alive = batch_connection_alive(&warmup_attempts);
        if !warmup_connection_alive {
            outcome.terminated_reason = TerminatedReason::ConnectionLost;
            outcome.catastrophic_ceiling_agents =
                u32::try_from(outcome.max_oplog_entries_reached).ok();
            break 'ramp;
        }

        let oplog_last_indices =
            collect_resume_oplog_last_indices(config, user, components, prefill).await?;
        let observed_oplog_entries_per_agent =
            oplog_last_indices.iter().copied().max().unwrap_or(0);
        let observed_oplog_entries_coord = u32::try_from(observed_oplog_entries_per_agent)
            .map_err(|_| anyhow::anyhow!("observed oplog index exceeds u32 ceiling coordinate"))?;
        outcome.max_oplog_entries_reached = observed_oplog_entries_per_agent;
        let min_observed = oplog_last_indices.iter().copied().min().unwrap_or(0);
        info!(
            "Density-agent[{}]: warmed {prefill} agents with {requested_host_calls_per_agent} requested host calls each; observed oplog last-index range {min_observed}..={observed_oplog_entries_per_agent}",
            config.cell_name(),
        );
        if min_observed != observed_oplog_entries_per_agent {
            anyhow::bail!(
                "resume target {requested_host_calls_per_agent} requested host calls produced inconsistent oplog last-index range {min_observed}..={observed_oplog_entries_per_agent} across warmed agents"
            );
        }

        if config.snapshotting {
            assert_sampled_resume_snapshots_present(
                config,
                user,
                components,
                prefill,
                requested_host_calls_per_agent,
            )
            .await?;
        }

        warm_resume_canary_agent(
            config,
            user,
            components,
            requested_host_calls_per_agent,
            timeout.current,
        )
        .await?;

        probe.restart_executor().await?;
        probe.restart_worker_service().await?;
        wait_for_resume_canary_agent(config, user, components, requested_host_calls_per_agent)
            .await?;

        let mut snapshot_event_capture = if config.snapshotting {
            start_snapshot_event_capture(config, user, components, 0).await
        } else {
            None
        };

        info!(
            "Density-agent[{}]: reviving {prefill} warmed agents concurrently after {requested_host_calls_per_agent} requested host calls each (observed max oplog last-index {observed_oplog_entries_per_agent})",
            config.cell_name()
        );
        let (resumed_batch, snapshot_recovery_event_seen) =
            if let Some(snapshot_event_capture) = snapshot_event_capture.take() {
                let (resumed_batch, snapshot_recovery_event_seen) = tokio::join!(
                    invoke_agent_indices(
                        config,
                        user,
                        components,
                        (0..prefill).collect(),
                        "increment",
                        data_value!(),
                        super::ceiling::ESCALATED_TIMEOUT,
                    ),
                    observe_snapshot_recovery_event(
                        config,
                        user,
                        components,
                        0,
                        snapshot_event_capture
                    ),
                );
                (resumed_batch, snapshot_recovery_event_seen)
            } else {
                (
                    invoke_agent_indices(
                        config,
                        user,
                        components,
                        (0..prefill).collect(),
                        "increment",
                        data_value!(),
                        super::ceiling::ESCALATED_TIMEOUT,
                    )
                    .await,
                    false,
                )
            };

        if config.snapshotting && !snapshot_recovery_event_seen {
            warn!(
                "Density-agent[{}]: snapshot recovery event not observed during resume; waiting {:?} before cleanup and checking stream history again",
                config.cell_name(),
                SNAPSHOT_EVENT_POST_RESUME_GRACE,
            );
            tokio::time::sleep(SNAPSHOT_EVENT_POST_RESUME_GRACE).await;
            if let Some(snapshot_event_capture) =
                start_snapshot_event_capture(config, user, components, 0).await
            {
                observe_snapshot_recovery_event(
                    config,
                    user,
                    components,
                    0,
                    snapshot_event_capture,
                )
                .await;
            }
        }

        detector.set_elapsed_secs(started.elapsed().as_secs_f64());
        let attempt_refs: Vec<&AttemptOutcome> = resumed_batch.iter().map(|(_, a)| a).collect();
        let connection_alive = batch_connection_alive(&attempt_refs);
        for (_, resume) in &resumed_batch {
            outcome
                .resume_existing_ms
                .push(resume.latency.as_secs_f64() * 1000.0);
            outcome
                .invoke_latencies
                .entry(observed_oplog_entries_coord)
                .or_default()
                .push(resume.latency);
            let sample = Sample {
                latency: resume.latency,
                coord: SampleCoord::Agents(observed_oplog_entries_coord),
                // The executor restart above is intentional; this phase relies
                // on connection-lost and timeout signals for catastrophic state.
                pod_restart_count: 0,
                connection_alive,
                overloaded: resume.overloaded,
                snapshot: CrossAxisSnapshot::default(),
                queue_depth: None,
            };
            for event in detector.observe(&sample) {
                handle_event(event, &mut outcome, &mut timeout);
            }
            if detector.is_terminal() {
                break 'ramp;
            }
        }

        if detector.is_terminal() {
            break 'ramp;
        }

        info!(
            "Density-agent[{}]: revived {prefill} warmed agents after {requested_host_calls_per_agent} requested host calls each (observed max oplog last-index {observed_oplog_entries_per_agent})",
            config.cell_name()
        );

        let step_outcome = CellOutcome {
            max_agents_reached: prefill,
            ..CellOutcome::default()
        };
        if cleanup_cell_agents(config, user, components, &step_outcome).await
            == CleanupResult::Failed
        {
            outcome.terminated_reason = TerminatedReason::ConnectionLost;
            outcome.catastrophic_ceiling_agents = Some(observed_oplog_entries_coord);
            break 'ramp;
        }

        outcome.cleanup_already_done = true;
    }

    Ok(outcome)
}

type SnapshotEventCapture = (UnboundedReceiver<Option<LogEvent>>, Option<Sender<()>>);

async fn start_snapshot_event_capture(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    index: u32,
) -> Option<SnapshotEventCapture> {
    let (component, parsed) = match agent_for_index(config, index, components) {
        Ok(agent) => agent,
        Err(err) => {
            warn!(
                "Density-agent[{}]: failed to resolve snapshot event stream agent {index}: {err:?}",
                config.cell_name()
            );
            return None;
        }
    };
    let agent_id = match AgentId::from_agent_id(component.id, &parsed) {
        Ok(agent_id) => agent_id,
        Err(err) => {
            warn!(
                "Density-agent[{}]: failed to build snapshot event stream agent id {index}: {err}",
                config.cell_name()
            );
            return None;
        }
    };

    let deadline = Instant::now() + SNAPSHOT_EVENT_STREAM_BUDGET;
    let mut attempt_no = 0u32;
    loop {
        attempt_no += 1;
        info!(
            "Density-agent[{}]: capturing snapshot recovery events for warmed agent {index} ({agent_id}), attempt {attempt_no}",
            config.cell_name()
        );
        match user.capture_output_with_termination(&agent_id).await {
            Ok((receiver, abort_tx)) => return Some((receiver, Some(abort_tx))),
            Err(err) => {
                warn!(
                    "Density-agent[{}]: failed to capture snapshot recovery events for warmed agent {index} ({agent_id}) attempt {attempt_no}: {err:?}",
                    config.cell_name()
                );
                if Instant::now() >= deadline {
                    return None;
                }
                tokio::time::sleep(SNAPSHOT_EVENT_STREAM_RETRY_DELAY).await;
            }
        }
    }
}

async fn observe_snapshot_recovery_event(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    index: u32,
    capture: SnapshotEventCapture,
) -> bool {
    const SNAPSHOT_EVENT_STREAM_RECONNECTS: u32 = 2;

    let deadline = Instant::now() + SNAPSHOT_EVENT_OBSERVATION_BUDGET;
    let mut seen_events = 0u64;
    let mut stream_attempts = 1u32;
    let (mut receiver, mut abort_tx) = capture;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            warn!(
                "Density-agent[{}]: no snapshot recovery event observed for sampled warmed agent {index}; seen {seen_events} other event(s)",
                config.cell_name(),
            );
            return false;
        }

        match tokio::time::timeout(remaining, receiver.recv()).await {
            Ok(Some(Some(event))) => {
                seen_events += 1;
                match AgentEvent::try_from(event) {
                    Ok(event @ AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. }) => {
                        info!(
                            "Density-agent[{}]: sampled warmed agent {index} event #{seen_events}: {event:?}",
                            config.cell_name(),
                        );
                        info!(
                            "Density-agent[{}]: sampled warmed agent {index} snapshot recovery succeeded from {snapshot_index}",
                            config.cell_name()
                        );
                        return true;
                    }
                    Ok(
                        ref event @ AgentEvent::SnapshotRecoveryFailed {
                            snapshot_index,
                            ref error,
                            ..
                        },
                    ) => {
                        warn!(
                            "Density-agent[{}]: sampled warmed agent {index} event #{seen_events}: {event:?}",
                            config.cell_name(),
                        );
                        warn!(
                            "Density-agent[{}]: sampled warmed agent {index} snapshot recovery failed from {snapshot_index}: {error}",
                            config.cell_name()
                        );
                        return true;
                    }
                    Ok(event) => {
                        info!(
                            "Density-agent[{}]: sampled warmed agent {index} event #{seen_events}: {event:?}",
                            config.cell_name(),
                        );
                    }
                    Err(error) => {
                        warn!(
                            "Density-agent[{}]: sampled warmed agent {index} event #{seen_events} could not be decoded: {error}",
                            config.cell_name(),
                        );
                    }
                }
            }
            Ok(Some(None)) | Ok(None) => {
                if stream_attempts <= SNAPSHOT_EVENT_STREAM_RECONNECTS {
                    let _ = abort_tx.take().map(|tx| tx.send(()));
                    stream_attempts += 1;
                    warn!(
                        "Density-agent[{}]: snapshot event stream ended before recovery event for sampled warmed agent {index}; seen {seen_events} other event(s); reconnecting (attempt {stream_attempts})",
                        config.cell_name(),
                    );
                    if let Some((new_receiver, new_abort_tx)) =
                        start_snapshot_event_capture(config, user, components, index).await
                    {
                        receiver = new_receiver;
                        abort_tx = new_abort_tx;
                        continue;
                    }
                }
                warn!(
                    "Density-agent[{}]: snapshot event stream ended before recovery event for sampled warmed agent {index}; seen {seen_events} other event(s)",
                    config.cell_name(),
                );
                return false;
            }
            Err(_) => {
                warn!(
                    "Density-agent[{}]: timed out waiting for snapshot recovery event for sampled warmed agent {index}; seen {seen_events} other event(s)",
                    config.cell_name(),
                );
                return false;
            }
        }
    }
}

async fn collect_resume_oplog_last_indices(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    prefill: u32,
) -> anyhow::Result<Vec<u64>> {
    let indices: Vec<anyhow::Result<u64>> = futures::stream::iter(0..prefill)
        .map(|index| async move {
            let (component, parsed) = agent_for_index(config, index, components)?;
            let agent_id = AgentId::from_agent_id(component.id, &parsed)
                .map_err(|err| anyhow::anyhow!(err))?;
            user.get_oplog_last_index(&agent_id).await
        })
        .buffer_unordered(RESUME_WARMUP_CONCURRENCY)
        .collect()
        .await;

    indices.into_iter().collect()
}

async fn assert_sampled_resume_snapshots_present(
    config: &CellConfig,
    user: &TestUserContext<BenchmarkTestDependencies>,
    components: &[ComponentDto],
    prefill: u32,
    requested_host_calls_per_agent: u32,
) -> anyhow::Result<()> {
    let sampled = prefill.min(3);
    for index in 0..sampled {
        let (component, parsed) = agent_for_index(config, index, components)?;
        let agent_id =
            AgentId::from_agent_id(component.id, &parsed).map_err(|err| anyhow::anyhow!(err))?;
        let snapshot_entries = user.search_oplog(&agent_id, "snapshot").await?;
        let snapshot_indices: Vec<_> = snapshot_entries
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Snapshot(_) => Some(entry.oplog_index),
                _ => None,
            })
            .collect();
        info!(
            "Density-agent[{}]: warmed agent {index} ({agent_id}) has {} snapshot oplog entries before restart: {:?}",
            config.cell_name(),
            snapshot_indices.len(),
            snapshot_indices,
        );
        if snapshot_indices.is_empty() {
            anyhow::bail!(
                "snapshot-enabled resume target {requested_host_calls_per_agent} requested host calls produced no snapshot oplog entries for sampled warmed agent {index} ({agent_id}) before restart"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn load_count_respects_fraction() {
        let cell = CellConfig {
            scenario: Scenario::CreateWithActiveFraction,
            mode: AgentMode::Durable,
            sharing: ComponentSharing::Shared,
            snapshotting: false,
            active_fraction: Some(50),
            prefill_n: None,
            ramp: vec![100, 250, 500],
        };
        assert_eq!(load_count(&cell, 1000), 500);
    }

    #[test]
    fn concurrent_active_loads_all() {
        let cell = CellConfig {
            scenario: Scenario::ConcurrentActive,
            mode: AgentMode::Durable,
            sharing: ComponentSharing::Shared,
            snapshotting: false,
            active_fraction: None,
            prefill_n: None,
            ramp: vec![100, 250, 500],
        };
        assert_eq!(load_count(&cell, 1000), 1000);
    }

    #[test]
    fn create_only_does_not_run_load_rounds() {
        let cell = CellConfig {
            scenario: Scenario::CreateOnly,
            mode: AgentMode::Durable,
            sharing: ComponentSharing::Shared,
            snapshotting: false,
            active_fraction: None,
            prefill_n: None,
            ramp: vec![100, 250, 500],
        };
        assert_eq!(load_count(&cell, 1000), 0);
    }

    #[test]
    fn snapshot_resume_warmup_aligns_first_snapshot_after_100k_host_calls() {
        let chunks = resume_warmup_chunks(true, 100_000);
        assert_eq!(chunks.len(), 9);
        assert_eq!(chunks.iter().sum::<u32>(), 100_000);
    }

    #[test]
    fn snapshot_resume_warmup_aligns_final_partial_interval_to_snapshot() {
        let chunks = resume_warmup_chunks(true, 170_000);
        assert_eq!(chunks.len(), 19);
        assert_eq!(chunks.iter().sum::<u32>(), 170_000);
        assert_eq!(chunks[..9].iter().sum::<u32>(), 100_000);
        assert_eq!(chunks[9..].iter().sum::<u32>(), 70_000);
    }

    #[test]
    fn non_snapshot_resume_warmup_uses_fixed_chunks() {
        let chunks = resume_warmup_chunks(false, 25_000);
        assert_eq!(chunks, vec![10_000, 10_000, 5_000]);
    }

    #[test]
    fn active_scenarios_do_not_precreate_agents() {
        let mut cell = CellConfig {
            scenario: Scenario::CreateWithActiveFraction,
            mode: AgentMode::Durable,
            sharing: ComponentSharing::Shared,
            snapshotting: false,
            active_fraction: Some(50),
            prefill_n: None,
            ramp: vec![100, 250, 500],
        };
        assert!(!has_create_phase(&cell));

        cell.scenario = Scenario::ConcurrentActive;
        cell.active_fraction = None;
        assert!(!has_create_phase(&cell));

        cell.scenario = Scenario::CreateOnly;
        assert!(has_create_phase(&cell));
    }
}

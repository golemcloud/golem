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

use crate::config::benchmark::TestMode;
use clap::Subcommand;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Subcommand)]
pub enum BenchmarkConfig {
    Benchmark {
        /// Name of the benchmark to be executed
        name: String,

        /// Number of repetitions of the benchmark with the same configuration.
        #[arg(long, default_value = "3")]
        iterations: usize,

        /// Cluster size. Can be repeated for multiple benchmarks runs.
        ///
        /// Not applicable to provided cluster.
        /// The total number of runs is multiplication for the number of different cluster sizes, sizes and lengths.
        #[arg(long, default_values_t = [3])]
        cluster_size: Vec<usize>,

        /// Benchmark-specific size parameter. Can be repeated for multiple benchmarks runs.
        ///
        /// The total number of runs is multiplication for the number of different cluster sizes, sizes and lengths.
        #[arg(long, default_values_t = [10])]
        size: Vec<usize>,

        /// Benchmark-specific length parameter.
        ///
        /// The total number of runs is multiplication for the number of different cluster sizes, sizes and lengths.
        #[arg(long, default_values_t = [100])]
        length: Vec<usize>,

        #[arg(long, default_value = "false")]
        disable_compilation_cache: bool,

        #[command(subcommand)]
        mode: TestMode,
    },
    Suite {
        /// Path to the benchmark suite specification
        path: PathBuf,

        /// Save the results to a new JSON file
        #[arg(long)]
        save_to_json: Option<PathBuf>,

        /// Save the results by appending a new run to an existing JSON file
        #[arg(long)]
        add_to_json: Option<PathBuf>,

        #[command(subcommand)]
        mode: TestMode,
    },
    /// Cloud density benchmarks (golemcloud/golem#3516). The buildspec drives
    /// the cell-by-cell loop; this subcommand runs exactly one action per
    /// invocation. `--action prep` performs the one-time density-prep and
    /// writes the prep manifest; `--action cell` runs one density cell using a
    /// previously-written manifest.
    Density {
        /// Action to perform: `prep` (one-time setup) or `cell` (run one cell).
        #[arg(long, value_enum)]
        action: DensityAction,

        /// Density section.
        #[arg(long, value_enum, default_value = "agent")]
        section: DensitySectionArg,

        /// Path to the prep manifest. Written by `--action prep`, read by
        /// `--action cell`.
        #[arg(long)]
        prep_manifest: PathBuf,

        /// Cell scenario (required for `--action cell`).
        #[arg(long, value_enum)]
        scenario: Option<DensityScenarioArg>,

        /// Agent durability mode (required for `--action cell`).
        #[arg(long, value_enum)]
        agent_mode: Option<DensityAgentModeArg>,

        /// Component sharing mode (required for `--action cell`).
        #[arg(long, value_enum)]
        sharing: Option<DensitySharingArg>,

        /// Snapshotting mode for cells that support it.
        #[arg(long, value_enum, default_value = "disabled")]
        snapshotting: DensitySnapshottingArg,

        /// Active fraction percentage (scenario create-with-active only).
        #[arg(long)]
        active_fraction: Option<u32>,

        /// Pre-fill agent count (scenario resume-under-saturation only).
        #[arg(long)]
        prefill: Option<u32>,

        /// Comma-separated increasing agent-count ramp the cell walks (e.g.
        /// `100,250,500,1000`). Supplied per-cell by the buildspec from the
        /// suite YAML. When omitted, the built-in default ramp is used.
        #[arg(long, value_delimiter = ',')]
        ramp: Option<Vec<u32>>,

        /// Schedule-density target residency.
        #[arg(long, value_enum)]
        schedule_target_residency: Option<DensityScheduleTargetResidencyArg>,

        /// Number of invocation-context spans added before scheduling an action.
        #[arg(long)]
        schedule_context_spans: Option<u32>,

        /// Schedule-density target distribution.
        #[arg(long, value_enum)]
        schedule_target_pattern: Option<DensityScheduleTargetPatternArg>,

        /// Duration of each schedule-density rate step in seconds.
        #[arg(long)]
        schedule_rate_period_secs: Option<u64>,

        /// Bytes passed to `complete_promise` for a promise-density cell.
        #[arg(long)]
        promise_payload_size: Option<usize>,

        /// Whether a promise-density completion finds an active waiter.
        #[arg(long, value_enum)]
        promise_waiter_presence: Option<DensityPromiseWaiterPresenceArg>,

        /// Worker-executor topology selected by the workflow for this cell.
        #[arg(long, value_enum)]
        promise_topology: Option<DensityPromiseTopologyArg>,

        /// Guest runtime used by a promise-density cell.
        #[arg(long, value_enum)]
        promise_runtime: Option<DensityPromiseRuntimeArg>,

        /// Optional executor pod name for `kubectl` restart-count polling
        /// (drives the catastrophic pod-restart condition).
        #[arg(long)]
        executor_pod_name: Option<String>,

        /// Kubernetes namespace of the executor pod.
        #[arg(long, default_value = "golem-release")]
        executor_namespace: String,

        /// Save the cell result to a JSON file.
        #[arg(long)]
        save_to_json: Option<PathBuf>,

        #[command(subcommand)]
        mode: TestMode,
    },
    /// Cloud chaos scenarios (GOL-363). One scenario per invocation, driven by
    /// the workflow the same way density cells are.
    ///
    /// The driver deliberately knows nothing about Kubernetes or Chaos Mesh: the
    /// workflow injects the fault and tells the driver about it by dropping JSON
    /// files into `--signal-dir` (see `integration_tests::chaos::signal`).
    Chaos {
        /// Action to perform: `prep` (one-time setup) or `scenario` (run one
        /// scenario end to end).
        #[arg(long, value_enum)]
        action: ChaosAction,

        /// Scenario code to run (required for `--action scenario`).
        #[arg(long, value_enum)]
        scenario: Option<ChaosScenarioArg>,

        /// Run a scenario the suite YAML marks `enabled: false`.
        ///
        /// `enabled` means "part of the default suite", and the workflow is what
        /// enforces it. This flag exists so a prototype scenario can be off for
        /// every ordinary run and still runnable on demand, without the two
        /// gates disagreeing — the workflow passes it exactly when an operator
        /// named a disabled scenario explicitly.
        #[arg(long, default_value_t = false)]
        allow_disabled: bool,

        /// Path to the chaos suite YAML holding the scenario's configuration.
        /// Required for `--action scenario`; the entry whose `code` matches
        /// `--scenario` supplies the fault, phase and workload settings.
        #[arg(long)]
        suite: Option<PathBuf>,

        /// Path to the prep manifest. Written by `--action prep`, read by
        /// `--action scenario`.
        #[arg(long)]
        prep_manifest: PathBuf,

        /// Directory the driver and the workflow exchange fault signals through.
        /// Required for `--action scenario`.
        #[arg(long)]
        signal_dir: Option<PathBuf>,

        /// Save the scenario result to a JSON file. Also written on abort, with
        /// whatever the run got through.
        #[arg(long)]
        save_to_json: Option<PathBuf>,

        /// Save the operation history to a JSON file.
        #[arg(long)]
        save_history_to_json: Option<PathBuf>,

        #[command(subcommand)]
        mode: TestMode,
    },
}

/// Chaos subcommand action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ChaosAction {
    Prep,
    Scenario,
}

/// Chaos scenario selector for the CLI. Codes match the scenario catalog in the
/// Cloud Chaos Tests v1 project and the `code` fields in the suite YAML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ChaosScenarioArg {
    /// Shard-manager to executor network partition.
    S1,
    /// Executor pod kill with pinned HTTP invocations in flight.
    S8,
    /// Executor pod kill during an automatic component update.
    S5,
    /// Shard-manager pod restart under mixed workload.
    S12,
    /// Rolling executor restarts under load.
    S13,
    /// Executor pod kill while scheduled actions are between claim and fire.
    S10,
    /// Executor pod kill while agents are suspended on promises being completed.
    S11,
    /// Executor cut off from worker-service while it keeps its shards.
    S3,
    /// Two executors cut off from each other while agents on one invoke agents
    /// on the other. The control: the link carries no traffic.
    S2,
    /// Executor pod kill while agents are having their state reverted.
    S7,
    /// Executor pod kill while agents are being deleted.
    S6,
    /// Executor pod kill while a component rollback is in flight.
    S9,
    /// Executors cut off from the key-value PostgreSQL cluster for about as
    /// long as an AWS storage failover takes.
    S16,
    /// The same cut, held for longer than the key-value retry budget.
    S22,
    /// Executors cut off from the indexed-oplog PostgreSQL cluster for the
    /// length of a writer failover.
    S14,
    /// Executors cut off from the Redis cache in front of the key-value layer,
    /// for longer than a caller is willing to wait.
    S18,
    /// The same Redis cache slowed rather than removed.
    S17,
    /// The key-value PostgreSQL cluster slowed rather than removed. S17's
    /// mirror on the other half of the split key-value layer.
    S15,
    /// S15 driving `ephemeral` alone: the control for the elimination series.
    S15A,
    /// S15A plus `durable`.
    S15B,
    /// S15B plus `promise`.
    S15C,
    /// The indexed-oplog PostgreSQL cluster slowed rather than removed. S14's
    /// mirror, and the one delay with no control stream: every agent commits
    /// its oplog to that cluster.
    S23,
    /// The first composed fault: a worker-executor killed while the key-value
    /// PostgreSQL cluster is unreachable.
    Mf1,
    /// MF1 with the overlap pushed past the shard-manager's assign timeout.
    Mf1b,
    /// Worker-service CPU saturation while agents drive cross-pod RPC. S2's
    /// populations with the fault moved off the link and onto the relay.
    S21,
    /// One executor's wall clock moved half a minute behind the cluster, where
    /// the quota lease is the only contract two clocks are compared across.
    S19,
}

/// Density subcommand action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensityAction {
    Prep,
    Cell,
}

/// Density section selector for the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensitySectionArg {
    Agent,
    Schedule,
    Promise,
}

/// Agent-density scenario selector for the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensityScenarioArg {
    CreateOnly,
    CreateWithActive,
    ConcurrentActive,
    ResumeUnderSaturation,
}

/// Agent durability mode selector for the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensityAgentModeArg {
    Durable,
    Ephemeral,
}

/// Component sharing mode selector for the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensitySharingArg {
    Shared,
    PerAgent,
}

/// Snapshotting mode selector for density cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensitySnapshottingArg {
    Disabled,
    Enabled,
}

/// Target residency mode for schedule-density cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensityScheduleTargetResidencyArg {
    Warm,
    Cold,
}

/// Target distribution for schedule-density cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensityScheduleTargetPatternArg {
    Spread,
    Realistic,
}

/// Waiter state at promise completion time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensityPromiseWaiterPresenceArg {
    Cold,
    Warm,
    Mixed,
}

/// Worker-executor topology used by a promise-density cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensityPromiseTopologyArg {
    OnePod,
    TwoPod,
}

/// Guest runtime used by a promise-density cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DensityPromiseRuntimeArg {
    Rust,
    Ts,
}

impl BenchmarkConfig {
    pub fn mode(&self) -> &TestMode {
        match self {
            BenchmarkConfig::Benchmark { mode, .. }
            | BenchmarkConfig::Suite { mode, .. }
            | BenchmarkConfig::Density { mode, .. }
            | BenchmarkConfig::Chaos { mode, .. } => mode,
        }
    }

    pub fn iterations(&self) -> usize {
        match self {
            BenchmarkConfig::Benchmark { iterations, .. } => *iterations,
            BenchmarkConfig::Suite { .. }
            | BenchmarkConfig::Density { .. }
            | BenchmarkConfig::Chaos { .. } => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    pub cluster_size: usize,
    pub size: usize,
    pub length: usize,
    pub disable_compilation_cache: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkSuite {
    pub name: String,
    pub benchmarks: Vec<BenchmarkSuiteItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkSuiteItem {
    pub name: String,
    pub iterations: usize,
    pub cluster_size: Vec<usize>,
    pub size: Vec<usize>,
    pub length: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_compilation_cache: Option<bool>,
}

impl BenchmarkSuiteItem {
    pub fn runs(&self, mode: &TestMode) -> Vec<RunConfig> {
        let cluster_size: Vec<usize> = match mode {
            TestMode::Provided { .. } | TestMode::Cloud { .. } => {
                vec![0]
            }
            _ => self
                .cluster_size
                .iter()
                .copied()
                .unique()
                .sorted()
                .collect(),
        };

        let size = self
            .size
            .iter()
            .copied()
            .unique()
            .sorted()
            .collect::<Vec<_>>();
        let length = self
            .length
            .iter()
            .copied()
            .unique()
            .sorted()
            .collect::<Vec<_>>();

        let mut res = Vec::new();

        for cluster_size in cluster_size {
            for &size in &size {
                for &length in &length {
                    res.push(RunConfig {
                        cluster_size,
                        size,
                        length,
                        disable_compilation_cache: self
                            .disable_compilation_cache
                            .unwrap_or_default(),
                    })
                }
            }
        }

        res
    }
}

/// Smoke tests for cloud-mode wiring that do not require running services.
///
/// For a full end-to-end smoke test that exercises actual HTTP clients,
/// cleanup, and the benchmark API contract, run the binary directly against a
/// local Spawned cluster:
///
/// ```text
/// cargo run --bin benchmarks -- benchmark cold-start-unknown-small \
///   --size 1 --iterations 1 --length 0 \
///   cloud \
///   --api-url http://localhost:8081 \
///   --apps-base-domain golem.cloud \
///   --admin-account-id <uuid> \
///   --admin-account-email <email> \
///   --admin-account-token <token> \
///   --builtin-plugin-owner-account-id <uuid> \
///   --default-plan-id <uuid>
/// ```
#[cfg(test)]
mod cloud_mode_smoke {
    use super::*;
    use test_r::test;
    use url::Url;
    use uuid::Uuid;

    fn cloud_mode() -> TestMode {
        TestMode::Cloud {
            api_url: Url::parse("https://release.dev-api.golem.cloud").unwrap(),
            apps_base_domain: "apps.dev.golem.cloud".to_string(),
            admin_account_token: "test-token".to_string(),
            builtin_plugin_owner_account_id: Uuid::nil(),
            default_plan_id: Uuid::nil(),
            shard_manager_grpc_host: None,
            shard_manager_grpc_port: None,
            component_directory: "test-components".to_string(),
        }
    }

    /// Cloud mode always returns exactly one `RunConfig` with `cluster_size=0`,
    /// regardless of how many `cluster_size` values the suite item specifies.
    #[test]
    fn runs_returns_single_cluster_size_zero_run() {
        let mode = cloud_mode();
        let item = BenchmarkSuiteItem {
            name: "cold-start-unknown-small".to_string(),
            iterations: 3,
            cluster_size: vec![1, 3, 5], // must be ignored in cloud mode
            size: vec![10],
            length: vec![100],
            disable_compilation_cache: None,
        };
        let runs = item.runs(&mode);
        assert_eq!(runs.len(), 1, "cloud mode ignores cluster_size variations");
        assert_eq!(runs[0].cluster_size, 0, "cloud mode cluster_size must be 0");
        assert_eq!(runs[0].size, 10);
        assert_eq!(runs[0].length, 100);
    }

    /// Multiple size and length combinations still expand normally; only
    /// `cluster_size` is collapsed.
    #[test]
    fn runs_expands_size_and_length_but_not_cluster_size() {
        let mode = cloud_mode();
        let item = BenchmarkSuiteItem {
            name: "latency-small".to_string(),
            iterations: 1,
            cluster_size: vec![1, 3],
            size: vec![5, 10],
            length: vec![50, 100],
            disable_compilation_cache: None,
        };
        let runs = item.runs(&mode);
        // 1 (collapsed cluster_size) × 2 sizes × 2 lengths = 4 runs
        assert_eq!(runs.len(), 4);
        for r in &runs {
            assert_eq!(r.cluster_size, 0);
        }
    }
}

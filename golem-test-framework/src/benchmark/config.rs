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

        /// Check that every component artifact required by the suite exists, then exit
        #[arg(long, default_value = "false")]
        check_artifacts: bool,

        /// Save the results to a new JSON file
        #[arg(long)]
        save_to_json: Option<PathBuf>,

        /// Save the results by appending a new run to an existing JSON file
        #[arg(long)]
        add_to_json: Option<PathBuf>,

        /// Stable identifier for the runner hardware class
        #[arg(long)]
        runner_id: Option<String>,

        /// Human-readable runner name
        #[arg(long, requires = "runner_id")]
        runner_label: Option<String>,

        /// Source repository in owner/name form
        #[arg(long)]
        source_repository: Option<String>,

        /// Source commit SHA
        #[arg(long)]
        source_commit_sha: Option<String>,

        /// Source ref, such as refs/heads/main
        #[arg(long)]
        source_ref: Option<String>,

        #[command(subcommand)]
        mode: TestMode,
    },
    /// Cloud density benchmarks. The buildspec drives
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
            | BenchmarkConfig::Density { mode, .. } => mode,
        }
    }

    pub fn iterations(&self) -> usize {
        match self {
            BenchmarkConfig::Benchmark { iterations, .. } => *iterations,
            BenchmarkConfig::Suite { .. } | BenchmarkConfig::Density { .. } => 0,
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
    /// Leave the TypeScript agents out of the throughput benchmarks. The other
    /// benchmarks choose one agent type each, so a suite selects them by name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_ts_agents: bool,
    /// Settings of the ingress-load benchmark. The other benchmarks ignore them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingress: Option<IngressSettings>,
    pub benchmarks: Vec<BenchmarkSuiteItem>,
}

/// How the ingress-load benchmark makes its clients behave. `size` gives the number of
/// accounts and `length` the target request rate; these give the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IngressSettings {
    /// How long each step sends requests. Longer than the 600 s TTL of the worker-service
    /// auth and router caches, so every entry expires at least once during a step.
    pub step_secs: u64,
    /// Agents that each account calls, so no single agent limits the rate.
    pub agents_per_account: usize,
    /// Requests a client sends before it closes its connection and opens a new one.
    /// 0 keeps one connection for the whole step.
    pub requests_per_connection: usize,
    /// Share of the clients that use HTTP/1.1. The others negotiate HTTP/2.
    pub http1_percent: usize,
    /// Share of the requests that go through the HTTP API. The others go to the REST
    /// invoke endpoint with the token of the account.
    pub http_api_percent: usize,
    /// Number of HTTP API domains. Each one has its own environment and component.
    pub http_api_domains: usize,
    /// Accounts set up at the same time.
    pub setup_concurrency: usize,
}

impl Default for IngressSettings {
    fn default() -> Self {
        Self {
            step_secs: 720,
            agents_per_account: 4,
            requests_per_connection: 0,
            http1_percent: 0,
            http_api_percent: 50,
            http_api_domains: 8,
            setup_concurrency: 32,
        }
    }
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

#[cfg(test)]
mod metadata_cli {
    use super::*;
    use clap::Parser;
    use test_r::test;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        config: BenchmarkConfig,
    }

    #[test]
    fn suite_accepts_runner_and_source_metadata() {
        let cli = Cli::try_parse_from([
            "test",
            "suite",
            "--check-artifacts",
            "--runner-id",
            "amp-orb-a1.xxlarge",
            "--runner-label",
            "Amp orb (a1.xxlarge)",
            "--source-repository",
            "golemcloud/golem",
            "--source-commit-sha",
            "0123456789abcdef",
            "--source-ref",
            "refs/heads/main",
            "suite.yaml",
            "spawned",
        ])
        .unwrap();

        let BenchmarkConfig::Suite {
            check_artifacts,
            runner_id,
            runner_label,
            source_repository,
            source_commit_sha,
            source_ref,
            ..
        } = cli.config
        else {
            panic!("expected suite configuration");
        };
        assert!(check_artifacts);
        assert_eq!(runner_id.as_deref(), Some("amp-orb-a1.xxlarge"));
        assert_eq!(runner_label.as_deref(), Some("Amp orb (a1.xxlarge)"));
        assert_eq!(source_repository.as_deref(), Some("golemcloud/golem"));
        assert_eq!(source_commit_sha.as_deref(), Some("0123456789abcdef"));
        assert_eq!(source_ref.as_deref(), Some("refs/heads/main"));
    }

    #[test]
    fn runner_label_requires_runner_id() {
        let result = Cli::try_parse_from([
            "test",
            "suite",
            "--runner-label",
            "Amp orb",
            "suite.yaml",
            "spawned",
        ]);

        assert!(result.is_err());
    }
}

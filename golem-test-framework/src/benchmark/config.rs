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
}

impl BenchmarkConfig {
    pub fn mode(&self) -> &TestMode {
        match self {
            BenchmarkConfig::Benchmark { mode, .. } | BenchmarkConfig::Suite { mode, .. } => mode,
        }
    }

    pub fn iterations(&self) -> usize {
        match self {
            BenchmarkConfig::Benchmark { iterations, .. } => *iterations,
            BenchmarkConfig::Suite { .. } => 0,
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

// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
            TestMode::Provided { .. } => {
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

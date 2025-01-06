// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::{Display, Formatter};

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use strum_macros::EnumIter;
use test_r::test_dep;
use tracing::info;

pub mod cli;

mod api_definition;
mod api_deployment;
mod api_deployment_fileserver;
mod component;
mod get;
mod profile;
mod text;
mod worker;

#[derive(Debug, Copy, Clone, EnumIter)]
pub enum RefKind {
    Name,
    Url,
    Urn,
}

impl Display for RefKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RefKind::Name => write!(f, "name"),
            RefKind::Url => write!(f, "url"),
            RefKind::Urn => write!(f, "urn"),
        }
    }
}

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(&TracingConfig::test("cli-tests"));
        Self
    }
}

#[test_dep]
pub fn tracing() -> Tracing {
    Tracing::init()
}

#[test_dep]
async fn test_dependencies(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        keep_docker_containers: false,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await;

    let cluster = deps.worker_executor_cluster(); // forcing startup by getting it
    info!("Using cluster with {:?} worker executors", cluster.size());

    deps
}

test_r::enable!();

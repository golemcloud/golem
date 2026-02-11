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

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_wasm::analysis::wit_parser::{AnalysedTypeResolve, SharedAnalysedTypeResolve};
use golem_worker_executor_test_utils::{LastUniqueId, WorkerExecutorTestDependencies};
use std::fmt::Debug;
use std::path::Path;
use std::sync::atomic::AtomicU16;
use test_r::{tag_suite, test_dep};

pub mod agent;
pub mod api;
pub mod blobstore;
pub mod compatibility;
pub mod durability;
pub mod hot_update;
pub mod http;
pub mod indexed_storage;
pub mod key_value_storage;
pub mod keyvalue;
pub mod observability;
pub mod rdbms;
pub mod rdbms_service;
pub mod revert;
pub mod rpc;
pub mod scalability;
pub mod transactions;
pub mod wasi;

test_r::enable!();

tag_suite!(api, group1);
tag_suite!(blobstore, group1);
tag_suite!(keyvalue, group1);
tag_suite!(http, group1);
tag_suite!(rdbms, group1);
tag_suite!(agent, group1);

tag_suite!(transactions, group2);
tag_suite!(wasi, group2);
tag_suite!(revert, group2);
tag_suite!(durability, group2);
tag_suite!(observability, group2);
tag_suite!(scalability, group2);
tag_suite!(hot_update, group2);
tag_suite!(rpc, group2);

tag_suite!(rdbms_service, rdbms_service);

#[derive(Debug)]
pub struct Tracing;

#[test_dep]
pub fn tracing() -> Tracing {
    init_tracing_with_default_debug_env_filter(
        &TracingConfig::test_pretty_without_time("worker-executor-tests").with_env_overrides(),
    );

    Tracing
}

#[test_dep]
pub async fn test_dependencies(_tracing: &Tracing) -> WorkerExecutorTestDependencies {
    WorkerExecutorTestDependencies::new().await
}

#[test_dep]
pub fn last_unique_id() -> LastUniqueId {
    LastUniqueId {
        id: AtomicU16::new(0),
    }
}

#[test_dep(tagged_as = "golem_host")]
pub fn golem_host_analysed_type_resolve() -> SharedAnalysedTypeResolve {
    SharedAnalysedTypeResolve::new(
        AnalysedTypeResolve::from_wit_directory(Path::new("../wit")).unwrap(),
    )
}

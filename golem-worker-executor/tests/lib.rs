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

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_wasm::analysis::wit_parser::{AnalysedTypeResolve, SharedAnalysedTypeResolve};
use golem_worker_executor_test_utils::{
    test_component, LastUniqueId, PrecompiledComponent, WorkerExecutorTestDependencies,
};
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
pub mod routed_key_value_storage;
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

tag_suite!(hot_update, group2);
tag_suite!(transactions, group2);
tag_suite!(observability, group2);

tag_suite!(durability, group3);
tag_suite!(rpc, group3);
tag_suite!(wasi, group3);
tag_suite!(scalability, group3);
tag_suite!(revert, group3);

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

// Pre-compiled test components - these warm the analysis cache during
// test-r dependency initialization so that tests don't pay the cold
// compilation cost of `extract_agent_types`.

test_component!(
    host_api_tests,
    "host_api_tests",
    "golem_it_host_api_tests_release",
    "golem-it:host-api-tests"
);
test_component!(
    agent_rpc,
    "agent_rpc",
    "golem_it_agent_rpc",
    "golem-it:agent-rpc"
);
test_component!(
    agent_rpc_rust,
    "agent_rpc_rust",
    "golem_it_agent_rpc_rust_release",
    "golem-it:agent-rpc-rust"
);
test_component!(
    agent_rpc_rust_as_resolve_target,
    "agent_rpc_rust_as_resolve_target",
    "golem_it_agent_rpc_rust_release",
    "component-resolve-target"
);
test_component!(
    agent_counters,
    "agent_counters",
    "it_agent_counters_release",
    "it:agent-counters"
);
test_component!(
    http_tests,
    "http_tests",
    "golem_it_http_tests_release",
    "golem-it:http-tests"
);
test_component!(
    constructor_parameter_echo,
    "constructor_parameter_echo",
    "golem_it_constructor_parameter_echo",
    "golem-it:constructor-parameter-echo"
);
test_component!(
    constructor_parameter_echo_unnamed,
    "constructor_parameter_echo_unnamed",
    "golem_it_constructor_parameter_echo",
    "golem_it_constructor_parameter_echo"
);
test_component!(
    agent_update_v1,
    "agent_update_v1",
    "it_agent_update_v1_release",
    "it:agent-update"
);
test_component!(
    agent_update_v2,
    "agent_update_v2",
    "it_agent_update_v2_release",
    "it:agent-update"
);
test_component!(
    initial_file_system,
    "initial_file_system",
    "it_initial_file_system_release",
    "golem-it:initial-file-system"
);
test_component!(
    large_dynamic_memory,
    "large_dynamic_memory",
    "scalability_large_dynamic_memory_release",
    "scalability:large-dynamic-memory"
);
test_component!(
    large_initial_memory,
    "large_initial_memory",
    "scalability_large_initial_memory_release",
    "scalability:large-initial-memory"
);

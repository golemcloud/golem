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

use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{LastUniqueId, PrecompiledComponent, WorkerExecutorTestDependencies};
use test_r::{inherit_test_dep, timeout_suite};

use crate::Tracing;

pub mod host_services;
pub mod http_request;
pub mod http_servers;
pub mod http_streams;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

// Default per-test timeouts for retry submodules. `timeout_suite!` matches
// tests by module-path prefix, so these apply to tests within each named
// submodule and its descendants.
timeout_suite!(host_services, "2 minutes");
timeout_suite!(http_request, "2 minutes");
timeout_suite!(http_streams, "2 minutes");

/// Helper: count oplog Error entries whose error message contains the given substring.
pub(crate) async fn count_oplog_errors_containing(
    executor: &impl TestDsl,
    worker_id: &golem_common::model::AgentId,
    substring: &str,
) -> anyhow::Result<usize> {
    let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
    Ok(oplog
        .iter()
        .filter(|e| {
            if let PublicOplogEntry::Error(params) = &e.entry {
                params.error.contains(substring)
            } else {
                false
            }
        })
        .count())
}

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

//! The Go SDK's logging against the real host: slog, the standard log package
//! and `logging.Log` reach the oplog as log entries with their levels.

use crate::Tracing;
use golem_common::model::oplog::{LogLevel, OplogIndex, PublicOplogEntry};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// Each record lands as one oplog log entry at its level; a record below the
/// default Info level is not emitted, and slog attributes follow the message.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_log_records_reach_the_oplog_with_their_levels(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("LogAgent", "go-log-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "emit", data_value!("x1"))
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let records: Vec<(LogLevel, String, String)> = oplog
        .iter()
        .filter_map(|e| match &e.entry {
            PublicOplogEntry::Log(log)
                if log.level != LogLevel::Stdout && log.level != LogLevel::Stderr =>
            {
                Some((log.level, log.context.clone(), log.message.clone()))
            }
            _ => None,
        })
        .collect();
    let find = |message: &str| {
        records
            .iter()
            .find(|(_, _, m)| m == message)
            .unwrap_or_else(|| panic!("no log record {message:?} in {records:?}"))
    };

    assert_eq!(find("info x1 n=1").0, LogLevel::Info);
    assert_eq!(find("warn x1").0, LogLevel::Warn);
    assert_eq!(find("error x1").0, LogLevel::Error);
    assert_eq!(find("stdlog x1").0, LogLevel::Info);
    let trace = find("trace x1");
    assert_eq!((trace.0, trace.1.as_str()), (LogLevel::Trace, "raw"));
    let critical = find("critical x1");
    assert_eq!(
        (critical.0, critical.1.as_str()),
        (LogLevel::Critical, "raw")
    );
    assert!(
        !records.iter().any(|(_, _, m)| m == "debug x1"),
        "a debug record is below the default level: {records:?}"
    );
    Ok(())
}

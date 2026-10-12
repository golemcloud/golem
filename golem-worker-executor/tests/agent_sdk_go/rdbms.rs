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

//! The Go SDK's Postgres wrapper against a real database: committed and
//! rolled-back transactions, Query and QueryStream agreeing, a classified
//! error, and recorded statements replaying after a crash instead of running
//! again. Mirrors `rdbms::rdbms_postgres_crud`.

use crate::Tracing;
use golem_common::{agent_id, data_value};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use test_r::{inherit_test_dep, test, test_dep, timeout};
use uuid::Uuid;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

#[test_dep(scope = Shared)]
async fn postgres() -> DockerPostgresRdb {
    DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn go_postgres_transactions_queries_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let address = postgres.public_connection_string();
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("PgAgent", "go-pg-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let table = "go_sdk_users";

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create",
            data_value!(address.clone(), table),
        )
        .await?;
    let mut affected = Vec::new();
    for (id, name) in [(1i64, "alice"), (2i64, "bob")] {
        affected.push(
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "insert",
                    data_value!(address.clone(), table, id, name),
                )
                .await?
                .into_typed::<u64>()?,
        );
    }
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "insert_rolled_back",
            data_value!(address.clone(), table, 3i64, "carol"),
        )
        .await?;
    let names = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "names",
            data_value!(address.clone(), table),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    let error_kind = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "failing_query",
            data_value!(address.clone(), table),
        )
        .await?
        .into_typed::<String>()?;

    // Replaying the inserts against the database would violate the primary key.
    executor.simulated_crash(&worker_id).await?;

    let names_after_crash = executor
        .invoke_and_await_agent(&component, &agent_id, "names", data_value!(address, table))
        .await?
        .into_typed::<Vec<String>>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(affected, vec![1, 1]);
    assert_eq!(names, vec!["alice".to_string(), "bob".to_string()]);
    assert_eq!(error_kind, "query-execution-failure");
    assert_eq!(names_after_crash, names);
    Ok(())
}

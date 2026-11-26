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

//! A TypeScript agent method returns its result before its unawaited background work, and the
//! work then completes before the invocation settles. Exercises `TsBackgroundWork`.

use crate::Tracing;
use golem_common::model::PromiseId;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);

#[test]
#[timeout("3m")]
async fn ts_method_returns_before_its_background_work_and_settles_after_it(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent = agent_key!("TsBackgroundWork", "early-result");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    // The background work waits for the returned promise, which only the caller can complete,
    // so the result arrives only when it does not wait for the background work.
    let release: PromiseId = tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent(&component, &agent, "returnBeforeRelease", data_value!()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the result waited for the background work"))??
    .into_typed()?;
    executor
        .complete_promise(&component.environment_id, &release, vec![1])
        .await?;

    let creates_promise = |entry: &PublicOplogEntry| {
        matches!(entry, PublicOplogEntry::Start(start)
            if start.function_name == "golem::api::create_promise")
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        let position = |matches: &dyn Fn(&PublicOplogEntry) -> bool| {
            oplog
                .iter()
                .filter(|entry| matches(&entry.entry))
                .map(|entry| entry.oplog_index)
                .collect::<Vec<_>>()
        };
        let finished =
            position(&|entry| matches!(entry, PublicOplogEntry::AgentInvocationFinished(_)));
        let settled =
            position(&|entry| matches!(entry, PublicOplogEntry::AgentInvocationSettled(_)));
        let promises = position(&creates_promise);
        // The constructor and the method each finish and settle.
        if settled.len() == 2 {
            assert_eq!(finished.len(), 2, "{oplog:#?}");
            assert_eq!(promises.len(), 2, "{oplog:#?}");
            // The release is created before the result; the background promise between the
            // result and the settlement.
            assert!(promises[0] < finished[1], "{oplog:#?}");
            assert!(
                finished[1] < promises[1] && promises[1] < settled[1],
                "{oplog:#?}"
            );
            return Ok(());
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the background work did not settle: {oplog:#?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

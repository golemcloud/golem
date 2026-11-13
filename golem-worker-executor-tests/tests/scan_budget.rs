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

use crate::Tracing;
use golem_api_grpc::proto::golem::worker::OplogCursor;
use golem_api_grpc::proto::golem::worker::v1::{
    StaleCursor, WorkerExecutionError, worker_execution_error,
};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    GetOplogRequest, SearchOplogRequest, SearchOplogSuccessResponse, get_oplog_response,
    search_oplog_response,
};
use golem_common::base_model::worker_filter::StringFilterComparator;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogPageEntry};
use golem_common::model::{AgentFilter, AgentId, PromiseId, ScanCursor};
use golem_common::schema::{FromSchema, SchemaValue};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::ScanBudget;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::BTreeSet;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// Matches only the entries of the last invocation of `oplog_with_a_rare_match_at_the_end`.
const RARE_QUERY: &str = "generate_idempotency";

async fn start_with_scan_budget(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    budget: ScanBudget,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.limits.scan_budget = budget;
            })),
            ..Default::default()
        },
    )
    .await
}

/// Starts an agent with `calls` clock calls in its oplog, each a `Start` and an `End`, followed
/// by one invocation whose entries match `RARE_QUERY`. Returns the agent and its last index.
async fn oplog_with_a_rare_match_at_the_end(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    host_api_tests: &PrecompiledComponent,
    name: &str,
    calls: u64,
) -> anyhow::Result<(AgentId, u64)> {
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", name.to_string());
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let promise_id = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .expect("create_promise must return an id");
    // The promise is complete before the agent waits for it, so the agent does not suspend.
    executor
        .complete_promise(
            &PromiseId::from_value(&promise_id).expect("create_promise must return a promise id"),
            vec![1],
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "local_calls_then_await_promise",
            crate::raw_params(vec![SchemaValue::U64(calls), promise_id]),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?;
    let last_index = executor.get_oplog_last_index(&worker_id).await?;
    assert!(
        last_index >= 2 * calls,
        "every clock call appends a Start and an End, got {last_index} entries for {calls} calls"
    );
    Ok((worker_id, last_index))
}

async fn search_page(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
    cursor: Option<OplogCursor>,
) -> anyhow::Result<SearchOplogSuccessResponse> {
    let response = executor
        .client
        .clone()
        .search_oplog(SearchOplogRequest {
            agent_id: Some(worker_id.clone().into()),
            environment_id: Some(executor.context.default_environment_id.into()),
            cursor,
            count: 100,
            query: RARE_QUERY.to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
        })
        .await?
        .into_inner();
    match response.result {
        Some(search_oplog_response::Result::Success(page)) => Ok(page),
        other => Err(anyhow::anyhow!("the oplog search failed: {other:?}")),
    }
}

fn page_indices(page: &SearchOplogSuccessResponse) -> Vec<u64> {
    page.entries
        .iter()
        .map(
            |entry| match PublicOplogPageEntry::try_from(entry.clone()).unwrap() {
                PublicOplogPageEntry::Included(entry) => entry.oplog_index.as_u64(),
                PublicOplogPageEntry::Omitted(omitted) => omitted.oplog_index.as_u64(),
            },
        )
        .collect()
}

#[test]
#[timeout("15m")]
async fn oplog_search_stops_at_the_entry_budget_and_continues_with_its_cursor(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const BUDGET: u64 = 100_000;
    let context = TestContext::new(last_unique_id);
    let executor = start_with_scan_budget(
        deps,
        &context,
        ScanBudget {
            entries: NonZeroU64::new(BUDGET).unwrap(),
            time: Duration::from_secs(3600),
        },
    )
    .await?;
    let (worker_id, last_index) = oplog_with_a_rare_match_at_the_end(
        &executor,
        &context,
        host_api_tests,
        "scan-budget-entries",
        125_000,
    )
    .await?;
    assert!(last_index >= 250_000);

    // Each call reads exactly the budget and stops before the match, with an empty page and a
    // cursor to the first entry it did not read.
    let mut cursor = None;
    for call in 1..=2 {
        let page = search_page(&executor, &worker_id, cursor).await?;
        assert_eq!(page_indices(&page), Vec::<u64>::new());
        let next = page
            .next
            .expect("a search that the budget stops returns a cursor");
        assert_eq!(next.next_oplog_index, 1 + call * BUDGET);
        cursor = Some(next);
    }

    // The third call continues where the second stopped, reads the rest and finds the match.
    let page = search_page(&executor, &worker_id, cursor).await?;
    assert_eq!(page.next, None);
    let found = page_indices(&page);
    let expected = executor
        .get_oplog(&worker_id, OplogIndex::from_u64(2 * BUDGET + 1))
        .await?
        .into_iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name.contains("generate_idempotency")))
        .map(|entry| entry.oplog_index.as_u64())
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 2);
    assert!(
        expected.iter().all(|index| found.contains(index)),
        "the search found {found:?}, which lacks the key generation calls {expected:?}"
    );
    Ok(())
}

#[test]
#[timeout("10m")]
async fn oplog_search_stops_at_the_time_budget_and_continues_with_its_cursor(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_scan_budget(
        deps,
        &context,
        ScanBudget {
            entries: NonZeroU64::MAX,
            time: Duration::from_millis(1),
        },
    )
    .await?;
    let (worker_id, last_index) = oplog_with_a_rare_match_at_the_end(
        &executor,
        &context,
        host_api_tests,
        "scan-budget-time",
        10_000,
    )
    .await?;

    // The entry budget is unlimited, so only the time stops the first call, long before the end.
    let first = search_page(&executor, &worker_id, None).await?;
    assert_eq!(page_indices(&first), Vec::<u64>::new());
    let next = first
        .next
        .expect("a search that the time budget stops returns a cursor");
    assert!(
        next.next_oplog_index < last_index / 2,
        "the first call read up to {} of {last_index} entries",
        next.next_oplog_index
    );

    // Every call reads at least one chunk, so following the cursors reaches the match.
    let mut cursor = Some(next);
    let mut found = Vec::new();
    let mut calls = 1;
    while let Some(current) = cursor {
        let page = search_page(&executor, &worker_id, Some(current.clone())).await?;
        if let Some(next) = &page.next {
            assert!(next.next_oplog_index > current.next_oplog_index);
        }
        found.extend(page_indices(&page));
        cursor = page.next;
        calls += 1;
    }
    assert!(calls > 2);
    assert!(
        found.len() >= 2 && found.iter().all(|index| *index > last_index - 100),
        "the search found {found:?} in an oplog that ends at {last_index}"
    );
    Ok(())
}

#[test]
#[timeout("5m")]
async fn agent_list_with_an_unbounded_count_stops_at_the_scan_budget(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const BUDGET: u64 = 5;
    const AGENTS: u64 = 12;
    let context = TestContext::new(last_unique_id);
    let executor = start_with_scan_budget(
        deps,
        &context,
        ScanBudget {
            entries: NonZeroU64::new(BUDGET).unwrap(),
            time: Duration::from_secs(3600),
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let mut expected = BTreeSet::new();
    for i in 0..AGENTS {
        let name = if i == 3 || i == 9 {
            format!("scan-budget-match-{i}")
        } else {
            format!("scan-budget-other-{i}")
        };
        let worker_id = executor
            .start_agent(&component.id, agent_id!("GolemHostApi", name.clone()))
            .await?;
        if name.contains("match") {
            expected.insert(worker_id.agent_id);
        }
    }
    let filter = AgentFilter::new_name(StringFilterComparator::Like, "scan-budget-match".into());

    let (first_cursor, first_agents) = executor
        .get_workers_metadata(
            &component.id,
            Some(filter.clone()),
            ScanCursor::default(),
            i64::MAX as u64,
            false,
        )
        .await?;
    let mut cursor = first_cursor.expect("a listing that the scan budget stops returns a cursor");
    let mut found = first_agents
        .into_iter()
        .map(|agent| agent.agent_id.agent_id)
        .collect::<BTreeSet<_>>();
    let mut calls = 1;
    loop {
        let (next, agents) = executor
            .get_workers_metadata(
                &component.id,
                Some(filter.clone()),
                cursor,
                i64::MAX as u64,
                false,
            )
            .await?;
        calls += 1;
        found.extend(agents.into_iter().map(|agent| agent.agent_id.agent_id));
        match next {
            Some(next) => cursor = next,
            None => break,
        }
    }
    assert_eq!(found, expected);
    assert!(
        calls >= AGENTS.div_ceil(BUDGET),
        "{calls} calls listed {AGENTS} agents with a budget of {BUDGET}"
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_cursor_past_the_end_of_the_oplog_is_refused_as_stale(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, Default::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let worker_id = executor
        .start_agent(&component.id, agent_id!("GolemHostApi", "stale-cursor"))
        .await?;
    let last_index = executor.get_oplog_last_index(&worker_id).await?;
    let cursor_at = |next_oplog_index| OplogCursor {
        next_oplog_index,
        current_component_revision: 0,
    };
    let get = |cursor| {
        let mut client = executor.client.clone();
        let request = GetOplogRequest {
            agent_id: Some(worker_id.clone().into()),
            environment_id: Some(executor.context.default_environment_id.into()),
            from_oplog_index: 1,
            cursor: Some(cursor),
            count: 100,
            auth_ctx: Some(executor.auth_ctx().into()),
        };
        async move { client.get_oplog(request).await }
    };
    // Far past the end, so a late entry of the new agent cannot make the cursor valid.
    let past_end = last_index + 1000;
    let is_stale = |failure: &WorkerExecutionError| {
        matches!(
            &failure.error,
            Some(worker_execution_error::Error::StaleCursor(StaleCursor {
                agent_id: Some(agent_id),
                next_oplog_index,
                last_oplog_index,
            })) if *agent_id == worker_id.clone().into()
                && *next_oplog_index == past_end
                && (last_index..past_end).contains(last_oplog_index)
        )
    };

    // A cursor at the last entry is valid and reads from there.
    let at_last = get(cursor_at(last_index)).await?.into_inner();
    let Some(get_oplog_response::Result::Success(page)) = at_last.result else {
        panic!("a cursor at the last entry must be accepted, got {at_last:?}");
    };
    assert!(!page.entries.is_empty());

    // A cursor past it did not come from a page of this oplog, for get and for search.
    let got = get(cursor_at(past_end)).await?.into_inner();
    assert!(
        matches!(&got.result, Some(get_oplog_response::Result::Failure(failure)) if is_stale(failure)),
        "expected a stale cursor failure, got {got:?}"
    );
    let searched = executor
        .client
        .clone()
        .search_oplog(SearchOplogRequest {
            agent_id: Some(worker_id.clone().into()),
            environment_id: Some(executor.context.default_environment_id.into()),
            cursor: Some(cursor_at(past_end)),
            count: 100,
            query: RARE_QUERY.to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
        })
        .await?
        .into_inner();
    assert!(
        matches!(&searched.result, Some(search_oplog_response::Result::Failure(failure)) if is_stale(failure)),
        "expected a stale cursor failure, got {searched:?}"
    );
    Ok(())
}

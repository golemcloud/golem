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
    StaleCursor, StaleCursorPastEnd, WorkerExecutionError, stale_cursor, worker_execution_error,
};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    GetOplogRequest, SearchOplogRequest, SearchOplogSuccessResponse, get_oplog_response,
    search_oplog_response,
};
use golem_common::base_model::worker_filter::StringFilterComparator;
use golem_common::config::RedisConfig;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{
    OplogCursor as CommonOplogCursor, OplogIndex, PublicOplogEntry, PublicOplogPageEntry,
};
use golem_common::model::{AgentFilter, AgentId, OwnedAgentId, PromiseId, ScanCursor};
use golem_common::schema::{FromSchema, SchemaValue};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::{IndexedStorageConfig, ScanBudget};
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
/// by one invocation whose entries match `RARE_QUERY`. Returns the component, the agent and its
/// last index.
async fn oplog_with_a_rare_match_at_the_end(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    host_api_tests: &PrecompiledComponent,
    name: &str,
    calls: u64,
) -> anyhow::Result<(ComponentDto, OwnedAgentId, u64)> {
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_key!("GolemHostApi", name.to_string());
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
            &worker_id.environment_id,
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
    Ok((component, worker_id, last_index))
}

async fn search_page(
    executor: &TestWorkerExecutor,
    worker_id: &OwnedAgentId,
    cursor: Option<OplogCursor>,
) -> anyhow::Result<SearchOplogSuccessResponse> {
    let response = executor
        .client
        .clone()
        .search_oplog(SearchOplogRequest {
            agent_id: Some(worker_id.agent_id.into()),
            environment_id: Some(worker_id.environment_id.into()),
            cursor,
            count: 100,
            query: RARE_QUERY.to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
            claims: golem_common::model::caller::ClaimVisibility::Hidden.to_protobuf(),
            from_oplog_index: None,
            to_oplog_index: None,
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
    let (_, worker_id, last_index) = oplog_with_a_rare_match_at_the_end(
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
    let (_, worker_id, last_index) = oplog_with_a_rare_match_at_the_end(
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

/// Each batch of a guest search appends entries to the oplog of the guest. A search of its own
/// oplog that read up to the current end on every batch would never reach it with a budget of one
/// entry, because the oplog grows faster than the search reads it.
#[test]
#[timeout("10m")]
async fn a_guest_search_of_its_own_oplog_ends_with_a_budget_of_one_entry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const NAME: &str = "scan-budget-own-search";
    const MAX_BATCHES: u64 = 10_000;
    let budget = ScanBudget {
        entries: NonZeroU64::MIN,
        time: Duration::from_secs(3600),
    };
    let context = TestContext::new(last_unique_id);
    let executor = start_with_scan_budget(deps, &context, budget).await?;
    let (component, worker_id, last_index) =
        oplog_with_a_rare_match_at_the_end(&executor, &context, host_api_tests, NAME, 100).await?;
    // More entries than the oplog buffers before it commits, so the search sees its own batches.
    assert!(last_index > 128);

    let search_to_end = async |executor: &TestWorkerExecutor| -> anyhow::Result<u64> {
        let batches = executor
            .invoke_and_await_agent(
                &component,
                &agent_key!("GolemHostApi", NAME.to_string()),
                "search_own_oplog_to_end",
                data_value!("message:__missing__", MAX_BATCHES),
            )
            .await?
            .into_typed::<Result<u64, String>>()?
            .map_err(anyhow::Error::msg)?;
        Ok(batches)
    };

    // A budget of one entry reads one entry per batch, so the search ends after about one batch
    // for each entry that the oplog had when the search started.
    let batches = search_to_end(&executor).await?;
    assert!(
        batches >= last_index,
        "the search ended after {batches} batches in an oplog of {last_index} entries"
    );

    // After a restart, the agent replays the first search from its oplog, then searches again.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_scan_budget(deps, &context, budget).await?;
    let replayed_last_index = executor.get_oplog_last_index(&worker_id).await?;
    let batches = search_to_end(&executor).await?;
    assert!(
        batches >= replayed_last_index,
        "the search after the restart ended after {batches} batches in an oplog of {replayed_last_index} entries"
    );
    Ok(())
}

/// A guest search that the executor reconstructs from the oplog keeps the end that its first batch
/// fixed, so it continues live without reading the entries that were added after that batch.
#[test]
#[timeout("10m")]
async fn a_reconstructed_guest_search_keeps_the_end_of_its_first_batch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const MAX_BATCHES: u64 = 10_000;
    let context = TestContext::new(last_unique_id);
    let executor = start_with_scan_budget(
        deps,
        &context,
        ScanBudget {
            entries: NonZeroU64::MIN,
            time: Duration::from_secs(3600),
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_key!("GolemHostApi", "scan-budget-kept-search");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    // The first batch reads one entry and fixes the end of the search.
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "open_kept_oplog_search",
            data_value!(RARE_QUERY),
        )
        .await?
        .into_typed::<Result<(), String>>()?
        .map_err(anyhow::Error::msg)?;
    let end_of_first_batch = executor.get_oplog_last_index(&worker_id).await?;

    // After a restart, the agent replays the first batch and adds entries that match the query.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_scan_budget(deps, &context, ScanBudget::default()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?;
    let added = executor
        .get_oplog(&worker_id, OplogIndex::from_u64(end_of_first_batch + 1))
        .await?
        .into_iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name.contains("generate_idempotency")))
        .map(|entry| entry.oplog_index.as_u64())
        .collect::<Vec<_>>();
    assert!(
        !added.is_empty(),
        "the agent added no matching entry after {end_of_first_batch}"
    );

    // The reconstructed search continues live and ends at the end of its first batch.
    let found = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "continue_kept_oplog_search",
            data_value!(MAX_BATCHES),
        )
        .await?
        .into_typed::<Result<Vec<u64>, String>>()?
        .map_err(anyhow::Error::msg)?;
    assert!(
        found.iter().all(|index| *index <= end_of_first_batch),
        "the search found {found:?} after the end {end_of_first_batch} of its first batch, where the agent added {added:?}"
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
            .start_agent(&component.id, agent_key!("GolemHostApi", name.clone()))
            .await?;
        if name.contains("match") {
            expected.insert(worker_id.agent_id);
        }
    }
    let filter = AgentFilter::new_key(StringFilterComparator::Like, "scan-budget-match".into());
    let agent_type = AgentTypeName("GolemHostApi".to_string());

    let (first_cursor, first_agents) = executor
        .get_workers_metadata(
            &component.environment_id,
            &agent_type,
            Some(filter.clone()),
            ScanCursor::default(),
            i64::MAX as u64,
            false,
        )
        .await?;
    let mut cursor = first_cursor.expect("a listing that the scan budget stops returns a cursor");
    let mut found = first_agents
        .into_iter()
        .map(|agent| agent.agent_id)
        .collect::<BTreeSet<_>>();
    let mut calls = 1;
    loop {
        let (next, agents) = executor
            .get_workers_metadata(
                &component.environment_id,
                &agent_type,
                Some(filter.clone()),
                cursor,
                i64::MAX as u64,
                false,
            )
            .await?;
        calls += 1;
        found.extend(agents.into_iter().map(|agent| agent.agent_id));
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

/// Redis `SCAN COUNT` is a hint: a scan returns the keys of whole hash buckets, so one storage
/// batch can hold more keys than the budget. A call keeps the whole batch, and the cursors skip no
/// agent and repeat none. That a call stops after the first batch that spends the budget is checked
/// with scripted batches in `agent_listing_charges_examined_keys_and_stops_after_the_batch_that_spends_the_budget`.
#[test]
#[timeout("10m")]
async fn agent_list_on_redis_stops_after_a_batch_larger_than_the_scan_budget(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const AGENTS: usize = 100;
    // A database of its own holds only the oplogs of this test, so that the agents share hash
    // buckets and Redis returns batches of more than one agent. It is emptied first, because a
    // Redis that outlives a test run keeps the oplogs of earlier runs.
    const DATABASE: u16 = 1;
    deps.redis.flush_db(DATABASE);
    let context = TestContext::new(last_unique_id);
    let indexed_storage = RedisConfig {
        host: deps.redis.public_host(),
        port: deps.redis.public_port(),
        database: DATABASE.into(),
        key_prefix: context.redis_prefix(),
        ..Default::default()
    };
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.indexed_storage = IndexedStorageConfig::Redis(indexed_storage.clone());
                config.limits.scan_budget = ScanBudget {
                    entries: NonZeroU64::MIN,
                    time: Duration::from_secs(3600),
                };
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let mut expected = BTreeSet::new();
    for i in 0..AGENTS {
        let worker_id = executor
            .start_agent(
                &component.id,
                agent_key!("GolemHostApi", format!("redis-scan-budget-{i}")),
            )
            .await?;
        expected.insert(worker_id.agent_id);
    }

    let mut pages: Vec<Vec<AgentId>> = Vec::new();
    let mut cursor = ScanCursor::default();
    loop {
        let (next, agents) = executor
            .get_workers_metadata(
                &component.environment_id,
                &AgentTypeName("GolemHostApi".to_string()),
                None,
                cursor,
                i64::MAX as u64,
                false,
            )
            .await?;
        pages.push(agents.into_iter().map(|agent| agent.agent_id).collect());
        match next {
            Some(next) => cursor = next,
            None => break,
        }
    }

    let listed = pages.iter().flatten().cloned().collect::<Vec<_>>();
    let unique = listed.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(listed.len(), unique.len(), "an agent was listed twice");
    assert_eq!(unique, expected);
    // A batch without keys costs nothing, so a call stops only after a batch with keys, and keeps
    // all the agents of that batch.
    let (last, stopped) = pages.split_last().unwrap();
    let sizes = stopped.iter().map(Vec::len).collect::<Vec<_>>();
    assert!(
        !stopped.is_empty() && sizes.iter().all(|size| *size > 0),
        "every call that the budget stopped must list one batch, got pages of {sizes:?} and a last page of {}",
        last.len()
    );
    assert!(
        sizes.iter().any(|size| *size > 1),
        "no batch held more than one agent, got pages of {sizes:?}"
    );
    Ok(())
}

async fn get_oplog(
    executor: &TestWorkerExecutor,
    worker_id: &OwnedAgentId,
    cursor: Option<OplogCursor>,
    count: u64,
) -> anyhow::Result<get_oplog_response::Result> {
    executor
        .client
        .clone()
        .get_oplog(GetOplogRequest {
            agent_id: Some(worker_id.agent_id.into()),
            environment_id: Some(worker_id.environment_id.into()),
            from_oplog_index: 1,
            cursor,
            count,
            auth_ctx: Some(executor.auth_ctx().into()),
            claims: golem_common::model::caller::ClaimVisibility::Hidden.to_protobuf(),
            to_oplog_index: None,
            scan_allowance: None,
        })
        .await?
        .into_inner()
        .result
        .ok_or_else(|| anyhow::anyhow!("the oplog read has no result"))
}

async fn search_oplog(
    executor: &TestWorkerExecutor,
    worker_id: &OwnedAgentId,
    cursor: Option<OplogCursor>,
    query: &str,
    count: u64,
) -> anyhow::Result<search_oplog_response::Result> {
    executor
        .client
        .clone()
        .search_oplog(SearchOplogRequest {
            agent_id: Some(worker_id.agent_id.into()),
            environment_id: Some(worker_id.environment_id.into()),
            cursor,
            count,
            query: query.to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
            claims: golem_common::model::caller::ClaimVisibility::Hidden.to_protobuf(),
            from_oplog_index: None,
            to_oplog_index: None,
        })
        .await?
        .into_inner()
        .result
        .ok_or_else(|| anyhow::anyhow!("the oplog search has no result"))
}

/// The reason of a stale cursor failure for `worker_id` and the cursor index `next`.
fn stale_reason(
    failure: &WorkerExecutionError,
    worker_id: &OwnedAgentId,
    next: u64,
) -> Option<stale_cursor::Reason> {
    match &failure.error {
        Some(worker_execution_error::Error::StaleCursor(StaleCursor {
            agent_id: Some(agent_id),
            next_oplog_index,
            reason,
        })) if *agent_id == worker_id.agent_id.into() && *next_oplog_index == next => *reason,
        _ => None,
    }
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
        .start_agent(&component.id, agent_key!("GolemHostApi", "stale-cursor"))
        .await?;
    let last_index = executor.get_oplog_last_index(&worker_id).await?;
    let cursor_at = |next_oplog_index, query| OplogCursor {
        next_oplog_index,
        current_component_revision: 0,
        query_hash: CommonOplogCursor::query_hash(query),
        scan_to: None,
    };
    // Far past the end, so a late entry of the new agent cannot make the cursor valid.
    let past_end = last_index + 1000;
    let is_past_end = |failure: &WorkerExecutionError| {
        matches!(
            stale_reason(failure, &worker_id, past_end),
            Some(stale_cursor::Reason::PastEnd(StaleCursorPastEnd { last_oplog_index }))
                if (last_index..past_end).contains(&last_oplog_index)
        )
    };

    // A cursor at the last entry is valid and reads from there.
    let at_last = get_oplog(
        &executor,
        &worker_id,
        Some(cursor_at(last_index, None)),
        100,
    )
    .await?;
    let get_oplog_response::Result::Success(page) = at_last else {
        panic!("a cursor at the last entry must be accepted, got {at_last:?}");
    };
    assert!(!page.entries.is_empty());

    // A cursor past it did not come from a page of this oplog, for get and for search.
    let got = get_oplog(&executor, &worker_id, Some(cursor_at(past_end, None)), 100).await?;
    assert!(
        matches!(&got, get_oplog_response::Result::Failure(failure) if is_past_end(failure)),
        "expected a stale cursor past the end, got {got:?}"
    );
    let searched = search_oplog(
        &executor,
        &worker_id,
        Some(cursor_at(past_end, Some(RARE_QUERY))),
        RARE_QUERY,
        100,
    )
    .await?;
    assert!(
        matches!(&searched, search_oplog_response::Result::Failure(failure) if is_past_end(failure)),
        "expected a stale cursor past the end, got {searched:?}"
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_cursor_of_another_query_is_refused_as_stale(
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
    let agent_id = agent_key!("GolemHostApi", "query-cursor");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?;

    // The query matches more than one entry, so a page of one match hands out a cursor.
    let search_cursor = match search_oplog(&executor, &worker_id, None, RARE_QUERY, 1).await? {
        search_oplog_response::Result::Success(page) => page
            .next
            .expect("a page of the first of several matches has a cursor"),
        other => panic!("the first search page failed: {other:?}"),
    };
    let get_cursor = match get_oplog(&executor, &worker_id, None, 1).await? {
        get_oplog_response::Result::Success(page) => {
            page.next.expect("a page of the first entry has a cursor")
        }
        other => panic!("the first oplog page failed: {other:?}"),
    };

    // The same query continues the search.
    let same = search_oplog(
        &executor,
        &worker_id,
        Some(search_cursor.clone()),
        RARE_QUERY,
        100,
    )
    .await?;
    let search_oplog_response::Result::Success(page) = &same else {
        panic!("the same query must continue its cursor, got {same:?}");
    };
    assert!(!page.entries.is_empty());

    // Another query, a read without a query, and a search of a cursor without a query refuse it.
    let is_query_mismatch = |failure: &WorkerExecutionError, cursor: &OplogCursor| {
        matches!(
            stale_reason(failure, &worker_id, cursor.next_oplog_index),
            Some(stale_cursor::Reason::QueryMismatch(_))
        )
    };
    let other_query = search_oplog(
        &executor,
        &worker_id,
        Some(search_cursor.clone()),
        "generate_idempotency_keys",
        100,
    )
    .await?;
    assert!(
        matches!(&other_query, search_oplog_response::Result::Failure(failure) if is_query_mismatch(failure, &search_cursor)),
        "a search of another query must refuse the cursor, got {other_query:?}"
    );
    let get = get_oplog(&executor, &worker_id, Some(search_cursor.clone()), 100).await?;
    assert!(
        matches!(&get, get_oplog_response::Result::Failure(failure) if is_query_mismatch(failure, &search_cursor)),
        "a read without a query must refuse the cursor of a search, got {get:?}"
    );
    let search_of_get = search_oplog(
        &executor,
        &worker_id,
        Some(get_cursor.clone()),
        RARE_QUERY,
        100,
    )
    .await?;
    assert!(
        matches!(&search_of_get, search_oplog_response::Result::Failure(failure) if is_query_mismatch(failure, &get_cursor)),
        "a search must refuse the cursor of a read without a query, got {search_of_get:?}"
    );

    // The query check comes before the executor resolves the agent, so it reads no storage. For an
    // agent that was never created, the resolution fails: a cursor of the same query fails with
    // it, and a cursor of another query is refused as stale first.
    let missing_id = crate::agent::keyed_agent_id(
        worker_id.environment_id,
        &agent_key!("GolemHostApi", "never-created"),
    );
    let missing_same_query = search_oplog(
        &executor,
        &missing_id,
        Some(search_cursor.clone()),
        RARE_QUERY,
        100,
    )
    .await?;
    assert!(
        matches!(&missing_same_query, search_oplog_response::Result::Failure(failure)
            if stale_reason(failure, &missing_id, search_cursor.next_oplog_index).is_none()),
        "a search of a missing agent must fail to resolve it, got {missing_same_query:?}"
    );
    let missing_other_query = search_oplog(
        &executor,
        &missing_id,
        Some(search_cursor.clone()),
        "generate_idempotency_keys",
        100,
    )
    .await?;
    assert!(
        matches!(&missing_other_query, search_oplog_response::Result::Failure(failure)
            if matches!(stale_reason(failure, &missing_id, search_cursor.next_oplog_index), Some(stale_cursor::Reason::QueryMismatch(_)))),
        "a search must refuse the cursor of another query before it resolves the agent, got {missing_other_query:?}"
    );
    let missing_get = get_oplog(&executor, &missing_id, Some(search_cursor.clone()), 100).await?;
    assert!(
        matches!(&missing_get, get_oplog_response::Result::Failure(failure)
            if matches!(stale_reason(failure, &missing_id, search_cursor.next_oplog_index), Some(stale_cursor::Reason::QueryMismatch(_)))),
        "a read must refuse the cursor of a search before it resolves the agent, got {missing_get:?}"
    );
    Ok(())
}

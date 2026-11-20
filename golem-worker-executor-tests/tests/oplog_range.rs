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

//! Oplog reads of a range, `from` to `to`, with and without a query: across the oplog layers,
//! across pages, past the scan budget of a search page, and with entries that pages omit.

use crate::Tracing;

use futures::{StreamExt, TryStreamExt};
use golem_api_grpc::proto::golem::worker::OplogCursor;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    GetOplogRequest, GetOplogSuccessResponse, SearchOplogRequest, get_oplog_response,
    search_oplog_response,
};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{
    OplogIndex, OplogRange, PublicOplogEntry, PublicOplogEntryWithIndex, PublicOplogPageEntry,
};
use golem_common::model::{AgentId, OwnedAgentId, ShardAssignment, ShardId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::{OplogConfig, ScanBudget};
use golem_worker_executor::storage::scheduler::SchedulerStorage;
use golem_worker_executor::storage::scheduler::sqlite::SqliteSchedulerStorage;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, agent_oplog_length, archived_oplog_chunks,
    blob_archived_oplog_length, scheduler_sqlite_storage_config, start_with_oplog_config,
    start_with_overrides, wait_until, wait_until_archived,
};
use pretty_assertions::assert_eq;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

fn indices(entries: &[PublicOplogEntryWithIndex]) -> Vec<u64> {
    entries
        .iter()
        .map(|entry| entry.oplog_index.as_u64())
        .collect()
}

fn page_entries(page: GetOplogSuccessResponse) -> Vec<PublicOplogPageEntry> {
    page.entries
        .into_iter()
        .map(|entry| PublicOplogPageEntry::try_from(entry).unwrap())
        .collect()
}

fn page_entry_index(entry: &PublicOplogPageEntry) -> u64 {
    match entry {
        PublicOplogPageEntry::Included(entry) => entry.oplog_index.as_u64(),
        PublicOplogPageEntry::Omitted(omitted) => omitted.oplog_index.as_u64(),
    }
}

async fn get_page(
    executor: &TestWorkerExecutor,
    agent_id: &AgentId,
    from: u64,
    to: Option<u64>,
    cursor: Option<OplogCursor>,
    count: u64,
) -> anyhow::Result<get_oplog_response::Result> {
    executor
        .client
        .clone()
        .get_oplog(GetOplogRequest {
            agent_id: Some(agent_id.clone().into()),
            environment_id: Some(executor.context.default_environment_id.into()),
            from_oplog_index: Some(from),
            to_oplog_index: to,
            cursor,
            count,
            auth_ctx: Some(executor.auth_ctx().into()),
        })
        .await?
        .into_inner()
        .result
        .ok_or_else(|| anyhow::anyhow!("the oplog read has no result"))
}

/// Reads `from..=to` in pages of `count` entries, following the cursors, and gives the entries
/// of every page in the order of the pages.
async fn read_pages(
    executor: &TestWorkerExecutor,
    agent_id: &AgentId,
    from: u64,
    to: u64,
    count: u64,
) -> anyhow::Result<Vec<PublicOplogPageEntry>> {
    let pages = futures::stream::try_unfold(Some(None), |cursor| async move {
        let Some(cursor) = cursor else {
            return Ok(None);
        };
        let get_oplog_response::Result::Success(page) =
            get_page(executor, agent_id, from, Some(to), cursor, count).await?
        else {
            anyhow::bail!("the oplog read of {from}..={to} failed");
        };
        let next = page.next.clone();
        Ok(Some((page_entries(page), next.map(Some))))
    });
    pages.try_concat().await
}

fn range(from: u64, to: u64) -> OplogRange {
    OplogRange::new(
        Some(OplogIndex::from_u64(from)),
        Some(OplogIndex::from_u64(to)),
    )
    .unwrap()
}

async fn counter(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    agent_counters: &PrecompiledComponent,
    name: &str,
) -> anyhow::Result<(
    ComponentDto,
    golem_common::model::agent::ParsedAgentKey,
    AgentId,
)> {
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_id!("Counter", name.to_string());
    let agent_id = executor.start_agent(&component.id, agent.clone()).await?;
    Ok((component, agent, agent_id))
}

async fn increment(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &golem_common::model::agent::ParsedAgentKey,
    times: usize,
) -> anyhow::Result<()> {
    futures::stream::iter(0..times)
        .map(Ok)
        .try_for_each(|_| async {
            executor
                .invoke_and_await_agent(component, agent, "increment", data_value!())
                .await
                .map(|_| ())
        })
        .await
}

/// Waits until the primary oplog of the agent is empty and no archive action is due or leased.
async fn wait_until_archived_and_idle(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    owned_agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    wait_until_archived(deps, context, owned_agent_id).await?;
    let scheduler =
        SqliteSchedulerStorage::configured(&scheduler_sqlite_storage_config(deps, context))
            .await
            .map_err(anyhow::Error::msg)?;
    let scheduler = &scheduler;
    let assignment = &ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    wait_until(
        Duration::from_secs(60),
        "no scheduled action is due",
        || async move {
            Ok(scheduler
                .count_due(chrono::DateTime::<chrono::Utc>::MAX_UTC, assignment)
                .await?
                == 0)
        },
    )
    .await
}

/// The history of the agent moves through the compressed layer to the blob layer: when the primary
/// oplog of an idle agent is empty, the archive moves the whole compressed layer to the blob
/// layer, so the compressed layer is empty when the agent is archived. After a real restart with
/// an executor that does not archive, the later entries stay in the primary oplog. Every range
/// read, across the blob layer, the primary oplog and the pages, returns each index of the range
/// once, in order, with the entries of a full read.
#[test]
#[timeout("4m")]
async fn range_reads_across_the_oplog_layers_return_every_index_once_in_order(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let archiving = start_with_oplog_config(
        deps,
        &context,
        Some(OplogConfig {
            indexed_storage_layers: 2,
            blob_storage_layers: 1,
            archive_interval: Duration::from_secs(1),
            entry_count_limit: 4,
            ..Default::default()
        }),
    )
    .await?;
    let (component, agent, agent_id) =
        counter(&archiving, &context, agent_counters, "range-layers").await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);
    increment(&archiving, &component, &agent, 6).await?;
    wait_until_archived_and_idle(deps, &context, &owned_agent_id).await?;
    let archived_last = archiving.get_oplog_last_index(&agent_id).await?;
    let compressed = archived_oplog_chunks(deps, &context, &owned_agent_id, 1).await?;
    let blob = blob_archived_oplog_length(deps, &context, &owned_agent_id, 0).await?;
    anyhow::ensure!(
        compressed.is_empty() && blob > 0,
        "expected the history in the blob layer only, got {} compressed chunks and a blob layer \
         of length {blob}",
        compressed.len()
    );
    archiving.shutdown_and_wait_for_invocation_loops().await?;
    drop(archiving);

    let executor = start_with_oplog_config(
        deps,
        &context,
        Some(OplogConfig {
            archive_interval: Duration::from_secs(3600),
            ..Default::default()
        }),
    )
    .await?;
    increment(&executor, &component, &agent, 4).await?;
    let last = executor.get_oplog_last_index(&agent_id).await?;
    let primary = agent_oplog_length(deps, &context, &owned_agent_id).await?;
    anyhow::ensure!(
        primary > 0 && primary <= last - archived_last,
        "expected only the entries after the archive in the primary oplog, got {primary} of {last}"
    );

    let full = executor
        .read_oplog(&agent_id, OplogRange::ALL, None)
        .await?;
    assert_eq!(indices(&full), (1..=last).collect::<Vec<_>>());

    // Windows of 7 that start at every third index cross the boundary between the archive and
    // the primary oplog, and pages of 3 entries cross it again.
    let (executor, agent_id, full) = (&executor, &agent_id, &full);
    futures::stream::iter((1..=last).step_by(3))
        .map(Ok)
        .try_for_each(|from| async move {
            let to = (from + 6).min(last);
            let paged = read_pages(executor, agent_id, from, to, 3).await?;
            assert_eq!(
                paged.iter().map(page_entry_index).collect::<Vec<_>>(),
                (from..=to).collect::<Vec<_>>(),
                "pages of {from}..={to}"
            );
            let read = executor.read_oplog(agent_id, range(from, to), None).await?;
            assert_eq!(
                read,
                full[(from - 1) as usize..to as usize].to_vec(),
                "entries of {from}..={to}"
            );
            anyhow::Ok(())
        })
        .await?;

    // A `to` after the end ends the read at the end, and a `to` before `from` is refused.
    let past_the_end = executor
        .read_oplog(agent_id, range(last - 1, last + 100), None)
        .await?;
    assert_eq!(indices(&past_the_end), vec![last - 1, last]);
    let refused = get_page(executor, agent_id, 5, Some(4), None, 10).await?;
    assert!(
        matches!(refused, get_oplog_response::Result::Failure(_)),
        "a range that ends before it starts must be refused, got {refused:?}"
    );
    Ok(())
}

/// A search within a range returns the matching entries of the range: the matches before and
/// after the range are left out.
#[test]
#[timeout("4m")]
async fn a_search_within_a_range_returns_only_the_matching_entries_in_the_range(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let (component, agent, agent_id) =
        counter(&executor, &context, agent_counters, "range-search").await?;
    increment(&executor, &component, &agent, 6).await?;

    let full = executor
        .read_oplog(&agent_id, OplogRange::ALL, None)
        .await?;
    let started = full
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .map(|entry| entry.oplog_index.as_u64())
        .collect::<Vec<_>>();
    assert!(started.len() >= 6, "{started:?}");
    // From just after the second invocation start to the fifth one.
    let (from, to) = (started[1] + 1, started[4]);

    const QUERY: &str = "agent-invocation-started";
    let all_matches = indices(&executor.search_oplog(&agent_id, QUERY).await?);
    assert_eq!(all_matches, started);
    let in_range = executor
        .read_oplog(&agent_id, range(from, to), Some(QUERY))
        .await?;
    assert_eq!(indices(&in_range), started[2..=4].to_vec());
    assert_eq!(
        in_range,
        full.iter()
            .filter(|entry| started[2..=4].contains(&entry.oplog_index.as_u64()))
            .cloned()
            .collect::<Vec<_>>()
    );

    let search_page = executor
        .client
        .clone()
        .search_oplog(SearchOplogRequest {
            agent_id: Some(agent_id.clone().into()),
            environment_id: Some(executor.context.default_environment_id.into()),
            cursor: None,
            count: 100,
            query: QUERY.to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
            from_oplog_index: Some(from),
            to_oplog_index: Some(to),
        })
        .await?
        .into_inner()
        .result;
    let Some(search_oplog_response::Result::Success(page)) = search_page else {
        anyhow::bail!("the search of {from}..={to} failed: {search_page:?}");
    };
    assert_eq!(
        page.next, None,
        "a search page ends at the end of its range"
    );
    assert_eq!(page.last_index, to);
    Ok(())
}

/// The scan budget ends a search page, not a read that follows the cursors: with a budget far
/// below the length of the oplog, one search page stops at the budget, and the read of the same
/// range and query returns every match.
#[test]
#[timeout("4m")]
async fn a_read_that_follows_the_cursors_is_not_limited_by_the_scan_budget(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const BUDGET: u64 = 4;
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.limits.scan_budget = ScanBudget {
                    entries: NonZeroU64::new(BUDGET).unwrap(),
                    time: Duration::from_secs(3600),
                };
            })),
            ..Default::default()
        },
    )
    .await?;
    let (component, agent, agent_id) =
        counter(&executor, &context, agent_counters, "range-budget").await?;
    increment(&executor, &component, &agent, 6).await?;
    let last = executor.get_oplog_last_index(&agent_id).await?;
    assert!(last > 4 * BUDGET, "the oplog has only {last} entries");

    const QUERY: &str = "agent-invocation-started";
    let page = executor
        .client
        .clone()
        .search_oplog(SearchOplogRequest {
            agent_id: Some(agent_id.clone().into()),
            environment_id: Some(executor.context.default_environment_id.into()),
            cursor: None,
            count: 100,
            query: QUERY.to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
            from_oplog_index: Some(1),
            to_oplog_index: Some(last),
        })
        .await?
        .into_inner()
        .result;
    let Some(search_oplog_response::Result::Success(page)) = page else {
        anyhow::bail!("the search failed: {page:?}");
    };
    assert_eq!(
        page.next.map(|next| next.next_oplog_index),
        Some(1 + BUDGET),
        "one search page reads only the budget"
    );

    let full = executor
        .read_oplog(&agent_id, OplogRange::ALL, None)
        .await?;
    assert_eq!(indices(&full), (1..=last).collect::<Vec<_>>());
    let started = full
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .map(|entry| entry.oplog_index.as_u64())
        .collect::<Vec<_>>();
    let found = executor
        .read_oplog(&agent_id, range(1, last), Some(QUERY))
        .await?;
    assert_eq!(indices(&found), started);
    Ok(())
}

/// A range page omits an entry that is larger than the page entry limit, but keeps its index in
/// the range, and the read of the range returns the entry in full.
#[test]
#[timeout("4m")]
async fn a_range_read_returns_the_entries_that_its_pages_omit_in_full(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let reference = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let (component, agent, agent_id) =
        counter(&reference, &context, agent_counters, "range-omitted").await?;
    increment(&reference, &component, &agent, 3).await?;
    let full = reference
        .read_oplog(&agent_id, OplogRange::ALL, None)
        .await?;
    reference.shutdown_and_wait_for_invocation_loops().await?;
    drop(reference);

    // An entry limit below the size of every entry, so the pages omit every entry.
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.limits.max_inline_oplog_entry_size = 1;
            })),
            ..Default::default()
        },
    )
    .await?;
    let last = full.len() as u64;
    let (from, to) = (2, last - 1);
    let paged = read_pages(&executor, &agent_id, from, to, 3).await?;
    assert_eq!(
        paged.iter().map(page_entry_index).collect::<Vec<_>>(),
        (from..=to).collect::<Vec<_>>()
    );
    assert!(
        paged
            .iter()
            .all(|entry| matches!(entry, PublicOplogPageEntry::Omitted(_))),
        "the pages must omit every entry above the limit"
    );
    let read = executor
        .read_oplog(&agent_id, range(from, to), None)
        .await?;
    assert_eq!(read, full[(from - 1) as usize..to as usize].to_vec());
    Ok(())
}

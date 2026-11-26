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

//! Oplog reads of a range, `from` to `to`, with and without a query. The reads cross the oplog
//! layers and the pages. They go past the scan budget of a search page. They read entries that
//! pages omit.

use crate::Tracing;

use futures::{StreamExt, TryStreamExt};
use golem_api_grpc::proto::golem::worker::OplogCursor;
use golem_api_grpc::proto::golem::worker::v1::{WorkerExecutionError, worker_execution_error};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    GetOplogRequest, GetOplogSuccessResponse, SearchOplogRequest, SearchOplogSuccessResponse,
    get_oplog_response, search_oplog_response,
};
use golem_common::model::caller::ClaimVisibility;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{
    OplogIndex, OplogRange, PublicOplogEntry, PublicOplogEntryWithIndex, PublicOplogPageEntry,
};
use golem_common::model::{OwnedAgentId, ShardAssignment, ShardId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::{OplogConfig, OplogSweepConfig, ScanBudget};
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

fn search_page_entries(page: SearchOplogSuccessResponse) -> Vec<PublicOplogPageEntry> {
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

fn is_invalid_request(failure: &WorkerExecutionError) -> bool {
    matches!(
        failure.error,
        Some(worker_execution_error::Error::InvalidRequest(_))
    )
}

async fn get_page(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
    from: u64,
    to: Option<u64>,
    cursor: Option<OplogCursor>,
    count: u64,
) -> anyhow::Result<get_oplog_response::Result> {
    executor
        .client
        .clone()
        .get_oplog(GetOplogRequest {
            agent_id: Some(agent_id.agent_id.into()),
            environment_id: Some(agent_id.environment_id.into()),
            from_oplog_index: Some(from),
            to_oplog_index: to,
            scan_allowance: None,
            cursor,
            count,
            auth_ctx: Some(executor.auth_ctx().into()),
            claims: ClaimVisibility::Hidden.to_protobuf(),
        })
        .await?
        .into_inner()
        .result
        .ok_or_else(|| anyhow::anyhow!("the oplog read has no result"))
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
    OwnedAgentId,
)> {
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_key!("Counter", name.to_string());
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
                .count_due(jiff::Timestamp::MAX, assignment)
                .await?
                == 0)
        },
    )
    .await
}

/// Reads `from` to `to` in pages of `count` entries, following the cursors, and gives the
/// entries of every page in the order of the pages. Without `to`, the read goes to the end of the
/// oplog, and the pages are the pages of a read without an end.
async fn read_pages_to(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
    from: u64,
    to: Option<u64>,
    count: u64,
) -> anyhow::Result<Vec<PublicOplogPageEntry>> {
    let pages = futures::stream::try_unfold(PageRead::First, |read| async move {
        let cursor = match read {
            PageRead::First => None,
            PageRead::Next(cursor) => Some(cursor),
            PageRead::Done => return Ok(None),
        };
        let get_oplog_response::Result::Success(page) =
            get_page(executor, agent_id, from, to, cursor, count).await?
        else {
            anyhow::bail!("the oplog read from {from} to {to:?} failed");
        };
        let next = PageRead::after(page.next.clone());
        Ok(Some((page_entries(page), next)))
    });
    pages.try_concat().await
}

/// The read of the pages of a range. It reads the first page, then each later page from the cursor
/// of the page before it. It ends after a page without a cursor.
enum PageRead {
    First,
    Next(OplogCursor),
    Done,
}

impl PageRead {
    /// The read after a page whose cursor is `next`.
    fn after(next: Option<OplogCursor>) -> Self {
        next.map_or(Self::Done, Self::Next)
    }
}

/// The count of entries after which the primary oplog of the second executor moves its entries to
/// the compressed layer. The compressed layer moves its chunks to the blob layer only after this
/// many chunks. So the second executor never moves entries to the blob layer.
const TRANSFER_AFTER: u64 = 32;

/// The oplog of the agent is in three layers at once. The blob layer holds the oldest entries. The
/// compressed layer holds the next entries. The primary oplog holds the newest entries. The first
/// executor archives the whole history to the blob layer. After a real restart, the second executor
/// does not archive idle agents. It moves the primary oplog to the compressed layer once, after
/// `TRANSFER_AFTER` entries. Every range read, with and without an end, returns each index of the
/// range once, in order, with the entries of a full read. Its pages end and start exactly at each
/// layer boundary.
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
    increment(&archiving, &component, &agent, 6).await?;
    wait_until_archived_and_idle(deps, &context, &agent_id).await?;
    let blob_last = archiving.get_oplog_last_index(&agent_id).await?;
    anyhow::ensure!(
        archived_oplog_chunks(deps, &context, &agent_id, 1)
            .await?
            .is_empty()
            && blob_archived_oplog_length(deps, &context, &agent_id, 0).await? > 0,
        "expected the history from 1 to {blob_last} in the blob layer only"
    );
    archiving.shutdown_and_wait_for_invocation_loops().await?;
    drop(archiving);

    let executor = start_with_oplog_config(
        deps,
        &context,
        Some(OplogConfig {
            indexed_storage_layers: 2,
            blob_storage_layers: 1,
            archive_interval: Duration::from_secs(3600),
            entry_count_limit: TRANSFER_AFTER,
            sweep: OplogSweepConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        }),
    )
    .await?;
    let (executor, agent_id) = (&executor, &agent_id);
    // Increments until the oplog has `TRANSFER_AFTER` entries after the blob layer, so that the
    // primary oplog moves its entries to the compressed layer.
    let (component, agent) = (&component, &agent);
    let transferred = futures::stream::iter(0..4 * TRANSFER_AFTER)
        .then(|_| async move {
            increment(executor, component, agent, 1).await?;
            executor.get_oplog_last_index(agent_id).await
        })
        .try_skip_while(|last| futures::future::ready(Ok(*last < blob_last + TRANSFER_AFTER)))
        .boxed()
        .try_next()
        .await?;
    anyhow::ensure!(
        transferred.is_some(),
        "the oplog never reached {} entries",
        blob_last + TRANSFER_AFTER
    );
    // The transfer runs in the background, and the primary oplog then drops what it moved.
    let compressed_last = || async {
        Ok::<_, anyhow::Error>(
            archived_oplog_chunks(deps, &context, agent_id, 1)
                .await?
                .iter()
                .map(|(last, _)| *last)
                .max(),
        )
    };
    wait_until(
        Duration::from_secs(60),
        "the primary oplog moved its entries to the compressed layer",
        || async {
            let last = executor.get_oplog_last_index(agent_id).await?;
            Ok(match compressed_last().await? {
                Some(compressed) => {
                    agent_oplog_length(deps, &context, agent_id).await? == last - compressed
                }
                None => false,
            })
        },
    )
    .await?;
    // Two more invocations add fewer than `TRANSFER_AFTER` entries, which stay in the primary
    // oplog.
    increment(executor, component, agent, 2).await?;
    let last = executor.get_oplog_last_index(agent_id).await?;
    let compressed_last = compressed_last()
        .await?
        .expect("the compressed layer has entries");
    let primary = agent_oplog_length(deps, &context, agent_id).await?;
    anyhow::ensure!(
        blob_last < compressed_last
            && compressed_last < last
            && primary == last - compressed_last
            && blob_archived_oplog_length(deps, &context, agent_id, 0).await? > 0,
        "expected 1 to {blob_last} in the blob layer, {} to {compressed_last} in the compressed \
         layer and the {primary} entries to {last} in the primary oplog",
        blob_last + 1
    );

    let full = executor.read_oplog(agent_id, OplogRange::ALL, None).await?;
    assert_eq!(indices(&full), (1..=last).collect::<Vec<_>>());

    // At each layer boundary, pages of `count` entries from `boundary + 1 - count` end exactly at
    // the boundary. The next page starts just after it. With an end, the range ends `count` entries
    // after the boundary. Without an end, the pages go on to the end of the oplog.
    let full = &full;
    let reads = [blob_last, compressed_last]
        .into_iter()
        .flat_map(|boundary| (1..=3).map(move |count| (boundary, count)));
    futures::stream::iter(reads)
        .map(Ok)
        .try_for_each(|(boundary, count)| async move {
            let from = boundary + 1 - count;
            let to = (boundary + count).min(last);
            let paged = read_pages_to(executor, agent_id, from, Some(to), count).await?;
            assert_eq!(
                paged.iter().map(page_entry_index).collect::<Vec<_>>(),
                (from..=to).collect::<Vec<_>>(),
                "pages of {count} from {from} to {to}"
            );
            let open = read_pages_to(executor, agent_id, from, None, count).await?;
            assert_eq!(
                open.iter().map(page_entry_index).collect::<Vec<_>>(),
                (from..=last).collect::<Vec<_>>(),
                "pages of {count} from {from} without an end"
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

    // A `to` after the end ends the read at the end. The executor refuses a `to` before `from`.
    let past_the_end = executor
        .read_oplog(agent_id, range(last - 1, last + 100), None)
        .await?;
    assert_eq!(indices(&past_the_end), vec![last - 1, last]);
    let refused = get_page(executor, agent_id, 5, Some(4), None, 10).await?;
    assert!(
        matches!(
            &refused,
            get_oplog_response::Result::Failure(failure) if is_invalid_request(failure)
        ),
        "a range that ends before it starts is an invalid request, got {refused:?}"
    );
    let refused_search = executor
        .client
        .clone()
        .search_oplog(SearchOplogRequest {
            agent_id: Some(agent_id.agent_id.into()),
            environment_id: Some(agent_id.environment_id.into()),
            cursor: None,
            count: 10,
            query: "agent-invocation-started".to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
            claims: ClaimVisibility::Hidden.to_protobuf(),
            from_oplog_index: Some(5),
            to_oplog_index: Some(4),
        })
        .await?
        .into_inner()
        .result;
    assert!(
        matches!(
            &refused_search,
            Some(search_oplog_response::Result::Failure(failure)) if is_invalid_request(failure)
        ),
        "a search of a range that ends before it starts is an invalid request, got \
         {refused_search:?}"
    );
    Ok(())
}

/// A search within a range returns the matching entries of the range. The search leaves out the
/// matches before and after the range.
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
            agent_id: Some(agent_id.agent_id.into()),
            environment_id: Some(agent_id.environment_id.into()),
            cursor: None,
            count: 100,
            query: QUERY.to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
            claims: ClaimVisibility::Hidden.to_protobuf(),
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

/// The scan budget ends a search page, not a read that follows the cursors. With a budget far below
/// the length of the oplog, one search page stops at the budget. The read of the same range and
/// query returns every match. A range read without an allowance has no budget, also when its start
/// is after the budget. A search whose start is after the budget gives empty pages while it scans
/// the oplog before its start. Its cursors then give only the matches in its range.
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
            agent_id: Some(agent_id.agent_id.into()),
            environment_id: Some(agent_id.environment_id.into()),
            cursor: None,
            count: 100,
            query: QUERY.to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
            claims: ClaimVisibility::Hidden.to_protobuf(),
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

    // A range read without an allowance has no budget. It reads the oplog before its start, which
    // is longer than the budget, and returns every index of its range.
    let from = 1 + BUDGET + 5;
    let late = executor
        .read_oplog(&agent_id, range(from, last), None)
        .await?;
    assert_eq!(indices(&late), (from..=last).collect::<Vec<_>>());

    // A search of the same range scans the oplog before its start for the component revision,
    // and the budget covers that scan. So its first page has no entry, and its cursor continues
    // the scan before `from`. The test follows the cursors without `from`, as `--page --cursor`
    // and the MCP tool send them. The pages give only the matches from `from`.
    let started_from = started
        .iter()
        .copied()
        .filter(|index| *index >= from)
        .collect::<Vec<_>>();
    assert!(!started_from.is_empty(), "{started:?}");
    let search_page = |from_oplog_index: Option<u64>, cursor: Option<OplogCursor>| {
        let mut client = executor.client.clone();
        let request = SearchOplogRequest {
            agent_id: Some(agent_id.agent_id.into()),
            environment_id: Some(agent_id.environment_id.into()),
            cursor,
            count: 100,
            query: QUERY.to_string(),
            auth_ctx: Some(executor.auth_ctx().into()),
            claims: ClaimVisibility::Hidden.to_protobuf(),
            from_oplog_index,
            to_oplog_index: Some(last),
        };
        async move {
            match client.search_oplog(request).await?.into_inner().result {
                Some(search_oplog_response::Result::Success(page)) => Ok(page),
                other => Err(anyhow::anyhow!("the search failed: {other:?}")),
            }
        }
    };
    let search_page = &search_page;
    let first = search_page(Some(from), None).await?;
    assert!(
        first.entries.is_empty(),
        "the budget ends the revision scan"
    );
    let first_cursor = first.next.clone().expect("the revision scan goes on");
    assert!(
        first_cursor.next_oplog_index < from,
        "the cursor continues the scan before {from}: {first_cursor:?}"
    );
    assert_eq!(first_cursor.scan_to, Some(from));
    let followed = futures::stream::try_unfold(first.next, |cursor| async move {
        match cursor {
            None => anyhow::Ok(None),
            Some(cursor) => {
                let page = search_page(None, Some(cursor)).await?;
                let next = page.next.clone();
                Ok(Some((search_page_entries(page), next)))
            }
        }
    })
    .take(100)
    .try_concat()
    .await?;
    assert_eq!(
        followed.iter().map(page_entry_index).collect::<Vec<_>>(),
        started_from
    );
    let read = executor
        .read_oplog(&agent_id, range(from, last), Some(QUERY))
        .await?;
    assert_eq!(indices(&read), started_from);
    Ok(())
}

/// A range page omits an entry that is larger than the page entry limit, but keeps its index in the
/// range. The read of the range returns the entry in full.
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
    let last = full
        .last()
        .map(|entry| entry.oplog_index.as_u64())
        .expect("the oplog has entries");
    assert_eq!(indices(&full), (1..=last).collect::<Vec<_>>());
    let (from, to) = (2, last - 1);
    let paged = read_pages_to(&executor, &agent_id, from, Some(to), 3).await?;
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
    let expected = full
        .iter()
        .filter(|entry| (from..=to).contains(&entry.oplog_index.as_u64()))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(read, expected);
    Ok(())
}

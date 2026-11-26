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

//! A stored read that fails on a shared call path gives no outcome. These tests damage an oplog
//! entry in the storage and drive the production call path that reads it: a guest read of the
//! oplog of another agent, and the start of an agent with a pending keyed invocation. The read
//! error reaches the caller, and no oplog gets an entry because of it.

use crate::Tracing;

use futures::{StreamExt, TryStreamExt};

use golem_common::model::agent::AgentMode;
use golem_common::model::card::parse_polymorphic_permission;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentStatus, IdempotencyKey, OwnedAgentId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::OplogConfig;
use golem_worker_executor::storage::indexed::sqlite::SqliteIndexedStorage;
use golem_worker_executor::storage::indexed::{IndexedStorageLabelledApi, IndexedStorageNamespace};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, sqlite_storage_config, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// The codec version byte of every stored oplog entry.
const ENTRY_VERSION: u8 = 3;
/// A codec version that no reader knows.
const UNKNOWN_VERSION: u8 = 255;

/// The stored oplog rows of one agent, in the SQLite indexed storage of executors started with
/// the SQLite storage config.
pub(crate) struct StoredOplogRows {
    indexed: SqliteIndexedStorage,
    database: String,
    namespace: IndexedStorageNamespace,
    key: String,
}

impl StoredOplogRows {
    pub(crate) async fn open(
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        owned_agent_id: &OwnedAgentId,
    ) -> anyhow::Result<Self> {
        let indexed_config = golem_worker_executor::derive_disjoint_sqlite_config(
            &sqlite_storage_config(deps, context),
            "indexed",
        );
        Ok(Self {
            indexed: SqliteIndexedStorage::configured(&indexed_config).await?,
            database: indexed_config.database.clone(),
            namespace: IndexedStorageNamespace::OpLog {
                owned_agent_id: owned_agent_id.clone(),
                agent_mode: AgentMode::Durable,
            },
            key: owned_agent_id.storage_key(),
        })
    }

    /// Every stored row, by oplog index, as the bytes the storage holds.
    pub(crate) async fn rows(&self) -> anyhow::Result<Vec<(u64, Vec<u8>)>> {
        Ok(self
            .indexed
            .with_entity("test", "read_rows", "entry")
            .read_raw(self.namespace.clone(), &self.key, 1, 1_000_000)
            .await?)
    }

    /// Replaces the stored bytes of the entry at `index`.
    pub(crate) async fn replace(&self, index: u64, bytes: &[u8]) -> anyhow::Result<()> {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", self.database)).await?;
        let updated = sqlx::query(
            "UPDATE index_storage SET value = ? WHERE namespace = 'durable-worker-oplog' AND key = ? AND id = ?",
        )
        .bind(bytes)
        .bind(&self.key)
        .bind(index as i64)
        .execute(&pool)
        .await?
        .rows_affected();
        pool.close().await;
        anyhow::ensure!(updated == 1, "no stored oplog entry {index} to replace");
        Ok(())
    }

    /// Sets the codec version byte of the entry at `index` to one that no reader knows, and gives
    /// the original bytes back.
    pub(crate) async fn make_unreadable(&self, index: u64) -> anyhow::Result<Vec<u8>> {
        let (_, original) = self
            .rows()
            .await?
            .into_iter()
            .find(|(id, _)| *id == index)
            .ok_or_else(|| anyhow::anyhow!("no stored oplog entry {index}"))?;
        assert_eq!(original[0], ENTRY_VERSION);
        let mut corrupt = original.clone();
        corrupt[0] = UNKNOWN_VERSION;
        self.replace(index, &corrupt).await?;
        Ok(original)
    }
}

/// Decodes the stored rows after `after`. Every one of them must decode.
pub(crate) fn decoded_after(rows: &[(u64, Vec<u8>)], after: u64) -> Vec<(u64, OplogEntry)> {
    rows.iter()
        .filter(|(index, _)| *index > after)
        .map(|(index, bytes)| {
            let entry = golem_common::serialization::decode::<OplogEntry>(bytes)
                .unwrap_or_else(|error| panic!("stored oplog entry {index} must decode: {error}"));
            (*index, entry)
        })
        .collect()
}

/// The entries among `entries` that record an outcome, cancel work or report a failure: what a
/// failed stored read must not cause.
pub(crate) fn outcome_entries(entries: &[(u64, OplogEntry)]) -> Vec<(u64, &OplogEntry)> {
    entries
        .iter()
        .filter(|(_, entry)| {
            matches!(
                entry,
                OplogEntry::Error { .. }
                    | OplogEntry::AgentInvocationFinished { .. }
                    | OplogEntry::AgentInvocationFailed { .. }
                    | OplogEntry::AgentInvocationSettled { .. }
                    | OplogEntry::CancelPendingInvocation { .. }
                    | OplogEntry::Interrupted { .. }
                    | OplogEntry::Exited { .. }
                    | OplogEntry::End { .. }
                    | OplogEntry::Cancelled { .. }
                    | OplogEntry::StreamTerminal { .. }
            )
        })
        .map(|(index, entry)| (*index, entry))
        .collect()
}

/// An executor without archive layers, so every entry stays in the primary oplog, that retries a
/// failed recovery within half a second.
fn overrides() -> TestExecutorOverrides {
    let oplog = OplogConfig {
        indexed_storage_layers: 1,
        blob_storage_layers: 0,
        ..Default::default()
    };
    TestExecutorOverrides {
        configure: Some(Arc::new(move |runtime| {
            runtime.oplog = oplog.clone();
            runtime.suspend.suspend_after = Duration::from_secs(300);
            runtime.retry.min_delay = Duration::from_millis(50);
            runtime.retry.max_delay = Duration::from_millis(500);
        })),
        ..Default::default()
    }
}

/// The `Start` indexes of the calls of `function_name` in `oplog` after `after`, each with
/// whether the oplog holds a terminal (`End` or `Cancelled`) for it.
fn host_calls(
    oplog: &[PublicOplogEntryWithIndex],
    function_name: &str,
    after: OplogIndex,
) -> Vec<(OplogIndex, bool)> {
    oplog
        .iter()
        .filter(|entry| entry.oplog_index > after)
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == function_name => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .map(|start| {
            let terminated = oplog.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::End(end) => end.start_index == start,
                PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
                _ => false,
            });
            (start, terminated)
        })
        .collect()
}

/// The public entries of `oplog` after `after` that record an outcome or report a failure.
fn public_outcomes(
    oplog: &[PublicOplogEntryWithIndex],
    after: OplogIndex,
) -> Vec<&PublicOplogEntryWithIndex> {
    oplog
        .iter()
        .filter(|entry| entry.oplog_index > after)
        .filter(|entry| {
            matches!(
                entry.entry,
                PublicOplogEntry::Error(_)
                    | PublicOplogEntry::AgentInvocationFinished(_)
                    | PublicOplogEntry::AgentInvocationFailed(_)
                    | PublicOplogEntry::CancelPendingInvocation(_)
            )
        })
        .collect()
}

/// Waits until `reader` has started a call of `function_name` after `end` and its instance was
/// loaded again, which shows that the failed read made it recover.
async fn wait_for_a_retried_call(
    executor: &TestWorkerExecutor,
    reader: &OwnedAgentId,
    function_name: &str,
    end: OplogIndex,
) -> anyhow::Result<()> {
    let polls = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
        Duration::from_millis(100),
    ));
    let mut retried = polls
        .then(|_| async move {
            let oplog = executor.get_oplog(reader, OplogIndex::INITIAL).await?;
            anyhow::Ok(
                !host_calls(&oplog, function_name, end).is_empty()
                    && executor.instance_load_count(&reader.agent_id) >= 2,
            )
        })
        .try_filter(|retried| futures::future::ready(*retried))
        .boxed();
    tokio::time::timeout(Duration::from_secs(60), retried.try_next())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "{reader} did not start and retry its read of the unreadable oplog: loads {}",
                executor.instance_load_count(&reader.agent_id)
            )
        })??;
    Ok(())
}

const GET_OPLOG_NEXT: &str = "golem::api::get-oplog::get-next";
const SEARCH_OPLOG_NEXT: &str = "golem::api::search-oplog::get-next";

/// Agent A reads the oplog of agent B with `get-oplog` and `search-oplog`, and an entry of B does
/// not decode. The read is a failure of B, so for A it is a read that failed for a time: A
/// recovers and reads again, and its oplog records no terminal of the call, no invocation result
/// and no error. B's stored oplog stays as it is. When B reads again, the same calls of A complete
/// at their own `Start`, and the invocations of A give the result of the read.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_guest_read_of_an_unreadable_foreign_oplog_records_no_outcome(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .try_update_agent_provision_config("GolemHostApi", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(parse_polymorphic_permission(
                    "oplog(?env/*/*) @ * : read : *",
                )?);
            Ok::<_, golem_common::model::card::CardParseError>(())
        })?
        .store()
        .await?;
    let target = agent_key!("GolemHostApi", "unreadable-read-target");
    let getter = agent_key!("GolemHostApi", "unreadable-read-getter");
    let searcher = agent_key!("GolemHostApi", "unreadable-read-searcher");
    let target_id = executor.start_agent(&component.id, target.clone()).await?;
    let getter_id = executor.start_agent(&component.id, getter.clone()).await?;
    let searcher_id = executor
        .start_agent(&component.id, searcher.clone())
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &target,
            "get_self_metadata_result",
            data_value!(),
        )
        .await?;

    // The healthy reads pass, so the permissions and the path are in place.
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &getter,
                "read_oplog_result",
                data_value!(target_id.agent_id),
            )
            .await?
            .into_typed::<Result<(), String>>()?,
        Ok(())
    );
    assert!(
        executor
            .invoke_and_await_agent(
                &component,
                &searcher,
                "search_oplog_result",
                data_value!(target_id.agent_id, "get"),
            )
            .await?
            .into_typed::<Result<Option<Vec<u64>>, String>>()?
            .is_ok()
    );
    futures::try_join!(
        executor.wait_for_status(&getter_id, AgentStatus::Idle, Duration::from_secs(30)),
        executor.wait_for_status(&searcher_id, AgentStatus::Idle, Duration::from_secs(30)),
        executor.wait_for_status(&target_id, AgentStatus::Idle, Duration::from_secs(30)),
    )?;
    let getter_end = OplogIndex::from_u64(executor.get_oplog_last_index(&getter_id).await?);
    let searcher_end = OplogIndex::from_u64(executor.get_oplog_last_index(&searcher_id).await?);
    executor.release().await?;

    let target_rows = StoredOplogRows::open(deps, &context, &target_id).await?;
    let healthy_rows = target_rows.rows().await?;
    let original = target_rows.make_unreadable(2).await?;
    let corrupt_rows = target_rows.rows().await?;

    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let get_key = IdempotencyKey::fresh();
    let search_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &getter,
            &get_key,
            "read_oplog_result",
            data_value!(target_id.agent_id),
        )
        .await?;
    executor
        .invoke_agent_with_key(
            &component,
            &searcher,
            &search_key,
            "search_oplog_result",
            data_value!(target_id.agent_id, "get"),
        )
        .await?;

    let readers = [
        (&getter_id, GET_OPLOG_NEXT, getter_end),
        (&searcher_id, SEARCH_OPLOG_NEXT, searcher_end),
    ];
    // Each reader starts its call, fails the read and recovers at least once.
    futures::stream::iter(readers)
        .map(Ok)
        .try_for_each(|(reader, function_name, end)| {
            wait_for_a_retried_call(&executor, reader, function_name, end)
        })
        .await?;

    futures::stream::iter(readers)
        .map(Ok)
        .try_for_each(|(reader, function_name, end)| {
            let executor = &executor;
            async move {
                let oplog = executor.get_oplog(reader, OplogIndex::INITIAL).await?;
                let calls = host_calls(&oplog, function_name, end);
                assert_eq!(
                    calls.iter().filter(|(_, terminated)| *terminated).count(),
                    0,
                    "a failed foreign read must leave its call without a terminal: {calls:?}"
                );
                assert!(
                    public_outcomes(&oplog, end).is_empty(),
                    "a failed foreign read must record no result or error in {reader}: {:?}",
                    public_outcomes(&oplog, end)
                );
                anyhow::Ok(())
            }
        })
        .await?;
    assert_eq!(target_rows.rows().await?, corrupt_rows);

    target_rows.replace(2, &original).await?;
    let (got, searched) = tokio::time::timeout(Duration::from_secs(60), async {
        futures::try_join!(
            executor.invoke_and_await_agent_with_key(
                &component,
                &getter,
                &get_key,
                "read_oplog_result",
                data_value!(target_id.agent_id),
            ),
            executor.invoke_and_await_agent_with_key(
                &component,
                &searcher,
                &search_key,
                "search_oplog_result",
                data_value!(target_id.agent_id, "get"),
            ),
        )
    })
    .await
    .map_err(|_| anyhow::anyhow!("the readers did not complete after the oplog read again"))??;
    assert_eq!(got.into_typed::<Result<(), String>>()?, Ok(()));
    assert!(
        searched
            .into_typed::<Result<Option<Vec<u64>>, String>>()?
            .is_ok()
    );

    futures::stream::iter(readers)
        .map(Ok)
        .try_for_each(|(reader, function_name, end)| {
            let executor = &executor;
            async move {
                let oplog = executor.get_oplog(reader, OplogIndex::INITIAL).await?;
                let calls = host_calls(&oplog, function_name, end);
                assert_eq!(
                    calls.len(),
                    1,
                    "the reconstruction must complete the call at its own Start: {calls:?}"
                );
                assert!(
                    calls[0].1,
                    "the read call must end once it reads: {calls:?}"
                );
                assert!(
                    !oplog
                        .iter()
                        .filter(|entry| entry.oplog_index > end)
                        .any(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))),
                    "{reader} must have no error entry from the failed read"
                );
                anyhow::Ok(())
            }
        })
        .await?;
    assert_eq!(target_rows.rows().await?, healthy_rows);
    Ok(())
}

/// A keyed invocation of an idle agent is pending when the start of the agent reads an entry that
/// does not decode, before the head of its oplog. The start fails with the read error inside a
/// failed resume. The waiter learns the read error, a later lookup of the same key does not find
/// a completed result, and the oplog gets no error, cancellation or invocation terminal. After the
/// entry reads again, the pending invocation runs once.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_start_that_cannot_read_its_oplog_leaves_a_pending_keyed_invocation_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_key!("Counter", "unreadable-start-pending-key");
    let agent_id = executor.start_agent(&component.id, agent.clone()).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        1
    );
    executor
        .wait_for_status(&agent_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    executor.release().await?;

    let stored = StoredOplogRows::open(deps, &context, &agent_id).await?;
    let healthy_rows = stored.rows().await?;
    let head = healthy_rows.last().expect("the agent has an oplog").0;
    assert!(head > 2, "the unreadable entry must be before the head");
    let original = stored.make_unreadable(2).await?;
    let corrupt_rows = stored.rows().await?;

    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let key = IdempotencyKey::fresh();
    let assert_read_error = |result: anyhow::Result<_>| {
        let error = result.expect_err("an agent whose oplog does not read must not run");
        let message = format!("{error:#}");
        assert!(
            message.contains("unknown serialization version: 255")
                && message.contains("entry 2")
                && message.contains(&agent_id.agent_id.to_string()),
            "the waiter must learn the read error of the entry: {message}"
        );
    };
    assert_read_error(
        tokio::time::timeout(
            Duration::from_secs(60),
            executor.invoke_and_await_agent_with_key(
                &component,
                &agent,
                &key,
                "increment",
                data_value!(),
            ),
        )
        .await?,
    );
    assert!(!executor.worker_is_loaded(&agent_id).await);

    let after_failure = stored.rows().await?;
    assert_eq!(
        after_failure
            .iter()
            .filter(|(index, _)| *index <= head)
            .cloned()
            .collect::<Vec<_>>(),
        corrupt_rows,
        "the failed start must not change the stored history"
    );
    let admitted = decoded_after(&after_failure, head);
    let pending = admitted
        .iter()
        .filter(|(_, entry)| matches!(entry, OplogEntry::PendingAgentInvocation { .. }))
        .count();
    assert_eq!(
        pending, 1,
        "the keyed invocation must be pending when the start fails: {admitted:?}"
    );
    assert!(
        outcome_entries(&admitted).is_empty(),
        "a failed start read must not record an outcome: {admitted:?}"
    );

    // A second waiter for the same key finds no completed result: it learns the read error again.
    assert_read_error(
        tokio::time::timeout(
            Duration::from_secs(60),
            executor.invoke_and_await_agent_with_key(
                &component,
                &agent,
                &key,
                "increment",
                data_value!(),
            ),
        )
        .await?,
    );
    let after_lookup = stored.rows().await?;
    assert_eq!(after_lookup, after_failure);
    executor.release().await?;

    stored.replace(2, &original).await?;
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(&component, &agent, &key, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        2,
        "the pending invocation runs once after its oplog reads again"
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        3
    );
    let oplog = executor.get_oplog(&agent_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
            .count(),
        0
    );
    Ok(())
}

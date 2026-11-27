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

//! These tests damage an oplog entry in the storage so that it does not decode, and drive the
//! production call path that reads it. A read that does not decode fails for good. A guest read of
//! the oplog of another agent, and an awaited RPC to an agent whose oplog does not read, fail only
//! the call that made the read: the reader records the error as the result of that call and does
//! not recover. The start of an agent that does not decode its
//! own oplog fails that agent: the oplog gets one `Recovery` error and no other outcome, and the
//! pending keyed invocation stays pending until a later start reads the oplog.

use crate::Tracing;

use futures::{StreamExt, TryStreamExt};

use golem_common::model::agent::AgentMode;
use golem_common::model::card::parse_polymorphic_permission;
use golem_common::model::oplog::{
    AgentError, OplogEntry, OplogErrorKind, OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentId, AgentStatus, IdempotencyKey, OwnedAgentId};
use golem_common::schema::SchemaValue;
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::{AgentResult, TestDsl};
use golem_worker_executor::services::golem_config::OplogConfig;
use golem_worker_executor::storage::indexed::sqlite::SqliteIndexedStorage;
use golem_worker_executor::storage::indexed::{IndexedStorageLabelledApi, IndexedStorageNamespace};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, RecordingRpc, TestContext, TestExecutorOverrides,
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
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
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

/// The entries among `entries` that record an outcome, cancel work or report a failure.
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

/// Whether `entry` is the `Recovery` error that fails `agent_id` because its own oplog does not
/// read. The error text names the agent and holds each of `parts`.
pub(crate) fn is_own_read_failure(entry: &OplogEntry, agent_id: &AgentId, parts: &[&str]) -> bool {
    matches!(
        entry,
        OplogEntry::Error {
            kind: OplogErrorKind::Recovery,
            error: AgentError::InternalError(text),
            ..
        } if text.contains(&format!("The oplog of {agent_id} failed"))
            && parts.iter().all(|part| text.contains(part))
    )
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

const GET_OPLOG_NEXT: &str = "golem::api::get-oplog::get-next";
const SEARCH_OPLOG_NEXT: &str = "golem::api::search-oplog::get-next";

/// The text of `value` as it shows inside the debug form of a guest error string.
fn as_debug_text(value: &str) -> String {
    let quoted = format!("{value:?}");
    quoted[1..quoted.len() - 1].to_string()
}

/// Agent A reads the oplog of agent B with `get-oplog` and `search-oplog`, and entry 2 of B has a
/// version byte that no reader knows. The read error is the error of the call that made the read:
/// the call returns an error that names B, the entry and the version byte, and A records it as it
/// records any failed call. A does not recover and does not read again: A loads its instance one
/// time, its oplog holds one `Start` of the call with its `End` and no error, and A answers a
/// further call. B's stored oplog stays as it is.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_guest_read_of_an_unreadable_foreign_oplog_is_the_error_of_the_call(
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
    target_rows.make_unreadable(2).await?;
    let corrupt_rows = target_rows.rows().await?;

    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let assert_names_the_entry = |error: &str| {
        let target_text = as_debug_text(&format!("The oplog of {} failed", target_id.agent_id));
        assert!(
            error.contains(&target_text)
                && error.contains(&format!(
                    "oplog entry 2: unknown serialization version: {UNKNOWN_VERSION}"
                )),
            "the call error must name {target_text}, entry 2 and version {UNKNOWN_VERSION}: {error}"
        );
    };
    // A reader that recovers and reads again never completes the invocation.
    let (got, searched) = tokio::time::timeout(Duration::from_secs(60), async {
        futures::try_join!(
            executor.invoke_and_await_agent(
                &component,
                &getter,
                "read_oplog_result",
                data_value!(target_id.agent_id),
            ),
            executor.invoke_and_await_agent(
                &component,
                &searcher,
                "search_oplog_result",
                data_value!(target_id.agent_id, "get"),
            ),
        )
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "the readers did not complete their calls: loads {} and {}",
            executor.instance_load_count(&getter_id.agent_id),
            executor.instance_load_count(&searcher_id.agent_id)
        )
    })??;
    assert_names_the_entry(
        &got.into_typed::<Result<(), String>>()?
            .expect_err("a read of an unreadable oplog must fail the call"),
    );
    assert_names_the_entry(
        &searched
            .into_typed::<Result<Option<Vec<u64>>, String>>()?
            .expect_err("a search of an unreadable oplog must fail the call"),
    );

    // Each reader answers a further call.
    futures::stream::iter([&getter, &searcher])
        .map(Ok)
        .try_for_each(|reader| {
            let executor = &executor;
            let component = &component;
            async move {
                assert!(
                    executor
                        .invoke_and_await_agent(
                            component,
                            reader,
                            "get_self_metadata_result",
                            data_value!(),
                        )
                        .await?
                        .into_typed::<Result<String, String>>()?
                        .is_ok()
                );
                anyhow::Ok(())
            }
        })
        .await?;

    let readers = [
        (&getter_id, GET_OPLOG_NEXT, getter_end),
        (&searcher_id, SEARCH_OPLOG_NEXT, searcher_end),
    ];
    futures::stream::iter(readers)
        .map(Ok)
        .try_for_each(|(reader, function_name, end)| {
            let executor = &executor;
            async move {
                assert_eq!(
                    executor.instance_load_count(&reader.agent_id),
                    1,
                    "{reader} must not recover because of the failed read"
                );
                let oplog = executor.get_oplog(reader, OplogIndex::INITIAL).await?;
                let calls = host_calls(&oplog, function_name, end);
                assert_eq!(
                    calls.len(),
                    1,
                    "the failed read must be one call, not read again: {calls:?}"
                );
                assert!(calls[0].1, "the failed read call must end: {calls:?}");
                let failures = oplog
                    .iter()
                    .filter(|entry| entry.oplog_index > end)
                    .filter(|entry| {
                        matches!(
                            entry.entry,
                            PublicOplogEntry::Error(_) | PublicOplogEntry::AgentInvocationFailed(_)
                        )
                    })
                    .collect::<Vec<_>>();
                assert!(
                    failures.is_empty(),
                    "{reader} must record the failed read as a call result, not as a failure: {failures:?}"
                );
                anyhow::Ok(())
            }
        })
        .await?;
    assert_eq!(target_rows.rows().await?, corrupt_rows);
    Ok(())
}

const INVOKE_AND_AWAIT: &str = "golem::rpc::wasm-rpc::invoke_and_await";

/// The case of `RpcCallOutcome::Ok` in the `agent_rpc_rust` component.
const RPC_CALL_OK: u32 = 0;
/// The case of `RpcCallOutcome::RemoteInternalError` in the `agent_rpc_rust` component.
const RPC_CALL_REMOTE_INTERNAL_ERROR: u32 = 4;

/// The case of the `RpcCallOutcome` that `try_call_counter` returns, with its debug text.
fn rpc_call_outcome(result: &AgentResult) -> (u32, String) {
    match result.value() {
        Some(SchemaValue::Variant(value)) => (value.case, format!("{value:?}")),
        other => panic!("try_call_counter must return an RpcCallOutcome: {other:?}"),
    }
}

/// Agent A awaits a scalar RPC to agent B, and entry 2 of B has a version byte that no reader
/// knows. The start of B reads the entry while the call waits, so the call gets the read error of
/// B's history. That error is the failure of the call: A dispatches the call one time, records
/// one `End` with the failure that names B, the entry and the version byte, and returns it to the
/// guest. A does not retry the call, does not recover and answers a further call.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn an_awaited_rpc_to_an_agent_whose_oplog_does_not_read_is_the_failure_of_the_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let dispatches = Arc::new(std::sync::Mutex::new(Vec::new()));
    let overrides = TestExecutorOverrides {
        wrap_rpc: Some(Arc::new({
            let dispatches = dispatches.clone();
            move |rpc| Arc::new(RecordingRpc::new(rpc, "inc_by", dispatches.clone()))
        })),
        ..overrides()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let caller = agent_key!("RpcAuthTester", "unreadable-rpc-caller");
    let caller_id = executor.start_agent(&component.id, caller.clone()).await?;
    let target_id = crate::agent::keyed_agent_id(
        component.environment_id,
        &agent_key!("RpcCounter", "unreadable-rpc-target"),
    );
    let healthy = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "try_call_counter",
            data_value!("unreadable-rpc-target"),
        )
        .await?;
    assert_eq!(rpc_call_outcome(&healthy).0, RPC_CALL_OK, "{healthy:?}");
    futures::try_join!(
        executor.wait_for_status(&caller_id, AgentStatus::Idle, Duration::from_secs(30)),
        executor.wait_for_status(&target_id, AgentStatus::Idle, Duration::from_secs(30)),
    )?;
    let caller_end = OplogIndex::from_u64(executor.get_oplog_last_index(&caller_id).await?);
    executor.release().await?;

    StoredOplogRows::open(deps, &context, &target_id)
        .await?
        .make_unreadable(2)
        .await?;
    dispatches.lock().unwrap().clear();

    let executor = start_with_overrides(deps, &context, overrides).await?;
    // A caller that retries the call or recovers and calls again does not complete in time.
    let failed = tokio::time::timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent(
            &component,
            &caller,
            "try_call_counter",
            data_value!("unreadable-rpc-target"),
        ),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "the caller did not complete the call: {} dispatches, {} loads",
            dispatches.lock().unwrap().len(),
            executor.instance_load_count(&caller_id.agent_id)
        )
    })??;
    let (case, text) = rpc_call_outcome(&failed);
    assert_eq!(case, RPC_CALL_REMOTE_INTERNAL_ERROR, "{text}");
    let target_text = as_debug_text(&format!("The oplog of {} failed", target_id.agent_id));
    assert!(
        text.contains(&target_text)
            && text.contains("entry 2")
            && text.contains(&format!("unknown serialization version: {UNKNOWN_VERSION}")),
        "the call failure must name {target_text}, entry 2 and version {UNKNOWN_VERSION}: {text}"
    );
    assert_eq!(
        dispatches.lock().unwrap().len(),
        1,
        "the failed call must be dispatched one time"
    );
    assert_eq!(
        executor.instance_load_count(&caller_id.agent_id),
        1,
        "the caller must not recover because of the failed read"
    );
    let oplog = executor.get_oplog(&caller_id, OplogIndex::INITIAL).await?;
    let calls = host_calls(&oplog, INVOKE_AND_AWAIT, caller_end);
    assert!(
        matches!(calls.as_slice(), [(_, true)]),
        "the failed call must be one call with its End: {calls:?}"
    );
    let failures = oplog
        .iter()
        .filter(|entry| entry.oplog_index > caller_end)
        .filter(|entry| {
            matches!(
                entry.entry,
                PublicOplogEntry::Error(_) | PublicOplogEntry::AgentInvocationFailed(_)
            )
        })
        .collect::<Vec<_>>();
    assert!(
        failures.is_empty(),
        "the caller must record the failed read as a call result, not as a failure: {failures:?}"
    );

    // The caller answers a further call.
    let further = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "try_call_counter",
            data_value!("healthy-rpc-target"),
        )
        .await?;
    assert_eq!(rpc_call_outcome(&further).0, RPC_CALL_OK, "{further:?}");
    Ok(())
}

/// Agent A starts an asynchronous RPC to agent B and awaits it with `future-invoke-result.get`.
/// Entry 2 of B has a version byte that no reader knows, so the call gets the read error of B's
/// history. `get` keeps that error a failure of the call: A dispatches the call one time and
/// records one `End` with the failure that names B, the entry and the version byte. The guest
/// unwraps the result, so the invocation of A fails.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_future_get_of_an_rpc_to_an_agent_whose_oplog_does_not_read_is_the_failure_of_the_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let dispatches = Arc::new(std::sync::Mutex::new(Vec::new()));
    let overrides = TestExecutorOverrides {
        wrap_rpc: Some(Arc::new({
            let dispatches = dispatches.clone();
            move |rpc| {
                Arc::new(RecordingRpc::new(
                    rpc,
                    "inc_after_promise",
                    dispatches.clone(),
                ))
            }
        })),
        ..overrides()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let target_name = "unreadable-future-target";
    let target = agent_key!("RpcBlockingCounter", target_name);
    let target_id = executor.start_agent(&component.id, target.clone()).await?;
    let promise = executor
        .invoke_and_await_agent(&component, &target, "create_promise", data_value!())
        .await?
        .into_typed::<golem_common::model::PromiseId>()?;
    executor
        .complete_promise(&context.default_environment_id, &promise, vec![])
        .await?;
    let caller = agent_key!("CancelTester", "unreadable-future-caller");
    let caller_id = executor.start_agent(&component.id, caller.clone()).await?;
    let healthy = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "await_counter",
            data_value!(target_name, promise.clone(), true),
        )
        .await?
        .into_typed::<u64>()?;
    assert_eq!(healthy, 7);
    futures::try_join!(
        executor.wait_for_status(&caller_id, AgentStatus::Idle, Duration::from_secs(30)),
        executor.wait_for_status(&target_id, AgentStatus::Idle, Duration::from_secs(30)),
    )?;
    let caller_end = OplogIndex::from_u64(executor.get_oplog_last_index(&caller_id).await?);
    executor.release().await?;

    StoredOplogRows::open(deps, &context, &target_id)
        .await?
        .make_unreadable(2)
        .await?;
    dispatches.lock().unwrap().clear();

    let executor = start_with_overrides(deps, &context, overrides).await?;
    // A `get` that retries the call or a caller that recovers does not finish in time.
    let failed = tokio::time::timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent(
            &component,
            &caller,
            "await_counter",
            data_value!(target_name, promise, true),
        ),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "the caller did not finish the call: {} dispatches, {} loads",
            dispatches.lock().unwrap().len(),
            executor.instance_load_count(&caller_id.agent_id)
        )
    })?;
    assert!(
        failed.is_err(),
        "the guest unwraps the failed call, so the invocation fails: {failed:?}"
    );
    assert_eq!(
        dispatches.lock().unwrap().len(),
        1,
        "the failed call must be dispatched one time"
    );
    let oplog = executor.get_oplog(&caller_id, OplogIndex::INITIAL).await?;
    let calls = host_calls(&oplog, INVOKE_AND_AWAIT, caller_end);
    assert!(
        matches!(calls.as_slice(), [(_, true)]),
        "the failed call must be one call with its End: {calls:?}"
    );
    let (start, _) = calls[0];
    let end = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == start => Some(format!("{end:?}")),
            _ => None,
        })
        .expect("the call has its End");
    let target_text = as_debug_text(&format!("The oplog of {} failed", target_id.agent_id));
    assert!(
        end.contains(&target_text)
            && end.contains("entry 2")
            && end.contains(&format!("unknown serialization version: {UNKNOWN_VERSION}")),
        "the End must record the call failure that names {target_text}, entry 2 and version \
         {UNKNOWN_VERSION}: {end}"
    );
    Ok(())
}

/// A keyed invocation of an idle agent is pending when the start of the agent reads an entry that
/// does not decode, before the head of its oplog. The read fails for good, so the start fails the
/// agent. The waiter learns the read error. The oplog gets the pending invocation and one
/// `Recovery` error that names the agent, the entry and the version byte, and no cancellation or
/// invocation terminal. A later waiter for the same key learns the read error again. After the
/// entry reads again, the next start recovers the agent, and the pending invocation runs once.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_start_that_cannot_decode_its_oplog_fails_the_agent_and_leaves_a_keyed_invocation_pending(
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
    let added = decoded_after(&after_failure, head);
    assert!(
        matches!(
            added.as_slice(),
            [(_, OplogEntry::PendingAgentInvocation { .. }), (_, failure)]
                if is_own_read_failure(
                    failure,
                    &agent_id.agent_id,
                    &["entry 2", "unknown serialization version: 255"],
                )
        ),
        "the failed start must add the pending invocation and one Recovery error, and no other outcome: {added:?}"
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

    // The recorded failure stays until the next start. After the entry reads again, that start
    // recovers the agent, and the pending invocation runs once.
    stored.replace(2, &original).await?;
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    assert_eq!(
        executor.get_worker_metadata(&agent_id).await?.status,
        AgentStatus::Failed
    );
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
    let errors = decoded_after(&stored.rows().await?, head)
        .into_iter()
        .filter(|(_, entry)| matches!(entry, OplogEntry::Error { .. }))
        .collect::<Vec<_>>();
    assert!(
        matches!(
            errors.as_slice(),
            [(_, failure)]
                if is_own_read_failure(
                    failure,
                    &agent_id.agent_id,
                    &["entry 2", "unknown serialization version: 255"],
                )
        ),
        "the oplog must hold only the Recovery error of the failed start: {errors:?}"
    );
    Ok(())
}

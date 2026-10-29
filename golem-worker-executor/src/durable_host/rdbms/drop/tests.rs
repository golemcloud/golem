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

//! These tests exercise retained resolution/completion ownership and error classification.
//! They do not drive the production `drop_transaction` -> `TransactionDrop::finish` ->
//! `cleanup_pending` lifecycle. Task spawning, Store destruction, cleanup handoff, terminal
//! validation and live rollback admission are integration contracts, not proved by manually
//! constructing `PendingTransactionDrop` here. The worker-executor suites are
//! `tests/rdbms.rs::rdbms_postgres_snapshot_changes_transaction_drop_schedule` and
//! `tests/tool_streaming/rdbms_drop.rs::{trapped_tool_transaction_drop_survives_replay_store_disposal,
//! trapped_tool_transaction_drop_retains_terminal_read_failure_after_disposal}`.

use super::*;
use crate::durable_host::concurrent::LiveCallPermit;
use crate::services::rdbms::postgres::PostgresType;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::test;
use tokio::sync::oneshot;

struct WaitForDrop {
    wait: DropCompletion,
    started: oneshot::Sender<()>,
}

impl AccessorTask<ResourceTable> for WaitForDrop {
    async fn run(self, _store: &Accessor<ResourceTable>) -> wasmtime::Result<()> {
        self.started.send(()).unwrap();
        self.wait.await.map_err(wasmtime::Error::new)
    }
}

fn pending_drop(
    resolution: DropResolution,
    completion: Option<DropCompletion>,
    counter: Arc<AtomicUsize>,
) -> PendingTransactionDrop<PostgresType> {
    PendingTransactionDrop {
        tx: RdbmsTransactionEntry::new(
            RdbmsPoolKey::from("postgres://localhost/test").unwrap(),
            RdbmsTransactionState::Reconstructed(TransactionId::new("retained-drop")),
            OplogIndex::INITIAL,
            true,
        ),
        protocol: Some(TransactionProtocol {
            _invocation_id: None,
            replay_end: None,
            _live_call_permit: LiveCallPermit::new(counter),
        }),
        resolution,
        completion,
        resource_rep: 0,
    }
}

async fn destroy_waiting_store(
    mut table: ResourceTable,
    pending: PendingTransactionDrop<PostgresType>,
    wait: DropCompletion,
) -> (
    ResourceTable,
    Resource<PendingTransactionDrop<PostgresType>>,
) {
    let resource = table.push(pending).unwrap();
    let mut config = wasmtime::Config::new();
    config.concurrency_support(true);
    let engine = wasmtime::Engine::new(&config).unwrap();
    let mut store = wasmtime::Store::new(&engine, table);
    let (started, started_rx) = oneshot::channel();
    store
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            accessor.spawn(WaitForDrop { wait, started });
            started_rx.await.unwrap();
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
    (store.into_data(), resource)
}

#[test]
#[test_r::timeout("10s")]
async fn transaction_drop_retained_resolution_survives_store_destruction() {
    for outcome in [
        Some(SerializableRdbmsTransactionTerminal::Commit(Ok(()))),
        Some(SerializableRdbmsTransactionTerminal::Rollback(Ok(()))),
        Some(SerializableRdbmsTransactionTerminal::Drop(Ok(()))),
        None,
    ] {
        let expected_error = matches!(
            outcome,
            Some(
                SerializableRdbmsTransactionTerminal::Commit(_)
                    | SerializableRdbmsTransactionTerminal::Rollback(_)
            )
        );
        let incomplete = outcome.is_none();
        let (terminal, terminal_rx) = oneshot::channel();
        let resolution: DropResolution = async move {
            terminal_rx.await.unwrap();
            match outcome {
                Some(outcome) => {
                    validate_drop_response(HostResponse::GolemRdbmsTransactionTerminal(
                        HostResponseGolemRdbmsTransactionTerminal { outcome },
                    ))?;
                    Ok(TransactionDropResolution::Completed)
                }
                None => Ok(TransactionDropResolution::Incomplete),
            }
        }
        .boxed()
        .shared();
        let wait = resolution
            .clone()
            .map(|result| {
                result
                    .map(|_| ())
                    .map_err(|error| SharedDropError(Arc::new(error.into())))
            })
            .boxed()
            .shared();
        let counter = Arc::new(AtomicUsize::new(0));
        let pending = pending_drop(resolution, None, counter.clone());
        let (mut table, resource) =
            destroy_waiting_store(ResourceTable::new(), pending, wait).await;
        assert_eq!(
            counter.load(Ordering::Acquire),
            1,
            "destruction must retain the protocol"
        );
        terminal.send(()).unwrap();
        let result = table.get(&resource).unwrap().resolution.clone().await;
        if expected_error {
            assert!(
                result
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("Transaction terminal does not match recorded Drop outcome")
            );
        } else {
            assert_eq!(
                matches!(result.unwrap(), TransactionDropResolution::Incomplete),
                incomplete
            );
        }
        table.delete(resource).unwrap();
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }
}

#[test]
#[test_r::timeout("10s")]
async fn transaction_drop_retained_completion_does_not_repeat_after_store_destruction() {
    let appended = Arc::new(AtomicUsize::new(0));
    let (commit, committed) = oneshot::channel();
    let observed = appended.clone();
    let completion: DropCompletion = async move {
        observed.fetch_add(1, Ordering::AcqRel);
        committed.await.unwrap();
        Ok(())
    }
    .boxed()
    .shared();
    let pending = pending_drop(
        futures::future::ready(Ok(TransactionDropResolution::Incomplete))
            .boxed()
            .shared(),
        Some(completion.clone()),
        Arc::new(AtomicUsize::new(0)),
    );
    let (mut table, resource) =
        destroy_waiting_store(ResourceTable::new(), pending, completion).await;
    assert_eq!(appended.load(Ordering::Acquire), 1);
    commit.send(()).unwrap();
    let retained = table.get(&resource).unwrap().completion.clone().unwrap();
    retained.clone().await.unwrap();
    retained.await.unwrap();
    assert_eq!(
        appended.load(Ordering::Acquire),
        1,
        "resume the same commit future, do not append another End"
    );
    table.delete(resource).unwrap();
}

#[test]
fn transaction_drop_retained_error_keeps_original_start_and_classification() {
    use crate::durable_host::durability::find_durable_call_trap_context;
    let begin = OplogIndex::from_u64(42);
    let error = SharedDropError::new(
        begin,
        anyhow!(ClassifiedHostError {
            kind: HostFailureKind::Permanent,
            message: "transaction status resolution exhausted".to_string(),
        }),
    );
    for error in [
        anyhow::Error::new(error.clone()),
        anyhow::Error::from(wasmtime::Error::new(error)),
    ] {
        assert_eq!(
            find_durable_call_trap_context(&error).unwrap().retry_from,
            begin
        );
        assert!(
            error
                .chain()
                .any(|cause| cause.downcast_ref::<ClassifiedHostError>().is_some())
        );
    }
}

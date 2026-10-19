use super::*;
use crate::blob_mutation_storage::MutationStorage;
use anyhow::{Context as _, ensure};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use golem_worker_executor::services::blob_store::DefaultBlobStoreService;
use golem_worker_executor::services::resource_limits::ResourceLimitsDisabled;
use golem_worker_executor_test_utils::start_with_resource_limits_and_overrides;
use std::path::Path;
use test_r::test;

mod unavailable;

struct ReleaseMutation(Arc<MutationStorage>);

impl Drop for ReleaseMutation {
    fn drop(&mut self) {
        self.0.release.add_permits(1);
    }
}

#[test]
#[timeout("2m")]
async fn entered_blob_write_records_completion_before_stop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    completion_before_stop(last_unique_id, deps, component, false).await
}

#[test]
#[timeout("2m")]
async fn entered_blob_copy_records_completion_before_stop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    completion_before_stop(last_unique_id, deps, component, true).await
}

async fn completion_before_stop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    copy: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let storage = Arc::new(MutationStorage::default());
    let _release = ReleaseMutation(storage.clone());
    let executor = start_with_resource_limits_and_overrides(
        deps,
        &context,
        Arc::new(ResourceLimitsDisabled),
        TestExecutorOverrides {
            wrap_blob_store_service: Some(Arc::new({
                let storage = storage.clone();
                move |_| Arc::new(DefaultBlobStoreService::new(storage.clone()))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let name = agent_id!("BlobStore", "entered-mutation");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &name,
            "create_container",
            data_value!("container"),
        )
        .await?;
    let data = vec![1u8, 3, 5, 7];
    executor
        .invoke_and_await_agent(
            &component,
            &name,
            "write_data",
            data_value!("container", "source", data.clone()),
        )
        .await?;
    let worker = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
        .await
        .context("cached worker")?
        .primary();
    let key = IdempotencyKey::fresh();
    let method = if copy {
        "blobstore_probe"
    } else {
        "write_data_result"
    };
    let input = if copy {
        data_value!(
            "copy-object",
            "container",
            "source",
            "container",
            "destination"
        )
    } else {
        data_value!("container", "destination", data.clone())
    };
    storage.armed.store(true, Ordering::SeqCst);
    let mut invocation = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let name = name.clone();
        let key = key.clone();
        let input = input.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(&component, &name, &key, method, input)
                .await
        }
    }));
    storage.pending.notified().await;
    ensure!(storage.effects.load(Ordering::SeqCst) == 1);
    executor.commit_oplog(&id).await?;
    let before = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    let boundary = if copy {
        "::copy_object"
    } else {
        "::write_data"
    };
    let start = before
        .iter()
        .rev()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(call) if call.function_name.ends_with(boundary) => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .context("entered blob mutation Start")?;
    let end_count = |entries: &[golem_common::model::oplog::PublicOplogEntryWithIndex]| {
        entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start)).count()
    };
    ensure!(end_count(&before) == 0);
    ensure!(count_agent_invocation_pair_since(&before, OplogIndex::INITIAL) == (4, 3));
    ensure!(!invocation.is_finished());
    let signal = worker
        .owner_runtime_resources()
        .execution_status()
        .read()
        .unwrap()
        .create_await_interrupt_signal();
    let stopped = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let executor = executor.clone();
        let id = id.clone();
        async move { executor.interrupt(&id).await }
    }));
    ensure!(matches!(signal.await, InterruptKind::Interrupt(_)));
    storage.release.add_permits(1);
    // The API joins retirement, while the signal above only proves publication.
    stopped.await??;
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    ensure!(!worker.is_loaded().await);
    let stopped_entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    ensure!(count_agent_invocation_pair_since(&stopped_entries, OplogIndex::INITIAL) == (4, 3));
    ensure!(
        end_count(&stopped_entries) == 1,
        "the entered mutation lost its mandatory End: {stopped_entries:#?}"
    );
    ensure!(!stopped_entries.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(call) if call.start_index == start)));
    ensure!(
        storage.completed.load(Ordering::SeqCst) == 1
            && storage.dropped.load(Ordering::SeqCst) == 0
    );
    ensure!(storage.effects.load(Ordering::SeqCst) == 2);
    ensure!(
        storage
            .get_raw(
                "test",
                "read",
                BlobStorageNamespace::CustomStorage {
                    environment_id: context.default_environment_id
                },
                Path::new("container/destination")
            )
            .await?
            == Some(data)
    );
    ensure!(
        (&mut invocation).await?.is_err(),
        "the accepted Interrupt must remain the invocation outcome"
    );
    executor.resume(&id, false).await?;
    let output = executor
        .invoke_and_await_agent_with_key(&component, &name, &key, method, input)
        .await?;
    ensure!(output.into_typed::<Result<(), String>>()?.is_ok());
    ensure!(
        storage.completed.load(Ordering::SeqCst) == 1
            && storage.dropped.load(Ordering::SeqCst) == 0,
        "completed replay changed the entered mutation receipt"
    );
    let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    ensure!(
        storage.effects.load(Ordering::SeqCst) == 2,
        "completed replay repeated the backend effect"
    );
    ensure!(end_count(&recovered) == 1);
    ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (4, 4));
    Ok(())
}

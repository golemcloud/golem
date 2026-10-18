use super::*;
use futures::FutureExt;
use golem_common::model::account::AccountId;
use golem_common::model::oplog::{PublicAgentInvocation, PublicOplogEntryWithIndex};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_worker_executor::services::blob_store::{BlobMutationFailureCause, BlobStoreService};
use golem_worker_executor::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
use test_r::test;

struct SharedBlobLimits(Arc<AtomicResourceEntry>);

#[async_trait::async_trait]
impl ResourceLimits for SharedBlobLimits {
    async fn initialize_account(
        &self,
        _account_id: AccountId,
    ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
        Ok(self.0.clone())
    }
}

#[test]
#[timeout("2m")]
async fn unavailable_blob_write_traps_without_host_completion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    unavailable_after_prior_panic(last_unique_id, deps, component, false).await
}

#[test]
#[timeout("2m")]
async fn unavailable_blob_copy_traps_without_host_completion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    unavailable_after_prior_panic(last_unique_id, deps, component, true).await
}

async fn unavailable_after_prior_panic(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    copy: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_id = context.default_environment_id;
    let storage = Arc::new(MutationStorage::default());
    let _release = ReleaseMutation(storage.clone());
    let service = Arc::new(DefaultBlobStoreService::new(storage.clone()));
    let limits = Arc::new(AtomicResourceEntry::new_with_blob_storage_limit(10));
    service
        .create_container(environment_id, "container".into())
        .await
        .map_err(anyhow::Error::msg)?;
    ensure!(
        service
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "source",
                &[1, 3, 5, 7]
            )
            .await
            .map_err(anyhow::Error::msg)?
            .bytes_delta
            == 4
    );
    ensure!(
        service
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "destination",
                &[9]
            )
            .await
            .map_err(anyhow::Error::msg)?
            .bytes_delta
            == 1
    );
    let executor = start_with_resource_limits_and_overrides(
        deps,
        &context,
        Arc::new(SharedBlobLimits(limits.clone())),
        TestExecutorOverrides {
            // Observe the first rejected import rather than reconstructing it under retry.
            configure: Some(Arc::new(|config| config.retry.max_attempts = 0)),
            wrap_blob_store_service: Some(Arc::new({
                let service = service.clone();
                move |_| service.clone()
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&environment_id, fixture)
        .store()
        .await?;
    let name = agent_id!("BlobStore", "unknown-mutation");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    let worker = executor
        .production_active_agent(&OwnedAgentId::new(environment_id, &id))
        .await
        .context("cached worker")?
        .primary();

    storage.armed.store(true, Ordering::SeqCst);
    let work = async {
        if copy {
            service
                .copy_object(
                    limits.clone(),
                    environment_id,
                    "container".into(),
                    "source".into(),
                    "container".into(),
                    "destination".into(),
                )
                .await
        } else {
            service
                .write_data(
                    limits.clone(),
                    environment_id,
                    "container",
                    "destination",
                    &[1, 3, 5, 7],
                )
                .await
        }
    };
    let mut prior = Box::pin(std::panic::AssertUnwindSafe(work).catch_unwind());
    ensure!(futures::poll!(&mut prior).is_pending());
    storage.pending.notified().await;
    ensure!(storage.entered.load(Ordering::SeqCst) == 3);
    ensure!(storage.effects.load(Ordering::SeqCst) == 2);
    ensure!(!limits.try_record_blob_storage_delta(3));
    ensure!(service.mutation_failure(environment_id).is_none());
    storage.panic_next_mutation.store(true, Ordering::SeqCst);
    storage.release.add_permits(1);
    let panic = prior
        .await
        .expect_err("the prior service mutation must panic");
    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
    ensure!(message == Some("injected blob mutation panic"));
    let failure = service
        .mutation_failure(environment_id)
        .context("prior panic must publish actual service health before the guest call")?;
    ensure!(failure.environment_id == environment_id);
    ensure!(failure.operation == if copy { "copy_object" } else { "write_data" });
    ensure!(failure.cause == BlobMutationFailureCause::Panicked);
    ensure!(storage.completed.load(Ordering::SeqCst) == 0);
    ensure!(storage.dropped.load(Ordering::SeqCst) == 1);
    ensure!(!limits.try_record_blob_storage_delta(3));

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
        data_value!("container", "destination", vec![2u8, 4, 6])
    };
    let result = executor
        .invoke_and_await_agent_with_key(&component, &name, &key, method, input)
        .await;
    let error = result.expect_err("unavailable mutation must trap, not return a guest Result Err");
    ensure!(error.to_string().contains(&failure.to_string()), "{error}");
    ensure!(
        !error
            .to_string()
            .contains("Blob storage byte limit exceeded"),
        "{error}"
    );

    // Join ordinary physical disposal even if the failed invocation has already unloaded.
    worker.test_stop().await;
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
    ensure!(worker.unload_succeeded_for_test());

    let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    assert_incomplete_mutation(&entries, &key, method, copy, &failure.to_string())?;
    ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
    ensure!(
        storage.entered.load(Ordering::SeqCst) == 3,
        "guest admission entered the backend"
    );
    ensure!(storage.effects.load(Ordering::SeqCst) == 2);
    ensure!(storage.completed.load(Ordering::SeqCst) == 0);
    ensure!(storage.dropped.load(Ordering::SeqCst) == 1);
    ensure!(
        !limits.try_record_blob_storage_delta(3),
        "unknown reservation received credit"
    );
    ensure!(service.mutation_failure(environment_id).as_ref() == Some(&failure));
    for (object, expected) in [("source", vec![1, 3, 5, 7]), ("destination", vec![9])] {
        ensure!(
            storage
                .get_raw(
                    "test",
                    "read",
                    BlobStorageNamespace::CustomStorage { environment_id },
                    &Path::new("container").join(object),
                )
                .await?
                == Some(expected)
        );
    }
    Ok(())
}

fn assert_incomplete_mutation(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    method: &str,
    copy: bool,
    failure: &str,
) -> anyhow::Result<()> {
    let mut current = false;
    let mut accepted = 0;
    let mut started = 0;
    let mut finished = 0;
    let mut errors = 0;
    let mut calls = Vec::new();
    let boundary = if copy {
        "::copy_object"
    } else {
        "::write_data"
    };
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::PendingAgentInvocation(pending) => {
                if let PublicAgentInvocation::AgentMethodInvocation(call) = &pending.invocation
                    && &call.idempotency_key == key
                {
                    ensure!(call.method_name == method);
                    accepted += 1;
                }
            }
            PublicOplogEntry::AgentInvocationStarted(start) => {
                current = matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(call) if &call.idempotency_key == key && call.method_name == method);
                started += usize::from(current);
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current => finished += 1,
            PublicOplogEntry::Error(error) if current => {
                ensure!(error.error.contains(failure), "{error:?}");
                errors += 1;
            }
            PublicOplogEntry::Start(call) if current && call.function_name.ends_with(boundary) => {
                calls.push(entry.oplog_index);
            }
            _ => {}
        }
    }
    ensure!(accepted == 1 && started == 1 && finished == 0 && errors == 1);
    ensure!(
        calls.len() == 1,
        "expected one incomplete mutation Start: {calls:?}"
    );
    ensure!(
        !entries.iter().any(|entry| match &entry.entry {
            PublicOplogEntry::End(end) => end.start_index == calls[0],
            PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == calls[0],
            _ => false,
        }),
        "unavailable admission manufactured a normal host terminal"
    );
    Ok(())
}

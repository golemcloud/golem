use super::*;
use pretty_assertions::assert_eq;
use test_r::{test, timeout};

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn deletion_retry_preserves_actual_filesystem_failure_until_verified_cleanup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let root = tempfile::tempdir()?;
    let root_path = root.path().to_path_buf();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.suspend.suspend_after = Duration::from_secs(3600);
                config.filesystem_storage.mode = FilesystemStorageMode::Directory {
                    root: root_path.clone().into(),
                };
                config.filesystem_storage.cleanup_retry.max_attempts = 1;
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_key!("Counter", "delete-native-cleanup-failure");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "increment", data_value!())
        .await?;
    let owned = worker_id.clone();
    let mut hook = DeletionStageHook::new(owned.clone(), None, None);
    hook.cleanup_gate = true;
    let hook = Arc::new(hook);
    executor.set_worker_deletion_hook(hook.clone());
    let first_executor = executor.clone();
    let first_id = worker_id.clone();
    let first = tokio::spawn(async move { first_executor.delete_worker(&first_id).await });
    hook.cleanup_entered.acquire().await?.forget();
    let type_path = root
        .path()
        .join(context.default_environment_id.to_string())
        .join(format!("{:08x}", worker_id.agent_id.type_tag()));
    let moved = type_path.with_extension("moved");
    std::fs::rename(&type_path, &moved)?;
    std::fs::write(&type_path, b"block directory traversal")?;
    hook.cleanup_release.add_permits(1);
    let first_error = first.await?.unwrap_err().to_string();
    assert!(first_error.contains("Not a directory"), "{first_error}");
    let retry_error = executor
        .delete_worker(&worker_id)
        .await
        .unwrap_err()
        .to_string();
    assert!(retry_error.contains("Not a directory"), "{retry_error}");
    assert!(executor.worker_is_cached(&owned).await);
    executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(hook.calls(WorkerDeletionStage::DurableStateRemoved), 0);

    std::fs::remove_file(&type_path)?;
    std::fs::rename(&moved, &type_path)?;
    executor.delete_worker(&worker_id).await?;
    assert_eq!(hook.calls(WorkerDeletionStage::ExecutionFenced), 1);
    assert_eq!(hook.calls(WorkerDeletionStage::BarriersClosed), 1);
    assert_eq!(hook.calls(WorkerDeletionStage::RuntimeStopped), 3);
    assert_eq!(hook.calls(WorkerDeletionStage::DurableStateRemoved), 1);
    assert!(!executor.worker_is_cached(&owned).await);
    assert!(executor.get_worker_metadata(&worker_id).await.is_err());
    Ok(())
}

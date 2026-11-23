use super::*;
use pretty_assertions::assert_eq;
use test_r::{test, timeout};

#[test]
#[timeout("3m")]
async fn native_failed_cleanup_releases_execution_refuses_startup_and_repairs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use futures::FutureExt;
    use golem_worker_executor::services::{HasActiveAgents, HasOplog};
    use std::os::unix::fs::PermissionsExt;
    struct RestorePermissions(std::path::PathBuf, std::fs::Permissions);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, self.1.clone());
        }
    }
    for later in [false, true] {
        let root = tempfile::tempdir()?;
        let storage_root = root.path().to_path_buf();
        let context = TestContext::new(last_unique_id);
        let shutdown = CancellationToken::new();
        let metering = ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        };
        let registry = MutableResourceLimitsRegistry::new(available_quota_policy());
        let mut account_limits = registry.current_limits();
        account_limits.max_concurrent_agents_per_executor = 1;
        let registry = Arc::new(MutableResourceLimitsRegistry {
            limits: std::sync::Mutex::new(account_limits),
            ..registry
        });
        let limits = ResourceLimitsGrpc::new(
            registry,
            Duration::from_secs(3600),
            Duration::ZERO,
            metering,
            shutdown.clone(),
        );
        let configure_root = storage_root.clone();
        let executor = start_with_resource_limits_and_configure(deps, &context, limits.clone(),
            Arc::new(move |config| {
                config.resource_usage_metering = metering;
                config.filesystem_storage.mode = golem_worker_executor::services::golem_config::FilesystemStorageMode::Directory {
                    root: configure_root.clone().into_boxed_path(),
                };
            }),
        ).await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_counters)
            .store()
            .await?;
        let seed_id = executor
            .start_agent(&component.id, agent_key!("Counter", "native-cleanup-seed"))
            .await?;
        let seed = executor
            .production_active_agent(&seed_id.clone())
            .await
            .unwrap()
            .primary();
        wait_for_invocation_pair(&executor, &seed_id.agent_id, OplogIndex::INITIAL).await?;
        let entry = limits.initialize_account(context.account_id).await?;
        assert_eq!(entry.max_concurrent_agents_per_executor(), 1);
        let account = seed
            .active_agents()
            .register_account_concurrency(context.account_id, entry)
            .await;
        // A real account lease proves the seed has released capacity before the target starts.
        let seed_permit =
            tokio::time::timeout(Duration::from_secs(10), account.acquire(seed_id.agent_id))
                .await?;
        drop(seed_permit);
        let name = agent_key!("Counter", format!("native-cleanup-{later}"));
        let id = crate::agent::keyed_agent_id(component.environment_id, &name).agent_id;
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let worker = Worker::get_or_create_suspended_with_freshness(
            seed.all(),
            &owned,
            None,
            vec![],
            None,
            None,
            &InvocationContextStack::fresh(),
            callers::anonymous(),
            golem_common::model::agent::InvocationFreshnessDisposition::MayExist,
            golem_worker_executor::worker::CreateIntent::Invocation {
                owner: golem_worker_executor::worker::NewOwner::keyed(component.id, name.clone()),
                idempotency_key: None,
            },
        )
        .await?;
        if later {
            Worker::start_if_needed(worker.clone()).await?;
            wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.eviction_class().await.is_none() {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
        }
        let lane = worker.owner_execution().lane();
        let prebody = lane.acquire_exclusive().await;
        if later {
            executor
                .invoke_agent(&component, &name, "increment", data_value!())
                .await?;
        } else {
            Worker::start_if_needed(worker.clone()).await?;
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            while lane.metadata().active_invocation_count == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let directory = storage_root
            .join(context.default_environment_id.to_string())
            .join(format!("{:08x}", id.type_tag()))
            .join(golem_service_base::storage::blob::agent_path_segment(&id));
        assert!(directory.is_dir());
        let contender_id = crate::agent::keyed_agent_id(
            component.environment_id,
            &agent_key!("Counter", format!("native-cleanup-contender-{later}")),
        )
        .agent_id;
        assert_ne!(contender_id, id);
        let mut contender = Box::pin(account.acquire(contender_id));
        assert!(
            (&mut contender).now_or_never().is_none(),
            "the real FIFO contender must wait while Started retains the target's only account permit"
        );
        let parent = directory.parent().unwrap().to_path_buf();
        let permissions =
            RestorePermissions(parent.clone(), std::fs::metadata(&parent)?.permissions());
        // Keep reads available. Only removing the runtime root from its parent must fail.
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500))?;
        let denied = std::fs::create_dir(parent.join("dac-proof")).unwrap_err();
        assert_eq!(
            denied.kind(),
            std::io::ErrorKind::PermissionDenied,
            "this native witness requires ordinary DAC enforcement, not a privileged bypass"
        );
        assert!(
            matches!(
                &*worker
                    .owner_runtime_resources()
                    .execution_status()
                    .read()
                    .unwrap(),
                golem_worker_executor::model::ExecutionStatus::Suspended { .. }
            ),
            "Started persistence precedes entering the primary body"
        );
        let kind = InterruptKind::Suspend(Timestamp::now_utc());
        let receipt = worker.set_interrupting(kind).await?;
        assert!(
            receipt.is_none(),
            "the Suspended prebody phase promises no public stop receipt"
        );
        tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await??;
        assert_eq!(
            worker.selected_owner_failure_for_test().await,
            Some(ToolOwnerFailureMetadata::Lifecycle)
        );
        assert!((&mut contender).now_or_never().is_none());
        drop(prebody);
        let error = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Err(error) = worker.await_ready_to_process_commands().await {
                    break error;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(
            error.to_string().contains(&denied.to_string()),
            "actual runtime deletion must fail with the demonstrated DAC denial: {error}"
        );
        assert!(worker.retained_cleanup_for_test().await.is_err());
        assert!(directory.exists());
        let contender_permit =
            tokio::time::timeout(Duration::from_secs(10), contender.as_mut()).await?;
        assert!(
            directory.exists(),
            "execution capacity must return while the failed filesystem root still exists"
        );
        assert!(Worker::start_if_needed(worker.clone()).await.is_err());
        assert!(worker.await_ready_to_process_commands().await.is_err());
        assert_eq!(
            worker.await_ready_to_process_commands().await.unwrap_err(),
            error
        );
        let history = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await?;
        use golem_common::model::oplog::OplogEntry;
        assert_eq!(
            history
                .values()
                .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
                .count(),
            1
        );
        assert!(!history.values().any(|entry| matches!(
            entry,
            OplogEntry::Interrupted { .. } | OplogEntry::Error { .. }
        )));
        assert!(directory.exists());
        drop(permissions);
        tokio::time::timeout(Duration::from_secs(30), executor.delete_worker(&owned)).await??;
        worker.retained_cleanup_for_test().await?;
        worker.join_accepted_stops_for_test().await?;
        assert!(!directory.exists());
        assert!(Worker::start_if_needed(worker.clone()).await.is_err());
        drop(contender_permit);
        shutdown.cancel();
    }
    Ok(())
}

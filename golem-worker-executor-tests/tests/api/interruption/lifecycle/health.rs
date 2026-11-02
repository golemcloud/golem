use super::*;
use golem_common::model::Timestamp;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::UsesAllDeps;
use golem_worker_executor::worker::Worker;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

#[test]
#[timeout("2m")]
async fn unavailable_registry_policy_stops_pending_tcp_as_infrastructure_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for ephemeral in [false, true] {
        let context = TestContext::new(last_unique_id);
        let mut policy = available_quota_policy();
        policy.mode = MonthlyUsageMode::AllowOverage;
        let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
        let metering = ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        };
        let shutdown = CancellationToken::new();
        let limits = ResourceLimitsGrpc::new(
            registry.clone(),
            Duration::from_secs(3600),
            Duration::ZERO,
            metering,
            shutdown.clone(),
        );
        let executor = start_with_resource_limits_and_configure(
            deps,
            &context,
            limits.clone(),
            Arc::new(move |config| {
                config.agent_sockets.enabled = true;
                config.resource_usage_metering = metering;
            }),
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let seed_id = executor
            .start_agent(&component.id, agent_id!("Networking", "policy-health-seed"))
            .await?;
        let seed = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
            .await
            .unwrap()
            .primary();
        wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
        let name = if ephemeral {
            agent_id!("EphemeralNetworking", "policy-health")
        } else {
            agent_id!("Networking", "policy-health")
        };
        let key = IdempotencyKey::fresh();
        let physical = if ephemeral {
            name.clone()
                .with_ephemeral_invocation_phantom(&key)
                .unwrap()
        } else {
            name.clone()
        };
        let id = AgentId::from_agent_id(component.id, &physical).unwrap();
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let worker = Worker::get_or_create_suspended(
            seed.all(),
            &owned,
            None,
            vec![],
            None,
            None,
            &InvocationContextStack::fresh(),
            Principal::anonymous(),
        )
        .await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let invocation = tokio::spawn({
            let executor = executor.clone();
            let component = component.clone();
            let name = name.clone();
            let key = key.clone();
            async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &name,
                        &key,
                        "tcp_collect_p3",
                        data_value!(port),
                    )
                    .await
            }
        });
        let (mut socket, _) =
            tokio::time::timeout(Duration::from_secs(10), listener.accept()).await??;
        registry.mark_monthly_policy_unavailable();
        limits.run_batch_for_test().await;
        let mut byte = [0];
        let closed = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut byte)).await?;
        assert!(matches!(closed, Ok(0) | Err(_)));
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        if ephemeral {
            tokio::time::timeout(Duration::from_secs(10), async {
                while executor.worker_is_cached(&owned).await
                    || crate::oplog_sweep::ephemeral_layer_rows(deps, &context, &id).await? != 0
                {
                    tokio::task::yield_now().await;
                }
                anyhow::Ok(())
            })
            .await??;
            assert_eq!(
                executor.get_worker_metadata(&id).await?.status,
                AgentStatus::Failed
            );
        } else {
            worker.join_accepted_stops_for_test().await?;
            worker.retained_cleanup_for_test().await?;
            worker.await_ready_to_process_commands().await?;
        }
        let error = invocation.await?.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Monthly enforcement unavailable"),
            "{error}"
        );
        assert!(
            !error.to_string().contains("monthly memory exhausted"),
            "{error}"
        );
        if !ephemeral {
            use tokio::io::AsyncWriteExt;
            let cached = executor
                .production_active_agent(&owned)
                .await
                .unwrap()
                .primary();
            assert!(Arc::ptr_eq(&cached, &worker));
            registry.restore_monthly_policy(policy);
            limits.run_batch_for_test().await;
            assert_eq!(
                limits
                    .initialized_policy_revision_for_test(context.account_id)
                    .await,
                Some(registry.current_limits().monthly_policy_revision),
                "the restored policy must be applied before testing cached-Worker recovery"
            );
            let recovery_key = IdempotencyKey::fresh();
            let mut recovered = Box::pin(executor.invoke_and_await_agent_with_key(
                &component,
                &name,
                &recovery_key,
                "tcp_collect_p3",
                data_value!(port),
            ));
            let (mut peer, _) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::select! {
                    result = &mut recovered => anyhow::bail!(
                        "recovered cached Worker returned before reconnect: {result:?}"
                    ),
                    peer = listener.accept() => Ok(peer?),
                }
            })
            .await??;
            peer.write_all(b"recovered").await?;
            peer.shutdown().await?;
            let output = tokio::time::timeout(Duration::from_secs(10), recovered).await??;
            assert_eq!(
                output.into_typed::<Result<String, String>>()?,
                Ok("recovered".to_string())
            );
            let cached = executor
                .production_active_agent(&owned)
                .await
                .unwrap()
                .primary();
            assert!(Arc::ptr_eq(&cached, &worker));
        }
        shutdown.cancel();
    }
    Ok(())
}

#[cfg(unix)]
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
            .start_agent(&component.id, agent_id!("Counter", "native-cleanup-seed"))
            .await?;
        let seed = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
            .await
            .unwrap()
            .primary();
        wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
        let entry = limits.initialize_account(context.account_id).await?;
        assert_eq!(entry.max_concurrent_agents_per_executor(), 1);
        let account = seed
            .active_agents()
            .register_account_concurrency(context.account_id, entry)
            .await;
        // A real account lease proves the seed has released capacity before the target starts.
        let seed_permit =
            tokio::time::timeout(Duration::from_secs(10), account.acquire(seed_id.clone())).await?;
        drop(seed_permit);
        let name = agent_id!("Counter", format!("native-cleanup-{later}"));
        let id = AgentId::from_agent_id(component.id, &name).unwrap();
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let worker = Worker::get_or_create_suspended(
            seed.all(),
            &owned,
            None,
            vec![],
            None,
            None,
            &InvocationContextStack::fresh(),
            Principal::anonymous(),
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
        let mut started = executor.gate_next_invocation_started_append(&id).await;
        if later {
            executor
                .invoke_agent(&component, &name, "increment", data_value!())
                .await?;
        } else {
            Worker::start_if_needed(worker.clone()).await?;
        }
        tokio::time::timeout(Duration::from_secs(10), started.entered()).await?;
        let directory = storage_root
            .join(context.default_environment_id.to_string())
            .join(component.id.to_string())
            .join(golem_service_base::storage::blob::agent_path_segment(&id));
        assert!(directory.is_dir());
        let contender_id = AgentId::from_agent_id(
            component.id,
            &agent_id!("Counter", format!("native-cleanup-contender-{later}")),
        )
        .unwrap();
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
        drop(started);
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
        tokio::time::timeout(Duration::from_secs(30), executor.delete_worker(&id)).await??;
        worker.retained_cleanup_for_test().await?;
        worker.join_accepted_stops_for_test().await?;
        assert!(!directory.exists());
        assert!(Worker::start_if_needed(worker.clone()).await.is_err());
        drop(contender_permit);
        shutdown.cancel();
    }
    Ok(())
}

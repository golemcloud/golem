use super::*;
use anyhow::Context as _;
use golem_common::model::Timestamp;
use golem_common::model::account::AccountId;
use golem_common::model::oplog::OplogEntry;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_limits::ResourceUsageFlusher;
use golem_worker_executor::services::{HasEvents, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use golem_worker_executor::worker::instance::OwnerRuntimeResources;
use golem_worker_executor::workerctx::default::Context;
use golem_worker_executor_test_utils::AgentInitializationEnqueueGateHandle;
use pretty_assertions::assert_eq;
use std::sync::Mutex;
use test_r::test;
use tokio::io::AsyncReadExt;

#[derive(Debug)]
struct PanickingUsageFlusher {
    unwound: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    closing_owner: Option<(std::sync::Weak<Worker<Context>>, std::path::PathBuf)>,
}

impl ResourceUsageFlusher for PanickingUsageFlusher {
    fn flush_usage(&self) {
        if let Some((owner, _)) = &self.closing_owner {
            let Some(owner) = owner.upgrade() else {
                return;
            };
            if !owner.monthly_horizon_was_registered() || owner.monthly_window_active_for_test() {
                return;
            }
        }
        let unwound = self.unwound.lock().unwrap().take();
        if let Some(unwound) = unwound {
            struct Notice(Option<tokio::sync::oneshot::Sender<()>>);
            impl Drop for Notice {
                fn drop(&mut self) {
                    if std::thread::panicking() {
                        let _ = self.0.take().unwrap().send(());
                    }
                }
            }
            let _notice = Notice(Some(unwound));
            if let Some((_, root)) = &self.closing_owner {
                assert!(
                    root.is_dir(),
                    "initial runtime root must exist at the close-time fault"
                );
            }
            panic!("injected monthly monitor panic");
        }
    }
}

async fn fail_owner_usage_flusher(
    resources: Arc<OwnerRuntimeResources>,
    limits: &Arc<ResourceLimitsGrpc>,
    registry: &MutableResourceLimitsRegistry,
    account: AccountId,
) -> anyhow::Result<(Arc<dyn ResourceUsageFlusher>, tokio::task::JoinHandle<()>)> {
    let (unwound, notice) = tokio::sync::oneshot::channel();
    let source: Arc<dyn ResourceUsageFlusher> = Arc::new(PanickingUsageFlusher {
        unwound: Mutex::new(Some(unwound)),
        closing_owner: None,
    });
    resources.register_resource_usage_flusher(Arc::downgrade(&source));
    registry.set_policy(registry.current_limits().monthly_policy);
    let refresh = applied_refresh(limits, registry, account).await?;
    tokio::time::timeout(Duration::from_secs(10), notice)
        .await
        .context("usage flusher did not unwind")??;
    Ok((source, refresh))
}

async fn fail_growing_actor(
    mut growth: AgentInitializationEnqueueGateHandle,
    owner: &OwnedAgentId,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), growth.entered())
        .await
        .context("actual lifecycle memory-growth append did not reach the provider")?;
    fail_entered_growing_actor(growth, owner).await
}

async fn fail_entered_growing_actor(
    growth: AgentInitializationEnqueueGateHandle,
    owner: &OwnedAgentId,
) -> anyhow::Result<()> {
    assert!(matches!(growth.entry(), OplogEntry::GrowMemory { delta, .. } if delta > 0));
    let mut fence = crate::entity_fence_waits::lifecycle_fence(owner);
    // The existing FailOplogAdd1OnGrowMemory fixture fails this actual append after release.
    // The ordinary lifecycle job panics, and the normal monitor selects its own failure stop.
    drop(growth);
    tokio::time::timeout(Duration::from_secs(10), fence.entered())
        .await
        .context("monitor failure did not reach ordinary lifecycle fencing")??;
    Ok(())
}

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
        let acquisitions = worker.permit_acquisitions_for_test();
        let mut acceptances =
            crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);
        registry.mark_monthly_policy_unavailable();
        limits.run_batch_for_test().await;
        let mut byte = [0];
        let closed = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut byte)).await?;
        assert!(matches!(closed, Ok(0) | Err(_)));
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        worker.join_accepted_stops_for_test().await?;
        worker.retained_cleanup_for_test().await?;
        worker.await_ready_to_process_commands().await?;
        assert!(worker.unload_succeeded_for_test());
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
        assert!(
            matches!(
                acceptances.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "no quota stop may be fabricated"
        );
        assert!(!worker.concurrent_agent_permit_is_held().await);
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
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

#[test]
#[timeout("3m")]
async fn quota_monitor_cleanup_health_keeps_results_and_startup_refusal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    // Initial idle precedes guest work. The actor case retains a real guest-growth append
    // across completed construction and the later allocation before releasing its failure.
    for (later, failure) in [(false, "monitor"), (true, "monitor"), (true, "actor")] {
        let storage = tempfile::tempdir()?;
        let storage_root = storage.path().to_path_buf();
        let context = TestContext::new(last_unique_id);
        let metering = ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        };
        let shutdown = CancellationToken::new();
        let registry = Arc::new(MutableResourceLimitsRegistry::new(available_quota_policy()));
        let limits = ResourceLimitsGrpc::new(
            registry.clone(),
            Duration::from_secs(3600),
            Duration::ZERO,
            metering,
            shutdown.clone(),
        );
        let configure_root = storage_root.clone();
        let executor = start_with_resource_limits_and_configure(
            deps,
            &context,
            limits.clone(),
            Arc::new(move |config| {
                config.resource_usage_metering = metering;
                config.filesystem_storage.mode = golem_worker_executor::services::golem_config::FilesystemStorageMode::Directory {
                    root: configure_root.clone().into_boxed_path(),
                };
            }),
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_counters)
            .store()
            .await?;
        let case = if failure == "actor" {
            format!("health-{later}-{failure}-FailOplogAdd1OnGrowMemory")
        } else {
            format!("health-{later}-{failure}")
        };
        let name = agent_id!("Counter", case);
        let id = AgentId {
            component_id: component.id,
            agent_id: name.to_string(),
        };
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let directory = storage_root
            .join(context.default_environment_id.to_string())
            .join(component.id.to_string())
            .join(golem_service_base::storage::blob::agent_path_segment(&id));
        let seed_id = executor
            .start_agent(&component.id, agent_id!("Counter", "health-seed"))
            .await?;
        let seed = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
            .await
            .unwrap()
            .primary();
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
        let mut events = worker.events().subscribe();
        let initial_prefix = executor.stored_oplog(&id).await;
        // The fault fixture can hit guest construction growth, so intercept that actual
        // append before starting the constructor, not only before explicit allocation.
        let mut actor_growth = if failure == "actor" {
            Some(executor.gate_next_memory_growth_append(&id).await)
        } else {
            None
        };
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
        let prior_acquisitions = worker.permit_acquisitions_for_test();
        let prior_generation = worker.observed_creation_count();
        if let Some(growth) = &mut actor_growth {
            tokio::time::timeout(Duration::from_secs(10), growth.entered())
                .await
                .context("guest construction growth did not reach its actual append")?;
            assert!(matches!(growth.entry(), OplogEntry::GrowMemory { delta, .. } if delta > 0));
        }
        let mut finished = if later && failure != "actor" {
            Some(executor.gate_next_invocation_finished_append(&id).await)
        } else {
            None
        };
        let key = IdempotencyKey::fresh();
        let mut invoke = None;
        let mut initial_failure_source = None;
        let mut initial_attempt = None;
        if later {
            invoke = Some(tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
                let executor = executor.clone();
                let name = name.clone();
                let component = component.clone();
                let key = key.clone();
                async move {
                    let (method, input) = if failure == "actor" {
                        // Retain real guest allocation so the lifecycle actor has an actual
                        // positive growth append, rather than inventing a host-side hint.
                        ("allocate_memory", data_value!(4 * 1024 * 1024u32))
                    } else {
                        ("increment", data_value!())
                    };
                    executor
                        .invoke_and_await_agent_with_key(&component, &name, &key, method, input)
                        .await
                }
            })));
        } else {
            let (unwound, notice) = tokio::sync::oneshot::channel();
            let source: Arc<dyn ResourceUsageFlusher> = Arc::new(PanickingUsageFlusher {
                unwound: Mutex::new(Some(unwound)),
                closing_owner: Some((Arc::downgrade(&worker), directory.clone())),
            });
            worker
                .owner_runtime_resources()
                .register_resource_usage_flusher(Arc::downgrade(&source));
            initial_failure_source = Some(source);
            initial_attempt = Some(
                Worker::start_if_needed(worker.clone())
                    .await?
                    .context("initial startup attempt was not created")?,
            );
            tokio::time::timeout(Duration::from_secs(10), notice)
                .await
                .context("initial monitor did not unwind while closing its real window")??;
        }
        let mut rpc_commit = None;
        let mut actor_probe = None;
        let probe_key = IdempotencyKey::fresh();
        let prefix = if failure == "actor" {
            let completed = tokio::time::timeout(
                Duration::from_secs(10),
                invoke
                    .take()
                    .expect("actor case must invoke guest allocation"),
            )
            .await???;
            assert_eq!(completed.into_typed::<u32>()?, 1);
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.concurrent_agent_permit_is_held().await {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .context("completed allocation did not release its real execution window")?;
            // Keep the live monitor active on a subsequent real RPC. The earlier allocation
            // result is already published; neither idle teardown nor replay is paused.
            let mut commit = executor
                .gate_next_rpc_commit(&id, golem_worker_executor_test_utils::RpcCheckpoint::Start)
                .await;
            actor_probe = Some(tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
                let executor = executor.clone();
                let component = component.clone();
                let name = name.clone();
                let probe_key = probe_key.clone();
                async move {
                    executor
                        .invoke_and_await_agent_with_key(
                            &component,
                            &name,
                            &probe_key,
                            "increment_through_rpc",
                            data_value!(),
                        )
                        .await
                }
            })));
            tokio::time::timeout(Duration::from_secs(10), commit.committed())
                .await
                .context("live RPC did not reach its actual Start commit")?;
            rpc_commit = Some(commit);
            assert!(directory.is_dir());
            executor.stored_oplog(&id).await
        } else if let Some(finished) = &mut finished {
            tokio::time::timeout(Duration::from_secs(10), finished.entered())
                .await
                .context("successful result did not reach its actual Finished append")?;
            assert!(matches!(
                finished.entry(),
                OplogEntry::AgentInvocationFinished { .. }
            ));
            assert!(
                directory.is_dir(),
                "selected success must still own its actual runtime root"
            );
            executor.stored_oplog(&id).await
        } else {
            initial_prefix
        };
        let generation = worker.observed_creation_count();
        let acquisitions = worker.permit_acquisitions_for_test();
        assert_eq!(
            acquisitions,
            prior_acquisitions + 1 + usize::from(failure == "actor")
        );
        assert_eq!(generation, prior_generation + u64::from(!later));
        if later {
            assert!(worker.concurrent_agent_permit_is_held().await);
        }
        let actor_interrupt = (failure == "actor").then(|| worker.await_interrupt_for_test());
        let mut failure_source = None;
        let expected = match failure {
            "monitor" if !later => "Monthly monitor panicked: injected monthly monitor panic",
            "monitor" => {
                failure_source = Some(
                    fail_owner_usage_flusher(
                        worker.owner_runtime_resources(),
                        &limits,
                        &registry,
                        context.account_id,
                    )
                    .await?,
                );
                "Monthly monitor panicked: injected monthly monitor panic"
            }
            "actor" => {
                fail_entered_growing_actor(actor_growth.take().unwrap(), &owned).await?;
                assert!(matches!(
                    tokio::time::timeout(Duration::from_secs(10), actor_interrupt.unwrap()).await?,
                    InterruptKind::Suspend(_)
                ));
                "Monthly monitor lost the lifecycle actor"
            }
            _ => unreachable!(),
        };
        // Release actual dependency calls only after the monitor selected its failure.
        drop(finished);
        drop(rpc_commit);
        let error = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Err(error) = worker.await_ready_to_process_commands().await {
                    break error;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(
            error.to_string().contains(expected),
            "{later}/{failure}: {error}"
        );
        assert!(
            worker
                .retained_cleanup_for_test()
                .await
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert_eq!(
            tokio::fs::symlink_metadata(&directory)
                .await
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound,
            "physical deletion must succeed independently of retained health"
        );
        // Later failures close their live monitor during unload, so its aggregate result
        // includes the monitor failure even though physical deletion above succeeded.
        assert_eq!(worker.unload_succeeded_for_test(), !later);
        assert!(!worker.concurrent_agent_permit_is_held().await);
        assert!(worker.eviction_class().await.is_none());
        assert_eq!(worker.observed_creation_count(), generation);
        let retry = Worker::start_if_needed(worker.clone()).await.unwrap_err();
        assert!(retry.to_string().contains(expected), "{retry}");
        assert_eq!(worker.observed_creation_count(), generation);
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
        if !later {
            let loaded = events
                .wait_for(|event| match event {
                    Event::WorkerLoaded {
                        agent_id,
                        start_attempt,
                        result,
                        ..
                    } if agent_id == &id && Some(*start_attempt) == initial_attempt => {
                        Some(result.clone())
                    }
                    _ => None,
                })
                .await?;
            assert!(loaded.unwrap_err().to_string().contains(expected));
        }
        if let Some(invoke) = invoke {
            assert_eq!(invoke.await??.into_typed::<u32>()?, 1);
        }
        if let Some(probe) = actor_probe {
            assert!(
                tokio::time::timeout(Duration::from_secs(10), probe)
                    .await??
                    .is_err()
            );
        }
        if let Some((source, refresh)) = failure_source {
            refresh.await?;
            drop(source);
        }
        drop(initial_failure_source);
        let recorded = executor.stored_oplog(&id).await;
        assert!(recorded.len() >= prefix.len());
        assert_eq!(&recorded[..prefix.len()], prefix.as_slice());
        assert_eq!(
            recorded
                .iter()
                .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
                .count(),
            2 * usize::from(later) + usize::from(failure == "actor")
        );
        assert_eq!(
            recorded
                .iter()
                .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
                .count(),
            2 * usize::from(later)
        );
        assert!(!recorded.iter().any(|entry| matches!(
            entry,
            OplogEntry::Error { .. }
                | OplogEntry::Interrupted { .. }
                | OplogEntry::Suspend { .. }
                | OplogEntry::Jump { .. }
        )));
        if later {
            assert_eq!(recorded.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
        }
        if failure == "actor" {
            assert_eq!(recorded.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &probe_key)).count(), 1);
        }
        shutdown.cancel();
    }
    Ok(())
}

#[cfg(unix)]
#[test]
#[timeout("3m")]
async fn native_failed_cleanup_releases_execution_refuses_startup_and_repairs_without_new_generation(
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
                while worker.concurrent_agent_permit_is_held().await {
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
        let generation = worker.observed_creation_count();
        let acquisitions = worker.permit_acquisitions_for_test();
        assert!(worker.concurrent_agent_permit_is_held().await);
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
        assert!(worker.concurrent_agent_permit_is_held().await);
        assert_eq!(worker.observed_creation_count(), generation);
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
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
        assert_eq!(worker.observed_creation_count(), generation);
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
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
        assert!(!worker.concurrent_agent_permit_is_held().await);
        assert_eq!(worker.observed_creation_count(), generation);
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
        assert!(Worker::start_if_needed(worker.clone()).await.is_err());
        drop(contender_permit);
        shutdown.cancel();
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn quota_monitor_failure_stops_pending_silent_tcp_without_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for actor in [false, true] {
        let context = TestContext::new(last_unique_id);
        let metering = ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        };
        let shutdown = CancellationToken::new();
        let registry = Arc::new(MutableResourceLimitsRegistry::new(available_quota_policy()));
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
        let case = if actor {
            "monitor-failure-actor-FailOplogAdd1OnGrowMemory".to_string()
        } else {
            "monitor-failure-monitor".to_string()
        };
        let name = agent_id!("Networking", case);
        let expected_id =
            AgentId::from_agent_id(component.id, &name).map_err(anyhow::Error::msg)?;
        let mut actor_growth = if actor {
            Some(executor.gate_next_memory_growth_append(&expected_id).await)
        } else {
            None
        };
        let id = executor.start_agent(&component.id, name.clone()).await?;
        assert_eq!(id, expected_id);
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let worker = executor
            .production_active_agent(&owned)
            .await
            .unwrap()
            .primary();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let invocation = tokio::spawn({
            let executor = executor.clone();
            async move {
                executor
                    .invoke_and_await_agent(&component, &name, "tcp_collect_p3", data_value!(port))
                    .await
            }
        });
        let (mut socket, _) = listener.accept().await?;
        let generation = worker.observed_creation_count();
        let acquisitions = worker.permit_acquisitions_for_test();
        let mut failure_source = None;
        let expected = if actor {
            fail_growing_actor(actor_growth.take().unwrap(), &owned).await?;
            "Monthly monitor lost the lifecycle actor"
        } else {
            failure_source = Some(
                fail_owner_usage_flusher(
                    worker.owner_runtime_resources(),
                    &limits,
                    &registry,
                    context.account_id,
                )
                .await?,
            );
            "Monthly monitor panicked: injected monthly monitor panic"
        };
        let mut byte = [0];
        assert!(matches!(socket.read(&mut byte).await, Ok(0) | Err(_)));
        let error = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Err(error) = worker.await_ready_to_process_commands().await {
                    break error;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(error.to_string().contains(expected), "{error}");
        assert!(
            worker
                .retained_cleanup_for_test()
                .await
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert!(!worker.concurrent_agent_permit_is_held().await);
        assert_eq!(worker.observed_creation_count(), generation);
        assert!(
            Worker::start_if_needed(worker.clone())
                .await
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
        assert!(invocation.await?.is_err());
        if let Some((source, refresh)) = failure_source {
            refresh.await?;
            drop(source);
        }
        shutdown.cancel();
    }
    Ok(())
}

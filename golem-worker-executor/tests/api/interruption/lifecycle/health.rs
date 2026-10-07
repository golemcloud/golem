use super::*;
use golem_common::model::Timestamp;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::{HasEvents, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

#[test]
#[timeout("3m")]
async fn quota_monitor_cleanup_health_survives_initial_and_later_idle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for later in [false, true] {
        for failure in ["monitor", "actor", "settlement", "driver"] {
            let context = TestContext::new(last_unique_id);
            let metering = ResourceUsageMeteringConfig {
                compute: false,
                memory: true,
                filesystem: false,
            };
            let shutdown = CancellationToken::new();
            let limits = ResourceLimitsGrpc::new(
                Arc::new(MutableResourceLimitsRegistry::new(available_quota_policy())),
                Duration::from_secs(3600),
                Duration::ZERO,
                metering,
                shutdown.clone(),
            );
            let executor = start_with_resource_limits_and_configure(
                deps,
                &context,
                limits,
                Arc::new(move |config| config.resource_usage_metering = metering),
            )
            .await?;
            let component = executor
                .component_dep(&context.default_environment_id, agent_counters)
                .store()
                .await?;
            let name = agent_id!("Counter", format!("health-{later}-{failure}"));
            let id = AgentId {
                component_id: component.id,
                agent_id: name.to_string(),
            };
            let owned = OwnedAgentId::new(context.default_environment_id, &id);
            let seed_id = executor
                .start_agent(&component.id, agent_id!("Counter", "health-seed"))
                .await?;
            let seed = executor
                .production_active_agent(&OwnedAgentId::new(
                    context.default_environment_id,
                    &seed_id,
                ))
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
            let (entered, release_idle) = worker.pause_idle_close_for_test();
            let mut invoke = None;
            if later {
                invoke = Some(tokio::spawn({
                    let executor = executor.clone();
                    let name = name.clone();
                    let component = component.clone();
                    async move {
                        executor
                            .invoke_and_await_agent(&component, &name, "increment", data_value!())
                            .await
                    }
                }));
            } else {
                Worker::start_if_needed(worker.clone()).await?;
            }
            entered.await?;
            let generation = worker.resident_generation_for_test();
            let acquisitions = worker.permit_acquisitions_for_test();
            let attempt = worker.startup_attempt_for_test();
            if !later {
                assert!(matches!(attempt, Ok(Some(_))));
            }
            let mut events = worker.events().subscribe();
            assert!(worker.concurrent_agent_permit_is_held().await);
            let expected = match failure {
                "monitor" => {
                    worker.panic_monthly_monitor_for_test().await;
                    "Monthly monitor panicked: injected monthly monitor panic"
                }
                "actor" => {
                    worker.stop_lifecycle_actor_for_test().await;
                    assert!(
                        worker
                            .submit_monthly_capacity_for_test()
                            .unwrap_err()
                            .to_string()
                            .contains("Monthly monitor lost the lifecycle actor")
                    );
                    "Monthly monitor lost the lifecycle actor"
                }
                "settlement" => {
                    worker.lose_window_settlement_for_test();
                    "resource usage close task was lost"
                }
                "driver" => {
                    let (_, gated, release) = worker.pause_next_stop_driver_for_test();
                    drop(
                        worker
                            .set_interrupting(InterruptKind::Suspend(Timestamp::now_utc()))
                            .await?,
                    );
                    gated.await?;
                    release.send(true).unwrap();
                    assert!(
                        worker
                            .join_accepted_stops_for_test()
                            .await
                            .unwrap_err()
                            .to_string()
                            .contains("injected stop driver panic")
                    );
                    "injected stop driver panic"
                }
                _ => unreachable!(),
            };
            release_idle.send(false).unwrap();
            let error = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Some(error) = worker.cleanup_failure_for_test().await {
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
            assert!(
                worker.unload_succeeded_for_test(),
                "physical deletion must succeed independently of retained health"
            );
            assert!(!worker.concurrent_agent_permit_is_held().await);
            assert!(worker.eviction_class().await.is_none());
            assert_eq!(worker.resident_generation_for_test(), generation);
            let retry = Worker::start_if_needed(worker.clone()).await.unwrap_err();
            assert!(retry.to_string().contains(expected), "{retry}");
            assert_eq!(worker.resident_generation_for_test(), generation);
            assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
            if !later {
                let loaded = events
                    .wait_for(|event| match event {
                        Event::WorkerLoaded {
                            agent_id, result, ..
                        } if agent_id == &id => Some(result.clone()),
                        _ => None,
                    })
                    .await?;
                assert!(loaded.unwrap_err().to_string().contains(expected));
            }
            if let Some(invoke) = invoke {
                invoke.await??;
            }
            shutdown.cancel();
        }
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
        let limits = ResourceLimitsGrpc::new(
            Arc::new(MutableResourceLimitsRegistry::new(available_quota_policy())),
            Duration::from_secs(3600),
            Duration::ZERO,
            metering,
            shutdown.clone(),
        );
        let executor = start_with_resource_limits_and_configure(
            deps,
            &context,
            limits.clone(),
            Arc::new(move |config| config.resource_usage_metering = metering),
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let name = agent_id!("Networking", format!("monitor-failure-{actor}"));
        let id = executor.start_agent(&component.id, name.clone()).await?;
        let worker = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
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
        let generation = worker.resident_generation_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let expected = if actor {
            worker.stop_lifecycle_actor_for_test().await;
            "Monthly monitor lost the lifecycle actor"
        } else {
            worker.panic_monthly_monitor_for_test().await;
            "Monthly monitor panicked: injected monthly monitor panic"
        };
        let mut byte = [0];
        assert!(matches!(socket.read(&mut byte).await, Ok(0) | Err(_)));
        let error = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(error) = worker.cleanup_failure_for_test().await {
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
        assert_eq!(worker.resident_generation_for_test(), generation);
        assert!(
            Worker::start_if_needed(worker.clone())
                .await
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
        assert!(invocation.await?.is_err());
        assert_eq!(
            limits
                .initialize_account(context.account_id)
                .await?
                .monthly_observer_count_for_test(),
            0
        );
        shutdown.cancel();
    }
    Ok(())
}

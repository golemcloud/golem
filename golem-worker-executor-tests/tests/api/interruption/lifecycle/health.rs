use super::*;
use golem_common::model::Timestamp;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::UsesAllDeps;
use golem_worker_executor::worker::Worker;
use golem_worker_executor_test_utils::callers;
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
            .start_agent(
                &component.id,
                agent_key!("Networking", "policy-health-seed"),
            )
            .await?;
        let seed = executor
            .production_active_agent(&seed_id.clone())
            .await
            .unwrap()
            .primary();
        wait_for_invocation_pair(&executor, &seed_id.agent_id, OplogIndex::INITIAL).await?;
        let name = if ephemeral {
            agent_key!("EphemeralNetworking", "policy-health")
        } else {
            agent_key!("Networking", "policy-health")
        };
        let key = IdempotencyKey::fresh();
        let id = invocation_agent_id(
            &executor,
            component.environment_id,
            &name,
            !ephemeral,
            &key,
            "tcp_collect_p3",
        );
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
            invocation_create_intent(
                &executor,
                component.id,
                &name,
                !ephemeral,
                &key,
                "tcp_collect_p3",
            ),
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
                executor.get_worker_metadata(&owned).await?.status,
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
mod unix;

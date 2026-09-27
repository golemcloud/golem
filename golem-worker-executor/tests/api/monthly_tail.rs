//! Holds the response body after the guest export returns. Unlike a pending send alone,
//! the body recorder owns active tracked Store work that must retire before settlement.

use super::*;
use anyhow::Context as _;
use golem_common::model::oplog::OplogEntry;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use golem_worker_executor::services::oplog::CommitLevel;
use golem_worker_executor::worker::Worker;
use golem_worker_executor::workerctx::default::Context;
use pretty_assertions::assert_eq;
use test_r::test;

inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);

#[test]
#[timeout("60s")]
async fn monthly_memory_stop_is_accepted_during_post_export_tail_work(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let policy = MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    };
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let _shutdown_on_drop = shutdown.clone().drop_guard();
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: true,
        filesystem: false,
    };
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown,
    );
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (request_entered, request) = tokio::sync::oneshot::channel();
    let (release_response, response) = tokio::sync::oneshot::channel();
    let requests = Arc::new(AtomicUsize::new(0));
    let server_shutdown = CancellationToken::new();
    let _server_shutdown_on_drop = server_shutdown.clone().drop_guard();
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let requests = requests.clone();
        let server_shutdown = server_shutdown.clone();
        async move {
            let entered = Arc::new(Mutex::new(Some(request_entered)));
            let response = Arc::new(tokio::sync::Mutex::new(Some(response)));
            let route = Router::new().route(
                "/spawned",
                get(move || {
                    let entered = entered.clone();
                    let response = response.clone();
                    let requests = requests.clone();
                    async move {
                        assert_eq!(requests.fetch_add(1, Ordering::SeqCst), 0);
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        let held = response.lock().await.take().unwrap();
                        axum::body::Body::from_stream(futures::stream::once(async move {
                            let _ = held.await;
                            Ok::<_, std::convert::Infallible>("spawned-response")
                        }))
                    }
                }),
            );
            axum::serve(listener, route)
                .with_graceful_shutdown(server_shutdown.cancelled_owned())
                .await
        }
    }));
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let name = agent_id!("HttpClient4");
    let id = executor
        .start_agent_with(
            &component.id,
            name.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let active = executor.production_active_agent(&owned).await.unwrap();
    let worker = active.primary();
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("initial idle permit release")?;
    let root_returned = worker.observe_next_tail_drain_for_test();
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &name,
            &key,
            "get_in_spawned_task_after_return",
            data_value!(),
        )
        .await?;
    let tracker = tokio::time::timeout(Duration::from_secs(10), root_returned)
        .await
        .context("root export returned and tail drain started")??;
    tokio::time::timeout(Duration::from_secs(10), request)
        .await
        .context("spawned HTTP request entered held handler")??;
    let (started, chunk) = pending_tail(&worker, &key).await?;
    assert!(tracker.has_active());
    assert!(worker.concurrent_agent_permit_is_held().await);
    let generation = worker.resident_generation_for_test();
    let acquisitions = worker.permit_acquisitions_for_test();
    let account = limits.initialize_account(context.account_id).await?;
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    let admitted_revision = registry.current_limits().monthly_policy_revision;
    eprintln!(
        "[monthly-tail] root returned, active tail tasks={}, method Start={started}, HTTP chunk Start={chunk}, response body held, permit held, generation={generation}",
        tracker.active_count()
    );

    let (_, accepted, release_driver) = worker.pause_next_stop_driver_for_test();
    let mut raw = worker.raw_interrupt_for_test();
    let mut exhausted = policy;
    exhausted.available_memory_gb_seconds = 0;
    registry.set_policy(exhausted);
    let refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), accepted)
        .await
        .context("monthly stop accepted while post-export tail HTTP is held")??;
    assert_eq!(
        limits
            .initialized_policy_revision_for_test(context.account_id)
            .await,
        Some(registry.current_limits().monthly_policy_revision)
    );
    let cause = worker
        .monthly_stop_for_test()
        .expect("accepted monthly stop");
    assert!(matches!(cause, InterruptKind::Suspend(_)));
    assert_eq!(worker.pending_stop_for_test().await, Some(cause));
    assert_eq!(worker.frozen_stop_for_test(), None);
    assert_eq!(worker.owner_stop_for_test().await, None);
    assert_eq!(pending_tail(&worker, &key).await?, (started, chunk));
    assert!(tracker.has_active());
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    let mut receipt = worker.accepted_stop_receipt_for_test();
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    eprintln!(
        "[monthly-tail] accepted {cause:?}, active tail tasks={}, HTTP pending, response and driver held, permit held, one monitor, receipt pending",
        tracker.active_count()
    );
    release_driver.send(false).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        cause
    );
    assert_eq!(worker.frozen_stop_for_test(), Some(cause));
    assert_eq!(worker.owner_stop_for_test().await, Some(cause));
    // Keep peer bytes unavailable until the tracked HTTP task has cooperatively retired.
    // Release the peer even on timeout so a failed stop leaves a bounded diagnostic.
    let tail_retired = tokio::time::timeout(Duration::from_secs(10), async {
        while tracker.has_active() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let _ = release_response.send(());
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await
    .context("accepted stop driver joined")??;
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("physical tail cleanup and permit release")?;
    tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
    tokio::time::timeout(Duration::from_secs(10), refresh).await??;
    tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    tail_retired.context("tracked HTTP tail retires without peer body bytes")?;
    assert!(!tracker.has_active());
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let stopped = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(stopped.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        1
    );
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
            .count(),
        1
    );
    assert!(
        !stopped
            .values()
            .any(|e| matches!(e, OplogEntry::Error { .. } | OplogEntry::Interrupted { .. }))
    );
    assert!(!stopped.range(started.next()..).any(|(_, e)| matches!(
        e,
        OplogEntry::AgentInvocationFinished { .. } | OplogEntry::Cancelled { .. }
    )));
    assert!(
        !stopped.values().any(|e| matches!(e,
            OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. }
                if *start_index == chunk
        )),
        "the interrupted body chunk must remain incomplete"
    );
    executor.check_oplog_is_queryable(&id).await?;
    limits.run_batch_for_test().await;
    let usage = registry.applied_memory_byte_nanoseconds();
    assert!(usage > 0);
    assert!(registry.applied_memory_byte_nanoseconds_at_revision(admitted_revision) > 0);
    limits.run_batch_for_test().await;
    assert_eq!(registry.applied_memory_byte_nanoseconds(), usage);
    eprintln!(
        "[monthly-tail] cleanup joined, receipt once, one Suspend, keyed method unfinished, no false HTTP cancellation, tail inactive, permit released, observers=0, settled byte-nanoseconds={usage}; oplog={stopped:#?}"
    );
    server_shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), server).await???;
    Ok(())
}

async fn pending_tail(
    worker: &Worker<Context>,
    key: &IdempotencyKey,
) -> anyhow::Result<(OplogIndex, OplogIndex)> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let oplog = worker.oplog();
            let observed_tip = oplog.current_oplog_index().await;
            oplog.commit(CommitLevel::Always).await;
            let entries = oplog.read_exact(OplogIndex::INITIAL, observed_tip.as_u64()).await;
            let starts = entries.iter().filter_map(|(index, entry)| match entry {
                OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == key => Some(*index),
                _ => None,
            }).collect::<Vec<_>>();
            if let [started] = starts.as_slice() {
                assert!(!entries.range(started.next()..).any(|(_, e)| matches!(e, OplogEntry::AgentInvocationFinished { .. })));
                let chunks = entries.range(started.next()..).filter_map(|(index, entry)| match entry {
                    OplogEntry::Start { function_name, .. } if function_name.to_string() == "http::types::response::consume-body-chunk" => Some(*index),
                    _ => None,
                }).collect::<Vec<_>>();
                if let [chunk] = chunks.as_slice() {
                    assert!(!entries.values().any(|entry| matches!(entry,
                        OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. }
                            if start_index == chunk
                    )));
                    return Ok((*started, *chunk));
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.context("committed method and HTTP Starts without Finished or HTTP terminal")?
}

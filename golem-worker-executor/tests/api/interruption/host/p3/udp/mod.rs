mod completion;

use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{
    AgentError, OplogEntry, PublicAgentInvocation, PublicDurableFunctionType,
    PublicOplogEntryWithIndex,
};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_limits::ResourceUsageFlusher;
use golem_worker_executor::services::{HasOplog, HasOplogService, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use test_r::test;

use crate::api::interruption::support::{FUEL_BUDGET, Resource, check_accounting};

macro_rules! pending_case {
    ($name:ident, Storage, $mode:ident) => {
        pending_case!(#[cfg(target_os = "linux")] #[ignore = "requires the privileged managed XFS test runner"] $name, Storage, $mode);
    };
    ($(#[$attr:meta])* $name:ident, $resource:ident, $mode:ident) => {
        $(#[$attr])*
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_receive(
                last_unique_id,
                deps,
                host_api_tests,
                Resource::$resource,
                AgentMode::$mode,
            )
            .await
        }
    };
}

pending_case!(durable_p3_udp_receive_memory_quota, Memory, Durable);
pending_case!(ephemeral_p3_udp_receive_memory_quota, Memory, Ephemeral);
pending_case!(
    durable_p3_udp_receive_compute_quota_prepaid,
    Compute,
    Durable
);
pending_case!(
    ephemeral_p3_udp_receive_compute_quota_prepaid,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p3_udp_receive_managed_xfs_storage_quota,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p3_udp_receive_managed_xfs_storage_quota,
    Storage,
    Ephemeral
);

async fn ready_address(peer: &tokio::net::UdpSocket) -> anyhow::Result<std::net::SocketAddr> {
    let mut bytes = [0; 32];
    let (count, source) =
        tokio::time::timeout(Duration::from_secs(10), peer.recv_from(&mut bytes)).await??;
    ensure!(&bytes[..count] == b"ready");
    Ok(source)
}

async fn admitted_receive(
    executor: &TestWorkerExecutor,
    id: &AgentId,
) -> anyhow::Result<OplogIndex> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            executor.commit_oplog(id).await?;
            let entries = executor.get_oplog(id, OplogIndex::INITIAL).await?;
            if let Ok(start) = receive_start(&entries, 0) {
                return Ok::<_, anyhow::Error>(start);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("committed UDP receive Start")?
}

const RECEIVE: &str = "sockets::types::udp-socket::receive";

fn assert_invocation(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    finished: usize,
) -> anyhow::Result<()> {
    let mut current = false;
    let mut started = 0;
    let mut terminals = 0;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(start) => {
                current = matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if &method.idempotency_key == key);
                started += usize::from(current);
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current => terminals += 1,
            _ => {}
        }
    }
    ensure!(
        started == 1 && terminals == finished,
        "invocation {key}: {started} Started/{terminals} Finished"
    );
    Ok(())
}

fn receive_start(
    entries: &[PublicOplogEntryWithIndex],
    terminals: usize,
) -> anyhow::Result<OplogIndex> {
    let starts: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == RECEIVE => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    ensure!(starts.len() == 1, "one receive Start: {entries:#?}");
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(start) => {
                ensure!(start.parent_start_index.is_none() && start.observational_owner.is_none());
                if entry.oplog_index == starts[0] {
                    ensure!(matches!(
                        start.durable_function_type,
                        PublicDurableFunctionType::ReadRemote(_)
                    ));
                    ensure!(start.request.is_some());
                }
                ensure!(
                    terminal_count(entries, entry.oplog_index)
                        == if entry.oplog_index == starts[0] {
                            terminals
                        } else {
                            1
                        }
                );
            }
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Jump(_) => {
                anyhow::bail!("no invented terminal or replay jump: {entry:?}")
            }
            _ => {}
        }
    }
    let delivered = entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(delivery) if delivery.start_index == starts[0])).count();
    ensure!(delivered == terminals, "receive delivery: {entries:#?}");
    Ok(starts[0])
}

async fn pending_receive(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
) -> anyhow::Result<()> {
    let mut policy = resource.policy();
    policy.available_durable_storage_byte_seconds = 1;
    policy.available_ephemeral_storage_byte_seconds = 1;
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let metering = resource.metering();
    let shutdown = CancellationToken::new();
    let _shutdown_guard = shutdown.clone().drop_guard();
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown,
    );
    let context = TestContext::new(last_unique_id);
    let filesystem = managed_filesystem_config(metering)?;
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.agent_sockets.enabled = true;
            config.resource_usage_metering = metering;
            if let Some(filesystem) = &filesystem {
                config.filesystem_storage.mode = filesystem.clone();
            }
            config.limits.fuel_to_borrow = FUEL_BUDGET;
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
            agent_id!("Networking", "p3-udp-receive-seed"),
        )
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .unwrap()
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !seed.stop_if_idle().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("seed idle teardown")?;
    // Refund the seed Store before the target borrows the full account budget.
    if resource == Resource::Compute {
        limits.run_batch_for_test().await;
    }
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("Networking", "monthly-p3-udp-receive")
    } else {
        agent_id!("EphemeralNetworking", "monthly-p3-udp-receive")
    };
    let key = IdempotencyKey::fresh();
    let physical = if durable {
        name.clone()
    } else {
        name.clone()
            .with_ephemeral_invocation_phantom(&key)
            .unwrap()
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
    let (flushed, mut flushes) = tokio::sync::mpsc::unbounded_channel();
    let _usage_source = (resource != Resource::Memory).then(|| {
        let source: Arc<dyn ResourceUsageFlusher> = Arc::new(ObservedUsageFlusher {
            owner: Arc::downgrade(&worker),
            send: flushed,
        });
        worker
            .owner_runtime_resources()
            .register_resource_usage_flusher(Arc::downgrade(&source));
        source
    });
    let mut capacity_updates = if resource != Resource::Memory {
        let mut updates = limits
            .subscribe_capacity_updates_for_test(context.account_id)
            .await?;
        updates.borrow_and_update();
        Some(updates)
    } else {
        None
    };
    let oplog = worker.all().oplog_service();
    let read_stored = async || {
        let last = oplog.get_last_index(&owned, mode).await?;
        oplog
            .read_exact(&owned, mode, OplogIndex::INITIAL, last.as_u64())
            .await
    };
    let reservation = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let address = reservation.local_addr()?;
    let port = address.port();
    drop(reservation);
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let ready_port = peer.local_addr()?.port();
    let mut failure_address = None;
    let mut invocation = tokio::spawn({
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
                    "udp_receive_wait_p3",
                    data_value!(port, ready_port),
                )
                .await
        }
    });
    let result = async {
        ensure!(ready_address(&peer).await? == address);
        failure_address = Some(address);
        admitted_receive(&executor, &id).await?;
        executor.commit_oplog(&id).await?;
        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let receive_start = receive_start(&entries, 0)?;
        assert_invocation(&entries, &key, 0)?;
        ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
        let initial = worker.owner_runtime_resources().with_monthly_capacity(mode, |capacity| capacity);
        let mut next_flush = async || {
            loop {
                let revision = flushes.recv().await.context("owner usage flusher closed")?;
                if revision == initial.policy_revision {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        };
        if resource != Resource::Memory {
            tokio::time::timeout(Duration::from_secs(10), next_flush())
                .await.context("initial owner usage flush")??;
        }
        let prefix = read_stored().await?;
        let prefix_last = *prefix.last_key_value().context("committed UDP invocation prefix is empty")?.0;
        let mut raw = worker.raw_interrupt_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(worker.is_loaded().await);
        ensure!(!invocation.is_finished());
        info!(
            ?resource, ?mode, %key, ?id, %receive_start, ?initial,
            "UDP receive Start committed; peer has sent no response datagram"
        );

        if resource == Resource::Compute {
            ensure!(initial.fuel_generation.is_some());
            ensure!(!account.prepaid_fuel_is_exhausted(mode, initial.fuel_generation));
            ensure!(
                account.monthly_capacity_is_exhausted_for_test(mode),
                "Store must prepay the remaining fuel"
            );
            next_flush().await?;
            tokio::time::timeout(Duration::from_secs(10), next_flush())
                .await.context("next owner usage flush")??;
            ensure!(!capacity_updates.as_ref().unwrap().has_changed()?);
            drop(capacity_updates.take());
            ensure!(!invocation.is_finished());
        }
        let refresh = if resource == Resource::Storage {
            allocate_storage(&owned).await?;
            // Real timer delivery keeps this proposal on the original revision.
            // The unchanged whole-test bound includes waiting for ordinary cadence.
            next_flush().await?;
            ensure!(!capacity_updates.as_ref().unwrap().has_changed()?);
            drop(capacity_updates.take());
            None
        } else {
            let mut exhausted = policy.clone();
            if resource == Resource::Memory {
                exhausted.available_memory_gb_seconds = 0;
            } else {
                exhausted.available_fuel = 0;
            }
            registry.set_policy(exhausted);
            Some(applied_refresh(&limits, &registry, context.account_id).await?)
        };
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)));
        if resource == Resource::Storage {
            let other = if durable {
                AgentMode::Ephemeral
            } else {
                AgentMode::Durable
            };
            ensure!(!account.monthly_capacity_is_exhausted_for_test(other));
        }
        // No datagram is sent until stop-driven physical unload has finished.
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .with_context(|| format!("physical worker unload: invocation_finished={}", invocation.is_finished()))?;
        // UDP has no EOF. Rebinding the exact native address proves socket retirement.
        drop(tokio::net::UdpSocket::bind(address).await.context("retired UDP port must be reusable")?);
        tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        if let Some(refresh) = refresh {
            refresh.await?;
        }
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let stopped_raw = read_stored().await?;
        ensure!(stopped_raw.range(..=prefix_last).eq(prefix.iter()), "the full committed UDP prefix must survive stop");
        ensure!(stopped_raw.values().all(|entry| !matches!(entry, OplogEntry::Jump { .. })));
        assert_invocation(&stopped, &key, 0)?;
        ensure!(
            self::receive_start(&stopped, 0)? == receive_start,
            "interruption must leave the original receive Start open"
        );
        let entries = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await?;
        let errors: Vec<_> = entries
            .values()
            .filter_map(|entry| match entry {
                OplogEntry::Error { error, .. } => Some(error),
                _ => None,
            })
            .collect();
        let suspends = entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count();
        if durable {
            ensure!(errors.is_empty(), "{errors:?}");
            ensure!(suspends == 1);
            ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
            ensure!(!invocation.is_finished());
        } else {
            ensure!(suspends == 0);
            match resource {
                Resource::Compute => {
                    ensure!(
                        matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)]),
                        "{errors:?}"
                    );
                }
                _ => {
                    ensure!(
                        matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)]
                            if reason.reason == resource.reason(false)),
                        "{errors:?}"
                    );
                }
            }
            ensure!(
                tokio::time::timeout(Duration::from_secs(10), &mut invocation)
                    .await??
                    .is_err()
            );
        }
        limits.run_batch_for_test().await;
        let settled = registry.applied_updates();
        check_accounting(
            &settled,
            resource,
            durable,
            initial.policy_revision,
            initial.period,
        )?;
        let settled_count = settled.len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        limits.run_batch_for_test().await;
        if resource == Resource::Storage { assert_storage_deleted(&owned)?; }
        // Idle policy refreshes may still emit updates, but cannot charge a closed window.
        for update in &registry.applied_updates()[settled_count..] {
            ensure!(
                update.fuel_delta == 0
                    && update.memory_gb_seconds_delta == 0
                    && update.memory_byte_nanoseconds_remainder == 0
                    && update.durable_storage_byte_seconds_delta == 0
                    && update.durable_storage_byte_nanoseconds_remainder == 0
                    && update.ephemeral_storage_byte_seconds_delta == 0
                    && update.ephemeral_storage_byte_nanoseconds_remainder == 0,
                "closed window must stop accruing: {update:?}"
            );
        }
        info!(
            ?resource, ?mode, %receive_start,
            "Monthly stop released UDP port without a response datagram or receive terminal"
        );

        if durable {
            let mut grant = policy;
            if resource == Resource::Storage {
                grant.available_durable_storage_byte_seconds = u64::MAX;
            }
            registry.set_policy(grant.clone());
            applied_refresh(&limits, &registry, context.account_id)
                .await?
                .await?;
            executor.resume(&id, false).await?;
            executor.commit_oplog(&id).await?;
            ensure!(self::receive_start(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, 0)? == receive_start);
            let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
            // Bind executes again on replay; the recorded readiness send does not.
            let resumed = tokio::time::timeout(Duration::from_secs(10), async {
                let mut interval = tokio::time::interval(Duration::from_millis(20));
                loop {
                    tokio::select! {
                        result = &mut invocation => break Ok::<_, anyhow::Error>(result??),
                        _ = interval.tick() => { sender.send_to(b"recovered", address).await?; }
                    }
                }
            }).await.context("retained invocation continuation")??;
            ensure!(resumed.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"recovered".to_vec()));
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            let recovered_raw = read_stored().await?;
            let stopped_last = *stopped_raw.last_key_value().unwrap().0;
            ensure!(recovered_raw.range(..=stopped_last).eq(stopped_raw.iter()), "the full stopped prefix must survive UDP reconstruction");
            ensure!(self::receive_start(&recovered, 1)? == receive_start);
            assert_settled_history(&recovered)?;
            assert_invocation(&recovered, &key, 1)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, "udp_receive_wait_p3", data_value!(port, ready_port)).await?;
            ensure!(cached.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"recovered".to_vec()));
            ensure!(matches!(peer.try_recv_from(&mut [0; 32]), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock), "cached lookup must not send readiness");
            drop(sender);
            failure_address = None;
            // Force real reconstruction of completed history with the sender gone.
            tokio::time::timeout(Duration::from_secs(10), async {
                while !worker.stop_if_idle().await {
                    tokio::task::yield_now().await;
                }
            }).await.context("idle eviction before completed-history replay")?;
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.is_loaded().await {
                    tokio::task::yield_now().await;
                }
            }).await?;
            tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
            drop(tokio::net::UdpSocket::bind(address).await?);
            let fresh_key = IdempotencyKey::fresh();
            let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, "udp_receive_wait_p3", data_value!(port, ready_port));
            let release_fresh = async {
                // Only fresh work sends readiness; completed history needs no response.
                ensure!(ready_address(&peer).await? == address);
                let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
                ensure!(sender.send_to(b"fresh", address).await? == 5);
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(fresh, release_fresh)
            }).await.context("completed-history replay and fresh receive")??;
            ensure!(fresh.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"fresh".to_vec()));
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            let history_raw = read_stored().await?;
            let recovered_last = *recovered_raw.last_key_value().unwrap().0;
            ensure!(history_raw.range(..=recovered_last).eq(recovered_raw.iter()), "completed-history reconstruction must preserve the full recovered prefix");
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
            let starts: Vec<_> = history.iter().filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start) if start.function_name == RECEIVE => Some(entry.oplog_index),
                _ => None,
            }).collect();
            ensure!(starts.len() == 2 && starts[0] == receive_start);
            for start in starts {
                ensure!(terminal_count(&history, start) == 1);
                ensure!(history.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(delivery) if delivery.start_index == start)).count() == 1);
            }
            assert_settled_history(&history)?;
            assert_invocation(&history, &key, 1)?;
            assert_invocation(&history, &fresh_key, 1)?;
            info!(?resource, %key, %receive_start,
                "Original receive Start repaired once; completed history needed no datagram; fresh receive consumed its own datagram");
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // Release a stuck native receive only after recording failure, never on the passing stop path.
    if result.is_err() {
        if let Some(address) = failure_address
            && let Ok(sender) = tokio::net::UdpSocket::bind("127.0.0.1:0").await
        {
            let _ = sender.send_to(b"failure-cleanup", address).await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await;
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await;
        let _ =
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await;
        if !invocation.is_finished() {
            let _ = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await;
        }
    }
    result
}

fn terminal_count(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> usize {
    entries
        .iter()
        .filter(|entry| match &entry.entry {
            PublicOplogEntry::End(end) => end.start_index == start,
            PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
            _ => false,
        })
        .count()
}

fn assert_settled_history(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    let last_finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("missing invocation terminal")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                ensure!(
                    terminal_count(entries, entry.oplog_index) == 1,
                    "unsettled/duplicate Start: {entry:?}"
                );
                ensure!(entry.oplog_index < last_finished);
            }
            PublicOplogEntry::End(_) => ensure!(entry.oplog_index < last_finished),
            PublicOplogEntry::Cancelled(_) | PublicOplogEntry::Error(_) => {
                anyhow::bail!("unexpected terminal: {entry:?}")
            }
            _ => {}
        }
    }
    Ok(())
}

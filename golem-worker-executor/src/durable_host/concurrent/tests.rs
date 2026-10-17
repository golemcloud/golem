// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;

use crate::durable_host::durability::find_durable_call_trap_context;
use golem_common::model::oplog::host_functions;
use golem_common::model::oplog::{
    HostRequestNoInput, HostResponseMonotonicClockTimestamp, LogLevel,
};
use golem_common::model::{NamedRetryPolicy, Predicate, RetryPolicy, RetryPolicyState};
use std::time::Duration;
use test_r::test;
use tokio::sync::mpsc;

fn idx(n: u64) -> OplogIndex {
    OplogIndex::from_u64(n)
}

fn durable_execution_state() -> DurableExecutionState {
    DurableExecutionState {
        is_live: true,
        snapshotting_mode: false,
        max_in_function_retry_delay: Duration::from_secs(20),
    }
}

fn completed(end_idx: u64) -> Resolution {
    Resolution::Completed {
        end_idx: idx(end_idx),
        response: None,
        delivery_marker: None,
        forced_commit: false,
    }
}

// ---- ConcurrentReplayResolver ----

#[test]
fn resolver_out_of_order_completion() {
    // [S1, S2, E2, E1]: claim both, then resolve E2 before E1.
    let mut resolver = ConcurrentReplayResolver::default();
    let mut rx1 = resolver.register(idx(1), None);
    let mut rx2 = resolver.register(idx(2), None);
    assert!(rx1.try_recv().is_err());
    assert!(rx2.try_recv().is_err());

    assert!(resolver.resolve_if_pending(idx(2), idx(3), completed(3)));
    match rx2.try_recv() {
        Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
            assert_eq!(end_idx, idx(3))
        }
        other => panic!("unexpected resolution for h2: {other:?}"),
    }
    assert!(rx1.try_recv().is_err());

    assert!(resolver.resolve_if_pending(idx(1), idx(4), completed(4)));
    match rx1.try_recv() {
        Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
            assert_eq!(end_idx, idx(4))
        }
        other => panic!("unexpected resolution for h1: {other:?}"),
    }
}

#[test]
fn resolver_cancelled() {
    // [S1, Cancelled1]
    let mut resolver = ConcurrentReplayResolver::default();
    let mut rx = resolver.register(idx(1), None);
    assert!(resolver.resolve_if_pending(
        idx(1),
        idx(2),
        Resolution::Cancelled {
            cancelled_idx: idx(2),
            partial: None,
        },
    ));
    match rx.try_recv() {
        Ok(ResolutionOutcome::Resolved(Resolution::Cancelled { cancelled_idx, .. })) => {
            assert_eq!(cancelled_idx, idx(2))
        }
        other => panic!("unexpected resolution: {other:?}"),
    }
}

#[test]
fn resolver_resolve_before_register_buffers() {
    let mut resolver = ConcurrentReplayResolver::default();
    resolver.resolve(idx(1), completed(2));
    assert!(!resolver.is_pending(idx(1)));
    let mut rx = resolver.register(idx(1), None);
    match rx.try_recv() {
        Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
            assert_eq!(end_idx, idx(2))
        }
        other => panic!("expected pre-resolved receiver, got {other:?}"),
    }
}

#[test]
fn resolver_missing_pending_is_dropped_not_buffered() {
    let mut resolver = ConcurrentReplayResolver::default();
    // No registration: resolve_if_pending must not buffer (this is the unregistered-End case).
    assert!(!resolver.resolve_if_pending(idx(1), idx(2), completed(2)));
    let mut rx = resolver.register(idx(1), None);
    assert!(rx.try_recv().is_err());
}

#[test]
fn resolver_fail_all_pending_incomplete_wakes_everyone() {
    // End-of-replay must wake every still-suspended awaiter as Incomplete, not leave them
    // hanging. A resolved awaiter is already gone and is unaffected.
    let mut resolver = ConcurrentReplayResolver::default();
    let mut rx1 = resolver.register(idx(1), None);
    let mut rx2 = resolver.register(idx(2), None);
    assert!(resolver.resolve_if_pending(idx(1), idx(3), completed(3)));

    resolver.fail_all_pending_incomplete();

    match rx1.try_recv() {
        Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
            assert_eq!(end_idx, idx(3))
        }
        other => panic!("rx1 should already be resolved: {other:?}"),
    }
    match rx2.try_recv() {
        Ok(ResolutionOutcome::Incomplete) => {}
        other => panic!("rx2 should be Incomplete: {other:?}"),
    }
    assert!(!resolver.is_pending(idx(2)));
}

#[test]
fn resolver_duplicate_resolution_is_ignored() {
    let mut resolver = ConcurrentReplayResolver::default();
    let mut rx = resolver.register(idx(1), None);
    assert!(resolver.resolve_if_pending(idx(1), idx(2), completed(2)));
    // Second resolution: no longer pending.
    assert!(!resolver.resolve_if_pending(idx(1), idx(3), completed(3)));
    match rx.try_recv() {
        Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
            assert_eq!(end_idx, idx(2))
        }
        other => panic!("unexpected resolution: {other:?}"),
    }
}

// ---- DurableCallSession drop policy ----

fn live_unfinished_handle<P: DropPolicy>(
    start_idx: OplogIndex,
    sink: mpsc::UnboundedSender<DropEvent>,
) -> DurableCallSession<host_functions::MonotonicClockNow, P> {
    use crate::durable_host::durability::DurableExecutionState;
    use std::time::Duration;
    let durable_execution_state = DurableExecutionState {
        is_live: true,
        snapshotting_mode: false,
        max_in_function_retry_delay: Duration::ZERO,
    };
    DurableCallSession {
        start_idx,
        boundary: DurableCallBoundary::from_begin_index(start_idx),
        is_live: true,
        persisted: true,
        request_upload: PendingUpload::already_durable(),
        replay: None,
        finished: false,
        scope_replay_recovery: ScopeReplayRecovery::default().into(),
        cancellation_span_id: None,
        span_observer: None,
        parked_undelivered_replay: false,
        host_internal_calls: false,
        execution_scope: CallExecutionScope {
            retry_from: start_idx,
            durable_scope: None,
            observational_owner: None,
            entity_parent_start_index: None,
            invocation_id: None,
        },
        retry: InFunctionRetryController::new(
            DurableFunctionType::ReadLocal,
            durable_execution_state,
            "test:monotonic_clock::now",
        ),
        executor_shutdown: tokio_util::sync::CancellationToken::new(),
        runtime_teardown: Arc::new(|| false),
        requires_agent_authority: false,
        agent_auth_ctx: None,
        drop_sink: Some(sink),
        cleanup_sink: None,
        live_call_permit: None,
        _phantom: PhantomData,
    }
}

/// A synthetic, already-finished handle carrying an arbitrary [`CallExecutionScope`]. Used by the
/// Terminal-failure tests use this to drive `trap_context()` (the call-owned classification a
/// terminal-step failure attaches) for a call initiated in a specific scope, without standing up
/// a full `DurableWorkerCtx`. `finished` is set so dropping the handle is a no-op (no drop
/// event), and no `drop_sink` is attached. `start_idx` and the boundary lease are set from
/// `scope.retry_from` only so the struct is well-formed; the tests read nothing but the scope.
fn synthetic_finished_handle_with_scope<P: DropPolicy>(
    scope: CallExecutionScope,
) -> DurableCallSession<host_functions::MonotonicClockNow, P> {
    use crate::durable_host::durability::DurableExecutionState;
    use std::time::Duration;
    let durable_execution_state = DurableExecutionState {
        is_live: true,
        snapshotting_mode: false,
        max_in_function_retry_delay: Duration::ZERO,
    };
    let start_idx = scope.retry_from;
    DurableCallSession {
        start_idx,
        boundary: DurableCallBoundary::from_begin_index(start_idx),
        is_live: true,
        persisted: true,
        request_upload: PendingUpload::already_durable(),
        replay: None,
        finished: true,
        scope_replay_recovery: ScopeReplayRecovery::default().into(),
        cancellation_span_id: None,
        span_observer: None,
        parked_undelivered_replay: false,
        host_internal_calls: false,
        execution_scope: scope,
        retry: InFunctionRetryController::new(
            DurableFunctionType::ReadLocal,
            durable_execution_state,
            "test:monotonic_clock::now",
        ),
        executor_shutdown: tokio_util::sync::CancellationToken::new(),
        runtime_teardown: Arc::new(|| false),
        requires_agent_authority: false,
        agent_auth_ctx: None,
        drop_sink: None,
        cleanup_sink: None,
        live_call_permit: None,
        _phantom: PhantomData,
    }
}

#[test]
fn live_call_permit_tracks_live_handle_lifetime() {
    let counter = Arc::new(AtomicUsize::new(0));

    {
        let _permit = LiveCallPermit::new(counter.clone());
        assert_eq!(counter.load(Ordering::Acquire), 1);
    }

    assert_eq!(counter.load(Ordering::Acquire), 0);
}

#[test]
fn cloning_a_live_call_permit_takes_an_extra_count() {
    let counter = Arc::new(AtomicUsize::new(0));
    let permit = LiveCallPermit::new(counter.clone());
    let cloned = permit.clone();
    assert_eq!(counter.load(Ordering::Acquire), 2);
    drop(permit);
    assert_eq!(counter.load(Ordering::Acquire), 1);
    drop(cloned);
    assert_eq!(counter.load(Ordering::Acquire), 0);
}

#[test]
async fn cleanup_after_terminal_keeps_live_permit_until_event_is_consumed() {
    // An `AccessTerminalGuard` armed with `cleanup_after_terminal` and then dropped (a torn
    // completion future) must keep the call counted as in flight — via the queued
    // `CleanupAfterTerminal` event — until the event is consumed, so a positional boundary
    // cannot be placed before the still-pending terminal append.
    //
    // The policy here is `NotCancellable`, whose `production_drop_sink` is `None`: the
    // cleanup event must be enqueued through the policy-independent cleanup sink, exactly as
    // it is for the p3 accessor calls that use this policy.
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
    {
        let mut guard = AccessTerminalGuard::<NotCancellable>::new(
            DroppedCall {
                start_idx: idx(5),
                begin_index: idx(4),
                function_type: DurableFunctionType::ReadRemote,
                request_upload: PendingUpload::already_durable(),
                span_finished: None,
                span_observer: None,
                trap_context: DurableCallTrapContext { retry_from: idx(4) },
                live_call_permit: Some(LiveCallPermit::new(counter.clone())),
            },
            NotCancellable::production_drop_sink(Some(tx.clone())),
            Some(tx),
            TeardownProbe::never(),
        );
        assert_eq!(counter.load(Ordering::Acquire), 1);
        let terminal = tokio::spawn(async move {
            let _ = gate_rx.await;
            Ok(())
        });
        guard.cleanup_after_terminal(terminal, None);
        assert_eq!(
            counter.load(Ordering::Acquire),
            1,
            "the state transition must keep the permit"
        );
        // The guard is dropped here while the terminal task is still pending.
    }
    assert_eq!(
        counter.load(Ordering::Acquire),
        1,
        "the queued CleanupAfterTerminal event must own the permit"
    );
    let event = rx
        .try_recv()
        .expect("expected a CleanupAfterTerminal drop event");
    assert_eq!(counter.load(Ordering::Acquire), 1);
    gate_tx.send(()).expect("terminal task is alive");
    drop(event);
    assert_eq!(counter.load(Ordering::Acquire), 0);
}

// ---- CompletionDelivery (deferred guest-delivery token) ----

/// Builds a live-armed [`CompletionDelivery`] over the given oplog, exactly as
/// [`AccessTerminalGuard::take_completion_delivery`] produces one for a persisted live call
/// whose `End` is already durable.
async fn live_delivery_token(
    oplog: Arc<InMemoryOplog>,
    permit_counter: Arc<AtomicUsize>,
    cleanup_tx: mpsc::UnboundedSender<DropEvent>,
) -> CompletionDelivery {
    live_delivery_token_with_teardown(oplog, permit_counter, cleanup_tx, TeardownProbe::never())
        .await
}

async fn live_delivery_token_with_teardown(
    oplog: Arc<InMemoryOplog>,
    permit_counter: Arc<AtomicUsize>,
    cleanup_tx: mpsc::UnboundedSender<DropEvent>,
    teardown: TeardownProbe,
) -> CompletionDelivery {
    CompletionDelivery {
        state: CompletionDeliveryState::Live(Box::new(LiveDelivery {
            marker: marker_record_over(oplog).await,
            trap_context: DurableCallTrapContext { retry_from: idx(1) },
            live_call_permit: Some(LiveCallPermit::new(permit_counter)),
            cleanup_sink: Some(cleanup_tx),
            pending_append: None,
            teardown,
        })),
    }
}

/// A `CompletionDiscarded` marker record for call `Start` 1 whose appends land on `oplog`.
async fn marker_record_over(oplog: Arc<InMemoryOplog>) -> CompletionMarkerRecord {
    let oplog_dyn: Arc<dyn Oplog> = oplog;
    // The replay state is built over a separately seeded [Start, End] oplog so the observed
    // oplog contains only what the token itself appends (and so an End gate installed on the
    // observed oplog is not tripped by the seeding).
    let seed_oplog = Arc::new(InMemoryOplog::new());
    seed_oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::MonotonicClockNow,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                golem_common::model::oplog::HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadLocal,
            span_started: None,
        })
        .await
        .unwrap();
    seed_oplog
        .add(OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: idx(1),
            response: None,
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        })
        .await
        .unwrap();
    let seed_oplog_dyn: Arc<dyn Oplog> = seed_oplog;
    let replay_state = ReplayState::new_for_owner(
        golem_common::model::OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id: golem_common::model::AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "completion-delivery-test".to_string(),
            },
        },
        seed_oplog_dyn.clone(),
        golem_common::model::regions::DeletedRegions::default(),
        None,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        seed_oplog_dyn.current_oplog_index().await,
        None,
    )
    .await
    .expect("failed to build replay state");
    let recorder = CompletionMarkerRecorder::new(oplog_dyn, replay_state);
    CompletionMarkerRecord {
        start_idx: idx(1),
        recorder,
    }
}

#[test]
async fn completion_delivery_suppress_records_no_marker() {
    // `suppress` represents a caller-observed post-`End` error. It records no delivery marker and
    // releases the in-flight permit without queueing a drain event.
    let oplog = Arc::new(InMemoryOplog::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
    assert_eq!(counter.load(Ordering::Acquire), 1);
    token.suppress();
    assert_eq!(counter.load(Ordering::Acquire), 0);
    assert!(rx.try_recv().is_err(), "no drain event may be queued");
    assert!(
        oplog.entries.lock().await.is_empty(),
        "no marker may be recorded"
    );
}

#[test]
#[test_r::timeout("10s")]
async fn replay_discarded_accessor_delivery_waits_for_cancellation_after_restoration() {
    let mut config = wasmtime::Config::new();
    config.concurrency_support(true);
    let engine = wasmtime::Engine::new(&config).unwrap();
    let mut store = wasmtime::Store::new(&engine, None::<u32>);

    store
        .run_concurrent(async |accessor| -> wasmtime::Result<()> {
            // The caller restores its taken resource before reaching the delivery boundary.
            let token = CompletionDelivery::replay_discarded();
            accessor.with(|mut store| *store.data_mut() = Some(42));
            let mut delivery = Box::pin(token.deliver_at_accessor_terminal(accessor));
            assert!(
                futures::poll!(&mut delivery).is_pending(),
                "a discarded completion must not return its result to the guest"
            );
            accessor.with(|mut store| assert_eq!(*store.data_mut(), Some(42)));
            // Cancelling the host future must leave the restored resource available.
            drop(delivery);
            accessor.with(|mut store| assert_eq!(*store.data_mut(), Some(42)));
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
}

#[test]
async fn completion_delivery_delivered_records_marker_via_drain() {
    // The successful guest handoff is itself durable. Queueing the marker is synchronous; the
    // receipt and permit remain in the drain queue until the append completes.
    let oplog = Arc::new(InMemoryOplog::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
    token.delivered();
    assert_eq!(counter.load(Ordering::Acquire), 1);
    match rx.try_recv() {
        Ok(DropEvent::AwaitCompletionMarker {
            mut receipt,
            live_call_permit,
            ..
        }) => {
            await_marker_receipt(
                receipt
                    .as_mut()
                    .expect("the drain event must carry the marker receipt"),
            )
            .await
            .expect("marker append must succeed");
            let entries = oplog.entries.lock().await;
            assert!(matches!(
                entries.as_slice(),
                [OplogEntry::CompletionDelivered { start_index, .. }] if *start_index == idx(1)
            ));
            drop(live_call_permit);
            assert_eq!(counter.load(Ordering::Acquire), 0);
        }
        other => panic!("expected an AwaitCompletionMarker drop event, got {other:?}"),
    }
}

/// A completion marker whose oplog write is refused because the shard moved must be reported to
/// whoever awaits the receipt. Panicking here would abort the executor - and every other healthy
/// agent resident on it - over one agent that another executor now owns.
#[test]
async fn a_fenced_completion_marker_is_reported_rather_than_panicked() {
    let agent_id = golem_common::model::AgentId {
        component_id: golem_common::model::component::ComponentId::new(),
        agent_id: "fenced-completion-marker-test".to_string(),
    };
    let oplog = Arc::new(InMemoryOplog::fenced(crate::services::oplog::OplogFence {
        agent_id: agent_id.clone(),
        expected_epoch: golem_common::model::ShardEpoch(3),
        actual_epoch: Some(golem_common::model::ShardEpoch(4)),
    }));
    let seed_oplog = Arc::new(InMemoryOplog::new());
    seed_oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    let seed_oplog_dyn: Arc<dyn Oplog> = seed_oplog;
    let replay_state = ReplayState::new_for_owner(
        golem_common::model::OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id,
        },
        seed_oplog_dyn.clone(),
        golem_common::model::regions::DeletedRegions::default(),
        None,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        seed_oplog_dyn.current_oplog_index().await,
        None,
    )
    .await
    .expect("failed to build replay state");
    let oplog_dyn: Arc<dyn Oplog> = oplog;
    let recorder = CompletionMarkerRecorder::new(oplog_dyn, replay_state);

    let mut receipt = recorder.record(idx(10), CompletionMarkerKind::Delivered, None);

    match await_marker_receipt(&mut receipt).await {
        Err(WorkerExecutorError::OplogFenced {
            expected_epoch,
            actual_epoch,
            ..
        }) => {
            assert_eq!(expected_epoch, 3);
            assert_eq!(actual_epoch, Some(4));
        }
        other => panic!("expected the fenced marker append to be reported, got {other:?}"),
    }
}

#[test]
async fn completion_delivery_markers_preserve_handoff_order() {
    let oplog = Arc::new(InMemoryOplog::new());
    let seed_oplog = Arc::new(InMemoryOplog::new());
    seed_oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    let seed_oplog_dyn: Arc<dyn Oplog> = seed_oplog;
    let replay_state = ReplayState::new_for_owner(
        golem_common::model::OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id: golem_common::model::AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "completion-marker-order-test".to_string(),
            },
        },
        seed_oplog_dyn.clone(),
        golem_common::model::regions::DeletedRegions::default(),
        None,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        seed_oplog_dyn.current_oplog_index().await,
        None,
    )
    .await
    .expect("failed to build replay state");
    let oplog_dyn: Arc<dyn Oplog> = oplog.clone();
    let recorder = CompletionMarkerRecorder::new(oplog_dyn, replay_state);

    // These synchronous submissions model two terminal-observer callbacks. Even though the
    // appends are asynchronous, their oplog order must equal the callback handoff order.
    let mut first = recorder.record(idx(10), CompletionMarkerKind::Delivered, None);
    let mut second = recorder.record(idx(20), CompletionMarkerKind::Delivered, None);
    await_marker_receipt(&mut second)
        .await
        .expect("second marker append must succeed");
    await_marker_receipt(&mut first)
        .await
        .expect("first marker append must succeed");

    let entries = oplog.entries.lock().await;
    assert!(matches!(
        entries.as_slice(),
        [
            OplogEntry::CompletionDelivered {
                start_index: first,
                ..
            },
            OplogEntry::CompletionDelivered {
                start_index: second,
                ..
            }
        ] if *first == idx(10) && *second == idx(20)
    ));
}

#[test]
async fn completion_delivery_armed_drop_records_marker_via_drain() {
    // Dropping the token while still armed (a torn delivering future) must spawn the owned
    // marker append and hand its join plus the in-flight permit to the drain queue, so
    // invocation settlement waits for the marker.
    let oplog = Arc::new(InMemoryOplog::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
        assert_eq!(counter.load(Ordering::Acquire), 1);
        drop(token);
    }
    assert_eq!(
        counter.load(Ordering::Acquire),
        1,
        "the queued drain event must own the permit"
    );
    match rx.try_recv() {
        Ok(DropEvent::AwaitCompletionMarker {
            mut receipt,
            live_call_permit,
            ..
        }) => {
            await_marker_receipt(
                receipt
                    .as_mut()
                    .expect("the drain event must carry the marker receipt"),
            )
            .await
            .expect("marker append must succeed");
            let entries = oplog.entries.lock().await;
            assert_eq!(entries.len(), 1, "expected exactly one marker entry");
            match &entries[0] {
                OplogEntry::CompletionDiscarded { start_index, .. } => {
                    assert_eq!(*start_index, idx(1))
                }
                other => panic!("expected CompletionDiscarded, got {other:?}"),
            }
            drop(live_call_permit);
            assert_eq!(counter.load(Ordering::Acquire), 0);
        }
        other => panic!("expected an AwaitCompletionMarker drop event, got {other:?}"),
    }
}

#[test]
async fn completion_delivery_terminal_failure_retains_call_attribution() {
    let oplog = Arc::new(InMemoryOplog::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let recorder = marker_record_over(oplog.clone()).await.recorder;
    let context = DurableCallTrapContext {
        retry_from: idx(47),
    };
    let mut delivery = CompletionDelivery::live(
        idx(47),
        recorder,
        context,
        tx,
        LiveCallPermit::new(counter.clone()),
        TeardownProbe::never(),
    );
    let terminal = tokio::spawn(async { Err(WorkerExecutorError::runtime("terminal refused")) });
    let error: anyhow::Error = delivery.wait_terminal(terminal).await.unwrap_err().into();
    assert_eq!(find_durable_call_trap_context(&error), Some(context));
    assert!(matches!(
        error.root_cause().downcast_ref::<WorkerExecutorError>(),
        Some(WorkerExecutorError::Runtime { details }) if details == "terminal refused"
    ));
    drop(delivery);
    assert_eq!(counter.load(Ordering::Acquire), 0);
    assert!(rx.try_recv().is_err());
    assert!(oplog.entries.lock().await.is_empty());
}

#[test]
async fn completion_delivery_cancelled_terminal_wait_preserves_append_ownership() {
    for (teardown, terminal_fails) in [(false, false), (true, false), (false, true)] {
        let oplog = Arc::new(InMemoryOplog::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let mut token = live_delivery_token_with_teardown(
            oplog.clone(),
            counter.clone(),
            tx,
            TeardownProbe::new(shutdown.clone(), Arc::new(|| false)),
        )
        .await;
        let (release, gate) = tokio::sync::oneshot::channel();
        let terminal_oplog = oplog.clone();
        let terminal = tokio::spawn(async move {
            gate.await.unwrap();
            if terminal_fails {
                return Err(WorkerExecutorError::runtime("terminal refused"));
            }
            terminal_oplog
                .add(OplogEntry::End {
                    timestamp: Timestamp::now_utc(),
                    start_index: idx(1),
                    response: None,
                    forced_commit: false,
                    span_finished: None,
                    span_attributes: None,
                })
                .await?;
            Ok(())
        });
        let mut finishing = Box::pin(token.wait_terminal(terminal));
        assert!(futures::poll!(finishing.as_mut()).is_pending());
        drop(finishing);
        if teardown {
            shutdown.cancel();
        }
        drop(token);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        let DropEvent::AwaitCompletionMarker {
            mut receipt,
            live_call_permit,
            ..
        } = rx.try_recv().expect("settlement must retain the terminal")
        else {
            panic!("expected terminal settlement receipt");
        };
        let mut settlement = Box::pin(await_marker_receipt(receipt.as_mut().unwrap()));
        assert!(futures::poll!(settlement.as_mut()).is_pending());
        assert!(oplog.entries.lock().await.is_empty());
        release.send(()).unwrap();
        let result = settlement.await;
        assert_eq!(result.is_err(), terminal_fails);
        let entries = oplog.entries.lock().await;
        if terminal_fails {
            assert!(
                entries.is_empty(),
                "a failed End must not leave a discard marker"
            );
        } else if teardown {
            assert!(matches!(entries.as_slice(), [OplogEntry::End { .. }]));
        } else {
            assert!(matches!(entries.as_slice(), [
                OplogEntry::End { .. },
                OplogEntry::CompletionDiscarded { start_index, .. }
            ] if *start_index == idx(1)));
        }
        drop(live_call_permit);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }
}

#[test]
async fn completion_delivery_armed_drop_during_teardown_leaves_end_markerless() {
    // A Store unload (simulated crash, interrupt, eviction) or an executor shutdown tears the
    // delivering future with the token still armed. The guest never observed the completion,
    // so no `CompletionDiscarded` marker may be recorded: replay must tail-gate the markerless
    // `End` and deliver it. Only the permit is released; nothing is queued for the drain.
    for executor_shutdown in [false, true] {
        let oplog = Arc::new(InMemoryOplog::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let tearing_down = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = tearing_down.clone();
        let teardown = TeardownProbe::new(
            shutdown.clone(),
            Arc::new(move || probe.load(Ordering::Acquire)),
        );
        let token =
            live_delivery_token_with_teardown(oplog.clone(), counter.clone(), tx, teardown).await;
        assert_eq!(counter.load(Ordering::Acquire), 1);
        if executor_shutdown {
            shutdown.cancel();
        } else {
            tearing_down.store(true, Ordering::Release);
        }
        drop(token);
        tokio::task::yield_now().await;

        assert_eq!(
            counter.load(Ordering::Acquire),
            0,
            "the torn token must release the in-flight permit"
        );
        assert!(
            matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Disconnected)),
            "no marker receipt may be queued for the drain"
        );
        assert!(
            oplog.entries.lock().await.is_empty(),
            "a teardown tear must not append a CompletionDiscarded marker"
        );
    }
}

#[test]
async fn terminal_guard_torn_during_teardown_keeps_cleanup_but_no_marker() {
    // The same tear can hit the guard before it is converted into a token (the delivery future
    // is torn between the owned `End` append and `take_completion_delivery`). The queued
    // `CleanupAfterTerminal` event must still carry the terminal join and the permit, but the
    // armed discard marker must not be recorded.
    let oplog = Arc::new(InMemoryOplog::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tearing_down = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let probe = tearing_down.clone();
    {
        let mut guard = AccessTerminalGuard::<NotCancellable>::new(
            DroppedCall {
                start_idx: idx(1),
                begin_index: idx(1),
                function_type: DurableFunctionType::ReadLocal,
                request_upload: PendingUpload::already_durable(),
                span_finished: None,
                span_observer: None,
                trap_context: DurableCallTrapContext { retry_from: idx(1) },
                live_call_permit: Some(LiveCallPermit::new(counter.clone())),
            },
            NotCancellable::production_drop_sink(Some(tx.clone())),
            Some(tx),
            TeardownProbe::new(
                tokio_util::sync::CancellationToken::new(),
                Arc::new(move || probe.load(Ordering::Acquire)),
            ),
        );
        let terminal = tokio::spawn(async { Ok(()) });
        guard.cleanup_after_terminal(terminal, Some(marker_record_over(oplog.clone()).await));
        tearing_down.store(true, Ordering::Release);
    }
    let event = rx
        .try_recv()
        .expect("expected a CleanupAfterTerminal drop event");
    let DropEvent::CleanupAfterTerminal {
        terminal,
        live_call_permit,
        ..
    } = event
    else {
        panic!("expected CleanupAfterTerminal, got {event:?}");
    };
    assert_eq!(counter.load(Ordering::Acquire), 1);
    terminal
        .expect("the terminal join must still be handed to the drain")
        .await
        .expect("terminal task must not panic")
        .expect("terminal must succeed");
    drop(live_call_permit);
    assert_eq!(counter.load(Ordering::Acquire), 0);
    assert!(
        oplog.entries.lock().await.is_empty(),
        "a teardown tear must not append a CompletionDiscarded marker"
    );
}

#[test]
async fn completion_delivery_discarded_appends_marker_inline() {
    // `discarded` (caller-detected silent discard) appends exactly one marker inline and
    // returns once it is durable; the permit is released and no drain event is queued.
    let oplog = Arc::new(InMemoryOplog::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
    token.discarded().await.expect("discarded must succeed");
    {
        let entries = oplog.entries.lock().await;
        assert_eq!(entries.len(), 1, "expected exactly one marker entry");
        assert!(matches!(
            &entries[0],
            OplogEntry::CompletionDiscarded { start_index, .. } if *start_index == idx(1)
        ));
    }
    assert_eq!(counter.load(Ordering::Acquire), 0);
    assert!(rx.try_recv().is_err(), "no drain event may be queued");
}

#[test]
async fn completion_delivery_discarded_is_cancellation_safe() {
    // Tearing the future awaiting `discarded()` mid-wait must not lose the marker: the
    // owned append task keeps running, and the join plus the in-flight permit are handed to
    // the drain queue so settlement still waits for the marker.
    let (reached_tx, mut reached_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let oplog = Arc::new(InMemoryOplog::with_end_gate(reached_tx, gate.clone()));
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;

    let discard_task = tokio::spawn(async move { token.discarded().await });
    // The owned marker append is provably inside the gated `add`...
    reached_rx
        .recv()
        .await
        .expect("marker append must reach the gate");
    assert_eq!(counter.load(Ordering::Acquire), 1);
    assert!(rx.try_recv().is_err());

    // ...when the awaiting future is torn.
    discard_task.abort();
    let _ = discard_task.await;

    // The drain event owns the join and the permit; the marker still lands.
    match rx.try_recv() {
        Ok(DropEvent::AwaitCompletionMarker {
            mut receipt,
            live_call_permit,
            ..
        }) => {
            assert_eq!(
                counter.load(Ordering::Acquire),
                1,
                "the drain event must own the permit"
            );
            gate.add_permits(1);
            await_marker_receipt(
                receipt
                    .as_mut()
                    .expect("the drain event must carry the marker receipt"),
            )
            .await
            .expect("marker append must succeed");
            let entries = oplog.entries.lock().await;
            assert_eq!(entries.len(), 1, "expected exactly one marker entry");
            assert!(matches!(
                &entries[0],
                OplogEntry::CompletionDiscarded { start_index, .. } if *start_index == idx(1)
            ));
            drop(live_call_permit);
            assert_eq!(counter.load(Ordering::Acquire), 0);
        }
        other => panic!("expected an AwaitCompletionMarker drop event, got {other:?}"),
    }
}

#[test]
async fn completion_delivery_replay_tokens_are_inert() {
    // Replay tokens mirror the recorded delivery status and never touch the oplog again:
    // - `replay_discarded` reports the discard (the caller must not redeliver the recorded
    //   response) and consuming or dropping it records nothing;
    // - `replay_delivered` reports normal delivery and is equally inert.
    // Together with `replay_resolves_completed_but_discarded` (replay_state) and the live
    // marker tests above, this pins the full discard chain: a recorded
    // `[Start, End, CompletionDiscarded]` resolves to a discarded token, the call site skips
    // redelivery, and replay appends no second marker.
    let discarded = CompletionDelivery::replay_discarded();
    assert!(discarded.is_replay_discarded());
    assert!(!discarded.is_live_armed());
    discarded
        .discarded()
        .await
        .expect("discarding a replay-discarded token is a no-op");

    let delivered = CompletionDelivery::test_replay_delivered_immediate();
    assert!(!delivered.is_replay_discarded());
    assert!(!delivered.is_live_armed());
    delivered.delivered();

    // Dropping an unconsumed replay-discarded token is a no-op as well (the park path drops
    // the token after waiting for the guest to abandon the delivery boundary).
    drop(CompletionDelivery::replay_discarded());
}

/// Builds an in-memory oplog `[RecoverySucceeded, Start, End]` (a markerless completed call — the recorded
/// run crashed after the `End` became durable but before delivery), a replay state over it with
/// the call claimed and resolved (as `replay_access_deferred` guarantees), and the tail-gated
/// token exactly as the deferred accessor replay path constructs one.
async fn tail_gated_token_over_crash_tail(
    extra_tail: Vec<OplogEntry>,
    cleanup_sink: Option<mpsc::UnboundedSender<DropEvent>>,
) -> (Arc<InMemoryOplog>, ReplayState, CompletionDelivery) {
    let oplog = Arc::new(InMemoryOplog::new());
    oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            invocation_id: None,
            observational_owner: None,
            function_name: HostFunctionName::MonotonicClockNow,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadLocal,
            span_started: None,
        })
        .await
        .unwrap();
    oplog
        .add(OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: idx(2),
            response: Some(OplogPayload::Inline(Box::new(
                HostResponse::MonotonicClockTimestamp(HostResponseMonotonicClockTimestamp {
                    nanos: 42,
                }),
            ))),
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        })
        .await
        .unwrap();
    for entry in extra_tail {
        oplog.add(entry).await.unwrap();
    }
    let oplog_dyn: Arc<dyn Oplog> = oplog.clone();
    let replay_state = ReplayState::new_for_owner(
        golem_common::model::OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id: golem_common::model::AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "tail-gated-token-test".to_string(),
            },
        },
        oplog_dyn.clone(),
        golem_common::model::regions::DeletedRegions::default(),
        None,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        oplog_dyn.current_oplog_index().await,
        None,
    )
    .await
    .expect("failed to build replay state");
    let handle = replay_state
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .expect("claim must succeed");
    replay_state
        .await_resolution(handle)
        .await
        .expect("resolution must succeed");
    let token = CompletionDelivery::test_replay_at_tail(
        oplog_dyn,
        replay_state.clone(),
        idx(2),
        cleanup_sink,
    );
    (oplog, replay_state, token)
}

#[test]
async fn tail_gated_token_converts_to_live_and_delivered_records_marker() {
    // A markerless completed `End` replays tail-gated: `prepare_delivery` waits for the recorded
    // tail to exhaust naturally, then converts the token to live-armed so the eventual real
    // delivery records a durable `CompletionDelivered` marker — a second crash after the tail
    // delivery replays marker-gated instead of re-opening the crash window.
    let (oplog, replay_state, mut token) = tail_gated_token_over_crash_tail(
        vec![OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            level: LogLevel::Stdout,
            context: "stdout".to_string(),
            message: "crash tail hint".to_string(),
            trace_context: None,
        }],
        None,
    )
    .await;
    assert!(!token.is_live_armed());

    token
        .prepare_delivery(None)
        .await
        .expect("tail gating must succeed over a drainable tail");
    assert!(
        token.is_live_armed(),
        "the tail-gated token must convert to live-armed after the tail exhausts"
    );
    assert!(replay_state.is_live());

    token.delivered();
    // With no cleanup sink the marker append is only settlement-tracked via the drain queue;
    // poll the oplog for the spawned append.
    for _ in 0..100 {
        {
            let entries = oplog.entries.lock().await;
            if let Some(OplogEntry::CompletionDelivered { start_index, .. }) = entries.last() {
                assert_eq!(*start_index, idx(2));
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the delivered marker append never landed");
}

#[test]
async fn tail_gated_token_torn_after_conversion_records_discarded_marker() {
    // After tail conversion the token is a real live-armed token: tearing it (dropping the
    // delivering future) must record a `CompletionDiscarded` marker through the drain queue.
    let (oplog, _replay_state, mut token) = tail_gated_token_over_crash_tail(vec![], None).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    token
        .prepare_delivery(None)
        .await
        .expect("tail gating must succeed over a drainable tail");
    assert!(token.is_live_armed());
    if let CompletionDeliveryState::Live(live) = &mut token.state {
        live.cleanup_sink = Some(tx);
    }
    drop(token);

    match rx.try_recv() {
        Ok(DropEvent::AwaitCompletionMarker { mut receipt, .. }) => {
            await_marker_receipt(
                receipt
                    .as_mut()
                    .expect("the drain event must carry the marker receipt"),
            )
            .await
            .expect("marker append must succeed");
            let entries = oplog.entries.lock().await;
            match entries.last() {
                Some(OplogEntry::CompletionDiscarded { start_index, .. }) => {
                    assert_eq!(*start_index, idx(2))
                }
                other => panic!("expected CompletionDiscarded, got {other:?}"),
            }
        }
        other => panic!("expected an AwaitCompletionMarker drop event, got {other:?}"),
    }
}

#[test]
async fn tail_gated_token_delivered_without_prepare_poisons_replay() {
    // A delivery boundary firing while the completion is still tail-gated means the call site
    // never awaited `prepare_delivery`: replay must fail loudly instead of silently re-opening
    // the crash window the gate closes.
    let (_oplog, replay_state, token) = tail_gated_token_over_crash_tail(vec![], None).await;
    token.delivered();

    let err = replay_state
        .await_natural_tail_end(None)
        .await
        .expect_err("the poisoned cursor must reject further operations");
    assert!(err.to_string().contains("tail"), "unexpected error: {err}");
}

#[test]
async fn marker_gated_preparation_keeps_tail_activity_until_delivery() {
    use crate::durable_host::tail_work::TailWorkTracker;

    let (oplog, replay_state, mut token) = tail_gated_token_over_crash_tail(
        vec![
            OplogEntry::RemoveRetryPolicy {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                name: "test-policy".to_string(),
            },
            OplogEntry::CompletionDelivered {
                timestamp: Timestamp::now_utc(),
                start_index: idx(2),
            },
            OplogEntry::RemoveRetryPolicy {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                name: "test-policy".to_string(),
            },
        ],
        None,
    )
    .await;
    token.state = CompletionDeliveryState::ReplayDelivered(ReplayDelivery::AtMarker {
        replay_state: replay_state.clone(),
        start_index: idx(2),
        marker_index: idx(5),
    });
    let tracker = TailWorkTracker::new();
    let activity = tracker.activity();
    let mut preparation = Box::pin(token.prepare_delivery(Some(&activity)));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), preparation.as_mut())
            .await
            .is_err()
    );
    assert_eq!(
        tracker.active_count(),
        1,
        "recorded delivery must not be parked"
    );
    let (index, _) = replay_state.get_oplog_entry(None).await.unwrap();
    assert_eq!(index, idx(4));
    preparation.await.unwrap();
    assert_eq!(tracker.active_count(), 1);
    assert_eq!(replay_state.last_replayed_index(), idx(5));
    let mut next = Box::pin(replay_state.get_oplog_entry(None));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), next.as_mut())
            .await
            .is_err()
    );
    token.delivered();
    assert_eq!(next.await.unwrap().0, idx(6));
    assert_eq!(oplog.entries.lock().await.len(), 6, "replay adds no marker");
    drop(activity);
    assert_eq!(tracker.active_count(), 0);
}

/// Minimal in-memory [`Oplog`] recording appended entries, for tests that assert what a
/// drained drop event writes durably. With an `end_gate` installed, appending an `End` or
/// completion marker first signals `reached` and then blocks until the gate semaphore yields a
/// permit, so a test can hold the terminal or marker append open and observe what is (not) visible
/// meanwhile.
#[derive(Debug)]
pub(super) struct InMemoryOplog {
    entries: Arc<tokio::sync::Mutex<Vec<OplogEntry>>>,
    end_gate: Option<(mpsc::UnboundedSender<()>, Arc<tokio::sync::Semaphore>)>,
    next_reserved: Arc<std::sync::atomic::AtomicU64>,
    next_commit: Arc<tokio::sync::Mutex<u64>>,
    append_progress: Arc<tokio::sync::Notify>,
    /// When set, every append is refused the way a fencing backend refuses one whose asserted
    /// shard epoch is stale: the shard moved to another executor while this oplog was open.
    fence: Option<crate::services::oplog::OplogFence>,
}

impl InMemoryOplog {
    pub(super) fn new() -> Self {
        Self {
            entries: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            end_gate: None,
            next_reserved: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            next_commit: Arc::new(tokio::sync::Mutex::new(1)),
            append_progress: Arc::new(tokio::sync::Notify::new()),
            fence: None,
        }
    }

    /// An oplog whose shard has already moved: every append is refused with `fence`.
    fn fenced(fence: crate::services::oplog::OplogFence) -> Self {
        Self {
            fence: Some(fence),
            ..Self::new()
        }
    }

    pub(super) fn with_end_gate(
        reached: mpsc::UnboundedSender<()>,
        gate: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            entries: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            end_gate: Some((reached, gate)),
            next_reserved: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            next_commit: Arc::new(tokio::sync::Mutex::new(1)),
            append_progress: Arc::new(tokio::sync::Notify::new()),
            fence: None,
        }
    }
}

#[async_trait]
impl Oplog for InMemoryOplog {
    async fn add(
        &self,
        entry: OplogEntry,
    ) -> Result<OplogIndex, crate::services::oplog::OplogError> {
        self.enqueue_add(entry).await
    }

    fn enqueue_add(&self, entry: OplogEntry) -> crate::services::oplog::OplogAddReceipt {
        if let Some(fence) = self.fence.clone() {
            return Box::pin(async move { Err(crate::services::oplog::OplogError::Fenced(fence)) });
        }
        let index = self
            .next_reserved
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let entries = self.entries.clone();
        let end_gate = self.end_gate.clone();
        let next_commit = self.next_commit.clone();
        let append_progress = self.append_progress.clone();
        let (done, receipt) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            loop {
                let progress = append_progress.notified();
                tokio::pin!(progress);
                progress.as_mut().enable();
                let mut next = next_commit.lock().await;
                if *next == index {
                    if matches!(
                        &entry,
                        OplogEntry::End { .. }
                            | OplogEntry::CompletionDiscarded { .. }
                            | OplogEntry::CompletionDelivered { .. }
                    ) && let Some((reached, gate)) = &end_gate
                    {
                        let _ = reached.send(());
                        gate.acquire()
                            .await
                            .expect("end gate semaphore is never closed")
                            .forget();
                    }
                    entries.lock().await.push(entry);
                    *next += 1;
                    drop(next);
                    append_progress.notify_waiters();
                    let _ = done.send(OplogIndex::from_u64(index));
                    return;
                }
                drop(next);
                progress.await;
            }
        });
        Box::pin(async move {
            Ok({
                receipt
                    .await
                    .expect("the in-memory oplog append task must reply")
            })
        })
    }

    fn enqueue_add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> crate::services::oplog::OplogAddPairReceipt {
        if let Some(fence) = self.fence.clone() {
            return Box::pin(async move { Err(crate::services::oplog::OplogError::Fenced(fence)) });
        }
        let first_index = self
            .next_reserved
            .fetch_add(2, std::sync::atomic::Ordering::Relaxed);
        let second_index = first_index + 1;
        let entries = self.entries.clone();
        let end_gate = self.end_gate.clone();
        let next_commit = self.next_commit.clone();
        let append_progress = self.append_progress.clone();
        let (done, receipt) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            loop {
                let progress = append_progress.notified();
                tokio::pin!(progress);
                progress.as_mut().enable();
                let mut next = next_commit.lock().await;
                if *next == first_index {
                    if let Some((reached, gate)) = &end_gate {
                        let _ = reached.send(());
                        gate.acquire()
                            .await
                            .expect("end gate semaphore is never closed")
                            .forget();
                    }
                    let first_index = OplogIndex::from_u64(first_index);
                    let second = make_second(first_index);
                    entries.lock().await.extend([start, second]);
                    *next += 2;
                    drop(next);
                    append_progress.notify_waiters();
                    let _ = done.send((first_index, OplogIndex::from_u64(second_index)));
                    return;
                }
                drop(next);
                progress.await;
            }
        });
        Box::pin(async move {
            Ok(receipt
                .await
                .expect("the in-memory oplog pair append task must reply"))
        })
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: Box<
            dyn FnOnce(golem_common::model::oplog::RawOplogPayload) -> Result<OplogEntry, String>
                + Send,
        >,
    ) -> Result<crate::services::oplog::OrderedOplogStart, crate::services::oplog::OplogError> {
        let entry = build_start(
            golem_common::model::oplog::RawOplogPayload::SerializedInline(serialized_request),
        )?;
        let index = self.add(entry.clone()).await?;
        Ok(crate::services::oplog::OrderedOplogStart {
            index,
            entry,
            pending_upload: PendingUpload::already_durable(),
        })
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: crate::services::oplog::IndexedReservedStartBuilder,
    ) -> Result<crate::services::oplog::OrderedOplogStart, crate::services::oplog::OplogError> {
        let mut entries = self.entries.lock().await;
        let index = OplogIndex::from_u64(entries.len() as u64 + 1);
        let (serialized_request, build_start) = build_request(index)?;
        let entry = build_start(
            golem_common::model::oplog::RawOplogPayload::SerializedInline(serialized_request),
        )?;
        entries.push(entry.clone());
        Ok(crate::services::oplog::OrderedOplogStart {
            index,
            entry,
            pending_upload: PendingUpload::already_durable(),
        })
    }

    async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
        0
    }

    async fn commit(
        &self,
        _level: CommitLevel,
    ) -> Result<
        std::collections::BTreeMap<OplogIndex, OplogEntry>,
        crate::services::oplog::OplogError,
    > {
        Ok(std::collections::BTreeMap::new())
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        OplogIndex::from_u64(self.entries.lock().await.len() as u64)
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        None
    }

    async fn wait_for_replicas(
        &self,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<bool, crate::services::oplog::OplogError> {
        Ok(true)
    }

    async fn read_exact(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
        let entries = self.entries.lock().await;
        let start: u64 = oplog_index.into();
        let mut result = std::collections::BTreeMap::new();
        for i in start..(start + n) {
            let entry = entries.get((i - 1) as usize).unwrap_or_else(|| {
                panic!(
                    "Missing oplog entry in exact range [{oplog_index}..={}]",
                    OplogIndex::from_u64(start + n - 1)
                )
            });
            result.insert(OplogIndex::from_u64(i), entry.clone());
        }
        result
    }

    async fn length(&self) -> u64 {
        self.entries.lock().await.len() as u64
    }

    async fn upload_raw_payload(
        &self,
        data: Vec<u8>,
    ) -> Result<golem_common::model::oplog::RawOplogPayload, String> {
        Ok(golem_common::model::oplog::RawOplogPayload::SerializedInline(data))
    }

    async fn download_raw_payload(
        &self,
        _payload_id: golem_common::model::oplog::PayloadId,
        _md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        unimplemented!()
    }
}

#[test]
async fn dropped_cancellable_call_records_cancelled_at_next_drain_point() {
    // A live `Cancellable` call dropped mid-flight enqueues an `UnfinishedCancellable`
    // snapshot; this test drives the drain's *recording step* directly
    // (`DroppedCall::append_cancelled_with_oplog`, shared by both production drains) and
    // asserts the durable outcome. Invoking a production drain itself, and the ctx-side
    // effects (durable-scope close), require full-worker
    // integration coverage.
    let oplog = Arc::new(InMemoryOplog::new());
    let start_idx = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::MonotonicClockNow,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                golem_common::model::oplog::HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadLocal,
            span_started: None,
        })
        .await
        .unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let _handle = live_unfinished_handle::<Cancellable>(start_idx, tx);
        // Dropped mid-flight here, e.g. by a guest cancelling the subtask driving the call.
    }

    let call = match rx.try_recv() {
        Ok(DropEvent::UnfinishedCancellable { call }) => call,
        other => panic!("expected an UnfinishedCancellable drop event, got {other:?}"),
    };

    // The drain point: wait for the request upload, then record the terminal.
    call.wait_request_upload()
        .await
        .expect("request upload must succeed");
    call.append_cancelled_with_oplog(oplog.clone(), None)
        .await
        .expect("recording Cancelled must succeed");

    let entries = oplog.entries.lock().await;
    assert_eq!(entries.len(), 2, "expected [Start, Cancelled]");
    match &entries[1] {
        OplogEntry::Cancelled {
            start_index,
            partial,
            ..
        } => {
            assert_eq!(*start_index, start_idx);
            assert!(partial.is_none());
        }
        other => panic!("expected Cancelled, got {other:?}"),
    }
}

#[test]
fn dropped_call_span_closes_at_guest_drop_not_deferred_drain() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let span_id = SpanId::generate();
    let mut handle = live_unfinished_handle::<Cancellable>(idx(1), tx);
    handle.close_span_on_cancellation(span_id.clone());

    let before_drop = Timestamp::now_utc();
    drop(handle);
    let DropEvent::UnfinishedCancellable { call } = rx
        .try_recv()
        .expect("dropping the call must enqueue its cancellation snapshot")
    else {
        panic!("expected an UnfinishedCancellable drop event");
    };
    let after_receiving_event = Timestamp::now_utc();

    let span_finished = call
        .span_finished
        .expect("the cancellation snapshot must close the span");
    assert_eq!(span_finished.span_id, span_id);
    assert!(span_finished.finished_at >= before_drop);
    assert!(span_finished.finished_at <= after_receiving_event);
}

#[derive(Debug, Default)]
struct RecordingSpanObserver {
    closes: AtomicUsize,
}

impl CallSpanObserver for RecordingSpanObserver {
    fn started(self: Arc<Self>, _index: OplogIndex, _entry: &OplogEntry, _replay: bool) {}

    fn closed(&self, _finished: &golem_common::model::oplog::SpanFinished) {
        self.closes.fetch_add(1, Ordering::AcqRel);
    }
}

#[test]
fn span_observer_does_not_change_the_unfinished_call_policy() {
    fn check<P: DropPolicy>(cancellable: bool, teardown: bool) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let observer = Arc::new(RecordingSpanObserver::default());
        let mut handle = live_unfinished_handle::<P>(idx(12), tx);
        handle.close_span_on_cancellation(SpanId::generate());
        handle.span_observer = Some(observer.clone());
        handle.runtime_teardown = Arc::new(move || teardown);
        drop(handle);
        let closes = cancellable && !teardown;
        assert_eq!(observer.closes.load(Ordering::Acquire), usize::from(closes));
        let event = rx.try_recv();
        assert_eq!(event.is_ok(), closes);
        if let Ok(event) = event {
            assert!(matches!(event, DropEvent::UnfinishedCancellable { .. }));
        }
    }
    check::<Cancellable>(true, false);
    check::<Cancellable>(true, true);
    check::<LeaveIncompleteOnDrop>(false, false);
    check::<LeaveIncompleteOnDrop>(false, true);
}

#[test]
async fn access_terminal_end_is_appended_before_cleanup_and_permit_release() {
    // Ordering is proven against the production persistence stage itself
    // (`DurableCallSession::persist_access_terminal`, the exact code `complete_access_impl` runs for a
    // persisted live call): while the terminal `End` append is still in flight, the live-call
    // permit must stay held and no cleanup event may become visible; both are released only
    // after the append completes (production releases them via `disarm()` after
    // `end_durable_function_access`, strictly downstream of `wait_terminal()`).
    use golem_common::model::oplog::HostResponseP3HttpClientConsumeBodyChunk;
    use golem_common::model::oplog::payload::types::SerializableP3HttpBodyChunk;

    let (reached_tx, mut reached_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let oplog = Arc::new(InMemoryOplog::with_end_gate(reached_tx, gate.clone()));
    let start_idx = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::P3HttpClientConsumeBodyChunk,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                golem_common::model::oplog::HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadRemote,
            span_started: None,
        })
        .await
        .unwrap();

    let permit_counter = Arc::new(AtomicUsize::new(0));
    let (cleanup_tx, mut cleanup_rx) = mpsc::unbounded_channel();
    let span_observer = Arc::new(RecordingSpanObserver::default());
    let mut guard = AccessTerminalGuard::<NotCancellable>::new(
        DroppedCall {
            start_idx,
            begin_index: start_idx,
            function_type: DurableFunctionType::ReadRemote,
            request_upload: PendingUpload::already_durable(),
            span_finished: None,
            span_observer: Some(span_observer.clone()),
            trap_context: DurableCallTrapContext {
                retry_from: start_idx,
            },
            live_call_permit: Some(LiveCallPermit::new(permit_counter.clone())),
        },
        NotCancellable::production_drop_sink(Some(cleanup_tx.clone())),
        Some(cleanup_tx),
        TeardownProbe::never(),
    );
    assert_eq!(permit_counter.load(Ordering::Acquire), 1);

    let persist_oplog: Arc<dyn Oplog> = oplog.clone();
    let persist_replay_state = ReplayState::new_for_owner(
        golem_common::model::OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id: golem_common::model::AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "concurrent-test".to_string(),
            },
        },
        persist_oplog.clone(),
        golem_common::model::regions::DeletedRegions::default(),
        None,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        persist_oplog.current_oplog_index().await,
        None,
    )
    .await
    .expect("failed to build replay state");
    let completion_marker_recorder =
        CompletionMarkerRecorder::new(persist_oplog.clone(), persist_replay_state);
    let bytes = vec![42u8; 4096];
    let original_ptr = bytes.as_ptr() as usize;
    let persist = tokio::spawn(async move {
        let response = HostResponseP3HttpClientConsumeBodyChunk {
            chunk: SerializableP3HttpBodyChunk::Data(bytes),
        };
        let result = DurableCallSession::<
            host_functions::P3HttpClientConsumeBodyChunk,
            NotCancellable,
        >::persist_access_terminal(
            persist_oplog,
            Some(completion_marker_recorder),
            &mut guard,
            start_idx,
            response,
            Some(golem_common::model::oplog::SpanFinished {
                span_id: SpanId::generate(),
                finished_at: Timestamp::now_utc(),
                outcome: golem_common::model::oplog::SpanOutcome::Failed,
            }),
        )
        .await;
        (result, guard)
    });

    // The owned terminal task is now provably inside the gated `End` append...
    reached_rx
        .recv()
        .await
        .expect("terminal append must reach the gate");
    assert_eq!(
        span_observer.closes.load(Ordering::Acquire),
        1,
        "the End owns its span close before the append receipt resolves"
    );
    // ...and while it is held open, the call is still counted as in flight and no cleanup
    // event has escaped, so a positional boundary cannot slip in before the terminal.
    assert_eq!(
        permit_counter.load(Ordering::Acquire),
        1,
        "live-call permit must be held while the terminal append is pending"
    );
    assert!(
        cleanup_rx.try_recv().is_err(),
        "no cleanup event may be visible while the terminal append is pending"
    );
    assert_eq!(
        oplog.entries.lock().await.len(),
        1,
        "the End entry must not be durable yet"
    );

    gate.add_permits(1);
    let (result, mut guard) = persist.await.expect("persist task must not panic");
    let response = result.expect("persisting the terminal must succeed");
    let SerializableP3HttpBodyChunk::Data(bytes) = &response.chunk else {
        panic!("expected the original data chunk");
    };
    assert_eq!(bytes.as_ptr() as usize, original_ptr);

    // The terminal is durably appended when the persistence stage returns...
    let payload = {
        let entries = oplog.entries.lock().await;
        assert_eq!(entries.len(), 2, "expected [Start, End]");
        match &entries[1] {
            OplogEntry::End {
                start_index,
                response: Some(payload),
                ..
            } => {
                assert_eq!(*start_index, start_idx);
                assert!(matches!(
                    payload,
                    OplogPayload::SerializedInline { cached: None, .. }
                ));
                payload.clone()
            }
            other => panic!("expected End, got {other:?}"),
        }
    };
    let decoded = oplog
        .download_payload(payload)
        .await
        .expect("uncached response must decode");
    assert_eq!(decoded, response.into());
    // ...while the guard still owns the permit and nothing has been queued: release happens
    // only at the production `disarm()`, strictly after the terminal.
    assert_eq!(
        permit_counter.load(Ordering::Acquire),
        1,
        "the guard must still own the permit after the terminal is durable"
    );
    assert!(cleanup_rx.try_recv().is_err());

    guard.disarm();
    assert_eq!(
        permit_counter.load(Ordering::Acquire),
        0,
        "disarm after the terminal releases the in-flight count"
    );
    assert!(cleanup_rx.try_recv().is_err());
}

#[test]
fn dropped_unfinished_call_keeps_live_permit_until_drop_event_is_consumed() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let mut handle = live_unfinished_handle::<Cancellable>(idx(5), tx);
        handle.live_call_permit = Some(LiveCallPermit::new(counter.clone()));
        assert_eq!(counter.load(Ordering::Acquire), 1);
    }
    // The handle is gone, but the queued drop event now owns the permit: an in-flight check
    // A boundary guard running between the drop and
    // the next drain must still count this call.
    assert_eq!(counter.load(Ordering::Acquire), 1);
    let event = rx
        .try_recv()
        .expect("expected an UnfinishedCancellable drop event");
    assert_eq!(counter.load(Ordering::Acquire), 1);
    drop(event);
    assert_eq!(counter.load(Ordering::Acquire), 0);
}

#[test]
fn drop_cancellable_unfinished_enqueues_exactly_one_cancelled() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let _handle = live_unfinished_handle::<Cancellable>(idx(5), tx);
    }
    match rx.try_recv() {
        Ok(DropEvent::UnfinishedCancellable { call }) => {
            assert_eq!(call.start_idx(), idx(5));
            assert_eq!(call.trap_context().retry_from, idx(5));
        }
        other => panic!("expected one UnfinishedCancellable, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "expected exactly one drop event");
}

#[test]
fn drop_cancellable_unfinished_carries_call_owned_cancellation_state() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let _handle = live_unfinished_handle::<Cancellable>(idx(5), tx);
    }
    match rx.try_recv() {
        Ok(DropEvent::UnfinishedCancellable { call }) => {
            assert_eq!(call.start_idx(), idx(5));
            assert_eq!(call.trap_context().retry_from, idx(5));
        }
        other => panic!("expected one UnfinishedCancellable, got {other:?}"),
    }
}

#[test]
fn drop_not_cancellable_unfinished_signals_policy_violation() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let _handle = live_unfinished_handle::<NotCancellable>(idx(7), tx);
    }
    match rx.try_recv() {
        Ok(DropEvent::UnfinishedNotCancellable { call }) => {
            assert_eq!(call.start_idx(), idx(7))
        }
        other => panic!("expected UnfinishedNotCancellable, got {other:?}"),
    }
}

#[test]
fn executor_shutdown_leaves_unfinished_call_incomplete_for_replay() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let handle = live_unfinished_handle::<NotCancellable>(idx(8), tx);
        handle.executor_shutdown.cancel();
    }
    assert!(
        rx.try_recv().is_err(),
        "executor shutdown must not report cancellation or a policy violation"
    );
}

#[test]
fn owner_teardown_leaves_unfinished_call_incomplete_without_cancellation() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut handle = live_unfinished_handle::<Cancellable>(idx(9), tx);
    handle.close_span_on_cancellation(SpanId::generate());
    handle.runtime_teardown = Arc::new(|| true);
    drop(handle);
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut handle = live_unfinished_handle::<NotCancellable>(idx(10), tx);
    handle.drop_sink = None;
    handle.runtime_teardown = Arc::new(|| true);
    drop(handle);
}

#[test]
fn terminal_guard_distinguishes_guest_drop_from_owner_teardown() {
    for teardown in [false, true] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let span_id = SpanId::generate();
        let mut handle = live_unfinished_handle::<Cancellable>(idx(23), tx.clone());
        handle.close_span_on_cancellation(span_id.clone());
        drop(handle);
        let DropEvent::UnfinishedCancellable { call } = rx.try_recv().unwrap() else {
            panic!("expected a call snapshot");
        };
        let fenced = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = fenced.clone();
        let guard = AccessTerminalGuard::<Cancellable>::new(
            call,
            Some(tx.clone()),
            None,
            TeardownProbe::new(
                tokio_util::sync::CancellationToken::new(),
                Arc::new(move || probe.load(Ordering::Acquire)),
            ),
        );
        fenced.store(teardown, Ordering::Release);
        drop(guard);
        if teardown {
            assert!(matches!(
                rx.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
        } else {
            let DropEvent::UnfinishedCancellable { call } = rx.try_recv().unwrap() else {
                panic!("terminal guard must preserve the existing cancellation policy");
            };
            assert_eq!(call.span_finished.unwrap().span_id, span_id);
            assert!(rx.try_recv().is_err());
        }
    }
}

#[test]
fn drop_after_finish_is_noop() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let mut handle = live_unfinished_handle::<Cancellable>(idx(9), tx);
        handle.finished = true;
    }
    assert!(
        rx.try_recv().is_err(),
        "finished handle must not emit a drop event"
    );
}

#[test]
fn seam2_unfinished_cancellable_drops_emit_independent_call_snapshots() {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let handle_a = live_unfinished_handle::<Cancellable>(idx(5), tx.clone());
    let handle_b = live_unfinished_handle::<Cancellable>(idx(9), tx);

    drop(handle_a);
    drop(handle_b);

    match rx.try_recv() {
        Ok(DropEvent::UnfinishedCancellable { call }) => {
            assert_eq!(call.start_idx(), idx(5));
            assert_eq!(call.trap_context().retry_from, idx(5));
        }
        other => panic!("expected first cancellable drop snapshot, got {other:?}"),
    }
    match rx.try_recv() {
        Ok(DropEvent::UnfinishedCancellable { call }) => {
            assert_eq!(call.start_idx(), idx(9));
            assert_eq!(call.trap_context().retry_from, idx(9));
        }
        other => panic!("expected second cancellable drop snapshot, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "expected exactly two drop events");
}

#[test]
fn abandon_for_trap_suppresses_unfinished_drop() {
    // A trap-abandoned NotCancellable handle must NOT trip the unfinished-drop policy: the
    // host-call Start is intentionally left incomplete for replay/retry, not cancelled.
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let mut handle = live_unfinished_handle::<NotCancellable>(idx(11), tx);
        handle.abandon_for_trap();
    }
    assert!(
        rx.try_recv().is_err(),
        "abandon_for_trap must not emit a drop event"
    );
}

// ---- call-scoped retry host ----

struct RetryHostProbe {
    current_retry_point: OplogIndex,
    appended_retry_from: Vec<OplogIndex>,
}

impl RetryHostProbe {
    fn new(current_retry_point: OplogIndex) -> Self {
        Self {
            current_retry_point,
            appended_retry_from: Vec::new(),
        }
    }
}

#[async_trait]
impl InFunctionRetryHost for RetryHostProbe {
    fn current_retry_point(&self) -> OplogIndex {
        self.current_retry_point
    }

    async fn named_retry_policies(&mut self) -> Vec<golem_common::model::NamedRetryPolicy> {
        vec![NamedRetryPolicy {
            name: "test".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 1,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        }]
    }

    async fn current_retry_state_for(
        &self,
        retry_from: OplogIndex,
    ) -> Option<golem_common::model::RetryPolicyState> {
        if retry_from == self.current_retry_point {
            Some(RetryPolicyState::CountBox {
                attempts: 1,
                inner: Box::new(RetryPolicyState::Counter(1)),
            })
        } else {
            None
        }
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        durable_execution_state()
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        _retry_policy_state: Option<golem_common::model::RetryPolicyState>,
    ) -> Result<(), crate::services::oplog::OplogError> {
        self.appended_retry_from.push(retry_from);
        Ok(())
    }
}

#[test]
fn scoped_retry_host_uses_call_retry_point_not_inner_current() {
    let mut inner = RetryHostProbe::new(idx(99));
    let scope = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        observational_owner: None,
        entity_parent_start_index: None,
        invocation_id: None,
    };

    let retry_host = ScopedRetryHost::new(&mut inner, &scope);

    assert_eq!(retry_host.current_retry_point(), idx(42));
}

#[test]
async fn scoped_retry_host_trap_retry_uses_call_retry_point() {
    let mut inner = RetryHostProbe::new(idx(99));
    let scope = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        observational_owner: None,
        entity_parent_start_index: None,
        invocation_id: None,
    };
    let mut retry_host = ScopedRetryHost::new(&mut inner, &scope);

    let outcome = try_trigger_host_trap_retry(
        &mut retry_host,
        Error::msg("transient failure"),
        RetryProperties::new(),
    )
    .await;

    assert!(
        outcome.is_err(),
        "empty state at the call-owned retry point should allow one retry; using the inner/global retry point would be exhausted"
    );
}

#[test]
async fn seam2_overlapping_semantic_traps_carry_independent_retry_points() {
    let mut inner = RetryHostProbe::new(idx(99));
    let scope_a = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        observational_owner: None,
        entity_parent_start_index: None,
        invocation_id: None,
    };
    let scope_b = CallExecutionScope {
        retry_from: idx(77),
        durable_scope: Some(idx(70)),
        observational_owner: None,
        entity_parent_start_index: None,
        invocation_id: None,
    };

    let error_a = {
        let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_a);
        try_trigger_host_trap_retry(
            &mut retry_host,
            Error::msg("transient failure a"),
            RetryProperties::new(),
        )
        .await
        .expect_err("call A should trap for oplog-level retry")
    };

    let error_b = {
        let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_b);
        try_trigger_host_trap_retry(
            &mut retry_host,
            Error::msg("transient failure b"),
            RetryProperties::new(),
        )
        .await
        .expect_err("call B should trap for oplog-level retry")
    };

    let override_a = crate::durable_host::durability::find_semantic_trap_retry_override(&error_a)
        .expect("call A trap must carry a semantic retry override");
    let override_b = crate::durable_host::durability::find_semantic_trap_retry_override(&error_b)
        .expect("call B trap must carry a semantic retry override");

    assert_eq!(override_a.retry_from, idx(42));
    assert_eq!(override_b.retry_from, idx(77));
}

// ---- terminal-step failures ----

/// Classifies a terminal-step failure the way the invocation loop does, with a deliberately
/// *hostile* ambient retry point belonging to no real call. A call-owned
/// [`DurableCallTrapContext`] marker must override the ambient fallback.
fn classify_with_hostile_ambient(error: &anyhow::Error) -> crate::model::TrapType {
    crate::model::TrapType::from_error::<crate::workerctx::default::Context>(
        error,
        idx(99),
        golem_common::model::agent::AgentMode::Durable,
    )
}

#[test]
fn seam2_terminal_failure_carries_call_owned_trap_context() {
    // Both terminal-step errors and explicit host traps retain the call's retry point.
    let scope = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        observational_owner: None,
        entity_parent_start_index: None,
        invocation_id: None,
    };
    let mut handle = synthetic_finished_handle_with_scope::<Cancellable>(scope);
    let terminal_error: anyhow::Error = TerminalCallError::new(
        WorkerExecutorError::runtime("terminal step failure"),
        handle.trap_context(),
    )
    .into();
    let trap_error = handle.trap(WorkerExecutorError::runtime("wakeup scheduling failed"));

    for error in [terminal_error, trap_error] {
        match classify_with_hostile_ambient(&error) {
            crate::model::TrapType::Error { retry_from, .. } => {
                assert_eq!(retry_from, idx(42));
            }
            other => panic!("expected TrapType::Error, got {other:?}"),
        }
    }
}

#[test]
fn seam2_suspend_trap_remains_interrupt_without_cancellation() {
    let (drop_tx, mut drop_rx) = mpsc::unbounded_channel();
    let mut handle = live_unfinished_handle::<Cancellable>(idx(42), drop_tx);
    let suspend = InterruptKind::Suspend(Timestamp::now_utc());

    let error = handle.trap(suspend);

    assert!(matches!(
        classify_with_hostile_ambient(&error),
        crate::model::TrapType::Interrupt(kind) if kind == suspend
    ));
    drop(handle);
    assert!(
        drop_rx.try_recv().is_err(),
        "abandoning for Suspend must not enqueue a Cancelled event"
    );
}

#[test]
fn seam2_overlapping_terminal_failures_carry_independent_trap_contexts() {
    // Two durable calls overlap in flight and share the same ambient worker state. Each one's
    // terminal-step failure must still be classified against its OWN retry point.
    // Without the call-owned marker these would fall back to that
    // shared ambient state, so an overlapping sibling could clobber the grouping.
    let scope_a = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        observational_owner: None,
        entity_parent_start_index: None,
        invocation_id: None,
    };
    let scope_b = CallExecutionScope {
        retry_from: idx(77),
        durable_scope: Some(idx(70)),
        observational_owner: None,
        entity_parent_start_index: None,
        invocation_id: None,
    };
    let handle_a = synthetic_finished_handle_with_scope::<Cancellable>(scope_a);
    let handle_b = synthetic_finished_handle_with_scope::<Cancellable>(scope_b);

    let error_a: anyhow::Error =
        TerminalCallError::new(WorkerExecutorError::runtime("a"), handle_a.trap_context()).into();
    let error_b: anyhow::Error =
        TerminalCallError::new(WorkerExecutorError::runtime("b"), handle_b.trap_context()).into();

    // Both classified against the SAME hostile ambient state, yet each keeps its own grouping.
    match classify_with_hostile_ambient(&error_a) {
        crate::model::TrapType::Error { retry_from, .. } => {
            assert_eq!(retry_from, idx(42));
        }
        other => panic!("expected TrapType::Error for call A, got {other:?}"),
    }
    match classify_with_hostile_ambient(&error_b) {
        crate::model::TrapType::Error { retry_from, .. } => {
            assert_eq!(retry_from, idx(77));
        }
        other => panic!("expected TrapType::Error for call B, got {other:?}"),
    }
}

#[test]
fn seam2_dropped_call_drain_failure_uses_dropped_call_trap_context() {
    // A cancellation-drain failure (deferred request upload / terminal recorder join) is driven
    // by whichever later host call happens to run the drain, but it must be classified with the
    // *dropped* call's captured context, not the drainer's ambient state. `DroppedCall::trap_context`
    // carries that captured classification; the drain wraps failures with it via
    // `TerminalCallError`.
    let dropped_a = DroppedCall {
        start_idx: idx(5),
        begin_index: idx(4),
        function_type: DurableFunctionType::ReadRemote,
        request_upload: PendingUpload::already_durable(),
        span_finished: None,
        span_observer: None,
        trap_context: DurableCallTrapContext { retry_from: idx(4) },
        live_call_permit: None,
    };
    let dropped_b = DroppedCall {
        start_idx: idx(8),
        begin_index: idx(8),
        function_type: DurableFunctionType::ReadRemote,
        request_upload: PendingUpload::already_durable(),
        span_finished: None,
        span_observer: None,
        trap_context: DurableCallTrapContext { retry_from: idx(8) },
        live_call_permit: None,
    };

    for (dropped, expected_retry) in [(dropped_a, idx(4)), (dropped_b, idx(8))] {
        let error: anyhow::Error = TerminalCallError::new(
            WorkerExecutorError::runtime("cancellation drain failed"),
            dropped.trap_context(),
        )
        .into();

        // `classify_with_hostile_ambient` supplies idx(99) as the
        // drainer's ambient state; the dropped call's own context must win.
        match classify_with_hostile_ambient(&error) {
            crate::model::TrapType::Error { retry_from, .. } => {
                assert_eq!(retry_from, expected_retry);
            }
            other => panic!("expected TrapType::Error, got {other:?}"),
        }
    }
}

// ---- call-owned execution scope ----

#[test]
fn begun_execution_scope_uses_parent_scope_as_retry_from() {
    let begun = BegunCallExecutionScope {
        parent_start_index: Some(idx(10)),
        observational_owner: None,
        invocation_id: None,
        entity_parent_start_index: None,
    };

    let scope = begun.finish(idx(11));

    assert_eq!(scope.retry_from, idx(10));
    assert_eq!(scope.durable_scope, Some(idx(10)));
}

#[test]
fn begun_execution_scope_uses_call_start_as_retry_from_when_unscoped() {
    let begun = BegunCallExecutionScope {
        parent_start_index: None,
        observational_owner: None,
        invocation_id: None,
        entity_parent_start_index: None,
    };

    let scope = begun.finish(idx(12));

    assert_eq!(scope.retry_from, idx(12));
    assert_eq!(scope.durable_scope, None);
}

#[test]
fn begun_observational_scope_uses_custom_owner_as_retry_from() {
    let begun = BegunCallExecutionScope {
        parent_start_index: Some(idx(10)),
        observational_owner: Some(idx(7)),
        invocation_id: None,
        entity_parent_start_index: None,
    };

    let scope = begun.finish(idx(11));

    assert_eq!(scope.retry_from, idx(7));
    assert_eq!(scope.trap_retry_point(), idx(7));
    assert_eq!(scope.durable_scope, Some(idx(10)));
    assert_eq!(scope.observational_owner, Some(idx(7)));
}

#[test]
fn call_execution_scope_owns_call_retry_point() {
    let scope = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        observational_owner: None,
        entity_parent_start_index: None,
        invocation_id: None,
    };

    assert_eq!(scope.retry_from, idx(42));
}

// ---- function-type re-execution policy ----

#[test]
fn can_reexecute_matches_internal_retry_eligibility() {
    use crate::durable_host::durability::{DurableExecutionState, InFunctionRetryController};

    fn controller(ft: DurableFunctionType) -> InFunctionRetryController {
        InFunctionRetryController::new(
            ft,
            DurableExecutionState {
                is_live: true,
                snapshotting_mode: false,
                max_in_function_retry_delay: Duration::ZERO,
            },
            "test:fn",
        )
    }

    // Reads and local/idempotent writes are safe to re-execute on an incomplete Start.
    assert!(controller(DurableFunctionType::ReadLocal).can_reexecute_on_incomplete_replay());
    assert!(controller(DurableFunctionType::ReadRemote).can_reexecute_on_incomplete_replay());
    assert!(controller(DurableFunctionType::WriteLocal).can_reexecute_on_incomplete_replay());
    assert!(controller(DurableFunctionType::WriteRemote).can_reexecute_on_incomplete_replay());
}

// ---- replay resolution classification ----

fn payload(bytes: Vec<u8>) -> OplogPayload<HostResponse> {
    OplogPayload::SerializedInline {
        bytes,
        cached: None,
    }
}

#[test]
fn classify_incomplete_is_incomplete() {
    match classify_replay_resolution(ResolutionOutcome::Incomplete) {
        ReplayedResolution::Incomplete => {}
        other => panic!("expected Incomplete, got {other:?}"),
    }
}

#[test]
fn classify_completed_is_delivered() {
    let outcome = ResolutionOutcome::Resolved(Resolution::Completed {
        end_idx: idx(3),
        response: Some(payload(vec![1, 2, 3])),
        delivery_marker: None,
        forced_commit: false,
    });
    match classify_replay_resolution(outcome) {
        ReplayedResolution::Delivered(
            ReplayedPayload::Completed(Some(p)),
            ReplayDeliveryDisposition::AtReplayTail,
        ) => {
            assert_eq!(p, payload(vec![1, 2, 3]), "payload preserved")
        }
        other => panic!("expected Delivered(Completed), got {other:?}"),
    }
}

#[test]
fn classify_completed_without_response_payload_is_delivered() {
    // A missing `End` response payload is only rejected later, by the decode step
    // (`decode_completed_response`), not by classification.
    let outcome = ResolutionOutcome::Resolved(Resolution::Completed {
        end_idx: idx(3),
        response: None,
        delivery_marker: None,
        forced_commit: false,
    });
    match classify_replay_resolution(outcome) {
        ReplayedResolution::Delivered(
            ReplayedPayload::Completed(None),
            ReplayDeliveryDisposition::AtReplayTail,
        ) => {}
        other => panic!("expected Delivered(Completed(None)), got {other:?}"),
    }
}

#[test]
fn classify_cancelled_with_partial_is_delivered() {
    let outcome = ResolutionOutcome::Resolved(Resolution::Cancelled {
        cancelled_idx: idx(5),
        partial: Some(payload(vec![9])),
    });
    match classify_replay_resolution(outcome) {
        ReplayedResolution::Delivered(
            ReplayedPayload::CancelledPartial(p),
            ReplayDeliveryDisposition::Immediate,
        ) => {
            assert_eq!(p, payload(vec![9]), "partial payload preserved")
        }
        other => panic!("expected Delivered(CancelledPartial), got {other:?}"),
    }
}

#[test]
fn classify_cancelled_without_partial_is_undelivered_cancellation() {
    // In the recorded run this call never returned a value to the guest — its future was
    // dropped mid-flight. The accessor paths park on this terminal; the direct path reports it
    // as replay divergence (asserted separately below).
    let outcome = ResolutionOutcome::Resolved(Resolution::Cancelled {
        cancelled_idx: idx(5),
        partial: None,
    });
    match classify_replay_resolution(outcome) {
        ReplayedResolution::Undelivered(UndeliveredTerminal::CancelledWithoutPartial {
            cancelled_idx,
        }) => assert_eq!(cancelled_idx, idx(5)),
        other => panic!("expected Undelivered(CancelledWithoutPartial), got {other:?}"),
    }
}

#[test]
fn classify_discarded_completion_is_undelivered_with_response() {
    // A persisted successful `End` whose completion the guest discarded before delivery. The
    // access path parks on this terminal; the deferred path decodes the carried response and
    // re-delivers it to the caller's deterministic post-`End` continuation; the direct path
    // reports replay divergence (asserted separately below).
    let outcome = ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
        end_idx: idx(7),
        marker_idx: idx(8),
        response: Some(payload(vec![4])),
    });
    match classify_replay_resolution(outcome) {
        ReplayedResolution::Undelivered(UndeliveredTerminal::CompletionDiscarded {
            end_idx,
            marker_idx,
            response,
        }) => {
            assert_eq!(end_idx, idx(7));
            assert_eq!(marker_idx, idx(8));
            assert_eq!(
                response,
                Some(payload(vec![4])),
                "persisted response preserved for deferred re-delivery"
            );
        }
        other => panic!("expected Undelivered(CompletionDiscarded), got {other:?}"),
    }
}

#[test]
fn undelivered_cancellation_direct_divergence_error() {
    // The direct path is serialized by the store borrow, so an undelivered terminal cannot
    // occur on it: it is a replay divergence, never a park.
    let terminal = UndeliveredTerminal::CancelledWithoutPartial {
        cancelled_idx: idx(5),
    };
    match terminal.direct_divergence_error() {
        WorkerExecutorError::UnexpectedOplogEntry { expected, got } => {
            assert_eq!(expected, "End or Cancelled { partial: Some(..) }");
            assert!(got.contains("Cancelled without partial at 5"), "{got}");
        }
        other => panic!("expected UnexpectedOplogEntry, got {other:?}"),
    }
}

#[test]
#[test_r::timeout("30s")]
async fn access_terminal_effect_acceptance_rechecks_before_enqueue() {
    for cancelled in [true, false] {
        let oplog = Arc::new(InMemoryOplog::new());
        let erased: Arc<dyn Oplog> = oplog.clone();
        let state = std::sync::Mutex::new((cancelled, false));
        let accepted = accept_access_terminal_effect(
            &42,
            &erased,
            OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: idx(23),
                response: None,
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            },
            |response, enqueue| {
                assert_eq!(*response, 42);
                let mut state = state.lock().unwrap();
                if state.0 {
                    return;
                }
                state.1 = true;
                enqueue("reservation");
            },
        );
        assert_eq!(accepted.is_none(), cancelled);
        assert_eq!(state.lock().unwrap().1, !cancelled);
        if let Some((reservation, append)) = accepted {
            assert_eq!(reservation, "reservation");
            append.await.unwrap();
        }
        assert_eq!(oplog.entries.lock().await.len(), usize::from(!cancelled));
    }
}

#[test]
#[test_r::timeout("30s")]
async fn access_terminal_effect_survives_waiter_drop_and_requires_commit() {
    use crate::durable_host::tool::attachment::{
        AttachmentMemory, AttachmentRead, AttachmentWriteAcceptance, attachment_pair,
    };

    for failure in [None, Some("append"), Some("commit")] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        drop(live_unfinished_handle::<Cancellable>(idx(23), tx.clone()));
        let DropEvent::UnfinishedCancellable { mut call } = rx.try_recv().unwrap() else {
            panic!("expected unfinished call");
        };
        let counter = Arc::new(AtomicUsize::new(0));
        call.live_call_permit = Some(LiveCallPermit::new(counter.clone()));
        let mut guard = AccessTerminalGuard::<Cancellable>::new(
            call,
            Some(tx.clone()),
            Some(tx),
            TeardownProbe::never(),
        );
        let (append_tx, append_rx) = tokio::sync::oneshot::channel();
        let (commit_tx, commit_rx) = tokio::sync::oneshot::channel();
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let published = Arc::new(AtomicUsize::new(0));
        let effect = published.clone();
        let (producer, consumer, _) = attachment_pair(7, AttachmentMemory::inert());
        producer.configure_live();
        let mut prepared = producer
            .writer()
            .prepare_write(b"aBc123!".to_vec())
            .await
            .unwrap()
            .unwrap();
        let worker = Arc::new(());
        let worker_weak = Arc::downgrade(&worker);
        let tasks = crate::worker::tasks::WorkerTasks::default().for_worker(worker.clone());
        let oplog = Arc::new(InMemoryOplog::new());
        let mut marker = marker_record_over(oplog.clone()).await;
        marker.start_idx = idx(23);
        let erased: Arc<dyn Oplog> = oplog.clone();
        let failure_controller = producer.controller();
        let recordings = crate::worker::tasks::RuntimeAppendTasks::default();
        let (cleanup_tx, cleanup_rx) = tokio::sync::oneshot::channel();
        let (handoff, terminal) = prepare_access_terminal_effect(
            &tasks,
            recordings.register().unwrap(),
            async move {
                reached_tx.send(()).unwrap();
                commit_rx.await.unwrap();
                if failure == Some("commit") {
                    Err(WorkerExecutorError::runtime("storage commit failed"))
                } else {
                    Ok(())
                }
            },
            move |publication: crate::durable_host::tool::attachment::AttachmentWritePublication, end| {
                publication.publish(Some(golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd(end)));
                effect.fetch_add(1, Ordering::AcqRel);
            },
            move |_| {
                failure_controller.fence_owner();
                async move { cleanup_rx.await.unwrap() }
            },
        ).unwrap();
        let (publication, append) = accept_access_terminal_effect(
            &(),
            &erased,
            OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: idx(23),
                response: None,
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            },
            |_, enqueue| {
                assert!(matches!(
                    prepared.accept(enqueue).unwrap(),
                    AttachmentWriteAcceptance::Accepted(())
                ));
            },
        )
        .unwrap();
        let mut read = Box::pin(consumer.read_next());
        assert!(futures::poll!(read.as_mut()).is_pending());
        assert!(
            handoff
                .send((
                    publication,
                    Box::pin(async move {
                        append_rx.await.unwrap();
                        if failure == Some("append") {
                            Err(crate::services::oplog::OplogError::Payload(
                                "storage append failed".to_string(),
                            ))
                        } else {
                            append.await
                        }
                    }),
                ))
                .is_ok()
        );
        guard.cleanup_after_terminal(terminal, Some(marker));
        drop(worker);
        drop(guard);
        let DropEvent::CleanupAfterTerminal {
            terminal: Some(terminal),
            live_call_permit,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("accepted End must retain owned cleanup, not select cancellation");
        };
        assert_eq!(producer.controller().metadata().buffered_bytes, 7);
        assert!(worker_weak.upgrade().is_some());
        assert_eq!(counter.load(Ordering::Acquire), 1);
        let shutdown = tasks.stop_and_wait();
        tokio::pin!(shutdown);
        assert!(futures::poll!(shutdown.as_mut()).is_pending());
        assert_eq!(published.load(Ordering::Acquire), 0);
        append_tx.send(()).unwrap();
        if failure == Some("append") {
            assert!(reached_rx.await.is_err(), "append failure must skip commit");
        } else {
            reached_rx.await.unwrap();
            assert_eq!(
                published.load(Ordering::Acquire),
                0,
                "append is not a commit"
            );
            commit_tx.send(()).unwrap();
        }
        // The parked reader must progress before cleanup joins the terminal task.
        let observed = read.await;
        if failure.is_some() {
            assert!(matches!(observed, AttachmentRead::Item(Err(_))));
        } else {
            assert!(matches!(observed, AttachmentRead::Item(Ok(bytes)) if bytes == b"aBc123!"));
        }
        let result = terminal.await.unwrap();
        assert_eq!(result.is_err(), failure.is_some());
        assert_eq!(
            oplog
                .entries
                .lock()
                .await
                .iter()
                .filter(|entry| matches!(entry, OplogEntry::CompletionDiscarded { .. }))
                .count(),
            usize::from(failure.is_none()),
            "a failed terminal must not leave a discard marker"
        );
        if failure.is_some() {
            assert!(futures::poll!(shutdown.as_mut()).is_pending());
            let sealed = recordings.seal_and_wait();
            tokio::pin!(sealed);
            assert!(futures::poll!(sealed.as_mut()).is_pending());
            assert!(worker_weak.upgrade().is_some());
            cleanup_tx.send(()).unwrap();
            sealed.await;
        }
        shutdown.await.unwrap();
        assert!(worker_weak.upgrade().is_none());
        let stopped_recordings = crate::worker::tasks::RuntimeAppendTasks::default();
        assert!(
            prepare_access_terminal_effect(
                &tasks,
                stopped_recordings.register().unwrap(),
                async { panic!("closed metadata admission must not commit") },
                |(): (), _| panic!("closed metadata admission must not publish"),
                |_| async { panic!("refusal precedes acceptance and needs no failure cleanup") },
            )
            .is_err()
        );
        stopped_recordings.seal_and_wait().await;
        drop(live_call_permit);
        assert_eq!(counter.load(Ordering::Acquire), 0);
        assert_eq!(
            published.load(Ordering::Acquire),
            usize::from(failure.is_none())
        );
        assert_eq!(producer.controller().metadata().buffered_bytes, 0);
    }
}

#[test]
fn undelivered_discarded_completion_direct_divergence_error() {
    // Discarded completions are recorded only on the accessor completion path, and calls replay
    // through the same path they recorded on.
    let terminal = UndeliveredTerminal::CompletionDiscarded {
        end_idx: idx(7),
        marker_idx: idx(8),
        response: None,
    };
    match terminal.direct_divergence_error() {
        WorkerExecutorError::UnexpectedOplogEntry { expected, got } => {
            assert_eq!(expected, "End delivered to the guest");
            assert!(
                got.contains("End at 7 marked CompletionDiscarded at 8"),
                "{got}"
            );
        }
        other => panic!("expected UnexpectedOplogEntry, got {other:?}"),
    }
}

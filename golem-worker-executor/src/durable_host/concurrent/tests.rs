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

use golem_common::model::oplog::host_functions;
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
        persistence_level: PersistenceLevel::Smart,
        snapshotting_mode: None,
        assume_idempotence: true,
        max_in_function_retry_delay: Duration::from_secs(20),
    }
}

fn completed(end_idx: u64) -> Resolution {
    Resolution::Completed {
        end_idx: idx(end_idx),
        response: None,
        forced_commit: false,
    }
}

// ---- ConcurrentReplayResolver ----

#[test]
fn resolver_out_of_order_completion() {
    // [S1, S2, E2, E1]: claim both, then resolve E2 before E1.
    let mut resolver = ConcurrentReplayResolver::default();
    let mut rx1 = resolver.register(idx(1));
    let mut rx2 = resolver.register(idx(2));
    assert!(rx1.try_recv().is_err());
    assert!(rx2.try_recv().is_err());

    assert!(resolver.resolve_if_pending(idx(2), completed(3)));
    match rx2.try_recv() {
        Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
            assert_eq!(end_idx, idx(3))
        }
        other => panic!("unexpected resolution for h2: {other:?}"),
    }
    assert!(rx1.try_recv().is_err());

    assert!(resolver.resolve_if_pending(idx(1), completed(4)));
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
    let mut rx = resolver.register(idx(1));
    assert!(resolver.resolve_if_pending(
        idx(1),
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
    let mut rx = resolver.register(idx(1));
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
    // No registration: resolve_if_pending must not buffer (this is the unregistered-End case,
    // e.g. the guest-facing manual durability pair).
    assert!(!resolver.resolve_if_pending(idx(1), completed(2)));
    let mut rx = resolver.register(idx(1));
    assert!(rx.try_recv().is_err());
}

#[test]
fn resolver_fail_all_pending_incomplete_wakes_everyone() {
    // End-of-replay must wake every still-suspended awaiter as Incomplete, not leave them
    // hanging. A resolved awaiter is already gone and is unaffected.
    let mut resolver = ConcurrentReplayResolver::default();
    let mut rx1 = resolver.register(idx(1));
    let mut rx2 = resolver.register(idx(2));
    assert!(resolver.resolve_if_pending(idx(1), completed(3)));

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
    let mut rx = resolver.register(idx(1));
    assert!(resolver.resolve_if_pending(idx(1), completed(2)));
    // Second resolution: no longer pending.
    assert!(!resolver.resolve_if_pending(idx(1), completed(3)));
    match rx.try_recv() {
        Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
            assert_eq!(end_idx, idx(2))
        }
        other => panic!("unexpected resolution: {other:?}"),
    }
}

// ---- CallHandle drop policy ----

fn live_unfinished_handle<P: DropPolicy>(
    start_idx: OplogIndex,
    sink: mpsc::UnboundedSender<DropEvent>,
) -> CallHandle<host_functions::MonotonicClockNow, P> {
    live_unfinished_handle_with_atomic_region(start_idx, None, sink)
}

fn live_unfinished_handle_with_atomic_region<P: DropPolicy>(
    start_idx: OplogIndex,
    atomic_region: Option<OplogIndex>,
    sink: mpsc::UnboundedSender<DropEvent>,
) -> CallHandle<host_functions::MonotonicClockNow, P> {
    use crate::durable_host::durability::DurableExecutionState;
    use std::time::Duration;
    let durable_execution_state = DurableExecutionState {
        is_live: true,
        persistence_level: PersistenceLevel::Smart,
        snapshotting_mode: None,
        assume_idempotence: false,
        max_in_function_retry_delay: Duration::ZERO,
    };
    CallHandle {
        start_idx,
        begin_index: start_idx,
        is_live: true,
        persisted: true,
        request_upload: PendingUpload::already_durable(),
        replay: None,
        finished: false,
        parked_undelivered_replay: false,
        execution_scope: CallExecutionScope {
            retry_from: start_idx,
            durable_scope: None,
            atomic_lease: unregistered_atomic_lease(atomic_region, true),
            persistence_level: PersistenceLevel::Smart,
        },
        retry: InFunctionRetryController::new(
            DurableFunctionType::ReadLocal,
            durable_execution_state,
            "test:monotonic_clock::now",
        ),
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
/// event), and no `drop_sink` is attached. `start_idx`/`begin_index` are set from
/// `scope.retry_from` only so the struct is well-formed; the tests read nothing but the scope.
fn synthetic_finished_handle_with_scope<P: DropPolicy>(
    scope: CallExecutionScope,
) -> CallHandle<host_functions::MonotonicClockNow, P> {
    use crate::durable_host::durability::DurableExecutionState;
    use std::time::Duration;
    let durable_execution_state = DurableExecutionState {
        is_live: true,
        persistence_level: PersistenceLevel::Smart,
        snapshotting_mode: None,
        assume_idempotence: false,
        max_in_function_retry_delay: Duration::ZERO,
    };
    let start_idx = scope.retry_from;
    CallHandle {
        start_idx,
        begin_index: start_idx,
        is_live: true,
        persisted: true,
        request_upload: PendingUpload::already_durable(),
        replay: None,
        finished: true,
        parked_undelivered_replay: false,
        execution_scope: scope,
        retry: InFunctionRetryController::new(
            DurableFunctionType::ReadLocal,
            durable_execution_state,
            "test:monotonic_clock::now",
        ),
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
                atomic_lease: None,
                trap_context: DurableCallTrapContext {
                    retry_from: idx(4),
                    in_atomic_region: false,
                },
                live_call_permit: Some(LiveCallPermit::new(counter.clone())),
            },
            NotCancellable::production_drop_sink(Some(tx.clone())),
            Some(tx),
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
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                golem_common::model::oplog::HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadLocal,
        })
        .await;
    seed_oplog
        .add(OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: idx(1),
            response: None,
            forced_commit: false,
        })
        .await;
    let seed_oplog_dyn: Arc<dyn Oplog> = seed_oplog;
    let replay_state = ReplayState::new(
        golem_common::model::OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id: golem_common::model::AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "completion-delivery-test".to_string(),
            },
        },
        seed_oplog_dyn,
        golem_common::model::regions::DeletedRegions::default(),
    )
    .await
    .expect("failed to build replay state");
    CompletionDelivery {
        state: CompletionDeliveryState::Live(Box::new(LiveDelivery {
            marker: DiscardMarker {
                start_idx: idx(1),
                oplog: oplog_dyn,
                replay_state,
            },
            trap_context: DurableCallTrapContext {
                retry_from: idx(1),
                in_atomic_region: false,
            },
            live_call_permit: Some(LiveCallPermit::new(permit_counter)),
            cleanup_sink: Some(cleanup_tx),
            pending_append: None,
        })),
    }
}

#[test]
async fn completion_delivery_delivered_and_suppress_record_no_marker() {
    // `delivered` (successful final guest transfer) and `suppress` (caller-observed
    // post-`End` error) must not record a `CompletionDiscarded` marker and must release the
    // in-flight permit without queueing a drain event (no pending ordered append).
    let variants: [fn(CompletionDelivery); 2] =
        [CompletionDelivery::delivered, CompletionDelivery::suppress];
    for consume in variants {
        let oplog = Arc::new(InMemoryOplog::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
        assert_eq!(counter.load(Ordering::Acquire), 1);
        consume(token);
        assert_eq!(counter.load(Ordering::Acquire), 0);
        assert!(rx.try_recv().is_err(), "no drain event may be queued");
        assert!(
            oplog.entries.lock().await.is_empty(),
            "no marker may be recorded"
        );
    }
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
        Ok(DropEvent::AwaitDiscardMarker {
            terminal,
            live_call_permit,
            ..
        }) => {
            terminal
                .expect("the drain event must carry the marker join")
                .await
                .expect("marker task must not panic")
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
        other => panic!("expected an AwaitDiscardMarker drop event, got {other:?}"),
    }
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
        Ok(DropEvent::AwaitDiscardMarker {
            terminal,
            live_call_permit,
            ..
        }) => {
            assert_eq!(
                counter.load(Ordering::Acquire),
                1,
                "the drain event must own the permit"
            );
            gate.add_permits(1);
            terminal
                .expect("the drain event must carry the marker join")
                .await
                .expect("marker task must not panic")
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
        other => panic!("expected an AwaitDiscardMarker drop event, got {other:?}"),
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

    let delivered = CompletionDelivery::replay_delivered();
    assert!(!delivered.is_replay_discarded());
    assert!(!delivered.is_live_armed());
    delivered.delivered();

    // Dropping an unconsumed replay-discarded token is a no-op as well (the park path drops
    // the token after waiting for the guest to abandon the delivery boundary).
    drop(CompletionDelivery::replay_discarded());
}

#[test]
async fn completion_delivery_ordered_append_lands_before_marker() {
    // An `append_ordered` entry (e.g. a durable `FinishSpan`) handed to the token must land
    // *before* the torn-drop marker, preserving the recorded
    // `End → FinishSpan → CompletionDiscarded` order replay consumes positionally.
    let oplog = Arc::new(InMemoryOplog::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let mut token = live_delivery_token(oplog.clone(), counter.clone(), tx).await;
        token.append_ordered(OplogEntry::NoOp {
            timestamp: Timestamp::now_utc(),
        });
        drop(token);
    }
    match rx.try_recv() {
        Ok(DropEvent::AwaitDiscardMarker { terminal, .. }) => {
            terminal
                .expect("the drain event must carry the chained join")
                .await
                .expect("chained task must not panic")
                .expect("chained appends must succeed");
        }
        other => panic!("expected an AwaitDiscardMarker drop event, got {other:?}"),
    }
    let entries = oplog.entries.lock().await;
    assert_eq!(entries.len(), 2, "expected [ordered entry, marker]");
    assert!(matches!(&entries[0], OplogEntry::NoOp { .. }));
    assert!(matches!(
        &entries[1],
        OplogEntry::CompletionDiscarded { start_index, .. } if *start_index == idx(1)
    ));
}

/// Minimal in-memory [`Oplog`] recording appended entries, for tests that assert what a
/// drained drop event writes durably. With an `end_gate` installed, appending an `End` or
/// `CompletionDiscarded` entry first signals `reached` and then blocks until the gate
/// semaphore yields a permit, so a test can hold the terminal or marker append open and
/// observe what is (not) visible meanwhile.
#[derive(Debug)]
struct InMemoryOplog {
    entries: tokio::sync::Mutex<Vec<OplogEntry>>,
    end_gate: Option<(mpsc::UnboundedSender<()>, Arc<tokio::sync::Semaphore>)>,
}

impl InMemoryOplog {
    fn new() -> Self {
        Self {
            entries: tokio::sync::Mutex::new(Vec::new()),
            end_gate: None,
        }
    }

    fn with_end_gate(
        reached: mpsc::UnboundedSender<()>,
        gate: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            entries: tokio::sync::Mutex::new(Vec::new()),
            end_gate: Some((reached, gate)),
        }
    }
}

#[async_trait]
impl Oplog for InMemoryOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        if matches!(
            entry,
            OplogEntry::End { .. } | OplogEntry::CompletionDiscarded { .. }
        ) && let Some((reached, gate)) = &self.end_gate
        {
            let _ = reached.send(());
            gate.acquire()
                .await
                .expect("end gate semaphore is never closed")
                .forget();
        }
        let mut entries = self.entries.lock().await;
        entries.push(entry);
        OplogIndex::from_u64(entries.len() as u64)
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: Box<
            dyn FnOnce(golem_common::model::oplog::RawOplogPayload) -> Result<OplogEntry, String>
                + Send,
        >,
    ) -> Result<crate::services::oplog::OrderedOplogStart, String> {
        let entry = build_start(
            golem_common::model::oplog::RawOplogPayload::SerializedInline(serialized_request),
        )?;
        let index = self.add(entry.clone()).await;
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
    ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
        std::collections::BTreeMap::new()
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        OplogIndex::from_u64(self.entries.lock().await.len() as u64)
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        None
    }

    async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
        true
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        let entries = self.entries.lock().await;
        let idx: u64 = oplog_index.into();
        entries[(idx - 1) as usize].clone()
    }

    async fn read_many(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
        let entries = self.entries.lock().await;
        let start: u64 = oplog_index.into();
        let mut result = std::collections::BTreeMap::new();
        for i in start..(start + n) {
            if let Some(entry) = entries.get((i - 1) as usize) {
                result.insert(OplogIndex::from_u64(i), entry.clone());
            }
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

    async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}
}

#[test]
async fn dropped_cancellable_call_records_cancelled_at_next_drain_point() {
    // A live `Cancellable` call dropped mid-flight enqueues an `UnfinishedCancellable`
    // snapshot; this test drives the drain's *recording step* directly
    // (`DroppedCall::append_cancelled_with_oplog`, shared by both production drains) and
    // asserts the durable outcome. Invoking a production drain itself, and the ctx-side
    // effects (durable-scope close, atomic-region unregistration), require full-worker
    // integration coverage.
    let oplog = Arc::new(InMemoryOplog::new());
    let start_idx = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::MonotonicClockNow,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                golem_common::model::oplog::HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadLocal,
        })
        .await;

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
async fn access_terminal_end_is_appended_before_cleanup_and_permit_release() {
    // Ordering is proven against the production persistence stage itself
    // (`CallHandle::persist_access_terminal`, the exact code `complete_access_impl` runs for a
    // persisted live call): while the terminal `End` append is still in flight, the live-call
    // permit must stay held and no cleanup event may become visible; both are released only
    // after the append completes (production releases them via `disarm()` after
    // `end_durable_function_access`, strictly downstream of `wait_terminal()`).
    use golem_common::model::oplog::HostResponseMonotonicClockTimestamp;

    let (reached_tx, mut reached_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let oplog = Arc::new(InMemoryOplog::with_end_gate(reached_tx, gate.clone()));
    let start_idx = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::MonotonicClockNow,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                golem_common::model::oplog::HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadRemote,
        })
        .await;

    let permit_counter = Arc::new(AtomicUsize::new(0));
    let (cleanup_tx, mut cleanup_rx) = mpsc::unbounded_channel();
    let mut guard = AccessTerminalGuard::<NotCancellable>::new(
        DroppedCall {
            start_idx,
            begin_index: start_idx,
            function_type: DurableFunctionType::ReadRemote,
            request_upload: PendingUpload::already_durable(),
            atomic_lease: None,
            trap_context: DurableCallTrapContext {
                retry_from: start_idx,
                in_atomic_region: false,
            },
            live_call_permit: Some(LiveCallPermit::new(permit_counter.clone())),
        },
        NotCancellable::production_drop_sink(Some(cleanup_tx.clone())),
        Some(cleanup_tx),
    );
    assert_eq!(permit_counter.load(Ordering::Acquire), 1);

    let persist_oplog: Arc<dyn Oplog> = oplog.clone();
    let persist_replay_state = ReplayState::new(
        golem_common::model::OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id: golem_common::model::AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "concurrent-test".to_string(),
            },
        },
        persist_oplog.clone(),
        golem_common::model::regions::DeletedRegions::default(),
    )
    .await
    .expect("failed to build replay state");
    let persist = tokio::spawn(async move {
        let response = HostResponseMonotonicClockTimestamp { nanos: 42 };
        let result = CallHandle::<host_functions::MonotonicClockNow, NotCancellable>::
                persist_access_terminal(persist_oplog, persist_replay_state, &mut guard, start_idx, &response, None)
            .await;
        (result, guard)
    });

    // The owned terminal task is now provably inside the gated `End` append...
    reached_rx
        .recv()
        .await
        .expect("terminal append must reach the gate");
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
    result.expect("persisting the terminal must succeed");

    // The terminal is durably appended when the persistence stage returns...
    {
        let entries = oplog.entries.lock().await;
        assert_eq!(entries.len(), 2, "expected [Start, End]");
        match &entries[1] {
            OplogEntry::End { start_index, .. } => assert_eq!(*start_index, start_idx),
            other => panic!("expected End, got {other:?}"),
        }
    }
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
    // (e.g. the `set_oplog_persistence_level` boundary guard) running between the drop and
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
            assert_eq!(call.atomic_region(), None);
        }
        other => panic!("expected one UnfinishedCancellable, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "expected exactly one drop event");
}

#[test]
fn drop_cancellable_unfinished_carries_call_owned_cancellation_state() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let _handle =
            live_unfinished_handle_with_atomic_region::<Cancellable>(idx(5), Some(idx(2)), tx);
    }
    match rx.try_recv() {
        Ok(DropEvent::UnfinishedCancellable { call }) => {
            assert_eq!(call.start_idx(), idx(5));
            assert_eq!(call.atomic_region(), Some(idx(2)));
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

    let handle_a =
        live_unfinished_handle_with_atomic_region::<Cancellable>(idx(5), Some(idx(2)), tx.clone());
    let handle_b =
        live_unfinished_handle_with_atomic_region::<Cancellable>(idx(9), Some(idx(3)), tx);

    drop(handle_a);
    drop(handle_b);

    match rx.try_recv() {
        Ok(DropEvent::UnfinishedCancellable { call }) => {
            assert_eq!(call.start_idx(), idx(5));
            assert_eq!(call.atomic_region(), Some(idx(2)));
        }
        other => panic!("expected first cancellable drop snapshot, got {other:?}"),
    }
    match rx.try_recv() {
        Ok(DropEvent::UnfinishedCancellable { call }) => {
            assert_eq!(call.start_idx(), idx(9));
            assert_eq!(call.atomic_region(), Some(idx(3)));
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
    appended_inside_atomic_region: Vec<bool>,
    /// Atomic region begin indices the inner host reports as having recorded side effects.
    regions_with_side_effects: Vec<OplogIndex>,
    /// Ambient outermost-region side-effect state, used by the unscoped fallback path.
    outermost_has_side_effects: bool,
}

impl RetryHostProbe {
    fn new(current_retry_point: OplogIndex) -> Self {
        Self {
            current_retry_point,
            appended_retry_from: Vec::new(),
            appended_inside_atomic_region: Vec::new(),
            regions_with_side_effects: Vec::new(),
            outermost_has_side_effects: false,
        }
    }
}

#[async_trait]
impl InFunctionRetryHost for RetryHostProbe {
    fn in_atomic_region(&self) -> bool {
        false
    }

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

    fn atomic_region_has_side_effects_for(&self, begin_index: OplogIndex) -> bool {
        self.regions_with_side_effects.contains(&begin_index)
    }

    fn retry_context_atomic_region_had_side_effects(&self) -> bool {
        self.outermost_has_side_effects
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        inside_atomic_region: bool,
        _retry_policy_state: Option<golem_common::model::RetryPolicyState>,
    ) {
        self.appended_retry_from.push(retry_from);
        self.appended_inside_atomic_region
            .push(inside_atomic_region);
    }
}

#[test]
fn scoped_retry_host_uses_call_retry_point_not_inner_current() {
    let mut inner = RetryHostProbe::new(idx(99));
    let scope = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        atomic_lease: None,
        persistence_level: PersistenceLevel::PersistRemoteSideEffects,
    };

    let retry_host = ScopedRetryHost::new(&mut inner, &scope);

    assert!(!retry_host.in_atomic_region());
    assert_eq!(retry_host.current_retry_point(), idx(42));
    assert_eq!(
        retry_host.durable_execution_state().persistence_level,
        PersistenceLevel::PersistRemoteSideEffects
    );
}

#[test]
fn scoped_retry_host_uses_call_atomic_region_as_retry_point() {
    let mut inner = RetryHostProbe::new(idx(99));
    let scope = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        atomic_lease: unregistered_atomic_lease(Some(idx(7)), true),
        persistence_level: PersistenceLevel::Smart,
    };

    let retry_host = ScopedRetryHost::new(&mut inner, &scope);

    assert!(retry_host.in_atomic_region());
    assert_eq!(retry_host.current_retry_point(), idx(7));
}

#[test]
async fn scoped_retry_host_trap_retry_uses_call_retry_point() {
    let mut inner = RetryHostProbe::new(idx(99));
    let scope = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        atomic_lease: None,
        persistence_level: PersistenceLevel::Smart,
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
        atomic_lease: None,
        persistence_level: PersistenceLevel::Smart,
    };
    let scope_b = CallExecutionScope {
        retry_from: idx(77),
        durable_scope: Some(idx(70)),
        atomic_lease: None,
        persistence_level: PersistenceLevel::Smart,
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

#[test]
async fn seam2_overlapping_atomic_region_traps_use_initiation_membership() {
    let mut inner = RetryHostProbe::new(idx(99));
    let scope_a = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        atomic_lease: unregistered_atomic_lease(Some(idx(7)), true),
        persistence_level: PersistenceLevel::Smart,
    };
    let scope_b = CallExecutionScope {
        retry_from: idx(77),
        durable_scope: Some(idx(70)),
        atomic_lease: unregistered_atomic_lease(Some(idx(8)), true),
        persistence_level: PersistenceLevel::Smart,
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

    assert_eq!(override_a.retry_from, idx(7));
    assert_eq!(override_b.retry_from, idx(8));
}

// ---- terminal-step failures ----

/// Classifies a terminal-step failure the way the invocation loop does, with a deliberately
/// *hostile* ambient fallback (a retry point and atomic-region membership belonging to no real
/// call). A call-owned [`DurableCallTrapContext`] marker carried by the error must override both
/// fallbacks; if the terminal path lost the marker, the classifier would silently adopt these
/// ambient values instead.
fn classify_with_hostile_ambient(error: &anyhow::Error) -> crate::model::TrapType {
    crate::model::TrapType::from_error::<crate::workerctx::default::Context>(
        error,
        idx(99),
        true,
        false,
        golem_common::model::agent::AgentMode::Durable,
    )
}

#[test]
fn seam2_terminal_failure_carries_call_owned_trap_context() {
    // A failure escaping a *terminal* durable-call step (`complete` / `complete_access` /
    // `cancel` / the dropped-call drain) is wrapped in a `TerminalCallError` built from the
    // call's own execution scope. Classifying it must group the retry against the call's own
    // scope and use the call's own atomic-region membership, never the ambient worker state.
    let scope = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        atomic_lease: None,
        persistence_level: PersistenceLevel::Smart,
    };
    let handle = synthetic_finished_handle_with_scope::<Cancellable>(scope);

    let error: anyhow::Error = TerminalCallError::new(
        WorkerExecutorError::runtime("terminal step failure"),
        handle.trap_context(),
    )
    .into();

    match classify_with_hostile_ambient(&error) {
        crate::model::TrapType::Error {
            retry_from,
            in_atomic_region,
            ..
        } => {
            // Call-owned (idx 42, non-atomic) wins over hostile ambient (idx 99, atomic).
            assert_eq!(retry_from, idx(42));
            assert!(!in_atomic_region);
        }
        other => panic!("expected TrapType::Error, got {other:?}"),
    }
}

#[test]
fn seam2_overlapping_terminal_failures_carry_independent_trap_contexts() {
    // Two durable calls overlap in flight and share the same ambient worker state. Each one's
    // terminal-step failure must still be classified against its OWN retry point and
    // atomic-region membership. Without the call-owned marker these would fall back to that
    // shared ambient state, so an overlapping sibling could clobber the grouping.
    let scope_a = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        atomic_lease: unregistered_atomic_lease(Some(idx(7)), true),
        persistence_level: PersistenceLevel::Smart,
    };
    let scope_b = CallExecutionScope {
        retry_from: idx(77),
        durable_scope: Some(idx(70)),
        atomic_lease: None,
        persistence_level: PersistenceLevel::Smart,
    };
    let handle_a = synthetic_finished_handle_with_scope::<Cancellable>(scope_a);
    let handle_b = synthetic_finished_handle_with_scope::<Cancellable>(scope_b);

    let error_a: anyhow::Error =
        TerminalCallError::new(WorkerExecutorError::runtime("a"), handle_a.trap_context()).into();
    let error_b: anyhow::Error =
        TerminalCallError::new(WorkerExecutorError::runtime("b"), handle_b.trap_context()).into();

    // Both classified against the SAME hostile ambient state, yet each keeps its own grouping.
    match classify_with_hostile_ambient(&error_a) {
        crate::model::TrapType::Error {
            retry_from,
            in_atomic_region,
            ..
        } => {
            // Call A was initiated inside an atomic region: the whole region retries from its
            // begin index (idx 7) and membership is recorded.
            assert_eq!(retry_from, idx(7));
            assert!(in_atomic_region);
        }
        other => panic!("expected TrapType::Error for call A, got {other:?}"),
    }
    match classify_with_hostile_ambient(&error_b) {
        crate::model::TrapType::Error {
            retry_from,
            in_atomic_region,
            ..
        } => {
            // Call B was not in an atomic region: it retries from its own scope (idx 77).
            assert_eq!(retry_from, idx(77));
            assert!(!in_atomic_region);
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
    // An atomic dropped call: its own context (idx 3, atomic) must win over the hostile ambient.
    let dropped_atomic = DroppedCall {
        start_idx: idx(5),
        begin_index: idx(4),
        function_type: DurableFunctionType::ReadRemote,
        request_upload: PendingUpload::already_durable(),
        atomic_lease: unregistered_atomic_lease(Some(idx(3)), true),
        trap_context: DurableCallTrapContext {
            retry_from: idx(3),
            in_atomic_region: true,
        },
        live_call_permit: None,
    };
    // A non-atomic dropped call: its membership (false) must win over the hostile ambient's
    // `in_atomic_region = true`, so the membership assertion is independent of the retry point.
    let dropped_non_atomic = DroppedCall {
        start_idx: idx(8),
        begin_index: idx(8),
        function_type: DurableFunctionType::ReadRemote,
        request_upload: PendingUpload::already_durable(),
        atomic_lease: None,
        trap_context: DurableCallTrapContext {
            retry_from: idx(8),
            in_atomic_region: false,
        },
        live_call_permit: None,
    };

    for (dropped, expected_retry, expected_atomic) in [
        (dropped_atomic, idx(3), true),
        (dropped_non_atomic, idx(8), false),
    ] {
        let error: anyhow::Error = TerminalCallError::new(
            WorkerExecutorError::runtime("cancellation drain failed"),
            dropped.trap_context(),
        )
        .into();

        // `classify_with_hostile_ambient` supplies idx(99)/atomic-membership-true as the
        // drainer's ambient state; the dropped call's own context must win.
        match classify_with_hostile_ambient(&error) {
            crate::model::TrapType::Error {
                retry_from,
                in_atomic_region,
                ..
            } => {
                assert_eq!(retry_from, expected_retry);
                assert_eq!(in_atomic_region, expected_atomic);
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
        atomic_region: Some(idx(2)),
        persistence_level: PersistenceLevel::PersistNothing,
    };

    let lease = unregistered_atomic_lease(begun.atomic_region, true);
    let scope = begun.finish(idx(11), lease);

    assert_eq!(scope.retry_from, idx(10));
    assert_eq!(scope.durable_scope, Some(idx(10)));
    assert_eq!(scope.atomic_region(), Some(idx(2)));
    assert_eq!(scope.persistence_level, PersistenceLevel::PersistNothing);
}

#[test]
fn begun_execution_scope_uses_call_start_as_retry_from_when_unscoped() {
    let begun = BegunCallExecutionScope {
        parent_start_index: None,
        atomic_region: None,
        persistence_level: PersistenceLevel::Smart,
    };

    let scope = begun.finish(idx(12), None);

    assert_eq!(scope.retry_from, idx(12));
    assert_eq!(scope.durable_scope, None);
    assert_eq!(scope.atomic_region(), None);
    assert_eq!(scope.persistence_level, PersistenceLevel::Smart);
}

#[test]
fn call_execution_scope_owns_call_retry_point() {
    let scope = CallExecutionScope {
        retry_from: idx(42),
        durable_scope: Some(idx(40)),
        atomic_lease: None,
        persistence_level: PersistenceLevel::Smart,
    };

    assert_eq!(scope.retry_from, idx(42));
}

// ---- function-type re-execution policy ----

#[test]
fn can_reexecute_matches_internal_retry_eligibility() {
    use crate::durable_host::durability::{DurableExecutionState, InFunctionRetryController};

    fn controller(ft: DurableFunctionType, assume_idempotence: bool) -> InFunctionRetryController {
        InFunctionRetryController::new(
            ft,
            DurableExecutionState {
                is_live: true,
                persistence_level: PersistenceLevel::Smart,
                snapshotting_mode: None,
                assume_idempotence,
                max_in_function_retry_delay: Duration::ZERO,
            },
            "test:fn",
        )
    }

    // Reads and local/idempotent writes are safe to re-execute on an incomplete Start.
    assert!(controller(DurableFunctionType::ReadLocal, false).can_reexecute_on_incomplete_replay());
    assert!(
        controller(DurableFunctionType::ReadRemote, false).can_reexecute_on_incomplete_replay()
    );
    assert!(
        controller(DurableFunctionType::WriteLocal, false).can_reexecute_on_incomplete_replay()
    );
    assert!(
        controller(DurableFunctionType::WriteRemote, true).can_reexecute_on_incomplete_replay()
    );

    // Non-idempotent / batched / transaction writes are not — neither the `None` (scope-opening)
    // nor the `Some(begin_index)` (in-scope host call) variants. The `Some(..)` variants are the
    // ones a migrated batched/transaction host call carries, so a lone committed host-call
    // `Start` for them must hard-error on incomplete replay rather than re-execute and risk
    // duplicating an external write (`CallHandle::replay` Incomplete arm).
    assert!(
        !controller(DurableFunctionType::WriteRemote, false).can_reexecute_on_incomplete_replay()
    );
    assert!(
        !controller(DurableFunctionType::WriteRemoteBatched(None), true)
            .can_reexecute_on_incomplete_replay()
    );
    assert!(
        !controller(DurableFunctionType::WriteRemoteBatched(Some(idx(7))), true)
            .can_reexecute_on_incomplete_replay()
    );
    assert!(
        !controller(DurableFunctionType::WriteRemoteTransaction(None), true)
            .can_reexecute_on_incomplete_replay()
    );
    assert!(
        !controller(
            DurableFunctionType::WriteRemoteTransaction(Some(idx(9))),
            true
        )
        .can_reexecute_on_incomplete_replay()
    );
}

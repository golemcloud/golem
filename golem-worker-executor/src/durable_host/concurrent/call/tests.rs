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

use super::super::tests::live_unfinished_handle;
use super::*;
use futures::FutureExt;
use test_r::test;

#[test]
#[test_r::timeout("10s")]
async fn tool_trap_disposal_retains_pending_upload_before_terminal() {
    for terminal_started in [false, true] {
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let counter = Arc::new(AtomicUsize::new(0));
        let marker = Arc::new(AtomicBool::new(false));
        let (uploaded, upload) = oneshot::channel();
        let mut session =
            live_unfinished_handle::<NotCancellable>(OplogIndex::from_u64(8), sink.clone());
        session.cleanup_sink = Some(sink);
        session.drop_sink = None;
        session.tool_trap_disposal = marker.clone();
        session.runtime_teardown = Arc::new(|| true);
        session.live_call_permit = Some(LiveCallPermit::new(counter.clone()));
        session.request_upload = PendingUpload::spawned(
            async move {
                upload.await.unwrap();
                Ok(())
            }
            .boxed()
            .shared(),
        );
        let guard = terminal_started.then(|| session.terminal_guard());
        marker.store(true, Ordering::Release);
        drop(guard);
        drop(session);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        let DropEvent::UnfinishedCancellable { call } = events.try_recv().unwrap() else {
            panic!("trapped Store must transfer the unfinished call to cancellation");
        };
        assert!(futures::poll!(std::pin::pin!(call.wait_request_upload())).is_pending());
        uploaded.send(()).unwrap();
        call.wait_request_upload().await.unwrap();
        assert_eq!(call.start_idx(), OplogIndex::from_u64(8));
        drop(call);
        assert_eq!(counter.load(Ordering::Acquire), 0);
        assert!(events.try_recv().is_err());
    }
}

#[test]
fn tool_trap_disposal_does_not_cancel_replay_or_leave_incomplete_calls() {
    fn check<P: DropPolicy>(replay: bool, persisted: bool, entity_admission: bool) {
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let mut session = live_unfinished_handle::<P>(OplogIndex::from_u64(8), sink.clone());
        session.entity_admission = entity_admission;
        session.cleanup_sink = Some(sink);
        session.tool_trap_disposal.store(true, Ordering::Release);
        session.is_live = !replay;
        session.persisted = persisted;
        session.parked_undelivered_replay = replay;
        drop(session);
        assert!(events.try_recv().is_err());
    }
    check::<NotCancellable>(true, false, false);
    check::<NotCancellable>(false, false, false);
    check::<LeaveIncompleteOnDrop>(false, true, false);
    check::<LeaveIncompleteOnDrop>(true, false, true);
    check::<LeaveIncompleteOnDrop>(false, false, true);
}

#[test]
fn tool_trap_disposal_accepted_start_survives_both_oneshot_drop_boundaries() {
    fn check<P: DropPolicy>(sent_before_drop: bool, entity_admission: bool) {
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let counter = Arc::new(AtomicUsize::new(0));
        let mut session = live_unfinished_handle::<P>(OplogIndex::from_u64(8), sink.clone());
        session.entity_admission = entity_admission;
        let accepted = AcceptedAccessStart::<P> {
            result: Some((session.start_idx, PendingUpload::already_durable())),
            call: Some(session.dropped_call_snapshot(Some(LiveCallPermit::new(counter.clone())))),
            marker: Arc::new(AtomicBool::new(true)),
            sink: None,
            cleanup_sink: Some(sink),
            teardown: TeardownProbe::never(),
            _policy: PhantomData,
        };
        session.finished = true;
        drop(session);
        let (send, receive) = oneshot::channel();
        if sent_before_drop {
            assert!(send.send(accepted).is_ok());
            drop(receive);
        } else {
            drop(receive);
            drop(send.send(accepted));
        }
        assert_eq!(counter.load(Ordering::Acquire), 1);
        let DropEvent::UnfinishedCancellable { call } = events.try_recv().unwrap() else {
            panic!("accepted Start must retain its cancellation obligation");
        };
        assert_eq!(call.start_idx(), OplogIndex::from_u64(8));
        assert_eq!(call.entity_admission, entity_admission);
        drop(call);
        assert_eq!(counter.load(Ordering::Acquire), 0);
        assert!(events.try_recv().is_err());
    }
    for sent_before_drop in [false, true] {
        check::<NotCancellable>(sent_before_drop, false);
        check::<LeaveIncompleteOnDrop>(sent_before_drop, true);
    }
}

#[test]
#[test_r::timeout("10s")]
async fn tool_trap_disposal_entity_admission_records_typed_skipped_cancellation() {
    use super::super::tests::InMemoryOplog;
    use golem_common::model::oplog::payload::types::SerializableEntityBodyExecution;
    for semantic_disposal in [false, true] {
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let mut session =
            live_unfinished_handle::<LeaveIncompleteOnDrop>(OplogIndex::from_u64(8), sink.clone());
        session.entity_admission = true;
        session.cleanup_sink = Some(sink);
        session.cancellation_span_id = Some(SpanId::generate());
        session.executor_shutdown.cancel();
        session
            .tool_trap_disposal
            .store(semantic_disposal, Ordering::Release);
        let (send, receive) = oneshot::channel();
        if semantic_disposal {
            assert!(send.send(session).is_ok());
            drop(receive);
        } else {
            drop(receive);
            drop(send.send(session));
        }
        if !semantic_disposal {
            assert!(events.try_recv().is_err());
            continue;
        }
        let DropEvent::UnfinishedCancellable { call } = events.try_recv().unwrap() else {
            panic!("admission must retain typed settlement");
        };
        let oplog = Arc::new(InMemoryOplog::new());
        call.wait_request_upload().await.unwrap();
        call.append_cancelled_with_oplog(oplog.clone(), None)
            .await
            .unwrap();
        let expected = crate::durable_host::entity::cancelled_tool_terminal(
            SerializableEntityBodyExecution::Skipped,
        )
        .await
        .unwrap();
        assert!(matches!(oplog.read(OplogIndex::INITIAL).await.unwrap(),
            OplogEntry::Cancelled { start_index, partial: Some(OplogPayload::Inline(payload)), span_finished: Some(_), .. }
            if start_index == OplogIndex::from_u64(8) && *payload == expected.into()));
        assert!(events.try_recv().is_err());
    }
}

#[test]
#[test_r::timeout("10s")]
async fn tool_trap_disposal_joins_accepted_metadata_before_cancellation_cleanup() {
    let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
    let counter = Arc::new(AtomicUsize::new(0));
    let marker = Arc::new(AtomicBool::new(false));
    let mut session =
        live_unfinished_handle::<NotCancellable>(OplogIndex::from_u64(8), sink.clone());
    let accepted = AcceptedAccessStart::<NotCancellable> {
        result: Some((session.start_idx, PendingUpload::already_durable())),
        call: Some(session.dropped_call_snapshot(Some(LiveCallPermit::new(counter.clone())))),
        marker: marker.clone(),
        sink: None,
        cleanup_sink: Some(sink.clone()),
        teardown: TeardownProbe::never(),
        _policy: PhantomData,
    };
    session.finished = true;
    drop(session);
    let (release, held) = oneshot::channel();
    let (send, receive) = oneshot::channel();
    let (settled, settlement) = oneshot::channel();
    drop(
        crate::worker::tasks::WorkerTasks::default()
            .spawn_metadata(async move {
                held.await.unwrap();
                drop(send.send(accepted));
                let _ = settled.send(Ok(()));
            })
            .unwrap(),
    );
    let guard = AccessDropEventDrainGuard::new(
        sink,
        vec![DropEvent::AwaitCompletionMarker {
            receipt: Some(MarkerReceipt::pending(settlement)),
            trap_context: DurableCallTrapContext {
                retry_from: OplogIndex::from_u64(8),
            },
            live_call_permit: Some(LiveCallPermit::new(counter.clone())),
        }],
    );
    assert!(
        events.try_recv().is_err(),
        "active initiation must not block another call's drain"
    );
    marker.store(true, Ordering::Release);
    drop(receive);
    drop(guard);
    let DropEvent::AwaitCompletionMarker {
        receipt: Some(mut receipt),
        live_call_permit,
        ..
    } = events.try_recv().unwrap()
    else {
        panic!("disposed Store must retain the accepted metadata receipt");
    };
    assert!(futures::poll!(std::pin::pin!(await_marker_receipt(&mut receipt))).is_pending());
    release.send(()).unwrap();
    await_marker_receipt(&mut receipt).await.unwrap();
    drop(live_call_permit);
    let DropEvent::UnfinishedCancellable { call } = events.try_recv().unwrap() else {
        panic!("accepted metadata must transfer the abandoned Start");
    };
    assert_eq!(counter.load(Ordering::Acquire), 1);
    drop(call);
    assert_eq!(counter.load(Ordering::Acquire), 0);
    assert!(events.try_recv().is_err());
}

#[test]
#[test_r::timeout("10s")]
async fn tool_trap_disposal_preserves_a_selected_pending_terminal() {
    let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut session =
        live_unfinished_handle::<NotCancellable>(OplogIndex::from_u64(8), sink.clone());
    session.cleanup_sink = Some(sink);
    session.tool_trap_disposal.store(true, Ordering::Release);
    let counter = Arc::new(AtomicUsize::new(0));
    session.live_call_permit = Some(LiveCallPermit::new(counter.clone()));
    let mut guard = session.terminal_guard();
    let (finish, pending) = oneshot::channel();
    guard.cleanup_after_terminal(
        tokio::spawn(async move {
            pending.await.unwrap();
            Ok(())
        }),
        None,
    );
    drop(guard);
    drop(session);
    let DropEvent::CleanupAfterTerminal {
        terminal: Some(terminal),
        live_call_permit,
        ..
    } = events.try_recv().unwrap()
    else {
        panic!("selected terminal must not be replaced by Cancelled");
    };
    assert_eq!(counter.load(Ordering::Acquire), 1);
    finish.send(()).unwrap();
    terminal.await.unwrap().unwrap();
    drop(live_call_permit);
    assert_eq!(counter.load(Ordering::Acquire), 0);
    assert!(events.try_recv().is_err());
}

#[test]
#[test_r::timeout("10s")]
async fn direct_claim_with_a_wrong_invocation_id_does_not_consume_the_start() {
    use super::super::tests::InMemoryOplog;
    use golem_common::model::oplog::HostRequestGolemRpcResource;
    use golem_common::model::oplog::host_functions::GolemRpcWasmRpcDrop;

    let recorded_invocation_id = DurableInvocationId::new(OplogIndex::from_u64(1), 7);
    for parent_start_index in [None, Some(OplogIndex::from_u64(19))] {
        let request = HostRequestGolemRpcResource {
            creation_index: OplogIndex::from_u64(7),
        };
        let oplog = Arc::new(InMemoryOplog::new());
        oplog
            .add(OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            })
            .await
            .unwrap();
        let start_index = oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index,
                function_name: GolemRpcWasmRpcDrop::HOST_FUNCTION_NAME,
                invocation_id: Some(recorded_invocation_id),
                observational_owner: None,
                request: Some(OplogPayload::Inline(Box::new(request.clone().into()))),
                durable_function_type: DurableFunctionType::ReadLocal,
                span_started: None,
                denial: None,
            })
            .await
            .unwrap();
        oplog
            .add(OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index,
                response: None,
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            })
            .await
            .unwrap();
        let replay_state = ReplayState::new_for_owner(
            golem_common::model::OwnedAgentId {
                environment_id: golem_common::model::environment::EnvironmentId::new(),
                agent_id: golem_common::model::AgentId {
                    component_id: golem_common::model::component::ComponentId::new(),
                    agent_id: "direct-claim-test".to_string(),
                },
            },
            oplog.clone(),
            golem_common::model::regions::DeletedRegions::default(),
            None,
            crate::durable_host::tool::operation::OwnerToolOperations::new(),
            oplog.current_oplog_index().await,
            None,
        )
        .await
        .unwrap();
        let claim = |invocation_id| {
            DurableCallSession::<GolemRpcWasmRpcDrop, NotCancellable>::replay_start_claim(
                &AccessClaimOptions::default(),
                &BegunCallExecutionScope {
                    entity_parent_start_index: parent_start_index,
                    parent_start_index,
                    observational_owner: None,
                    invocation_id: Some(invocation_id),
                    logical_key_position: None,
                },
                &InFunctionRetryController::new(
                    DurableFunctionType::ReadLocal,
                    DurableExecutionState {
                        is_live: false,
                        snapshotting_mode: false,
                        max_in_function_retry_delay: std::time::Duration::ZERO,
                    },
                    GolemRpcWasmRpcDrop::FQFN,
                ),
            )
            .unwrap()
        };
        let wrong = replay_state
            .claim_start_or_replay_end(claim(DurableInvocationId::new(OplogIndex::from_u64(1), 8)))
            .await;
        // A top-level Start with another invocation id belongs to a concurrent sibling call,
        // so the claim retains it and goes live; under an unknown parent it is divergence.
        if parent_start_index.is_none() {
            assert!(matches!(wrong, Ok(ReplayStartClaimOutcome::ReplayEnded)));
        } else {
            assert!(wrong.is_err());
        }
        let claimed = replay_state
            .claim_start_or_replay_end(claim(recorded_invocation_id))
            .await
            .expect("a wrong invocation id must leave the recorded operation available");
        let ReplayStartClaimOutcome::Claimed { handle, .. } = claimed else {
            panic!("expected the matching Start to remain available");
        };
        assert_eq!(handle.start_idx(), start_index);
    }
}

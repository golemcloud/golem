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
use crate::durable_host::attribution::InvocationAttribution;
use futures::FutureExt;
use test_r::test;

#[test]
#[test_r::timeout("10s")]
async fn closing_owned_initiation_retires_window_and_protocol_waits() {
    use crate::durable_host::concurrent::initiation::InitiationAccess;
    use crate::durable_host::concurrent::initiation_owner::InitiationOwner;
    use crate::worker::tasks::WorkerTasks;
    use crate::workerctx::default::Context;

    for queued_window in [false, true] {
        let owner = InitiationOwner::<Context>::new();
        let registration = owner.register().unwrap();
        let tasks = WorkerTasks::default();
        let job_owner = owner.clone();
        let (ready, waiting) = oneshot::channel();
        let job = tasks
            .spawn_metadata(async move {
                let mut access = job_owner.access(InvocationAttribution::default().capture());
                let result = job_owner
                    .run(async move {
                        if queued_window {
                            let window = access.window(|_| ());
                            ready.send(()).unwrap();
                            window.await?;
                        } else {
                            ready.send(()).unwrap();
                            std::future::pending::<()>().await;
                        }
                        Ok::<_, WorkerExecutorError>(())
                    })
                    .await;
                registration.complete();
                result.and_then(|settled| settled)
            })
            .unwrap();
        waiting.await.unwrap();
        // A closed owner must also cancel protocol waits that have no pending window.
        owner.close();
        owner.wait_closed().await;
        assert!(job.await.unwrap().is_err());
        tasks.stop_and_wait().await.unwrap();
        assert!(owner.register().is_err());
        assert!(
            owner
                .access(InvocationAttribution::default().capture())
                .window(|_| ())
                .await
                .is_err()
        );
    }
}

#[test]
#[test_r::timeout("10s")]
async fn buffered_session_preserves_cleanup_after_initiation_retirement() {
    use crate::durable_host::concurrent::initiation_owner::InitiationOwner;
    use crate::worker::tasks::WorkerTasks;
    use crate::workerctx::default::Context;

    for buffered in [false, true] {
        let owner = InitiationOwner::<Context>::new();
        let registration = owner.register().unwrap();
        let tasks = WorkerTasks::default();
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let counter = Arc::new(AtomicUsize::new(0));
        let mut session = live_unfinished_handle_with_counter::<Cancellable>(
            OplogIndex::from_u64(8),
            sink.clone(),
            counter.clone(),
        );
        session.cleanup_sink = Some(sink);
        session.tool_trap_disposal.store(true, Ordering::Release);
        let (send, receive) = oneshot::channel();
        let receive = buffered.then_some(receive);
        let job = tasks
            .spawn_metadata(async move {
                if let Err(session) = send.send(session) {
                    session.discard_after_guest_drop();
                }
                registration.complete();
            })
            .unwrap();
        job.await.unwrap();
        owner.close();
        owner.wait_closed().await;
        tasks.stop_and_wait().await.unwrap();
        if buffered {
            assert!(
                events.try_recv().is_err(),
                "the buffered session still owns cleanup"
            );
        }
        drop(receive);
        let DropEvent::UnfinishedCancellable { call } = events.try_recv().unwrap() else {
            panic!("losing the guest receiver must retain accepted-call cleanup");
        };
        assert_eq!(call.start_idx(), OplogIndex::from_u64(8));
        assert_eq!(counter.load(Ordering::Acquire), 1);
        drop(call);
        assert_eq!(counter.load(Ordering::Acquire), 0);
        assert!(events.try_recv().is_err());
    }
}

// An accepted Start must retain its cancellation obligation while the final owned
// context window is queued, just as it does at the earlier metadata handoff.
#[test]
async fn abandoned_final_initiation_window_preserves_accepted_start_cleanup() {
    use crate::durable_host::concurrent::initiation::InitiationAccess;
    use crate::durable_host::concurrent::initiation_owner::InitiationOwner;
    use crate::workerctx::default::Context;

    let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
    let counter = Arc::new(AtomicUsize::new(0));
    let start_idx = OplogIndex::from_u64(8);
    let mut session = live_unfinished_handle::<Cancellable>(start_idx, sink.clone());
    let accepted = AcceptedAccessStart::<Cancellable> {
        result: Some((start_idx, PendingUpload::already_durable())),
        call: Some(session.dropped_call_snapshot(None)),
        marker: Arc::new(AtomicBool::new(false)),
        sink: Some(sink.clone()),
        cleanup_sink: Some(sink.clone()),
        teardown: TeardownProbe::never(),
        _policy: PhantomData,
    };
    session.abandon_for_trap();
    drop(session);
    let (start_idx, state) = accepted.into_executing(LiveCallPermit::new(counter.clone()));
    let executed = ExecutedAccessStart::<
        golem_common::model::oplog::host_functions::MonotonicClockNow,
        Cancellable,
    > {
        start_idx,
        state,
        requires_agent_authority: false,
        agent_auth_ctx: None,
        execution_scope: CallExecutionScope {
            retry_from: start_idx,
            durable_scope: None,
            observational_owner: None,
            entity_parent_start_index: None,
            invocation_id: None,
            logical_key_position: None,
            agent_invocation: None,
        },
        retry: InFunctionRetryController::new(
            DurableFunctionType::ReadLocal,
            DurableExecutionState {
                is_live: true,
                snapshotting_mode: false,
                max_in_function_retry_delay: std::time::Duration::ZERO,
                durable_wait: false,
            },
            "test:monotonic_clock::now",
        ),
        scope_replay_recovery: ScopeReplayRecovery::default(),
        drop_sink: Some(sink.clone()),
        cleanup_sink: Some(sink),
        _phantom: PhantomData,
    };
    let owner = InitiationOwner::<Context>::new();
    let mut access = owner.access(InvocationAttribution::default().capture());
    // This is the final window in start_owned_initiation. Drop the context
    // service before it can construct the session, as on runtime disposal.
    let window = access.window(move |ctx| DurableCallSession::finish_access_start(ctx, executed));
    assert_eq!(counter.load(Ordering::Acquire), 1);
    drop(access);
    drop(owner);
    assert!(
        window.await.is_err(),
        "the disposed service must abandon the window"
    );
    let event = events.try_recv().expect(
        "an accepted Start lost at the final context window must transfer to cancellation cleanup",
    );
    assert!(matches!(event, DropEvent::UnfinishedCancellable { .. }));
    assert_eq!(counter.load(Ordering::Acquire), 1);
    drop(event);
    assert_eq!(counter.load(Ordering::Acquire), 0);
}

pub fn live_unfinished_handle<P: DropPolicy>(
    start_idx: OplogIndex,
    sink: UnboundedSender<DropEvent>,
) -> DurableCallSession<golem_common::model::oplog::host_functions::MonotonicClockNow, P> {
    live_unfinished_handle_with_counter(start_idx, sink, Arc::new(AtomicUsize::new(0)))
}

pub fn live_unfinished_handle_with_counter<P: DropPolicy>(
    start_idx: OplogIndex,
    sink: UnboundedSender<DropEvent>,
    counter: Arc<AtomicUsize>,
) -> DurableCallSession<golem_common::model::oplog::host_functions::MonotonicClockNow, P> {
    DurableCallSession {
        start_idx,
        state: SessionState::executing(
            CallRecording::Recorded {
                request_upload: PendingUpload::already_durable(),
            },
            LiveCallPermit::new(counter),
        ),
        scope_replay_recovery: ScopeReplayRecovery::default().into(),
        cancellation_span_id: None,
        span_observer: None,
        host_internal_calls: false,
        execution_scope: CallExecutionScope {
            retry_from: start_idx,
            durable_scope: None,
            observational_owner: None,
            entity_parent_start_index: None,
            invocation_id: None,
            logical_key_position: None,
            agent_invocation: None,
        },
        retry: InFunctionRetryController::new(
            DurableFunctionType::ReadLocal,
            DurableExecutionState {
                is_live: true,
                snapshotting_mode: false,
                max_in_function_retry_delay: std::time::Duration::ZERO,
                durable_wait: false,
            },
            "test:monotonic_clock::now",
        ),
        executor_shutdown: tokio_util::sync::CancellationToken::new(),
        runtime_teardown: Arc::new(|| false),
        tool_trap_disposal: Arc::new(AtomicBool::new(false)),
        entity_admission: false,
        requires_agent_authority: false,
        agent_auth_ctx: None,
        drop_sink: Some(sink),
        cleanup_sink: None,
        _phantom: PhantomData,
    }
}

#[test]
fn extracted_execution_retains_cleanup_and_permit_after_session_drop() {
    for terminal in [false, true] {
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let counter = Arc::new(AtomicUsize::new(0));
        let mut session = live_unfinished_handle::<Cancellable>(OplogIndex::from_u64(8), sink);
        session.state = SessionState::executing(
            CallRecording::Recorded {
                request_upload: PendingUpload::already_durable(),
            },
            LiveCallPermit::new(counter.clone()),
        );
        assert!(session.take_replay_call().is_err());
        let execution = session.take_executing_call().unwrap();
        drop(session);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        assert!(events.try_recv().is_err());
        if terminal {
            let released = execution.enter_direct_terminal();
            assert_eq!(counter.load(Ordering::Acquire), 1);
            drop(released);
            assert!(events.try_recv().is_err());
        } else {
            drop(execution);
            let DropEvent::UnfinishedCancellable { call } = events.try_recv().unwrap() else {
                panic!("the extracted call must retain its cancellation obligation");
            };
            assert_eq!(call.start_idx(), OplogIndex::from_u64(8));
            assert_eq!(counter.load(Ordering::Acquire), 1);
            drop(call);
        }
        assert_eq!(counter.load(Ordering::Acquire), 0);
        assert!(events.try_recv().is_err());
    }
}

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
        session.state = SessionState::executing(
            CallRecording::Recorded {
                request_upload: PendingUpload::spawned(
                    async move {
                        upload.await.unwrap();
                        Ok(())
                    }
                    .boxed()
                    .shared(),
                ),
            },
            LiveCallPermit::new(counter.clone()),
        );
        let guard = terminal_started.then(|| session.terminal_guard().unwrap());
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
        session.state = if replay {
            let (_, receiver) = oneshot::channel();
            let mut state =
                SessionState::replaying(ReplayCallHandle::new(session.start_idx, receiver));
            state.discard_after_guest_drop();
            state
        } else {
            SessionState::executing(
                if persisted {
                    CallRecording::Recorded {
                        request_upload: PendingUpload::already_durable(),
                    }
                } else {
                    CallRecording::Unrecorded
                },
                LiveCallPermit::new(Arc::new(AtomicUsize::new(0))),
            )
        };
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
        session.abandon_for_trap();
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
    session.abandon_for_trap();
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
    session.state = SessionState::executing(
        CallRecording::Recorded {
            request_upload: PendingUpload::already_durable(),
        },
        LiveCallPermit::new(counter.clone()),
    );
    let mut guard = session.terminal_guard().unwrap();
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
    use golem_common::model::oplog::HostRequestGolemContextSpanResource;
    use golem_common::model::oplog::host_functions::GolemContextSpanFinish;

    let invocation_anchor = OplogIndex::from_u64(2);
    let recorded_invocation_id = DurableInvocationId::new(invocation_anchor, 7);
    let parent_invocation_id = DurableInvocationId::new(invocation_anchor, 6);
    for recorded_parent in [false, true] {
        let request = HostRequestGolemContextSpanResource {
            creation_index: Some(OplogIndex::from_u64(7)),
        };
        let oplog = Arc::new(InMemoryOplog::new());
        oplog
            .add(OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            })
            .await
            .unwrap();
        oplog
            .add(OplogEntry::AgentInvocationStarted {
                timestamp: Timestamp::now_utc(),
                invocation_id: uuid::Uuid::from_u128(1).into(),
                operation: Box::new(golem_common::model::InvocationRequestOperation::SaveSnapshot),
                payload: OplogPayload::Inline(Box::new(
                    golem_common::model::StoredAgentInvocationPayload::SaveSnapshot {
                        caller: golem_common::model::caller::Caller::anonymous(),
                    },
                )),
                trace_id: golem_common::model::invocation_context::TraceId::generate(),
                trace_states: Vec::new(),
                invocation_context: Vec::new(),
                wallet_pin: Box::new(golem_common::model::card::InvocationWalletPin {
                    wallet_token: golem_common::model::card::WalletVersionToken {
                        wallet_id_hash: [3; 32],
                        generation: 0,
                    },
                    pinned_card_ids: Vec::new(),
                    scope_card_id: None,
                }),
            })
            .await
            .unwrap();
        let parent_start_index = if recorded_parent {
            Some(
                oplog
                    .add(OplogEntry::Start {
                        timestamp: Timestamp::now_utc(),
                        parent_start_index: None,
                        function_name: HostFunctionName::MonotonicClockNow,
                        invocation_id: Some(parent_invocation_id),
                        observational_owner: None,
                        driver: None,
                        request: Some(OplogPayload::Inline(Box::new(
                            golem_common::model::oplog::HostRequestNoInput {}.into(),
                        ))),
                        durable_function_type: DurableFunctionType::ReadLocal,
                        span_started: None,
                        denial: None,
                    })
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        let start_index = oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index,
                function_name: GolemContextSpanFinish::HOST_FUNCTION_NAME,
                invocation_id: Some(recorded_invocation_id),
                observational_owner: None,
                driver: None,
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
                agent_id: golem_common::model::AgentId::from_parts(7, 1),
            },
            oplog.clone(),
            golem_common::model::regions::DeletedRegions::default(),
            None,
            crate::durable_host::tool::operation::OwnerToolOperations::new(),
            oplog.current_oplog_index().await,
            None,
            false,
        )
        .await
        .unwrap();
        replay_state
            .get_oplog_entry_agent_invocation_started()
            .await
            .unwrap()
            .expect("an invocation starts");
        let _parent = if recorded_parent {
            let ReplayStartClaimOutcome::Claimed { handle, .. } = replay_state
                .claim_start_or_replay_end(
                    StartClaim::unowned(
                        &HostFunctionName::MonotonicClockNow,
                        &DurableFunctionType::ReadLocal,
                    )
                    .with_invocation_id(Some(parent_invocation_id)),
                )
                .await
                .unwrap()
            else {
                panic!("expected the recorded parent Start to be claimed");
            };
            Some(handle)
        } else {
            None
        };
        let claim = |invocation_id| {
            DurableCallSession::<GolemContextSpanFinish, NotCancellable>::replay_start_claim(
                &AccessClaimOptions::default(),
                &BegunCallExecutionScope {
                    entity_parent_start_index: parent_start_index,
                    parent_start_index,
                    observational_owner: None,
                    driver: None,
                    invocation_id: Some(invocation_id),
                    logical_key_position: None,
                    agent_invocation: None,
                },
                &InFunctionRetryController::new(
                    DurableFunctionType::ReadLocal,
                    DurableExecutionState {
                        is_live: false,
                        snapshotting_mode: false,
                        max_in_function_retry_delay: std::time::Duration::ZERO,
                        durable_wait: false,
                    },
                    GolemContextSpanFinish::FQFN,
                ),
            )
            .unwrap()
        };
        let wrong = replay_state
            .claim_start_or_replay_end(claim(DurableInvocationId::new(invocation_anchor, 8)))
            .await;
        // A top-level Start with another invocation id belongs to a concurrent sibling call,
        // so the claim retains it and goes live; under the claim's own recorded parent it is
        // divergence.
        if parent_start_index.is_none() {
            assert!(matches!(wrong, Ok(ReplayStartClaimOutcome::ReplayEnded)));
        } else {
            assert!(
                matches!(
                    &wrong,
                    Err(WorkerExecutorError::UnexpectedOplogEntry { divergence })
                        if divergence.oplog_index == start_index
                            && divergence.mismatched
                                == [golem_common::model::oplog::StartIdentityField::InvocationId]
                ),
                "unexpected error: {:?}",
                wrong.as_ref().err()
            );
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

#[test]
fn a_replayed_cancellation_of_a_recorded_completion_has_the_end_as_expected_step() {
    use golem_common::model::oplog::ReplayStep;

    for resolution in [
        Resolution::Completed {
            end_idx: OplogIndex::from_u64(9),
            response: None,
            delivery_marker: None,
            forced_commit: false,
        },
        Resolution::CompletedButDiscarded {
            end_idx: OplogIndex::from_u64(9),
            marker_idx: OplogIndex::from_u64(11),
            response: None,
        },
    ] {
        let error = expect_cancelled_resolution(&resolution).unwrap_err();

        let WorkerExecutorError::UnexpectedOplogEntry { divergence } = error else {
            panic!("expected a replay divergence, got {error}");
        };
        assert_eq!(divergence.oplog_index, OplogIndex::from_u64(9));
        assert_eq!(divergence.expected, ReplayStep::described("End at 9"));
        assert_eq!(divergence.actual, ReplayStep::described("Cancelled"));
    }
    assert!(
        expect_cancelled_resolution(&Resolution::Cancelled {
            cancelled_idx: OplogIndex::from_u64(9),
            partial: None,
        })
        .is_ok()
    );
}

async fn replay_state_with(
    entries: Vec<OplogEntry>,
    deleted: golem_common::model::regions::DeletedRegions,
) -> crate::durable_host::replay_state::ReplayState {
    use super::super::tests::InMemoryOplog;

    let oplog = Arc::new(InMemoryOplog::new());
    for entry in entries {
        oplog.add(entry).await.unwrap();
    }
    crate::durable_host::replay_state::ReplayState::new_for_owner(
        golem_common::model::OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id: golem_common::model::AgentId::from_parts(7, 1),
        },
        oplog.clone(),
        deleted,
        None,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        oplog.current_oplog_index().await,
        None,
        false,
    )
    .await
    .unwrap()
}

fn clock_start(
    function_name: golem_common::model::oplog::host_functions::HostFunctionName,
) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name,
        invocation_id: None,
        observational_owner: None,
        driver: None,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            golem_common::model::oplog::HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::ReadLocal,
        span_started: None,
        denial: None,
    }
}

fn clock_end(start_index: u64) -> OplogEntry {
    OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
        response: Some(OplogPayload::Inline(Box::new(
            HostResponse::MonotonicClockTimestamp(
                golem_common::model::oplog::HostResponseMonotonicClockTimestamp { nanos: 1 },
            ),
        ))),
        forced_commit: false,
        span_finished: None,
        span_attributes: None,
    }
}

fn recovery_succeeded() -> OplogEntry {
    OplogEntry::RecoverySucceeded {
        timestamp: Timestamp::now_utc(),
    }
}

fn required_divergence(
    result: Result<ReplayStartClaimOutcome, WorkerExecutorError>,
) -> golem_common::model::oplog::ReplayDivergence {
    match result {
        Err(WorkerExecutorError::UnexpectedOplogEntry { divergence }) => *divergence,
        Err(error) => panic!("expected a replay divergence, got {error}"),
        Ok(_) => panic!("a required claim without its Start must fail"),
    }
}

fn replayed_function_name(step: &golem_common::model::oplog::ReplayStep) -> String {
    match step {
        golem_common::model::oplog::ReplayStep::HostCall(call) => call.function_name.clone(),
        other => panic!("expected the replayed host call, got {other}"),
    }
}

#[test]
#[test_r::timeout("10s")]
async fn a_required_claim_of_a_locally_live_store_names_the_surviving_history() {
    use golem_common::model::oplog::ReplayStep;
    use golem_common::model::oplog::host_functions::HostFunctionName;

    // [RecoverySucceeded, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5), RemoveRetryPolicy(6),
    // Start(C=7), End(C=7→8)]. A's awaiter retains B and stops at the positional
    // RemoveRetryPolicy(6), so history survives past the cursor at 5.
    let replay_state = replay_state_with(
        vec![
            recovery_succeeded(),
            clock_start(HostFunctionName::MonotonicClockNow),
            clock_start(HostFunctionName::MonotonicClockNow),
            clock_end(2),
            clock_end(3),
            OplogEntry::RemoveRetryPolicy {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                name: "replay-policy".to_string(),
            },
            clock_start(HostFunctionName::MonotonicClockResolution),
            clock_end(7),
        ],
        Default::default(),
    )
    .await;
    let handle = replay_state
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    replay_state.await_resolution(handle).await.unwrap();
    assert_eq!(replay_state.last_replayed_index(), OplogIndex::from_u64(5));
    let claim = || {
        StartClaim::unowned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
        )
    };

    // An ordinary claim of a Store that continued live is a new live call of that Store.
    let ordinary =
        claims::claim_for_store(replay_state.clone(), claim(), true, false, |_, _| {}, None)
            .await
            .unwrap();
    assert!(matches!(
        ordinary,
        ReplayStartClaimOutcome::StoreAlreadyLive
    ));

    // A required claim runs with historical admission and names the entry the cursor holds.
    let divergence = required_divergence(
        claims::claim_for_store(replay_state.clone(), claim(), true, true, |_, _| {}, None).await,
    );
    assert_eq!(
        divergence.oplog_index,
        OplogIndex::from_u64(6),
        "{divergence}"
    );
    assert_ne!(
        divergence.expected,
        ReplayStep::end_of_history(),
        "{divergence}"
    );
    assert!(
        divergence
            .expected
            .to_string()
            .starts_with("RemoveRetryPolicy"),
        "the surviving positional entry is the expected step: {divergence}"
    );
    assert_eq!(
        replayed_function_name(&divergence.actual),
        HostFunctionName::WallClockNow.to_string()
    );
}

#[test]
#[test_r::timeout("10s")]
async fn a_required_claim_at_the_end_of_the_history_has_end_of_history_as_expected_step() {
    use golem_common::model::oplog::ReplayStep;
    use golem_common::model::oplog::host_functions::HostFunctionName;

    let replay_state = replay_state_with(vec![recovery_succeeded()], Default::default()).await;
    for store_continued_live in [false, true] {
        let divergence = required_divergence(
            claims::claim_for_store(
                replay_state.clone(),
                StartClaim::unowned(
                    &HostFunctionName::MonotonicClockNow,
                    &DurableFunctionType::ReadLocal,
                ),
                store_continued_live,
                true,
                |_, _| {},
                None,
            )
            .await,
        );
        assert_eq!(
            divergence.oplog_index,
            OplogIndex::from_u64(2),
            "{divergence}"
        );
        assert_eq!(divergence.expected, ReplayStep::end_of_history());
        assert_eq!(
            replayed_function_name(&divergence.actual),
            HostFunctionName::MonotonicClockNow.to_string()
        );
    }
}

#[test]
#[test_r::timeout("10s")]
async fn a_required_claim_of_a_deleted_start_keeps_its_index() {
    use golem_common::model::oplog::ReplayStep;
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::regions::{DeletedRegions, OplogRegion};

    // [RecoverySucceeded, Start(2), End(2→3), RemoveRetryPolicy(4)] with 2..=3 deleted.
    let replay_state = replay_state_with(
        vec![
            recovery_succeeded(),
            clock_start(HostFunctionName::MonotonicClockNow),
            clock_end(2),
            OplogEntry::RemoveRetryPolicy {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                name: "replay-policy".to_string(),
            },
        ],
        DeletedRegions::from_regions([OplogRegion::from_range(2..=3)]),
    )
    .await;
    for store_continued_live in [false, true] {
        let divergence = required_divergence(
            claims::claim_for_store(
                replay_state.clone(),
                StartClaim::unowned(
                    &HostFunctionName::MonotonicClockNow,
                    &DurableFunctionType::ReadLocal,
                ),
                store_continued_live,
                true,
                |_, _| {},
                None,
            )
            .await,
        );
        assert_eq!(
            divergence.oplog_index,
            OplogIndex::from_u64(2),
            "a deleted-region Start keeps its own index: {divergence}"
        );
        assert!(
            matches!(divergence.expected, ReplayStep::Other(_)),
            "{divergence}"
        );
        assert_eq!(
            replayed_function_name(&divergence.actual),
            HostFunctionName::MonotonicClockNow.to_string()
        );
    }
}

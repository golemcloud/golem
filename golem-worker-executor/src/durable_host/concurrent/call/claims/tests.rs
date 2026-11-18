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

use super::{AccessClaimOptions, CallExecutionScope, HostInternalCallOwner, ScopeReplayRecovery};
use golem_common::model::oplog::OplogIndex;
use test_r::test;

#[test]
fn entity_lineage_does_not_authorize_explicit_protocol_children() {
    let entity = OplogIndex::from_u64(17);
    let mut options = AccessClaimOptions::default();
    assert!(options.authorizes_incomplete_repair(Some(entity)));
    assert!(!options.authorizes_incomplete_repair(None));
    options.parent_start_index = Some(entity);
    assert!(!options.authorizes_incomplete_repair(Some(entity)));
    options.incomplete_repair_authorized_by_scope = true;
    assert!(options.authorizes_incomplete_repair(Some(entity)));

    options.incomplete_repair_authorized_by_scope = false;
    options.parent_start_index = None;
    options.host_internal_owner = Some(HostInternalCallOwner {
        start_index: entity,
        execution_scope: CallExecutionScope {
            entity_parent_start_index: Some(entity),
            retry_from: entity,
            durable_scope: Some(entity),
            observational_owner: None,
            invocation_id: None,
            logical_key_position: None,
            agent_invocation: None,
        },
        declared: false,
    });
    assert!(!options.authorizes_incomplete_repair(Some(entity)));
    options.incomplete_repair_authorized_by_scope = true;
    assert!(options.authorizes_incomplete_repair(Some(entity)));
}

#[test]
fn reexecute_policy_authorizes_existing_incomplete_scope() {
    let recovery = ScopeReplayRecovery::reexecute_when(async { Ok(()) });

    assert!(recovery.allows_new_call());
    assert!(ScopeReplayRecovery::Default.allows_new_call());
}

#[test]
fn typed_loss_requires_an_existing_start() {
    assert!(!ScopeReplayRecovery::CompleteWithLoss.allows_new_call());
}

fn replaying_call(
    recovery: ScopeReplayRecovery,
    child_authorized: Option<bool>,
) -> super::DurableCallSession<
    golem_common::model::oplog::host_functions::MonotonicClockNow,
    super::LeaveIncompleteOnDrop,
> {
    use super::*;
    let index = OplogIndex::from_u64(5);
    let (_, receiver) = tokio::sync::oneshot::channel();
    let state = SessionState::replaying(ReplayCallHandle::new(index, receiver));
    DurableCallSession::<
        golem_common::model::oplog::host_functions::MonotonicClockNow,
        LeaveIncompleteOnDrop,
    > {
        start_idx: index,
        state,
        cancellation_span_id: None,
        span_observer: None,
        host_internal_calls: false,
        execution_scope: CallExecutionScope {
            retry_from: index,
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
                is_live: false,
                snapshotting_mode: false,
                max_in_function_retry_delay: std::time::Duration::ZERO,
            },
            "test:monotonic_clock::now",
        )
        .with_parent_start_index(child_authorized.map(|_| OplogIndex::from_u64(2)))
        .with_incomplete_repair_authorized_by_scope(child_authorized.unwrap_or(false)),
        scope_replay_recovery: recovery.into(),
        executor_shutdown: tokio_util::sync::CancellationToken::new(),
        runtime_teardown: Arc::new(|| false),
        tool_trap_disposal: Arc::new(AtomicBool::new(false)),
        entity_admission: false,
        requires_agent_authority: false,
        agent_auth_ctx: None,
        drop_sink: None,
        cleanup_sink: None,
        _phantom: PhantomData,
    }
}

#[test]
async fn incomplete_repair_readiness_blocks_snapshot_boundary() {
    use super::*;
    let counter = Arc::new(AtomicUsize::new(0));
    let (ready, readiness) = tokio::sync::oneshot::channel();
    let mut call = replaying_call(
        ScopeReplayRecovery::reexecute_when(async { readiness.await.unwrap() }),
        None,
    );
    let (_, pending_call) = call.take_replay_call().unwrap().begin_resolution();
    let mut repair =
        Box::pin(call.prepare_incomplete_live_repair(pending_call, || counter.clone()));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(repair.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert!(
        !crate::durable_host::PrivateDurableWorkerState::clean_checkpoint_boundary(
            false,
            counter.load(Ordering::Acquire) > 0,
            false,
        ),
        "repair waiting for readiness must block snapshot/checkpoint admission"
    );
    ready
        .send(Err(WorkerExecutorError::recovery_required(
            "recording unavailable",
        )))
        .unwrap();
    assert!(matches!(
        repair.await,
        Err(WorkerExecutorError::RecoveryRequired { .. })
    ));
    assert!(!call.is_live());
    assert!(call.state.is_released());
    assert_eq!(counter.load(Ordering::Acquire), 0);
}

#[test]
async fn cancelling_pending_repair_releases_permit_without_cancelling_the_call() {
    use super::*;
    let counter = Arc::new(AtomicUsize::new(0));
    let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut call = replaying_call(
        ScopeReplayRecovery::reexecute_when(std::future::pending()),
        None,
    );
    call.entity_admission = true;
    call.tool_trap_disposal.store(true, Ordering::Release);
    call.cleanup_sink = Some(sink);
    let (_, pending_call) = call.take_replay_call().unwrap().begin_resolution();
    let mut repair =
        Box::pin(call.prepare_incomplete_live_repair(pending_call, || counter.clone()));
    assert!(futures::poll!(&mut repair).is_pending());
    assert_eq!(counter.load(Ordering::Acquire), 1);
    drop(repair);
    assert_eq!(counter.load(Ordering::Acquire), 0);
    assert!(!call.is_live());
    assert_eq!(call.start_index(), OplogIndex::from_u64(5));
    drop(call);
    assert!(events.try_recv().is_err());
}

#[test]
#[test_r::timeout("10s")]
async fn failed_parent_replay_resolution_does_not_warn_on_drop() {
    use super::*;
    use crate::durable_host::concurrent::tests::InMemoryOplog;
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::prelude::*;

    struct Warnings(Arc<AtomicUsize>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Warnings {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    let oplog = Arc::new(InMemoryOplog::new());
    oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    let replay_state = ReplayState::new_for_owner(
        golem_common::model::OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id: golem_common::model::AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "parent-replay-failure-test".to_string(),
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
    let warnings = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(Warnings(warnings.clone()));
    async {
        // Check that the subscriber sees an actually abandoned replay handle.
        let mut unfinished = replaying_call(ScopeReplayRecovery::Default, None);
        let (_, pending) = unfinished.take_replay_call().unwrap().begin_resolution();
        drop(pending);
        drop(unfinished);
        assert_eq!(warnings.swap(0, Ordering::Relaxed), 1);

        // The fixture's resolver sender is closed, so resolution must return its error.
        let mut call = replaying_call(ScopeReplayRecovery::Default, None);
        let error = match call.resolve_incomplete_scope(&replay_state).await {
            Err(error) => error,
            Ok(_) => panic!("a closed replay resolver must fail"),
        };
        assert!(
            error
                .to_string()
                .contains("resolver channel closed for Start at 5")
        );
        call.abandon_for_trap();
        drop(call);
        assert_eq!(warnings.load(Ordering::Relaxed), 0);
        assert_eq!(oplog.current_oplog_index().await, OplogIndex::INITIAL);
    }
    .with_subscriber(subscriber)
    .await;
}

#[test]
async fn demand_children_share_the_parent_readiness_decision() {
    use super::*;
    use golem_common::model::oplog::host_functions::MonotonicClockNow;
    for unavailable in [false, true] {
        let (ready, readiness) = tokio::sync::oneshot::channel();
        let mut parent = replaying_call(
            ScopeReplayRecovery::reexecute_when(async { readiness.await.unwrap() }),
            None,
        );
        let mut child = replaying_call(parent.child_replay_recovery(), Some(true));
        let mut new_child = parent.child_replay_recovery();
        let counter = Arc::new(AtomicUsize::new(0));
        let (_, pending_child) = child.take_replay_call().unwrap().begin_resolution();
        let mut repair =
            Box::pin(child.prepare_incomplete_live_repair(pending_child, || counter.clone()));
        let mut begin = Box::pin(new_child.await_new_call_readiness::<MonotonicClockNow>());
        assert!(futures::poll!(&mut repair).is_pending());
        assert!(futures::poll!(&mut begin).is_pending());
        assert!(!parent.is_live());
        ready
            .send(if unavailable {
                Err(WorkerExecutorError::recovery_required(
                    "payload unavailable",
                ))
            } else {
                Ok(())
            })
            .unwrap();
        assert_eq!(repair.await.is_err(), unavailable);
        assert_eq!(begin.await.is_err(), unavailable);
        assert_eq!(child.is_live(), !unavailable);
        let (_, pending_parent) = parent.take_replay_call().unwrap().begin_resolution();
        assert_eq!(
            parent
                .prepare_incomplete_live_repair(pending_parent, || counter.clone())
                .await
                .is_err(),
            unavailable
        );
        assert_eq!(parent.is_live(), !unavailable);
        parent.abandon_for_trap();
        child.abandon_for_trap();
    }
}

#[test]
async fn deferred_typed_loss_allows_only_the_existing_parent_and_its_children() {
    use super::*;
    use golem_common::model::oplog::host_functions::MonotonicClockNow;
    let mut recovery =
        ScopeReplayRecovery::decide_when(async { Ok(ScopeReplayDecision::CompleteWithLoss) });
    let mut child = recovery.for_child();
    child
        .await_new_call_readiness::<MonotonicClockNow>()
        .await
        .unwrap();
    recovery.await_readiness().await.unwrap();
    assert!(
        recovery
            .await_new_call_readiness::<MonotonicClockNow>()
            .await
            .unwrap_err()
            .to_string()
            .contains("without a recorded Start")
    );
}

#[test]
async fn typed_owner_loss_requires_child_authorization() {
    use super::*;
    let counter = Arc::new(AtomicUsize::new(0));
    let mut unauthorized = replaying_call(ScopeReplayRecovery::CompleteWithLoss, Some(false));
    let (_, pending_call) = unauthorized.take_replay_call().unwrap().begin_resolution();
    let error = unauthorized
        .prepare_incomplete_live_repair(pending_call, || counter.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no owner authorization"));
    assert!(!unauthorized.is_live());
    assert_eq!(counter.load(Ordering::Acquire), 0);

    let mut authorized = replaying_call(ScopeReplayRecovery::CompleteWithLoss, Some(true));
    let (_, pending_call) = authorized.take_replay_call().unwrap().begin_resolution();
    authorized
        .prepare_incomplete_live_repair(pending_call, || counter.clone())
        .await
        .unwrap();
    assert!(authorized.is_live());
    assert_eq!(authorized.start_index(), OplogIndex::from_u64(5));
    authorized.abandon_for_trap();
}

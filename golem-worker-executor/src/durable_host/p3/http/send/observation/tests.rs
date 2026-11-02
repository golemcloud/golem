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
use crate::durable_host::concurrent::{CompletionDelivery, DropEvent, cleanup_recorder};
use crate::durable_host::p3::http::test_support::FrameTestOplog;
use crate::services::oplog::Oplog;
use golem_common::model::Timestamp;
use golem_common::model::invocation_context::TraceId;
use golem_common::model::oplog::{HostPayloadPair, HostRequestNoInput, OplogEntry, OplogPayload};
use test_r::test;

fn opening() -> OplogEntry {
    OplogEntry::Start {
        timestamp: "2025-03-12T13:14:00Z".parse().unwrap(),
        parent_start_index: None,
        function_name: P3HttpClientSend::HOST_FUNCTION_NAME,
        invocation_id: None,
        observational_owner: Some(OplogIndex::from_u64(7)),
        request: Some(OplogPayload::Inline(Box::new(HostRequestNoInput {}.into()))),
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: Some(Box::new(SpanStarted {
            span_id: SpanId::generate(),
            trace_id: TraceId::generate(),
            trace_states: Vec::new(),
            parent_span_id: None,
            links: Vec::new(),
            started_at: "2025-03-12T13:14:00Z".parse().unwrap(),
            attributes: AttributeMap(HashMap::new()),
            kind: SpanKind::Client,
        })),
        denial: None,
    }
}

#[test]
async fn send_span_abandonment_waits_for_opening_and_preserves_drop_time() {
    for abandon_before_start in [true, false] {
        let oplog = FrameTestOplog::new();
        oplog
            .add(OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            })
            .await
            .unwrap();
        let (recorder, mut events) = cleanup_recorder(oplog.clone(), true, true).await;
        let lifecycle = Arc::new(P3SendSpanLifecycle::new(recorder));
        let entry = opening();
        let finished_at: Timestamp = "2025-03-12T13:14:15.123Z".parse().unwrap();
        if abandon_before_start {
            lifecycle.state.lock().unwrap().abandonment = Some(finished_at);
            lifecycle.abandon();
            assert!(events.try_recv().is_err());
            assert_eq!(oplog.entry_count(), 1);
        }
        let index = oplog.add(entry.clone()).await.unwrap();
        lifecycle.clone().started(index, &entry, false);
        if !abandon_before_start {
            lifecycle.state.lock().unwrap().abandonment = Some(finished_at);
            lifecycle.abandon();
        }
        lifecycle.abandon();
        let DropEvent::FinishP3HttpSpan { cleanup } = events.try_recv().unwrap() else {
            panic!("expected cleanup");
        };
        cleanup.progress.await.unwrap();
        let entries = oplog.entries();
        assert_eq!(entries.len(), 3);
        assert!(
            matches!(&entries[2], OplogEntry::SpanClosed { resource_start_index: Some(send_index), span_finished: finished, .. }
            if *send_index == index && finished.finished_at == finished_at
                && finished.span_id == lifecycle.started_span().unwrap().span_id
                && finished.outcome == golem_common::model::oplog::SpanOutcome::Abandoned)
        );
        assert!(events.try_recv().is_err());
    }
}

#[test]
async fn send_span_terminal_owner_prevents_abandonment_cleanup() {
    for outcome in [
        golem_common::model::oplog::SpanOutcome::Failed,
        golem_common::model::oplog::SpanOutcome::Cancelled,
    ] {
        let oplog = FrameTestOplog::new();
        oplog
            .add(OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            })
            .await
            .unwrap();
        let (recorder, mut events) = cleanup_recorder(oplog.clone(), true, true).await;
        let lifecycle = Arc::new(P3SendSpanLifecycle::new(recorder));
        let entry = opening();
        let index = oplog.add(entry.clone()).await.unwrap();
        lifecycle.clone().started(index, &entry, false);
        lifecycle.closed(&SpanFinished {
            span_id: lifecycle.started_span().unwrap().span_id,
            finished_at: Timestamp::now_utc(),
            outcome,
        });
        lifecycle.abandon();
        assert_eq!(oplog.entries()[1..], [entry]);
        assert!(events.try_recv().is_err());
    }
}

#[test]
async fn send_span_teardown_and_snapshot_do_not_append_cleanup() {
    for persisted in [true, false] {
        let oplog = FrameTestOplog::new();
        oplog
            .add(OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            })
            .await
            .unwrap();
        let (recorder, mut events) = cleanup_recorder(oplog.clone(), true, persisted).await;
        let lifecycle = Arc::new(P3SendSpanLifecycle::new(recorder));
        let entry = opening();
        lifecycle
            .clone()
            .started(OplogIndex::from_u64(3), &entry, false);
        if !persisted {
            lifecycle.abandon();
            assert!(matches!(
                events.try_recv(),
                Ok(DropEvent::FinishP3HttpSpan { .. })
            ));
        }
        drop(lifecycle);
        assert!(events.try_recv().is_err());
        assert_eq!(oplog.entry_count(), 1);
    }
}

#[test]
#[test_r::timeout("30s")]
async fn repaired_send_not_delivered_closes_the_original_span_live() {
    assert_send_abandonment_after_live_tail(false, false).await;
}

#[test]
#[test_r::timeout("30s")]
async fn markerless_completed_send_not_delivered_closes_the_original_span_live() {
    assert_send_abandonment_after_live_tail(true, false).await;
}

#[test]
#[test_r::timeout("30s")]
async fn completed_entity_send_abandonment_does_not_borrow_owner_liveness() {
    assert_send_abandonment_after_live_tail(true, true).await;
}

async fn assert_send_abandonment_after_live_tail(recorded_end: bool, completed_entity: bool) {
    use crate::durable_host::concurrent::{
        Resolution, ResolutionOutcome, cleanup_recorder_for_store,
    };
    use crate::durable_host::replay_state::{
        ReplayStartClaimOutcome, ReplayState, ReplayToLiveRole, StartClaim,
    };
    use golem_common::model::entity::InvocationExecutionMode;
    use golem_common::model::{AgentId, OwnedAgentId};

    let oplog = FrameTestOplog::new();
    oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    let mut entry = opening();
    if let OplogEntry::Start {
        observational_owner,
        ..
    } = &mut entry
    {
        *observational_owner = None;
    }
    let index = oplog.add(entry.clone()).await.unwrap();
    // Successful sends leave the span open for body consumption or response abandonment.
    let end = OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index: index,
        response: None,
        forced_commit: false,
        span_finished: None,
        span_attributes: None,
    };
    if recorded_end {
        oplog.add(end.clone()).await.unwrap();
        oplog
            .add(OplogEntry::RemoveRetryPolicy {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                name: "http-tail-sentinel".to_string(),
            })
            .await
            .unwrap();
    }
    let replay = ReplayState::new_for_owner(
        OwnedAgentId {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            agent_id: AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "http-repair".to_string(),
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
    let (recorder, mut events) = cleanup_recorder_for_store(
        oplog.clone(),
        replay.clone(),
        completed_entity.then_some(InvocationExecutionMode::ReplayingCompleted),
        Arc::new(AtomicBool::new(false)),
        true,
    );
    let lifecycle = Arc::new(P3SendSpanLifecycle::new(recorder));
    let ReplayStartClaimOutcome::Claimed { handle, .. } = replay
        .claim_start_or_replay_end(StartClaim::unowned(
            &P3HttpClientSend::HOST_FUNCTION_NAME,
            &DurableFunctionType::WriteRemote,
        ))
        .await
        .unwrap()
    else {
        panic!("send Start must replay")
    };
    lifecycle.clone().started(index, &entry, true);
    let resolution = replay.await_resolution_outcome(handle).await.unwrap();
    if recorded_end {
        assert!(matches!(
            resolution,
            ResolutionOutcome::Resolved(Resolution::Completed {
                delivery_marker: None,
                ..
            })
        ));
    } else {
        assert!(matches!(resolution, ResolutionOutcome::Incomplete));
    }
    let mut delivery = recorded_end.then(|| {
        if completed_entity {
            CompletionDelivery::test_replay_discarded()
        } else {
            CompletionDelivery::test_replay_at_tail(oplog.clone(), replay.clone(), index, None)
        }
    });
    if recorded_end && !completed_entity {
        let preparation = delivery.as_mut().unwrap().prepare_delivery(None);
        tokio::pin!(preparation);
        assert!(futures::poll!(preparation.as_mut()).is_pending());
    }
    if recorded_end {
        let (_, sentinel) = replay.get_oplog_entry(None).await.unwrap();
        assert!(
            matches!(sentinel, OplogEntry::RemoveRetryPolicy { name, .. }
            if name == "http-tail-sentinel")
        );
    }
    let memory = crate::services::linear_memory::LinearMemoryTracker::new(
        2,
        2,
        golem_common::model::agent::AgentMode::Durable,
        true,
        Arc::new(crate::services::resource_limits::AtomicResourceEntry::new(
            0, 10, 0, 0, 0,
        )),
        Arc::new(Mutex::new(
            crate::services::active_agents::MemoryGrant::inert(2),
        )),
        std::time::Instant::now(),
    );
    replay
        .switch_to_live(&memory, ReplayToLiveRole::PrimaryAgent)
        .await
        .unwrap();
    assert!(replay.is_live_published());
    let mut delivery = if let Some(delivery) = delivery.take() {
        delivery
    } else {
        oplog.add(end).await.unwrap();
        CompletionDelivery::test_live_armed(oplog.clone(), index)
            .await
            .unwrap()
    };
    delivery.prepare_delivery(None).await.unwrap();
    lifecycle.abandon();
    settle_send_terminal(TerminalConsumption::NotDelivered, delivery);
    let DropEvent::FinishP3HttpSpan { cleanup } = events.try_recv().unwrap() else {
        panic!("abandonment must enqueue resident cleanup");
    };
    cleanup.progress.await.unwrap();
    let entries = oplog.entries();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::Start { .. }))
            .count(),
        1
    );
    assert_eq!(
        entries.iter().any(|entry| matches!(entry,
        OplogEntry::SpanClosed { resource_start_index: Some(source), span_finished, .. }
        if *source == index && span_finished.span_id == lifecycle.started_span().unwrap().span_id
            && span_finished.outcome == golem_common::model::oplog::SpanOutcome::Abandoned)),
        !completed_entity
    );
    assert_eq!(
        entries.iter().any(|entry| matches!(entry,
        OplogEntry::CompletionDiscarded { start_index, .. } if *start_index == index)),
        !completed_entity
    );
    assert!(events.try_recv().is_err());
}

#[test]
async fn send_observer_records_only_positive_guest_consumption() {
    for consumption in [
        None,
        Some(TerminalConsumption::Superseded),
        Some(TerminalConsumption::Delivered),
        Some(TerminalConsumption::NotDelivered),
    ] {
        let oplog = FrameTestOplog::new();
        let start_index = oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name:
                    golem_common::model::oplog::host_functions::HostFunctionName::P3HttpClientSend,
                invocation_id: None,
                observational_owner: None,
                request: Some(OplogPayload::Inline(Box::new(HostRequestNoInput {}.into()))),
                durable_function_type: DurableFunctionType::WriteRemote,
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
        let delivery = CompletionDelivery::test_live_armed(oplog.clone(), start_index)
            .await
            .unwrap();
        let mut state = SendTerminalState {
            consumption: None,
            delivery: Some(delivery),
        };
        if let Some(consumption) = consumption {
            settle_send_terminal(consumption, state.delivery.take().unwrap());
        }
        drop(state);
        let entries = oplog.entries();
        match consumption {
            None | Some(TerminalConsumption::Superseded) => assert_eq!(entries.len(), 2),
            Some(TerminalConsumption::Delivered) => assert!(matches!(
                entries.as_slice(),
                [_, _, OplogEntry::CompletionDelivered { start_index: recorded, .. }]
                    if *recorded == start_index
            )),
            Some(TerminalConsumption::NotDelivered) => assert!(matches!(
                entries.as_slice(),
                [_, _, OplogEntry::CompletionDiscarded { start_index: recorded, .. }]
                    if *recorded == start_index
            )),
        }
    }
}

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
use golem_common::model::oplog::{
    HostPayloadPair, HostRequest, HostRequestNoInput, OplogEntry, OplogPayload,
};
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
        let entries = oplog.entries();
        assert_eq!(entries.len(), 4);
        assert!(matches!(&entries[2], OplogEntry::Start {
            function_name, observational_owner: Some(owner), request: Some(OplogPayload::Inline(request)), ..
        } if *function_name == golem_common::model::oplog::host_functions::P3HttpSpanCleanup::HOST_FUNCTION_NAME
            && *owner == OplogIndex::from_u64(7)
            && matches!(request.as_ref(), HostRequest::P3HttpSpanCleanup(request) if request.send_start_index == index)));
        assert!(
            matches!(&entries[3], OplogEntry::End { span_finished: Some(finished), .. }
            if finished.finished_at == finished_at
                && finished.span_id == lifecycle.started_span().unwrap().span_id
                && finished.outcome == golem_common::model::oplog::SpanOutcome::Abandoned)
        );
        assert!(matches!(
            events.try_recv(),
            Ok(DropEvent::FinishP3HttpSpan { .. })
        ));
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

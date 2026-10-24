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

use super::oplog;
use std::collections::HashMap;

use golem_common::base_model::oplog::{
    QueuedCardEvent, QueuedCardEventTransfer, QueuedCardEventTransferReceived,
};
use golem_common::model::agent::Principal;
use golem_common::model::card::{
    AccountCardHolder, ApplicationCardHolder, Card, CardHolder, CardId, InvocationWalletPin,
    PublicInvocationWalletPin, WalletVersionToken,
};
use golem_common::model::invocation_context::{AttributeValue, SpanId, TraceId};
use golem_common::model::oplog::payload::host_functions::HostFunctionName;
use golem_common::model::oplog::public_oplog_entry::{
    AgentInvocationStartedParams, CancelledParams, EndParams, StartParams,
};
use golem_common::model::oplog::{
    AttributeMap, DurableFunctionType, OplogEntry, OplogPayload, PublicAgentInvocation,
    PublicOplogEntry, SpanAttributes, SpanFinished, SpanKind, SpanLink, SpanOutcome, SpanStarted,
};
use golem_common::model::{AgentInvocationPayload, Empty, IdempotencyKey, Timestamp};
use golem_common::schema::SchemaValue;
use test_r::test;
use uuid::Uuid;

fn test_card(card_id: CardId) -> Card {
    Card {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: chrono::Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    }
}

#[test]
fn span_lifecycle_projections_keep_asymmetric_ids_and_timestamps() {
    let origin_trace_id = TraceId::from_string("11111111111111111111111111111111").unwrap();
    let linked_trace_id = TraceId::from_string("22222222222222222222222222222222").unwrap();
    let started_span_id = SpanId::from_string("1111111111111111").unwrap();
    let linked_span_id = SpanId::from_string("2222222222222222").unwrap();
    let finished_span_id = SpanId::from_string("3333333333333333").unwrap();
    let attributes_span_id = SpanId::from_string("4444444444444444").unwrap();
    let cancelled_span_id = SpanId::from_string("5555555555555555").unwrap();
    let started_at = Timestamp::from(1_700_000_000_123);
    let finished_at = Timestamp::from(1_700_000_001_456);
    let cancelled_at = Timestamp::from(1_700_000_002_789);
    let attributes = AttributeMap(HashMap::from([(
        "phase".to_string(),
        AttributeValue::String("projection".to_string()),
    )]));
    let started = SpanStarted {
        span_id: started_span_id.clone(),
        trace_id: origin_trace_id.clone(),
        trace_states: vec!["origin=1".to_string()],
        parent_span_id: None,
        links: vec![SpanLink {
            trace_id: linked_trace_id.clone(),
            span_id: linked_span_id.clone(),
            trace_states: vec!["link=1".to_string()],
        }],
        started_at,
        attributes: attributes.clone(),
        kind: SpanKind::Client,
    };
    let finished = SpanFinished {
        span_id: finished_span_id.clone(),
        finished_at,
        outcome: SpanOutcome::Failed,
    };
    let terminal_attributes = SpanAttributes {
        span_id: attributes_span_id.clone(),
        attributes,
    };
    let cancelled = SpanFinished {
        span_id: cancelled_span_id.clone(),
        finished_at: cancelled_at,
        outcome: SpanOutcome::Cancelled,
    };

    let raw_entries = [
        OplogEntry::Start {
            timestamp: started_at,
            parent_start_index: None,
            function_name: HostFunctionName::MonotonicClockNow,
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::ReadLocal,
            span_started: Some(Box::new(started.clone())),
            denial: None,
        },
        OplogEntry::End {
            timestamp: finished_at,
            start_index: golem_common::base_model::OplogIndex::INITIAL,
            response: None,
            forced_commit: false,
            span_finished: Some(finished.clone()),
            span_attributes: Some(terminal_attributes.clone()),
        },
        OplogEntry::Cancelled {
            timestamp: cancelled_at,
            start_index: golem_common::base_model::OplogIndex::INITIAL,
            partial: None,
            span_finished: Some(cancelled.clone()),
        },
    ];
    for entry in raw_entries {
        let encoded = oplog::OplogEntry::try_from(entry.clone()).unwrap();
        assert_eq!(OplogEntry::try_from(encoded).unwrap(), entry);
    }

    let public_started = super::super::public_span_started(started);
    assert_eq!(public_started.trace_id, origin_trace_id);
    assert_eq!(public_started.links[0].trace_id, linked_trace_id);
    assert_ne!(public_started.trace_id, public_started.links[0].trace_id);
    assert_eq!(public_started.started_at, started_at);
    let public_finished = super::super::public_span_finished(finished);
    let public_attributes = super::super::public_span_attributes(terminal_attributes);
    let public_cancelled = super::super::public_span_finished(cancelled);
    assert_ne!(public_finished.span_id, public_attributes.span_id);
    assert_ne!(public_finished.span_id, public_cancelled.span_id);

    let public_entries = [
        PublicOplogEntry::Start(StartParams {
            timestamp: started_at,
            parent_start_index: None,
            function_name: HostFunctionName::MonotonicClockNow.to_string(),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::ReadLocal.into(),
            span_started: Some(public_started),
            denial: None,
        }),
        PublicOplogEntry::End(EndParams {
            timestamp: finished_at,
            start_index: golem_common::base_model::OplogIndex::INITIAL,
            response: None,
            forced_commit: false,
            span_finished: Some(public_finished),
            span_attributes: Some(public_attributes),
        }),
        PublicOplogEntry::Cancelled(CancelledParams {
            timestamp: cancelled_at,
            start_index: golem_common::base_model::OplogIndex::INITIAL,
            partial: None,
            span_finished: Some(public_cancelled),
        }),
    ];
    let encoded: Vec<_> = public_entries
        .into_iter()
        .map(|entry| oplog::PublicOplogEntry::try_from(entry).unwrap())
        .collect();
    let oplog::PublicOplogEntry::Start(start) = &encoded[0] else {
        panic!("expected start")
    };
    let projected_start = start.span_started.as_ref().unwrap();
    assert_eq!(projected_start.trace_id, origin_trace_id.to_string());
    assert_eq!(
        projected_start.links[0].trace_id,
        linked_trace_id.to_string()
    );
    let expected_started_at: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant =
        started_at.into();
    assert_eq!(
        projected_start.started_at.seconds,
        expected_started_at.seconds
    );
    assert_eq!(
        projected_start.started_at.nanoseconds,
        expected_started_at.nanoseconds
    );
    let oplog::PublicOplogEntry::End(end) = &encoded[1] else {
        panic!("expected end")
    };
    assert_eq!(
        end.span_finished.as_ref().unwrap().span_id,
        finished_span_id.to_string()
    );
    let expected_finished_at: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant =
        finished_at.into();
    assert_eq!(
        end.span_finished.as_ref().unwrap().finished_at.seconds,
        expected_finished_at.seconds
    );
    assert_eq!(
        end.span_finished.as_ref().unwrap().finished_at.nanoseconds,
        expected_finished_at.nanoseconds
    );
    assert_eq!(
        end.span_attributes.as_ref().unwrap().span_id,
        attributes_span_id.to_string()
    );
    let oplog::PublicOplogEntry::Cancelled(cancelled) = &encoded[2] else {
        panic!("expected cancelled")
    };
    assert_eq!(
        cancelled.span_finished.as_ref().unwrap().span_id,
        cancelled_span_id.to_string()
    );
    let expected_cancelled_at: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant =
        cancelled_at.into();
    assert_eq!(
        cancelled
            .span_finished
            .as_ref()
            .unwrap()
            .finished_at
            .seconds,
        expected_cancelled_at.seconds
    );
    assert_eq!(
        cancelled
            .span_finished
            .as_ref()
            .unwrap()
            .finished_at
            .nanoseconds,
        expected_cancelled_at.nanoseconds
    );
}

#[test]
fn raw_snapshot_wit_roundtrip_preserves_wallet_state() {
    let card = test_card(CardId::new());
    let entry = OplogEntry::Snapshot {
        timestamp: Timestamp::now_utc().rounded(),
        data: OplogPayload::Inline(Box::new(vec![1, 2, 3])),
        mime_type: "application/octet-stream".to_string(),
        active_cards: vec![card.clone().into()],
        wallet_generation: 73,
        filesystem_snapshot: None,
    };

    let encoded = oplog::OplogEntry::try_from(entry).unwrap();
    let decoded = OplogEntry::try_from(encoded).unwrap();
    let OplogEntry::Snapshot {
        active_cards,
        wallet_generation,
        ..
    } = decoded
    else {
        panic!("expected snapshot")
    };
    assert_eq!(active_cards, vec![card.into()]);
    assert_eq!(wallet_generation, 73);
}

#[test]
fn wit_projection_preserves_wallet_generation_outcomes() {
    let timestamp = Timestamp::now_utc().rounded();
    let card = test_card(CardId::new());
    let card_id = card.card_id;
    let entries = [
        OplogEntry::CardInstalled {
            timestamp,
            entity_parent_start_index: None,
            queued_event_index: None,
            card: Box::new(card.clone().into()),
            wallet_generation: 1,
        },
        OplogEntry::CardRevoked {
            timestamp,
            entity_parent_start_index: None,
            queued_event_index: golem_common::model::OplogIndex::NONE,
            card_id,
            wallet_generation: 3,
        },
        OplogEntry::CardExpired {
            timestamp,
            entity_parent_start_index: None,
            card_id,
            wallet_generation: 4,
        },
    ];

    for entry in entries {
        let encoded = oplog::OplogEntry::try_from(entry.clone()).unwrap();
        assert_eq!(OplogEntry::try_from(encoded).unwrap(), entry);
    }

    assert!(
        oplog::OplogEntry::try_from(OplogEntry::CardDerived {
            timestamp,
            entity_parent_start_index: None,
            card: Box::new(card.into()),
            wallet_generation: 2,
            transfer_id: None,
        })
        .is_err()
    );
}

#[test]
fn wit_projection_preserves_invocation_wallet_pins() {
    let wallet_token = WalletVersionToken {
        wallet_id_hash: [0x42; 32],
        generation: 73,
    };
    let scope_card_id = CardId::new();
    let pinned_card_ids = vec![CardId::new(), CardId::new()];
    let raw_entry = OplogEntry::AgentInvocationStarted {
        timestamp: Timestamp::now_utc().rounded(),
        idempotency_key: IdempotencyKey::new("wallet-pin".to_string()),
        payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
            method_name: "test".to_string(),
            input: SchemaValue::Record { fields: Vec::new() },
            principal: Principal::anonymous(),
            scope_card: None,
        })),
        trace_id: TraceId::generate(),
        trace_states: Vec::new(),
        invocation_context: Vec::new(),
        wallet_pin: Box::new(InvocationWalletPin {
            wallet_token: wallet_token.clone(),
            pinned_card_ids: pinned_card_ids.clone(),
            scope_card_id: Some(scope_card_id),
        }),
    };

    let encoded = oplog::OplogEntry::try_from(raw_entry).unwrap();
    match OplogEntry::try_from(encoded).unwrap() {
        OplogEntry::AgentInvocationStarted { wallet_pin, .. } => {
            assert_eq!(
                wallet_pin,
                Box::new(InvocationWalletPin {
                    wallet_token: wallet_token.clone(),
                    pinned_card_ids,
                    scope_card_id: Some(scope_card_id),
                })
            );
        }
        other => panic!("expected raw invocation-started entry, got {other:?}"),
    }

    let public_entry = PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
        timestamp: Timestamp::now_utc().rounded(),
        invocation: PublicAgentInvocation::SaveSnapshot(Empty {}),
        wallet_pin: PublicInvocationWalletPin {
            wallet_token: wallet_token.clone(),
            scope_card_id: Some(scope_card_id),
        },
    });
    match oplog::PublicOplogEntry::try_from(public_entry).unwrap() {
        oplog::PublicOplogEntry::AgentInvocationStarted(params) => {
            assert_eq!(
                params.wallet_pin.wallet_token.wallet_id_hash,
                wallet_token.wallet_id_hash
            );
            assert_eq!(
                params.wallet_pin.wallet_token.generation,
                wallet_token.generation
            );
            assert_eq!(
                params.wallet_pin.scope_card_id.map(super::card_id_from_wit),
                Some(scope_card_id)
            );
        }
        other => panic!("expected public invocation-started entry, got {other:?}"),
    }
}

#[test]
fn raw_card_transferred_encoders_reject_mismatched_card_id() {
    let payload_card_id = CardId::new();
    let entry = OplogEntry::CardTransferred {
        timestamp: Timestamp::now_utc().rounded(),
        entity_parent_start_index: None,
        transfer_id: Uuid::new_v4(),
        source_card_id: payload_card_id,
        installed_card_id: CardId::new(),
        target_holder: CardHolder::Account(AccountCardHolder {
            account_id: Uuid::new_v4(),
        }),
        card: Box::new(test_card(payload_card_id).into()),
        target_wallet_generation: 0,
    };

    let protobuf_result: Result<golem_api_grpc::proto::golem::worker::RawOplogEntry, String> =
        entry.clone().try_into();
    assert!(protobuf_result.is_err());

    let wit_result = oplog::OplogEntry::try_from(entry);
    assert!(wit_result.is_err());
}

#[test]
fn unsupported_transfer_holders_roundtrip_through_protobuf_but_not_legacy_wit() {
    let unsupported_targets = [
        CardHolder::Account(AccountCardHolder {
            account_id: Uuid::new_v4(),
        }),
        CardHolder::Application(ApplicationCardHolder {
            application_id: Uuid::new_v4(),
        }),
    ];

    for target_holder in unsupported_targets {
        let payload_card_id = CardId::new();
        let entry = OplogEntry::CardEventQueued {
            timestamp: Timestamp::now_utc().rounded(),
            entity_parent_start_index: None,
            event: Box::new(QueuedCardEvent::TransferStarted(QueuedCardEventTransfer {
                transfer_id: Uuid::new_v4(),
                card_id: CardId::new(),
                card: Some(test_card(payload_card_id).into()),
                target_holder,
            })),
        };

        let protobuf_result: Result<golem_api_grpc::proto::golem::worker::RawOplogEntry, String> =
            entry.clone().try_into();
        let protobuf = protobuf_result.expect("unsupported holder transfer intent must encode");
        assert_eq!(OplogEntry::try_from(protobuf).unwrap(), entry);

        assert!(oplog::OplogEntry::try_from(entry).is_err());
    }
}

#[test]
fn legacy_wit_rejects_received_card_transfers_without_fabricating_an_event() {
    let card_id = CardId::new();
    let source_card_id = CardId::new();
    let entry = OplogEntry::CardEventQueued {
        timestamp: Timestamp::now_utc().rounded(),
        entity_parent_start_index: None,
        event: Box::new(QueuedCardEvent::TransferReceived(
            QueuedCardEventTransferReceived {
                transfer_id: Uuid::new_v4(),
                source_card_id,
                card_id,
                card: Some(test_card(card_id).into()),
            },
        )),
    };

    assert!(oplog::OplogEntry::try_from(entry).is_err());
}

#[test]
fn log_conversions_keep_asymmetric_fields_and_omit_the_raw_parent() {
    use golem_common::model::OplogIndex;
    use golem_common::model::oplog::public_oplog_entry::LogParams;
    use golem_common::model::oplog::{LogLevel, LogTraceContext};

    let timestamp = Timestamp::from(1_700_000_123_456);
    let trace_context = LogTraceContext {
        trace_id: TraceId::from_string("11112222333344445555666677778888").unwrap(),
        span_id: SpanId::from_string("9999aaaabbbbcccc").unwrap(),
    };
    let raw = OplogEntry::Log {
        timestamp,
        parent_start_index: Some(OplogIndex::from_u64(37)),
        level: LogLevel::Warn,
        context: "raw context".to_string(),
        message: "raw message".to_string(),
        trace_context: Some(trace_context.clone()),
    };
    let oplog::OplogEntry::Log(raw) = oplog::OplogEntry::try_from(raw).unwrap() else {
        panic!("expected raw log")
    };
    assert_eq!(raw.timestamp.seconds, 1_700_000_123);
    assert_eq!(raw.timestamp.nanoseconds, 456_000_000);
    assert!(matches!(raw.level, oplog::LogLevel::Warn));
    assert_eq!(raw.context, "raw context");
    assert_eq!(raw.message, "raw message");
    assert_eq!(
        raw.trace_context.as_ref().unwrap().trace_id,
        trace_context.trace_id.to_string()
    );
    assert_eq!(
        raw.trace_context.as_ref().unwrap().span_id,
        trace_context.span_id.to_string()
    );

    let public = PublicOplogEntry::Log(LogParams {
        timestamp,
        level: LogLevel::Error,
        context: "public context".to_string(),
        message: "public message".to_string(),
        trace_context: Some(trace_context),
    });
    let oplog::PublicOplogEntry::Log(public) = oplog::PublicOplogEntry::try_from(public).unwrap()
    else {
        panic!("expected public log")
    };
    assert_eq!(public.timestamp.seconds, 1_700_000_123);
    assert_eq!(public.timestamp.nanoseconds, 456_000_000);
    assert!(matches!(public.level, oplog::LogLevel::Error));
    assert_eq!(public.context, "public context");
    assert_eq!(public.message, "public message");

    let decoded = OplogEntry::try_from(oplog::OplogEntry::Log(oplog::LogParameters {
        timestamp: Timestamp::from(1_800_000_321_654).into(),
        level: oplog::LogLevel::Debug,
        context: "decoded context".to_string(),
        message: "decoded message".to_string(),
        trace_context: Some(oplog::LogTraceContext {
            trace_id: "22223333444455556666777788889999".to_string(),
            span_id: "aaaabbbbccccdddd".to_string(),
        }),
    }))
    .unwrap();
    assert_eq!(
        decoded,
        OplogEntry::Log {
            timestamp: Timestamp::from(1_800_000_321_654),
            parent_start_index: None,
            level: LogLevel::Debug,
            context: "decoded context".to_string(),
            message: "decoded message".to_string(),
            trace_context: Some(LogTraceContext {
                trace_id: TraceId::from_string("22223333444455556666777788889999").unwrap(),
                span_id: SpanId::from_string("aaaabbbbccccdddd").unwrap(),
            }),
        }
    );
}

#[test]
fn timestamp_payload_conversions_and_trace_validation_keep_their_contract() {
    use golem_common::model::oplog::public_oplog_entry::SuspendParams;

    let timestamp = Timestamp::from(1_700_000_123_456);
    let oplog::OplogEntry::Suspend(raw) =
        oplog::OplogEntry::try_from(OplogEntry::Suspend { timestamp }).unwrap()
    else {
        panic!("expected raw suspend")
    };
    assert_eq!(raw.timestamp.seconds, 1_700_000_123);
    assert_eq!(raw.timestamp.nanoseconds, 456_000_000);
    let oplog::PublicOplogEntry::Suspend(public) =
        oplog::PublicOplogEntry::try_from(PublicOplogEntry::Suspend(SuspendParams { timestamp }))
            .unwrap()
    else {
        panic!("expected public suspend")
    };
    assert_eq!(public.timestamp.seconds, 1_700_000_123);
    assert_eq!(public.timestamp.nanoseconds, 456_000_000);
    assert_eq!(
        OplogEntry::try_from(oplog::OplogEntry::Suspend(oplog::Timestamp {
            timestamp: Timestamp::from(1_800_000_321_654).into(),
        }))
        .unwrap(),
        OplogEntry::Suspend {
            timestamp: Timestamp::from(1_800_000_321_654)
        }
    );

    let error = OplogEntry::try_from(oplog::OplogEntry::Log(oplog::LogParameters {
        timestamp: timestamp.into(),
        level: oplog::LogLevel::Info,
        context: String::new(),
        message: String::new(),
        trace_context: Some(oplog::LogTraceContext {
            trace_id: "invalid-trace".to_string(),
            span_id: "invalid-span".to_string(),
        }),
    }))
    .unwrap_err();
    assert_eq!(error, TraceId::from_string("invalid-trace").unwrap_err());
}

#[test]
fn raw_create_conversions_flatten_parameters_without_swapping_fields() {
    use golem_common::model::AgentId;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::{AgentMode, OwnerKind};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::CreateParameters;

    let agent_id = AgentId {
        component_id: ComponentId(Uuid::from_u128(0x11112222333344445555666677778888)),
        agent_id: "created-agent".to_string(),
    };
    let parent = AgentId {
        component_id: ComponentId(Uuid::from_u128(0x9999aaaabbbbccccddddeeeeffff1234)),
        agent_id: "parent-agent".to_string(),
    };
    let environment_id = EnvironmentId(Uuid::from_u128(0x123456789abcdef0fedcba9876543210));
    let created_by = AccountId(Uuid::from_u128(0x23456789abcdef01edcba9876543210f));
    let instance_id = Uuid::from_u128(0x3456789abcdef012dcba9876543210fe);
    let phantom_id = Uuid::from_u128(0x456789abcdef0123cba9876543210fed);
    let env = vec![
        ("second".to_string(), "2".to_string()),
        ("first".to_string(), "1".to_string()),
    ];
    let entry = OplogEntry::Create {
        timestamp: Timestamp::from(1_700_000_123_456),
        parameters: Box::new(CreateParameters {
            agent_id: agent_id.clone(),
            owner_kind: OwnerKind::EphemeralExternalTool,
            agent_mode: AgentMode::Ephemeral,
            component_revision: ComponentRevision::new(23).unwrap(),
            env: env.clone(),
            environment_id,
            created_by,
            parent: Some(parent.clone()),
            component_size: 317,
            initial_total_linear_memory_size: 941,
            initial_active_plugins: Default::default(),
            local_agent_config: Vec::new(),
            original_phantom_id: Some(phantom_id),
            instance_id,
        }),
    };
    let oplog::OplogEntry::Create(mut encoded) = oplog::OplogEntry::try_from(entry).unwrap() else {
        panic!("expected create")
    };
    assert_eq!(encoded.timestamp.seconds, 1_700_000_123);
    assert_eq!(encoded.timestamp.nanoseconds, 456_000_000);
    assert_eq!(
        golem_common::model::AgentId::from(encoded.agent_id.clone()),
        agent_id
    );
    assert_eq!(
        golem_common::model::AgentId::from(encoded.parent.clone().unwrap()),
        parent
    );
    assert!(matches!(
        encoded.owner_kind,
        oplog::OwnerKind::EphemeralExternalTool
    ));
    assert!(matches!(encoded.agent_mode, oplog::AgentMode::Ephemeral));
    assert_eq!(encoded.component_revision, 23);
    assert_eq!(encoded.env, env);
    assert_eq!(
        Uuid::from_u64_pair(
            encoded.environment_id.uuid.high_bits,
            encoded.environment_id.uuid.low_bits
        ),
        environment_id.0
    );
    assert_eq!(
        Uuid::from_u64_pair(
            encoded.created_by.uuid.high_bits,
            encoded.created_by.uuid.low_bits
        ),
        created_by.0
    );
    assert_eq!(encoded.component_size, 317);
    assert_eq!(encoded.initial_total_linear_memory_size, 941);
    assert_eq!(
        Uuid::from_u64_pair(encoded.instance_id.high_bits, encoded.instance_id.low_bits),
        instance_id
    );
    let encoded_phantom = encoded.original_phantom_id.as_ref().unwrap();
    assert_eq!(
        Uuid::from_u64_pair(encoded_phantom.high_bits, encoded_phantom.low_bits),
        phantom_id
    );

    encoded.component_revision = 29;
    encoded.component_size = 419;
    encoded.initial_total_linear_memory_size = 1031;
    let OplogEntry::Create {
        timestamp,
        parameters,
    } = OplogEntry::try_from(oplog::OplogEntry::Create(encoded)).unwrap()
    else {
        panic!("expected decoded create")
    };
    assert_eq!(timestamp, Timestamp::from(1_700_000_123_456));
    assert_eq!(parameters.agent_id, agent_id);
    assert_eq!(parameters.parent, Some(parent));
    assert_eq!(
        parameters.component_revision,
        ComponentRevision::new(29).unwrap()
    );
    assert_eq!(parameters.component_size, 419);
    assert_eq!(parameters.initial_total_linear_memory_size, 1031);
    assert_eq!(parameters.env, env);
    assert_eq!(parameters.environment_id, environment_id);
    assert_eq!(parameters.created_by, created_by);
    assert_eq!(parameters.original_phantom_id, Some(phantom_id));
    assert_eq!(parameters.instance_id, instance_id);
}

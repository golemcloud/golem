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

use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::model::agent::Principal;
use crate::model::card::{
    AccountCardHolder, AgentCardHolder, ApplicationCardHolder, Card, CardHolder, CardId,
    InvocationWalletPin, PublicInvocationWalletPin, WalletVersionToken,
};
use crate::model::component::PluginPriority;
use crate::model::invocation_context::{SpanId, TraceId};
use crate::model::lucene::Query;
use crate::model::oplog::host_functions::HostFunctionName;
use crate::model::oplog::payload::types::{SecretRevealAudit, SerializableDateTime};
use crate::model::oplog::payload::{HostRequestSecretReveal, HostResponseSecretRevealed};
use crate::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginAtomicRegionParams, BeginRemoteTransactionParams, CancelPendingInvocationParams,
    CancelledParams, CardDerivedParams, CardExpiredParams, CardInstalledParams,
    CardRevokedCascadeParams, CardRevokedParams, CardTransferConfirmedParams,
    CardTransferStartedParams, CardTransferredParams, CommittedRemoteTransactionParams,
    CreateParams, CreateResourceParams, DeactivatePluginParams, DropResourceParams,
    EndAtomicRegionParams, EndParams, ErrorParams, ExitedParams, FailedUpdateParams,
    GrowMemoryParams, InterruptedParams, JumpParams, LogParams, NoOpParams,
    PendingAgentInvocationParams, PendingUpdateParams, PreCommitRemoteTransactionParams,
    PreRollbackRemoteTransactionParams, RemoveRetryPolicyParams, RestartParams, ResumedParams,
    RevertParams, RolledBackRemoteTransactionParams, SetRetryPolicyParams, SnapshotParams,
    StartParams, SuccessfulUpdateParams, SuspendParams,
};
use crate::model::oplog::{
    AgentInitializationParameters, AgentInvocationOutputParameters,
    AgentMethodInvocationParameters, AgentResourceId, DurableFunctionType, JsonSnapshotData,
    LogLevel, MultipartPartData, MultipartSnapshotData, MultipartSnapshotPart, OplogEntry,
    OplogErrorKind, OplogPayload, PluginInstallationDescription, PublicAgentEntity,
    PublicAgentEntityKind, PublicAgentInvocation, PublicAgentInvocationResult, PublicAttribute,
    PublicAttributeValue, PublicDurableFunctionType, PublicEntityCallMode, PublicEntityInvocation,
    PublicEntityInvocationContext, PublicEntityInvocationOperation, PublicLocalSpanData,
    PublicOplogEntry, PublicOplogEntryAttribution, PublicOplogEntryWithIndex,
    PublicQueuedCardEvent, PublicSnapshotData, PublicSpanAttributes, PublicSpanData,
    PublicSpanFinished, PublicSpanKind, PublicSpanLink, PublicSpanOutcome, PublicSpanStarted,
    PublicToolInvocationOperation, PublicTypedAgentConfigEntry, PublicUpdateDescription,
    QueuedCardEvent, RawSnapshotData, SnapshotBasedUpdateParameters, StringAttributeValue,
};
use crate::model::regions::OplogRegion;
use crate::model::{
    AccountId, AgentId, AgentInvocationPayload, ComponentId, Empty, IdempotencyKey, OplogIndex,
    Timestamp, TransactionId,
};
use crate::schema::IntoTypedSchemaValue;
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};
use crate::schema::schema_value::{ResultValuePayload, SchemaValue, VariantValuePayload};
use poem_openapi::types::ToJSON;
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use test_r::test;
use uuid::Uuid;

#[cfg(target_pointer_width = "64")]
#[test]
fn raw_oplog_entry_fits_in_176_bytes() {
    assert!(std::mem::size_of::<OplogEntry>() <= 176);
}

#[test]
fn start_desert_roundtrip() {
    use crate::model::invocation_context::AttributeValue;
    use crate::model::oplog::raw_types::{AttributeMap, SpanKind, SpanStarted};

    let entry = OplogEntry::Start {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: Some(OplogIndex::from_u64(7)),
        function_name: HostFunctionName::Custom("operation".to_string()),
        invocation_id: Some(Uuid::new_v4()),
        observational_owner: Some(OplogIndex::from_u64(6)),
        request: None,
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: Some(Box::new(SpanStarted {
            span_id: SpanId::generate(),
            trace_id: TraceId::generate(),
            trace_states: vec!["origin=one".into()],
            parent_span_id: Some(SpanId::generate()),
            links: vec![],
            started_at: Timestamp::now_utc().rounded(),
            attributes: AttributeMap(HashMap::from([(
                "name".into(),
                AttributeValue::String("operation".into()),
            )])),
            kind: SpanKind::Client,
        })),
    };

    let bytes = crate::serialization::serialize(&entry).unwrap();
    let decoded: OplogEntry = crate::serialization::deserialize(&bytes).unwrap();
    assert_eq!(decoded, entry);
}

#[test]
fn end_and_cancelled_desert_roundtrip_with_spans() {
    use crate::model::invocation_context::AttributeValue;
    use crate::model::oplog::raw_types::{AttributeMap, SpanAttributes, SpanFinished, SpanOutcome};

    let span_id = SpanId::generate();
    let entries = [
        OplogEntry::End {
            timestamp: Timestamp::now_utc().rounded(),
            start_index: OplogIndex::from_u64(7),
            response: None,
            forced_commit: false,
            span_finished: Some(SpanFinished {
                span_id: span_id.clone(),
                finished_at: Timestamp::now_utc().rounded(),
                outcome: SpanOutcome::Completed,
            }),
            span_attributes: Some(SpanAttributes {
                span_id: span_id.clone(),
                attributes: AttributeMap(HashMap::from([(
                    "result".into(),
                    AttributeValue::String("ok".into()),
                )])),
            }),
        },
        OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc().rounded(),
            start_index: OplogIndex::from_u64(8),
            partial: None,
            span_finished: Some(SpanFinished {
                span_id,
                finished_at: Timestamp::now_utc().rounded(),
                outcome: SpanOutcome::Cancelled,
            }),
        },
    ];

    for entry in entries {
        let bytes = crate::serialization::serialize(&entry).unwrap();
        let decoded: OplogEntry = crate::serialization::deserialize(&bytes).unwrap();
        assert_eq!(decoded, entry);
    }
}

#[test]
fn log_trace_context_binary_and_raw_protobuf_roundtrip() {
    use crate::model::oplog::LogTraceContext;

    let entry = OplogEntry::Log {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: Some(OplogIndex::from_u64(7)),
        level: LogLevel::Info,
        context: "request".to_string(),
        message: "handled".to_string(),
        trace_context: Some(LogTraceContext {
            trace_id: TraceId::from_string("00112233445566778899aabbccddeeff").unwrap(),
            span_id: SpanId::from_string("0123456789abcdef").unwrap(),
        }),
    };

    let bytes = crate::serialization::serialize(&entry).unwrap();
    let decoded: OplogEntry = crate::serialization::deserialize(&bytes).unwrap();
    assert_eq!(decoded, entry);

    let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
        entry.clone().try_into().unwrap();
    assert_eq!(OplogEntry::try_from(proto).unwrap(), entry);
}

#[test]
fn durable_stream_summaries_binary_and_raw_protobuf_roundtrip_without_cached_payloads() {
    use crate::model::durable_stream::{
        LocalStreamId, StreamEndRecord, StreamEndResult, StreamItemsPayload, StreamItemsRecord,
        StreamOffset, StreamRegistrationInvocation, StreamSessionFinishedRecord,
        StreamSessionInvocationResultRecord, StreamSessionRecord, StreamTerminalAuthor,
    };
    use crate::model::oplog::PayloadId;
    use crate::model::oplog::raw_types::{DurableStreamEventSummary, DurableStreamOutcome};

    let stream_id = LocalStreamId(OplogIndex::from_u64(4));
    let items = StreamItemsRecord {
        format_version: 1,
        stream_id,
        first_sequence: 0,
        nested_stream_ids: vec![],
        newly_registered_stream_ids: vec![],
        payload: StreamItemsPayload::Values(vec![vec![1], vec![2], vec![3]]),
        offsets: vec![],
    };
    let successful_end = StreamEndRecord {
        format_version: 1,
        stream_id,
        sequence: 3,
        offset: StreamOffset::new(OplogIndex::from_u64(5), 0),
        authored_by: StreamTerminalAuthor::Guest,
        result: StreamEndResult::Ok,
    };
    let failed_end = StreamEndRecord {
        result: StreamEndResult::ErrorContext(vec![9]),
        ..successful_end.clone()
    };
    let session_key = StreamRegistrationInvocation::Local(IdempotencyKey::new("stream".into()));
    let successful_session = StreamSessionRecord::Finished(StreamSessionFinishedRecord {
        format_version: 1,
        session_key: session_key.clone(),
        result: Ok(()),
    });
    let failed_session = StreamSessionRecord::Finished(StreamSessionFinishedRecord {
        format_version: 1,
        session_key: session_key.clone(),
        result: Err(vec![8]),
    });
    let session_result =
        StreamSessionRecord::InvocationResult(StreamSessionInvocationResultRecord {
            format_version: 1,
            session_key: session_key.clone(),
            result: vec![7],
            stream_mappings: vec![],
        });
    let session_cancellation = StreamSessionRecord::CancelRequested(
        crate::model::durable_stream::StreamSessionCancelRequestedRecord {
            format_version: 1,
            session_key: session_key.clone(),
        },
    );
    let session_expired =
        StreamSessionRecord::Expired(crate::model::durable_stream::StreamSessionExpiredRecord {
            format_version: 1,
            public_session_id: "public-stream".to_string(),
            session_key: session_key.idempotency_key().clone(),
            expected_deadline_millis: 100,
            expired_at_millis: 100,
        });
    let irrelevant_session = StreamSessionRecord::ExpiryRefreshed(
        crate::model::durable_stream::StreamSessionExpiryRefreshedRecord {
            format_version: 1,
            public_session_id: "public-stream".to_string(),
            session_key: session_key.idempotency_key().clone(),
            expected_deadline_millis: 100,
            refreshed_at_millis: 50,
            deadline_millis: 200,
        },
    );
    let summaries = [
        Some(DurableStreamEventSummary::Registered),
        Some(DurableStreamEventSummary::items(&items)),
        Some(DurableStreamEventSummary::end(&successful_end)),
        Some(DurableStreamEventSummary::end(&failed_end)),
        Some(DurableStreamEventSummary::Cancelled),
        DurableStreamEventSummary::session(&successful_session),
        DurableStreamEventSummary::session(&failed_session),
        DurableStreamEventSummary::session(&session_result),
        DurableStreamEventSummary::session(&session_cancellation),
        DurableStreamEventSummary::session(&session_expired),
        DurableStreamEventSummary::session(&irrelevant_session),
    ];
    assert_eq!(
        summaries[1],
        Some(DurableStreamEventSummary::Items { item_count: 3 })
    );
    assert_eq!(
        summaries[2],
        Some(DurableStreamEventSummary::End {
            outcome: DurableStreamOutcome::Success
        })
    );
    assert_eq!(
        summaries[3],
        Some(DurableStreamEventSummary::End {
            outcome: DurableStreamOutcome::Error
        })
    );
    assert_eq!(
        summaries[5],
        Some(DurableStreamEventSummary::SessionFinished {
            outcome: DurableStreamOutcome::Success
        })
    );
    assert_eq!(
        summaries[6],
        Some(DurableStreamEventSummary::SessionFinished {
            outcome: DurableStreamOutcome::Error
        })
    );
    assert_eq!(summaries[7], Some(DurableStreamEventSummary::SessionResult));
    assert_eq!(
        summaries[8],
        Some(DurableStreamEventSummary::SessionCancellation)
    );
    assert_eq!(
        summaries[9],
        Some(DurableStreamEventSummary::SessionExpired)
    );
    assert_eq!(summaries[10], None);

    for (index, summary) in summaries.into_iter().enumerate() {
        let records = [
            OplogPayload::External {
                payload_id: PayloadId::new(),
                md5_hash: vec![index as u8; 16],
                cached: None,
            },
            OplogPayload::SerializedInline {
                bytes: vec![index as u8, 0xff],
                cached: None,
            },
        ];
        for record in records {
            let entry = OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc().rounded(),
                entity_parent_start_index: None,
                summary: summary.clone(),
                record,
            };
            let bytes = crate::serialization::serialize(&entry).unwrap();
            let decoded: OplogEntry = crate::serialization::deserialize(&bytes).unwrap();
            assert_eq!(decoded, entry);
            let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
                entry.clone().try_into().unwrap();
            assert_eq!(OplogEntry::try_from(proto).unwrap(), entry);
        }
    }
}

#[test]
fn raw_durable_stream_summary_protobuf_rejects_invalid_fields() {
    use crate::model::oplog::raw_types::DurableStreamEventSummary;
    use golem_api_grpc::proto::golem::worker::RawDurableStreamEventSummary;
    use golem_api_grpc::proto::golem::worker::raw_durable_stream_event_summary::{Kind, Outcome};

    let invalid = [
        RawDurableStreamEventSummary {
            kind: 99,
            item_count: None,
            outcome: None,
        },
        RawDurableStreamEventSummary {
            kind: Kind::End as i32,
            item_count: None,
            outcome: Some(99),
        },
        RawDurableStreamEventSummary {
            kind: Kind::Items as i32,
            item_count: None,
            outcome: None,
        },
        RawDurableStreamEventSummary {
            kind: Kind::End as i32,
            item_count: None,
            outcome: None,
        },
        RawDurableStreamEventSummary {
            kind: Kind::Registered as i32,
            item_count: Some(3),
            outcome: None,
        },
        RawDurableStreamEventSummary {
            kind: Kind::Cancelled as i32,
            item_count: None,
            outcome: Some(Outcome::Success as i32),
        },
    ];
    for summary in invalid {
        assert!(DurableStreamEventSummary::try_from(summary).is_err());
    }
}

#[test]
fn observational_start_public_protobuf_roundtrip() {
    let owner = OplogIndex::from_u64(12);
    let entry = PublicOplogEntry::Start(StartParams {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: Some(OplogIndex::from_u64(13)),
        function_name: "wasi:http/outgoing-handler::handle".to_string(),
        invocation_id: None,
        observational_owner: Some(owner),
        request: None,
        durable_function_type: PublicDurableFunctionType::WriteRemote(Empty {}),
        span_started: None,
    });

    let proto: golem_api_grpc::proto::golem::worker::OplogEntry = entry.clone().try_into().unwrap();
    assert_eq!(PublicOplogEntry::try_from(proto).unwrap(), entry);
}

#[test]
fn entity_attribution_public_protobuf_and_json_roundtrip() {
    let ancestor = PublicEntityInvocation {
        entity: PublicAgentEntity {
            kind: PublicAgentEntityKind::ToolMiddleware,
            name: "audit".to_string(),
        },
        start_index: OplogIndex::from_u64(7),
        call_mode: PublicEntityCallMode::Synchronous,
        operation: None,
    };
    let invocation = PublicEntityInvocation {
        entity: PublicAgentEntity {
            kind: PublicAgentEntityKind::Tool,
            name: "lookup".to_string(),
        },
        start_index: OplogIndex::from_u64(11),
        call_mode: PublicEntityCallMode::Asynchronous,
        operation: Some(PublicEntityInvocationOperation::Tool(
            PublicToolInvocationOperation {
                command_path: vec!["bin".to_string(), "lookup".to_string()],
                has_stdin: false,
                has_stdout: true,
                declares_stdout: true,
            },
        )),
    };
    let entry = PublicOplogEntryWithIndex {
        oplog_index: OplogIndex::from_u64(13),
        attribution: PublicOplogEntryAttribution::entity(PublicEntityInvocationContext {
            invocation,
            ancestors: vec![ancestor],
        }),
        entry: PublicOplogEntry::NoOp(NoOpParams {
            timestamp: Timestamp::now_utc().rounded(),
        }),
    };

    let proto: golem_api_grpc::proto::golem::worker::OplogEntryWithIndex =
        entry.clone().try_into().unwrap();
    let decoded: PublicOplogEntryWithIndex = proto.try_into().unwrap();
    assert_eq!(decoded, entry);

    let json = serde_json::to_value(&entry).unwrap();
    assert_eq!(json["attribution"]["type"], "Entity");
    assert_eq!(
        json["attribution"]["ancestors"][0]["entity"]["kind"],
        "toolMiddleware"
    );
    assert_eq!(
        json["attribution"]["invocation"]["operation"]["type"],
        "Tool"
    );
}

#[test]
fn entity_attribution_public_protobuf_rejects_invalid_start_index() {
    let invocation = PublicEntityInvocation {
        entity: PublicAgentEntity {
            kind: PublicAgentEntityKind::Tool,
            name: "lookup".to_string(),
        },
        start_index: OplogIndex::from_u64(11),
        call_mode: PublicEntityCallMode::Synchronous,
        operation: None,
    };
    let mut proto: golem_api_grpc::proto::golem::worker::PublicEntityInvocation = invocation.into();

    proto.start_index = None;
    assert_eq!(
        PublicEntityInvocation::try_from(proto.clone()).unwrap_err(),
        "Missing entity start index"
    );

    proto.start_index = Some(0);
    assert_eq!(
        PublicEntityInvocation::try_from(proto).unwrap_err(),
        "Invalid entity start index: 0"
    );
}

#[test]
fn entity_attribution_raw_protobuf_roundtrip() {
    let parent_start_index = Some(OplogIndex::from_u64(17));
    let entries = vec![
        OplogEntry::no_op(parent_start_index),
        OplogEntry::Log {
            timestamp: Timestamp::now_utc().rounded(),
            parent_start_index,
            level: LogLevel::Info,
            context: "entity".to_string(),
            message: "message".to_string(),
            trace_context: None,
        },
    ];

    for entry in entries {
        let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
            entry.clone().try_into().unwrap();
        assert_eq!(OplogEntry::try_from(proto).unwrap(), entry);
    }

    let mut invalid: golem_api_grpc::proto::golem::worker::RawOplogEntry =
        OplogEntry::no_op(parent_start_index).try_into().unwrap();
    invalid.entity_parent_start_index = Some(0);
    assert_eq!(
        OplogEntry::try_from(invalid).unwrap_err(),
        "Invalid entity parent Start index: 0"
    );
}

/// Build a single-root [`TypedSchemaValue`] fixture from an anonymous schema
/// root and a value tree.
fn typed(root: SchemaType, value: SchemaValue) -> TypedSchemaValue {
    TypedSchemaValue::new(SchemaGraph::anonymous(root), value)
}

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

fn phase_four_fixture_card() -> Card {
    Card {
        card_id: CardId(Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap()),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
            .unwrap()
            .to_utc(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    }
}

fn nf(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.to_string(),
        body,
        metadata: Default::default(),
    }
}

fn vc(name: &str, payload: Option<SchemaType>) -> VariantCaseType {
    VariantCaseType {
        name: name.to_string(),
        payload,
        metadata: Default::default(),
    }
}

#[test]
fn create_serialization_poem_serde_equivalence() {
    use crate::model::component::ComponentRevision;
    use crate::model::environment::EnvironmentId;

    let entry = PublicOplogEntry::Create(CreateParams {
        owner_kind: crate::model::agent::OwnerKind::ComponentAgent,
        timestamp: Timestamp::now_utc().rounded(),
        agent_id: AgentId {
            component_id: ComponentId(
                Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
            ),
            agent_id: "test1".to_string(),
        },
        agent_mode: crate::base_model::agent::AgentMode::Durable,
        component_revision: ComponentRevision::new(1).unwrap(),
        env: vec![("x".to_string(), "y".to_string())]
            .into_iter()
            .collect(),
        created_by: AccountId::new(),
        local_agent_config: vec![PublicTypedAgentConfigEntry {
            path: vec!["foo".to_string(), "bar".to_string()],
            value: 1i32.into_typed_schema_value().unwrap(),
        }],
        environment_id: EnvironmentId::new(),
        parent: Some(AgentId {
            component_id: ComponentId(
                Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
            ),
            agent_id: "test2".to_string(),
        }),
        component_size: 100_000_000,
        initial_total_linear_memory_size: 200_000_000,
        initial_active_plugins: BTreeSet::from_iter(vec![PluginInstallationDescription {
            environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
            plugin_priority: PluginPriority(0),
            plugin_name: "plugin1".to_string(),
            plugin_version: "1".to_string(),
            parameters: BTreeMap::new(),
        }]),
        original_phantom_id: None,
        instance_id: Uuid::new_v4(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn start_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Start(StartParams {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: None,
        function_name: "test".to_string(),
        invocation_id: None,
        observational_owner: Some(OplogIndex::from_u64(7)),
        request: Some(typed(
            SchemaType::string(),
            SchemaValue::String("test".to_string()),
        )),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
        span_started: None,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn start_with_span_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Start(StartParams {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: None,
        function_name: "rpc".to_string(),
        invocation_id: None,
        observational_owner: None,
        request: None,
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
        span_started: Some(PublicSpanStarted {
            span_id: SpanId::generate(),
            trace_id: TraceId::generate(),
            trace_states: vec!["origin=one".into()],
            parent_span_id: Some(SpanId::generate()),
            links: vec![PublicSpanLink {
                trace_id: TraceId::generate(),
                span_id: SpanId::generate(),
                trace_states: vec!["linked=one".into()],
            }],
            started_at: Timestamp::now_utc().rounded(),
            attributes: vec![PublicAttribute {
                key: "name".into(),
                value: PublicAttributeValue::String(StringAttributeValue {
                    value: "rpc".into(),
                }),
            }],
            kind: PublicSpanKind::Client,
        }),
    });

    let poem_json = entry.to_json_string();
    let serde_json = serde_json::to_string(&entry).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&poem_json).unwrap(),
        serde_json::from_str::<serde_json::Value>(&serde_json).unwrap()
    );
    assert!(poem_json.contains(r#""kind":"client""#));
    assert_eq!(
        serde_json::from_str::<PublicOplogEntry>(&poem_json).unwrap(),
        entry
    );
}

#[test]
fn end_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::End(EndParams {
        timestamp: Timestamp::now_utc().rounded(),
        start_index: crate::base_model::OplogIndex::from_u64(7),
        response: Some(typed(
            SchemaType::list(SchemaType::u64()),
            SchemaValue::List {
                elements: vec![SchemaValue::U64(1)],
            },
        )),
        forced_commit: false,
        span_finished: None,
        span_attributes: None,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn start_with_handle_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Start(StartParams {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: Some(crate::base_model::OplogIndex::from_u64(3)),
        function_name: "golem:rpc/wasm-rpc.{invoke-and-await}".to_string(),
        invocation_id: None,
        observational_owner: None,
        request: Some(typed(
            SchemaType::record(vec![
                nf("uri", SchemaType::string()),
                nf("resource-id", SchemaType::u64()),
            ]),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("urn:worker:component-id/worker-name".to_string()),
                    SchemaValue::U64(42),
                ],
            },
        )),
        durable_function_type: PublicDurableFunctionType::WriteRemote(Empty {}),
        span_started: None,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn start_with_complex_values_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Start(StartParams {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: None,
        function_name: "wasi:keyvalue/store.{get}".to_string(),
        invocation_id: None,
        observational_owner: None,
        request: Some(typed(
            SchemaType::record(vec![
                nf("name", SchemaType::string()),
                nf(
                    "data",
                    SchemaType::option(SchemaType::list(SchemaType::u8())),
                ),
                nf(
                    "status",
                    SchemaType::result(ResultSpec {
                        ok: Some(Box::new(SchemaType::tuple(vec![
                            SchemaType::bool(),
                            SchemaType::f64(),
                        ]))),
                        err: None,
                    }),
                ),
                nf(
                    "kind",
                    SchemaType::variant(vec![
                        vc("none", Some(SchemaType::string())),
                        vc("some", Some(SchemaType::s32())),
                    ]),
                ),
                nf(
                    "perms",
                    SchemaType::flags(vec![
                        "read".to_string(),
                        "write".to_string(),
                        "exec".to_string(),
                    ]),
                ),
                nf(
                    "color",
                    SchemaType::r#enum(vec![
                        "red".to_string(),
                        "green".to_string(),
                        "blue".to_string(),
                    ]),
                ),
            ]),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("key".to_string()),
                    SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::List {
                            elements: vec![SchemaValue::U8(1), SchemaValue::U8(2)],
                        })),
                    },
                    SchemaValue::Result(ResultValuePayload::Ok {
                        value: Some(Box::new(SchemaValue::Tuple {
                            elements: vec![SchemaValue::Bool(true), SchemaValue::F64(1.23)],
                        })),
                    }),
                    SchemaValue::Variant(VariantValuePayload {
                        case: 1,
                        payload: Some(Box::new(SchemaValue::S32(-42))),
                    }),
                    SchemaValue::Flags {
                        bits: vec![true, false, true],
                    },
                    SchemaValue::Enum { case: 2 },
                ],
            },
        )),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
        span_started: None,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn matcher_matches_payload_less_variant_case_name() {
    let entry = PublicOplogEntry::Start(StartParams {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: None,
        function_name: "test".to_string(),
        invocation_id: None,
        observational_owner: None,
        request: Some(typed(
            SchemaType::variant(vec![vc("none", None), vc("some", Some(SchemaType::u32()))]),
            SchemaValue::Variant(VariantValuePayload {
                case: 0,
                payload: None,
            }),
        )),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
        span_started: None,
    });

    assert!(entry.matches(&Query::parse("none").unwrap()));
    assert!(entry.matches(&Query::parse("request:none").unwrap()));
}

#[test]
fn matcher_matches_variant_payload_under_case_path() {
    let entry = PublicOplogEntry::Start(StartParams {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: None,
        function_name: "test".to_string(),
        invocation_id: None,
        observational_owner: None,
        request: Some(typed(
            SchemaType::variant(vec![vc("none", None), vc("some", Some(SchemaType::u32()))]),
            SchemaValue::Variant(VariantValuePayload {
                case: 1,
                payload: Some(Box::new(SchemaValue::U32(42))),
            }),
        )),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
        span_started: None,
    });

    assert!(entry.matches(&Query::parse("some").unwrap()));
    assert!(entry.matches(&Query::parse("request.some:42").unwrap()));
}

#[test]
fn matcher_matches_secret_reveal_request_payload() {
    let secret_id = Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap();
    let request = HostRequestSecretReveal {
        secret_id,
        expected_type: SchemaGraph::anonymous(SchemaType::string()),
    }
    .into_typed_schema_value()
    .expect("secret reveal request must be schema-encodable");

    let entry = PublicOplogEntry::Start(StartParams {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: None,
        function_name: "golem::secrets::reveal".to_string(),
        invocation_id: None,
        observational_owner: None,
        request: Some(request),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
        span_started: None,
    });

    assert!(entry.matches(&Query::parse("reveal").unwrap()));
    assert!(entry.matches(&Query::parse("request.secret_id.low-bits:291").unwrap()));
}

#[test]
fn matcher_matches_span_started_trace_context() {
    let parent_span_id = SpanId::generate();
    let linked_trace_id = TraceId::generate();
    let linked_span_id = SpanId::generate();
    let entry = PublicOplogEntry::Start(StartParams {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: None,
        function_name: "rpc".into(),
        invocation_id: None,
        observational_owner: None,
        request: None,
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
        span_started: Some(PublicSpanStarted {
            span_id: SpanId::generate(),
            trace_id: TraceId::generate(),
            trace_states: vec!["originstate".into()],
            parent_span_id: Some(parent_span_id.clone()),
            links: vec![PublicSpanLink {
                trace_id: linked_trace_id.clone(),
                span_id: linked_span_id.clone(),
                trace_states: vec!["linkstate".into()],
            }],
            started_at: Timestamp::now_utc().rounded(),
            attributes: vec![],
            kind: PublicSpanKind::Client,
        }),
    });

    for query in [
        "originstate".to_string(),
        parent_span_id.to_string(),
        linked_trace_id.to_string(),
        linked_span_id.to_string(),
        "linkstate".to_string(),
        "span-started.trace-states:originstate".to_string(),
        format!("span-started.parent-span-id:{parent_span_id}"),
        format!("span-started.links.trace-id:{linked_trace_id}"),
        format!("span-started.links.span-id:{linked_span_id}"),
        "span-started.links.trace-states:linkstate".to_string(),
    ] {
        assert!(
            entry.matches(&Query::parse(&query).unwrap()),
            "query: {query}"
        );
    }
}

#[test]
fn matcher_matches_all_structured_span_terminal_fields() {
    let span_id = SpanId::from_string("0000000000000001").unwrap();
    let attribute_span_id = SpanId::from_string("0000000000000002").unwrap();
    let entry = PublicOplogEntry::End(EndParams {
        timestamp: Timestamp::now_utc().rounded(),
        start_index: OplogIndex::from_u64(1),
        response: None,
        forced_commit: false,
        span_finished: Some(PublicSpanFinished {
            span_id: span_id.clone(),
            finished_at: Timestamp::now_utc().rounded(),
            outcome: PublicSpanOutcome::Denied,
        }),
        span_attributes: Some(PublicSpanAttributes {
            span_id: attribute_span_id.clone(),
            attributes: vec![],
        }),
    });

    let queries = [
        format!("span-finished.span-id:{span_id}"),
        format!("span-attributes.span-id:{attribute_span_id}"),
    ];
    let unmatched = queries
        .into_iter()
        .filter(|query| !entry.matches(&Query::parse(query).unwrap()))
        .collect::<Vec<_>>();
    assert!(unmatched.is_empty(), "unmatched queries: {unmatched:?}");
    assert!(!entry.matches(&Query::parse(&format!("span-attributes.span-id:{span_id}")).unwrap()));
    assert!(
        !entry
            .matches(&Query::parse(&format!("span-finished.span-id:{attribute_span_id}")).unwrap())
    );
}

#[test]
fn matcher_matches_secret_revealed_response_payload() {
    let secret_id = Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap();
    let response = HostResponseSecretRevealed {
        secret_id,
        pinned_revision: 7,
        resolved_at: SerializableDateTime {
            seconds: 1_700_000_000,
            nanoseconds: 0,
        },
        result: Ok(()),
        audit: SecretRevealAudit {
            calling_agent: AgentId {
                component_id: ComponentId(Uuid::nil()),
                agent_id: "agent-1".to_string(),
            },
            config_key: Some(vec!["db".to_string(), "password".to_string()]),
            timestamp: SerializableDateTime {
                seconds: 1_700_000_001,
                nanoseconds: 0,
            },
        },
    }
    .into_typed_schema_value()
    .expect("secret revealed response must be schema-encodable");

    let entry = PublicOplogEntry::End(EndParams {
        timestamp: Timestamp::now_utc().rounded(),
        start_index: OplogIndex::from_u64(2),
        response: Some(response),
        forced_commit: false,
        span_finished: None,
        span_attributes: None,
    });

    assert!(entry.matches(&Query::parse("response.secret_id.low-bits:291").unwrap()));
    assert!(entry.matches(&Query::parse("response.pinned_revision:7").unwrap()));
    assert!(entry.matches(&Query::parse("response.audit.config_key:password").unwrap()));
}

#[test]
fn cancelled_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Cancelled(CancelledParams {
        timestamp: Timestamp::now_utc().rounded(),
        start_index: crate::base_model::OplogIndex::from_u64(7),
        partial: None,
        span_finished: None,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn agent_invocation_started_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
        timestamp: Timestamp::now_utc().rounded(),
        invocation: PublicAgentInvocation::AgentMethodInvocation(AgentMethodInvocationParameters {
            idempotency_key: IdempotencyKey::new("idempotency_key".to_string()),
            method_name: "test".to_string(),
            function_input: typed(
                SchemaType::tuple(vec![
                    SchemaType::string(),
                    SchemaType::record(vec![
                        nf("x", SchemaType::s16()),
                        nf("y", SchemaType::s16()),
                    ]),
                ]),
                SchemaValue::Tuple {
                    elements: vec![
                        SchemaValue::String("test".to_string()),
                        SchemaValue::Record {
                            fields: vec![SchemaValue::S16(1), SchemaValue::S16(-1)],
                        },
                    ],
                },
            ),
            trace_id: TraceId::generate(),
            trace_states: vec!["a".to_string(), "b".to_string()],
            invocation_context: vec![vec![PublicSpanData::LocalSpan(PublicLocalSpanData {
                span_id: SpanId::generate(),
                start: Timestamp::now_utc().rounded(),
                parent_id: None,
                linked_context: None,
                attributes: vec![PublicAttribute {
                    key: "a".to_string(),
                    value: PublicAttributeValue::String(StringAttributeValue {
                        value: "b".to_string(),
                    }),
                }],
                inherited: true,
            })]],
        }),
        wallet_pin: PublicInvocationWalletPin {
            wallet_token: WalletVersionToken {
                wallet_id_hash: [0; 32],
                generation: 0,
            },
            scope_card_id: None,
        },
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn agent_invocation_finished_serialization_poem_serde_equivalence() {
    use crate::model::component::ComponentRevision;

    let entry = PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
        timestamp: Timestamp::now_utc().rounded(),
        result: PublicAgentInvocationResult::AgentMethod(AgentInvocationOutputParameters {
            output: typed(
                SchemaType::r#enum(vec![
                    "red".to_string(),
                    "green".to_string(),
                    "blue".to_string(),
                ]),
                SchemaValue::Enum { case: 1 },
            ),
        }),
        method_name: Some("test".to_string()),
        consumed_fuel: 100,
        component_revision: ComponentRevision::INITIAL,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn suspend_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Suspend(SuspendParams {
        timestamp: Timestamp::now_utc().rounded(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn error_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Error(ErrorParams {
        timestamp: Timestamp::now_utc().rounded(),
        kind: OplogErrorKind::Invocation,
        error: "test".to_string(),
        retry_from: OplogIndex::INITIAL,
        inside_atomic_region: false,
        retry_policy_state: None,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn no_op_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::NoOp(NoOpParams {
        timestamp: Timestamp::now_utc().rounded(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn jump_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Jump(JumpParams {
        timestamp: Timestamp::now_utc().rounded(),
        jump: OplogRegion {
            start: OplogIndex::from_u64(1),
            end: OplogIndex::from_u64(2),
        },
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn interrupted_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Interrupted(InterruptedParams {
        timestamp: Timestamp::now_utc().rounded(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn exited_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Exited(ExitedParams {
        timestamp: Timestamp::now_utc().rounded(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn begin_atomic_region_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::BeginAtomicRegion(BeginAtomicRegionParams {
        timestamp: Timestamp::now_utc().rounded(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn end_atomic_region_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::EndAtomicRegion(EndAtomicRegionParams {
        timestamp: Timestamp::now_utc().rounded(),
        begin_index: OplogIndex::from_u64(1),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn agent_invocation_started_with_initialization_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
        timestamp: Timestamp::now_utc().rounded(),
        invocation: PublicAgentInvocation::AgentInitialization(AgentInitializationParameters {
            idempotency_key: IdempotencyKey::new("idempotency_key".to_string()),
            constructor_parameters: typed(
                SchemaType::tuple(vec![SchemaType::string()]),
                SchemaValue::Tuple {
                    elements: vec![SchemaValue::String("test".to_string())],
                },
            ),
            trace_id: TraceId::generate(),
            trace_states: vec![],
            invocation_context: vec![vec![PublicSpanData::LocalSpan(PublicLocalSpanData {
                span_id: SpanId::generate(),
                start: Timestamp::now_utc().rounded(),
                parent_id: None,
                linked_context: None,
                attributes: vec![],
                inherited: false,
            })]],
        }),
        wallet_pin: PublicInvocationWalletPin {
            wallet_token: WalletVersionToken {
                wallet_id_hash: [0; 32],
                generation: 0,
            },
            scope_card_id: None,
        },
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn pending_agent_invocation_with_initialization_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
        timestamp: Timestamp::now_utc().rounded(),
        invocation: PublicAgentInvocation::AgentInitialization(AgentInitializationParameters {
            idempotency_key: IdempotencyKey::new("idempotency_key".to_string()),
            constructor_parameters: typed(
                SchemaType::tuple(vec![SchemaType::tuple(vec![])]),
                SchemaValue::Tuple {
                    elements: vec![SchemaValue::Tuple { elements: vec![] }],
                },
            ),
            trace_id: TraceId::generate(),
            trace_states: vec![],
            invocation_context: vec![vec![PublicSpanData::LocalSpan(PublicLocalSpanData {
                span_id: SpanId::generate(),
                start: Timestamp::now_utc().rounded(),
                parent_id: None,
                linked_context: None,
                attributes: vec![],
                inherited: false,
            })]],
        }),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn pending_worker_invocation_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
        timestamp: Timestamp::now_utc().rounded(),
        invocation: PublicAgentInvocation::AgentMethodInvocation(AgentMethodInvocationParameters {
            idempotency_key: IdempotencyKey::new("idempotency_key".to_string()),
            method_name: "test".to_string(),
            function_input: typed(
                SchemaType::tuple(vec![
                    SchemaType::string(),
                    SchemaType::record(vec![
                        nf("x", SchemaType::s16()),
                        nf("y", SchemaType::s16()),
                    ]),
                ]),
                SchemaValue::Tuple {
                    elements: vec![
                        SchemaValue::String("test".to_string()),
                        SchemaValue::Record {
                            fields: vec![SchemaValue::S16(1), SchemaValue::S16(-1)],
                        },
                    ],
                },
            ),
            trace_id: TraceId::generate(),
            trace_states: vec!["a".to_string(), "b".to_string()],
            invocation_context: vec![vec![PublicSpanData::LocalSpan(PublicLocalSpanData {
                span_id: SpanId::generate(),
                start: Timestamp::now_utc().rounded(),
                parent_id: None,
                linked_context: None,
                attributes: vec![PublicAttribute {
                    key: "a".to_string(),
                    value: PublicAttributeValue::String(StringAttributeValue {
                        value: "b".to_string(),
                    }),
                }],
                inherited: true,
            })]],
        }),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn pending_update_serialization_poem_serde_equivalence_1() {
    use crate::model::component::ComponentRevision;

    let entry = PublicOplogEntry::PendingUpdate(PendingUpdateParams {
        timestamp: Timestamp::now_utc().rounded(),
        target_revision: ComponentRevision::new(1).unwrap(),
        description: PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
            payload: "test".as_bytes().to_vec(),
            mime_type: "application/octet-stream".to_string(),
        }),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn pending_update_serialization_poem_serde_equivalence_2() {
    use crate::model::component::ComponentRevision;

    let entry = PublicOplogEntry::PendingUpdate(PendingUpdateParams {
        timestamp: Timestamp::now_utc().rounded(),
        target_revision: ComponentRevision::new(1).unwrap(),
        description: PublicUpdateDescription::Automatic(Empty {}),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn successful_update_serialization_poem_serde_equivalence() {
    use crate::model::component::ComponentRevision;

    let entry = PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
        timestamp: Timestamp::now_utc().rounded(),
        target_revision: ComponentRevision::new(1).unwrap(),
        new_component_size: 100_000_000,
        new_active_plugins: BTreeSet::from_iter(vec![PluginInstallationDescription {
            environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
            plugin_priority: PluginPriority(0),
            plugin_name: "plugin1".to_string(),
            plugin_version: "1".to_string(),
            parameters: BTreeMap::new(),
        }]),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn failed_update_serialization_poem_serde_equivalence_1() {
    use crate::model::component::ComponentRevision;

    let entry = PublicOplogEntry::FailedUpdate(FailedUpdateParams {
        timestamp: Timestamp::now_utc().rounded(),
        target_revision: ComponentRevision::new(1).unwrap(),
        details: Some("test".to_string()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn failed_update_serialization_poem_serde_equivalence_2() {
    use crate::model::component::ComponentRevision;

    let entry = PublicOplogEntry::FailedUpdate(FailedUpdateParams {
        timestamp: Timestamp::now_utc().rounded(),
        target_revision: ComponentRevision::new(1).unwrap(),
        details: None,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn grow_memory_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::GrowMemory(GrowMemoryParams {
        timestamp: Timestamp::now_utc().rounded(),
        delta: 100_000_000,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn create_resource_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::CreateResource(CreateResourceParams {
        timestamp: Timestamp::now_utc().rounded(),
        id: AgentResourceId(100),
        name: "test".to_string(),
        owner: "owner".to_string(),
    });

    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn drop_resource_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::DropResource(DropResourceParams {
        timestamp: Timestamp::now_utc().rounded(),
        id: AgentResourceId(100),
        name: "test".to_string(),
        owner: "owner".to_string(),
    });

    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn log_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Log(LogParams {
        timestamp: Timestamp::now_utc().rounded(),
        level: LogLevel::Stderr,
        context: "test".to_string(),
        message: "test".to_string(),
        trace_context: None,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn restart_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Restart(RestartParams {
        timestamp: Timestamp::now_utc().rounded(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn resumed_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Resumed(ResumedParams {
        timestamp: Timestamp::now_utc().rounded(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn resumed_raw_and_public_protobuf_roundtrip() {
    let timestamp = Timestamp::now_utc().rounded();

    let raw = OplogEntry::Resumed { timestamp };
    let raw_proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
        raw.clone().try_into().unwrap();
    let raw_roundtrip: OplogEntry = raw_proto.try_into().unwrap();
    assert_eq!(raw, raw_roundtrip);

    let public = PublicOplogEntry::Resumed(ResumedParams { timestamp });
    let public_proto: golem_api_grpc::proto::golem::worker::OplogEntry =
        public.clone().try_into().unwrap();
    let public_roundtrip: PublicOplogEntry = public_proto.try_into().unwrap();
    assert_eq!(public, public_roundtrip);
}

#[test]
fn activate_plugin_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ActivatePlugin(ActivatePluginParams {
        timestamp: Timestamp::now_utc().rounded(),
        plugin: PluginInstallationDescription {
            environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
            plugin_priority: PluginPriority(1),
            plugin_name: "my-plugin".to_string(),
            plugin_version: "1.0.0".to_string(),
            parameters: BTreeMap::from_iter(vec![("key".to_string(), "value".to_string())]),
        },
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn deactivate_plugin_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::DeactivatePlugin(DeactivatePluginParams {
        timestamp: Timestamp::now_utc().rounded(),
        plugin: PluginInstallationDescription {
            environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
            plugin_priority: PluginPriority(2),
            plugin_name: "my-plugin".to_string(),
            plugin_version: "2.0.0".to_string(),
            parameters: BTreeMap::new(),
        },
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn revert_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Revert(RevertParams {
        timestamp: Timestamp::now_utc().rounded(),
        dropped_region: OplogRegion {
            start: OplogIndex::from_u64(5),
            end: OplogIndex::from_u64(10),
        },
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn cancel_pending_invocation_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::CancelPendingInvocation(CancelPendingInvocationParams {
        timestamp: Timestamp::now_utc().rounded(),
        idempotency_key: IdempotencyKey::new("cancel-key".to_string()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn begin_remote_transaction_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::BeginRemoteTransaction(BeginRemoteTransactionParams {
        timestamp: Timestamp::now_utc().rounded(),
        transaction_id: TransactionId::new("txn-123".to_string()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn pre_commit_remote_transaction_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::PreCommitRemoteTransaction(PreCommitRemoteTransactionParams {
        timestamp: Timestamp::now_utc().rounded(),
        begin_index: OplogIndex::from_u64(3),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn pre_rollback_remote_transaction_serialization_poem_serde_equivalence() {
    let entry =
        PublicOplogEntry::PreRollbackRemoteTransaction(PreRollbackRemoteTransactionParams {
            timestamp: Timestamp::now_utc().rounded(),
            begin_index: OplogIndex::from_u64(3),
        });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn committed_remote_transaction_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::CommittedRemoteTransaction(CommittedRemoteTransactionParams {
        timestamp: Timestamp::now_utc().rounded(),
        begin_index: OplogIndex::from_u64(3),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn rolled_back_remote_transaction_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::RolledBackRemoteTransaction(RolledBackRemoteTransactionParams {
        timestamp: Timestamp::now_utc().rounded(),
        begin_index: OplogIndex::from_u64(3),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn snapshot_raw_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Snapshot(SnapshotParams {
        timestamp: Timestamp::now_utc().rounded(),
        data: PublicSnapshotData::Raw(RawSnapshotData {
            data: vec![1, 2, 3, 4],
            mime_type: "application/octet-stream".to_string(),
        }),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn raw_snapshot_protobuf_roundtrip_preserves_active_cards() {
    let card = test_card(CardId::new());
    let active_cards = vec![card.clone().into()];
    let entry = OplogEntry::Snapshot {
        timestamp: Timestamp::now_utc().rounded(),
        data: OplogPayload::Inline(Box::new(vec![1, 2, 3, 4])),
        mime_type: "application/octet-stream".to_string(),
        active_cards,
        wallet_generation: 73,
    };

    let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
        entry.clone().try_into().unwrap();
    let decoded = OplogEntry::try_from(proto).unwrap();
    let bytes = crate::serialization::serialize(&entry).unwrap();
    let binary_decoded = crate::serialization::deserialize::<OplogEntry>(&bytes).unwrap();
    match binary_decoded {
        OplogEntry::Snapshot {
            active_cards,
            wallet_generation,
            ..
        } => {
            assert_eq!(active_cards, vec![card.clone().into()]);
            assert_eq!(wallet_generation, 73);
        }
        other => panic!("expected binary snapshot entry, got {other:?}"),
    }

    match decoded {
        OplogEntry::Snapshot {
            active_cards,
            wallet_generation,
            ..
        } => {
            assert_eq!(active_cards, vec![card.into()]);
            assert_eq!(wallet_generation, 73);
        }
        other => panic!("expected snapshot entry, got {other:?}"),
    }
}

#[test]
fn invocation_wallet_pin_protobuf_roundtrip_and_required_validation() {
    let pinned_card_ids = vec![CardId::new(), CardId::new()];
    let scope_card_id = CardId::new();
    let wallet_token = WalletVersionToken {
        wallet_id_hash: [0x42; 32],
        generation: 73,
    };
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

    let mut raw_proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
        raw_entry.clone().try_into().unwrap();
    match OplogEntry::try_from(raw_proto.clone()).unwrap() {
        OplogEntry::AgentInvocationStarted { wallet_pin, .. } => {
            assert_eq!(
                wallet_pin.as_ref(),
                &InvocationWalletPin {
                    wallet_token: wallet_token.clone(),
                    pinned_card_ids: pinned_card_ids.clone(),
                    scope_card_id: Some(scope_card_id),
                }
            );
        }
        other => panic!("expected raw invocation-started entry, got {other:?}"),
    }
    if let Some(
        golem_api_grpc::proto::golem::worker::raw_oplog_entry::Entry::AgentInvocationStarted(
            params,
        ),
    ) = &mut raw_proto.entry
    {
        params.wallet_pin = None;
    } else {
        panic!("expected raw invocation-started protobuf entry");
    }
    assert!(OplogEntry::try_from(raw_proto).is_err());

    let public_entry = PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
        timestamp: Timestamp::now_utc().rounded(),
        invocation: PublicAgentInvocation::SaveSnapshot(Empty {}),
        wallet_pin: PublicInvocationWalletPin {
            wallet_token,
            scope_card_id: Some(scope_card_id),
        },
    });
    let mut public_proto: golem_api_grpc::proto::golem::worker::OplogEntry =
        public_entry.clone().try_into().unwrap();
    assert_eq!(
        PublicOplogEntry::try_from(public_proto.clone()).unwrap(),
        public_entry
    );
    if let Some(golem_api_grpc::proto::golem::worker::oplog_entry::Entry::AgentInvocationStarted(
        params,
    )) = &mut public_proto.entry
    {
        params.wallet_pin = None;
    } else {
        panic!("expected public invocation-started protobuf entry");
    }
    assert!(PublicOplogEntry::try_from(public_proto).is_err());
}

#[test]
fn phase_four_queued_card_event_binary_fixtures_remain_decodable() {
    let card = phase_four_fixture_card();
    let card_id = card.card_id;
    let fixtures = [
        (
            include_bytes!(
                "../../../tests/fixtures/permission-cards/phase4/queued_card_event_install.bin"
            )
            .as_slice(),
            QueuedCardEvent::install(card),
        ),
        (
            include_bytes!(
                "../../../tests/fixtures/permission-cards/phase4/queued_card_event_revoke.bin"
            )
            .as_slice(),
            QueuedCardEvent::revoke(card_id),
        ),
    ];

    for (bytes, expected) in fixtures {
        assert_eq!(
            crate::serialization::deserialize::<QueuedCardEvent>(bytes).unwrap(),
            expected
        );
    }
}

#[test]
fn phase_four_public_queued_card_event_binary_fixtures_remain_decodable() {
    let card_id = phase_four_fixture_card().card_id;
    let fixtures = [
        (
            include_bytes!(
                "../../../tests/fixtures/permission-cards/phase4/public_queued_card_event_install.bin"
            )
            .as_slice(),
            PublicQueuedCardEvent::Install(crate::model::oplog::PublicQueuedCardEventCard {
                card_id,
            }),
        ),
        (
            include_bytes!(
                "../../../tests/fixtures/permission-cards/phase4/public_queued_card_event_revoke.bin"
            )
            .as_slice(),
            PublicQueuedCardEvent::Revoke(crate::model::oplog::PublicQueuedCardEventCard {
                card_id,
            }),
        ),
    ];

    for (bytes, expected) in fixtures {
        assert_eq!(
            crate::serialization::deserialize::<PublicQueuedCardEvent>(bytes).unwrap(),
            expected
        );
    }
}

#[test]
fn phase_five_raw_card_oplog_entries_protobuf_roundtrip() {
    let source_card_id = CardId::new();
    let installed_card_id = CardId::new();
    let source_card = test_card(source_card_id);
    let installed_card = test_card(installed_card_id);
    let transfer_id = Uuid::new_v4();
    let source_holder = CardHolder::Account(AccountCardHolder {
        account_id: Uuid::new_v4(),
    });
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "phase-five-target".to_string(),
        },
    });
    let application_holder = CardHolder::Application(ApplicationCardHolder {
        application_id: Uuid::new_v4(),
    });
    let timestamp = Timestamp::now_utc().rounded();
    let entries = vec![
        OplogEntry::CardInstalled {
            timestamp,
            entity_parent_start_index: None,
            queued_event_index: None,
            card: Box::new(source_card.clone().into()),
            wallet_generation: 1,
        },
        OplogEntry::CardDerived {
            timestamp,
            entity_parent_start_index: None,
            card: Box::new(source_card.clone().into()),
            wallet_generation: 3,
        },
        OplogEntry::CardRevoked {
            timestamp,
            entity_parent_start_index: None,
            queued_event_index: OplogIndex::from_u64(1),
            card_id: source_card_id,
            wallet_generation: 4,
        },
        OplogEntry::CardExpired {
            timestamp,
            entity_parent_start_index: None,
            card_id: installed_card_id,
            wallet_generation: 5,
        },
        OplogEntry::CardTransferStarted {
            timestamp,
            entity_parent_start_index: None,
            transfer_id,
            card_id: source_card_id,
            source_holder: source_holder.clone(),
            target_holder: target_holder.clone(),
            source_wallet_generation: 4,
        },
        OplogEntry::CardTransferred {
            timestamp,
            entity_parent_start_index: None,
            transfer_id,
            source_card_id,
            installed_card_id,
            target_holder: target_holder.clone(),
            card: Box::new(installed_card.into()),
            target_wallet_generation: 7,
        },
        OplogEntry::CardRevokedCascade {
            timestamp,
            entity_parent_start_index: None,
            revoked_card_ids: vec![source_card_id, installed_card_id],
            affected_wallets: vec![source_holder.clone(), target_holder.clone()],
            local_wallet_generation: 8,
        },
        OplogEntry::CardTransferConfirmed {
            timestamp,
            entity_parent_start_index: None,
            transfer_id,
            source_card_id,
            installed_card_id,
            target_holder: target_holder.clone(),
        },
        OplogEntry::CardEventQueued {
            timestamp,
            entity_parent_start_index: None,
            event: Box::new(QueuedCardEvent::transfer_started(
                transfer_id,
                source_card,
                application_holder,
            )),
        },
        OplogEntry::CardTransferStarted {
            timestamp,
            entity_parent_start_index: None,
            transfer_id: Uuid::new_v4(),
            card_id: installed_card_id,
            source_holder,
            target_holder,
            source_wallet_generation: 9,
        },
    ];

    for entry in entries {
        let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
            entry.clone().try_into().unwrap();
        assert_eq!(OplogEntry::try_from(proto).unwrap(), entry);

        let bytes = crate::serialization::serialize(&entry).unwrap();
        assert_eq!(
            crate::serialization::deserialize::<OplogEntry>(&bytes).unwrap(),
            entry
        );
    }
}

#[test]
fn phase_five_public_card_oplog_entries_protobuf_roundtrip() {
    let card_id = CardId::new();
    let installed_card_id = CardId::new();
    let transfer_id = Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "phase-five-public-target".to_string(),
        },
    });
    let timestamp = Timestamp::now_utc().rounded();
    let entries = vec![
        PublicOplogEntry::CardInstalled(CardInstalledParams {
            timestamp,
            queued_event_index: None,
            card_id,
            wallet_generation: 1,
        }),
        PublicOplogEntry::CardDerived(CardDerivedParams {
            timestamp,
            card_id,
            parent_ids: vec![CardId::new(), CardId::new()],
            wallet_generation: 3,
        }),
        PublicOplogEntry::CardRevoked(CardRevokedParams {
            timestamp,
            queued_event_index: OplogIndex::from_u64(1),
            card_id,
            wallet_generation: 4,
        }),
        PublicOplogEntry::CardExpired(CardExpiredParams {
            timestamp,
            card_id: installed_card_id,
            wallet_generation: 5,
        }),
        PublicOplogEntry::CardTransferStarted(CardTransferStartedParams {
            timestamp,
            transfer_id,
            card_id,
            target_holder: target_holder.clone(),
            source_wallet_generation: 4,
        }),
        PublicOplogEntry::CardTransferred(CardTransferredParams {
            timestamp,
            transfer_id,
            source_card_id: card_id,
            installed_card_id,
            target_holder: target_holder.clone(),
            target_wallet_generation: 7,
        }),
        PublicOplogEntry::CardRevokedCascade(CardRevokedCascadeParams {
            timestamp,
            revoked_card_ids: vec![card_id, installed_card_id],
            local_wallet_generation: 8,
        }),
        PublicOplogEntry::CardTransferConfirmed(CardTransferConfirmedParams {
            timestamp,
            transfer_id,
            source_card_id: card_id,
            installed_card_id,
            target_holder,
        }),
        PublicOplogEntry::CardEventQueued(
            crate::model::oplog::public_oplog_entry::CardEventQueuedParams {
                timestamp,
                event: PublicQueuedCardEvent::TransferStarted(
                    crate::model::oplog::PublicQueuedCardEventTransfer {
                        transfer_id,
                        card_id,
                        target_holder: CardHolder::Application(ApplicationCardHolder {
                            application_id: Uuid::new_v4(),
                        }),
                    },
                ),
            },
        ),
    ];

    for entry in entries {
        let proto: golem_api_grpc::proto::golem::worker::OplogEntry =
            entry.clone().try_into().unwrap();
        assert_eq!(PublicOplogEntry::try_from(proto).unwrap(), entry);

        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(
            serde_json::from_str::<PublicOplogEntry>(&json).unwrap(),
            entry
        );
    }
}

#[test]
fn snapshot_json_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Snapshot(SnapshotParams {
        timestamp: Timestamp::now_utc().rounded(),
        data: PublicSnapshotData::Json(JsonSnapshotData {
            data: serde_json::json!({"key": "value", "count": 42}),
        }),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn snapshot_multipart_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Snapshot(SnapshotParams {
        timestamp: Timestamp::now_utc().rounded(),
        data: PublicSnapshotData::Multipart(MultipartSnapshotData {
            mime_type: "multipart/mixed; boundary=test-boundary".to_string(),
            parts: vec![
                MultipartSnapshotPart {
                    name: "state".to_string(),
                    content_type: "application/json".to_string(),
                    data: MultipartPartData::Json(JsonSnapshotData {
                        data: serde_json::json!({"version": 1, "properties": {"counter": 42}}),
                    }),
                },
                MultipartSnapshotPart {
                    name: "db:main".to_string(),
                    content_type: "application/x-sqlite3".to_string(),
                    data: MultipartPartData::Raw(RawSnapshotData {
                        data: vec![0x53, 0x51, 0x4C, 0x69, 0x74, 0x65],
                        mime_type: "application/x-sqlite3".to_string(),
                    }),
                },
            ],
        }),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn set_retry_policy_serialization_poem_serde_equivalence() {
    use crate::model::oplog::PublicNamedRetryPolicy;
    use crate::model::retry_policy::{NamedRetryPolicy, Predicate, PredicateValue, RetryPolicy};

    let named_policy = NamedRetryPolicy {
        name: "http-transient".to_string(),
        priority: 10,
        predicate: Predicate::PropEq {
            property: "error-type".to_string(),
            value: PredicateValue::Text("transient".to_string()),
        },
        policy: RetryPolicy::CountBox {
            max_retries: 3,
            inner: Box::new(RetryPolicy::Exponential {
                base_delay: std::time::Duration::from_secs(1),
                factor: 2.0,
            }),
        },
    };
    let entry = PublicOplogEntry::SetRetryPolicy(SetRetryPolicyParams {
        timestamp: Timestamp::now_utc().rounded(),
        policy: PublicNamedRetryPolicy::from(named_policy),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn remove_retry_policy_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::RemoveRetryPolicy(RemoveRetryPolicyParams {
        timestamp: Timestamp::now_utc().rounded(),
        name: "http-transient".to_string(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

mod scope_scan {
    use crate::model::card::CardId;
    use crate::model::invocation_context::SpanId;
    use crate::model::oplog::host_functions::HostFunctionName;
    use crate::model::oplog::raw_types::PayloadId;
    use crate::model::oplog::{
        AgentError, AttributeMap, DurableFunctionType, LogLevel, OplogEntry, OplogErrorKind,
        OplogPayload, OplogScopeProjection, ScopeScanState,
    };
    use crate::model::regions::OplogRegion;
    use crate::model::{
        AgentInvocationResult, ComponentRevision, OplogIndex, Timestamp, TransactionId,
    };
    use std::collections::HashMap;
    use test_r::test;

    fn idx(i: u64) -> OplogIndex {
        OplogIndex::from_u64(i)
    }

    fn start(parent: Option<u64>, durable_function_type: DurableFunctionType) -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: parent.map(idx),
            function_name: HostFunctionName::Custom("test".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type,
            span_started: None,
        }
    }

    /// Replays the forward scan that `lookup_oplog_entry_with_condition_and_state` performs,
    /// returning `true` if no entry between the scope `Start` (`root`) and its `End` is a foreign
    /// concurrent side effect (i.e. `for_all_intermediate` holds for all of `entries`).
    fn scan(root: u64, entries: &[(u64, OplogEntry)]) -> bool {
        let mut state = ScopeScanState::new(idx(root));
        let mut ok = true;
        for (i, entry) in entries {
            entry.track_scope_membership(idx(*i), &mut state);
            if !entry.no_concurrent_side_effect(idx(root), &state) {
                ok = false;
            }
        }
        ok
    }

    fn invocation_finished() -> OplogEntry {
        OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            result: OplogPayload::Inline(Box::new(AgentInvocationResult::AgentInitialization)),
            method_name: None,
            consumed_fuel: 0,
            component_revision: ComponentRevision(0),
        }
    }

    fn end(start: u64) -> OplogEntry {
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: idx(start),
            response: None,
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        }
    }

    #[test]
    fn scope_projection_selects_only_transitive_call_tree_entries() {
        let entries = vec![
            (10, start(None, DurableFunctionType::WriteLocal)),
            (11, start(None, DurableFunctionType::WriteLocal)),
            (12, start(Some(10), DurableFunctionType::ReadLocal)),
            (13, start(Some(12), DurableFunctionType::ReadLocal)),
            (14, start(Some(11), DurableFunctionType::ReadLocal)),
            (15, end(13)),
            (16, end(14)),
            (17, end(12)),
            (18, end(10)),
        ];
        let mut projection = OplogScopeProjection::new(idx(10));

        let projected = entries
            .iter()
            .filter_map(|(index, entry)| projection.includes(idx(*index), entry).then_some(*index))
            .collect::<Vec<_>>();

        assert_eq!(projected, vec![10, 12, 13, 15, 17, 18]);
    }

    #[test]
    fn scope_projection_uses_entity_anchor_instead_of_retry_grouping() {
        let entries = [
            (10, start(None, DurableFunctionType::WriteLocal)),
            (
                11,
                OplogEntry::error(
                    Some(idx(10)),
                    OplogErrorKind::Invocation,
                    AgentError::TransientError("entity-owned".to_string()),
                    idx(99),
                    false,
                    None,
                ),
            ),
            (
                12,
                OplogEntry::error(
                    None,
                    OplogErrorKind::Invocation,
                    AgentError::TransientError("agent-owned".to_string()),
                    idx(10),
                    false,
                    None,
                ),
            ),
            (13, OplogEntry::no_op(Some(idx(10)))),
            (14, OplogEntry::no_op(None)),
            (
                15,
                OplogEntry::CardExpired {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: Some(idx(10)),
                    card_id: CardId::new(),
                    wallet_generation: 1,
                },
            ),
            (
                16,
                OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: Some(idx(10)),
                    summary: None,
                    record: OplogPayload::External {
                        payload_id: PayloadId::new(),
                        md5_hash: vec![0; 16],
                        cached: None,
                    },
                },
            ),
        ];
        let mut projection = OplogScopeProjection::new(idx(10));

        let projected = entries
            .iter()
            .filter_map(|(index, entry)| projection.includes(idx(*index), entry).then_some(*index))
            .collect::<Vec<_>>();

        assert_eq!(projected, vec![10, 11, 13, 15, 16]);
    }

    #[test]
    fn scope_projection_includes_attributed_logs_spans_and_transaction_markers() {
        use crate::model::invocation_context::TraceId;
        use crate::model::oplog::{SpanFinished, SpanKind, SpanOutcome, SpanStarted};

        let span_id = SpanId::generate();
        let mut span_start = start(Some(10), DurableFunctionType::ReadLocal);
        if let OplogEntry::Start { span_started, .. } = &mut span_start {
            *span_started = Some(Box::new(SpanStarted {
                span_id: span_id.clone(),
                trace_id: TraceId::generate(),
                trace_states: Vec::new(),
                parent_span_id: None,
                links: Vec::new(),
                started_at: Timestamp::now_utc(),
                attributes: AttributeMap(HashMap::new()),
                kind: SpanKind::Internal,
            }));
        }
        let entries = vec![
            (10, start(None, DurableFunctionType::WriteLocal)),
            (
                11,
                OplogEntry::Log {
                    timestamp: Timestamp::now_utc(),
                    parent_start_index: Some(idx(10)),
                    level: LogLevel::Info,
                    context: "entity".to_string(),
                    message: "included".to_string(),
                    trace_context: None,
                },
            ),
            (
                12,
                OplogEntry::Log {
                    timestamp: Timestamp::now_utc(),
                    parent_start_index: None,
                    level: LogLevel::Info,
                    context: "owner".to_string(),
                    message: "excluded".to_string(),
                    trace_context: None,
                },
            ),
            (13, span_start),
            (
                14,
                OplogEntry::end(
                    idx(13),
                    None,
                    false,
                    Some(SpanFinished {
                        span_id,
                        finished_at: Timestamp::now_utc(),
                        outcome: SpanOutcome::Completed,
                    }),
                    None,
                ),
            ),
            (
                15,
                start(Some(10), DurableFunctionType::WriteRemoteTransaction(None)),
            ),
            (
                16,
                OplogEntry::BeginRemoteTransaction {
                    timestamp: Timestamp::now_utc(),
                    transaction_id: TransactionId::new("entity-tx".to_string()),
                    original_begin_index: None,
                },
            ),
            (
                17,
                OplogEntry::PreCommitRemoteTransaction {
                    timestamp: Timestamp::now_utc(),
                    begin_index: idx(15),
                },
            ),
            (
                18,
                OplogEntry::CommittedRemoteTransaction {
                    timestamp: Timestamp::now_utc(),
                    begin_index: idx(15),
                },
            ),
            (19, end(15)),
            (
                20,
                OplogEntry::CompletionDiscarded {
                    timestamp: Timestamp::now_utc(),
                    start_index: idx(10),
                },
            ),
        ];
        let mut projection = OplogScopeProjection::new(idx(10));

        let projected = entries
            .iter()
            .filter_map(|(index, entry)| projection.includes(idx(*index), entry).then_some(*index))
            .collect::<Vec<_>>();

        assert_eq!(projected, vec![10, 11, 13, 14, 15, 16, 17, 18, 19, 20]);
    }

    #[test]
    fn scope_projection_includes_retried_transaction_begin_marker() {
        let entries = [
            (
                10,
                start(None, DurableFunctionType::WriteRemoteTransaction(None)),
            ),
            (
                11,
                OplogEntry::BeginRemoteTransaction {
                    timestamp: Timestamp::now_utc(),
                    transaction_id: TransactionId::new("initial".to_string()),
                    original_begin_index: None,
                },
            ),
            (
                12,
                OplogEntry::Jump {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    jump: OplogRegion {
                        start: idx(11),
                        end: idx(12),
                    },
                },
            ),
            (
                13,
                OplogEntry::BeginRemoteTransaction {
                    timestamp: Timestamp::now_utc(),
                    transaction_id: TransactionId::new("retry".to_string()),
                    original_begin_index: Some(idx(10)),
                },
            ),
        ];
        let mut projection = OplogScopeProjection::new(idx(10));

        let projected = entries
            .iter()
            .filter_map(|(index, entry)| projection.includes(idx(*index), entry).then_some(*index))
            .collect::<Vec<_>>();

        assert_eq!(projected, vec![10, 11, 13]);
    }

    #[test]
    fn scope_projection_excludes_adjacent_foreign_retried_transaction_begin_marker() {
        let entries = [
            (10, start(None, DurableFunctionType::WriteLocal)),
            (
                11,
                start(Some(10), DurableFunctionType::WriteRemoteTransaction(None)),
            ),
            (
                12,
                OplogEntry::BeginRemoteTransaction {
                    timestamp: Timestamp::now_utc(),
                    transaction_id: TransactionId::new("foreign-retry".to_string()),
                    original_begin_index: Some(idx(20)),
                },
            ),
        ];
        let mut projection = OplogScopeProjection::new(idx(10));

        let projected = entries
            .iter()
            .filter_map(|(index, entry)| projection.includes(idx(*index), entry).then_some(*index))
            .collect::<Vec<_>>();

        assert_eq!(projected, vec![10, 11]);
    }

    #[test]
    fn stream_hints_are_benign_revision_neutral_and_excluded_from_scope_projection() {
        macro_rules! external_payload {
            () => {
                OplogPayload::External {
                    payload_id: PayloadId::new(),
                    md5_hash: vec![0; 16],
                    cached: None,
                }
            };
        }
        let entries = [
            OplogEntry::StreamRegistered {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: external_payload!(),
            },
            OplogEntry::StreamItems {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: external_payload!(),
            },
            OplogEntry::StreamEnd {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: external_payload!(),
            },
            OplogEntry::StreamCancel {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: external_payload!(),
            },
            OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: external_payload!(),
            },
        ];
        let root = idx(10);
        let state = ScopeScanState::new(root);
        let mut projection = OplogScopeProjection::new(root);
        assert!(projection.includes(root, &start(None, DurableFunctionType::WriteLocal)));

        for (offset, entry) in entries.iter().enumerate() {
            assert!(entry.no_concurrent_side_effect(root, &state));
            assert_eq!(entry.specifies_component_revision(), None);
            assert!(!projection.includes(idx(11 + offset as u64), entry));
        }
    }

    #[test]
    fn direct_child_scope_is_allowed() {
        // An HTTP call inside a batched-write scope writes a `Start` whose parent is the scope root.
        let entries = vec![(
            11,
            start(
                Some(10),
                DurableFunctionType::WriteRemoteBatched(Some(idx(10))),
            ),
        )];
        assert!(scan(10, &entries));
    }

    #[test]
    fn transitive_grandchild_scope_is_allowed() {
        // A grandchild's parent is the inner scope (11), not the root (10); transitive tracking
        // must still recognise it as part of the scope.
        let entries = vec![
            (
                11,
                start(Some(10), DurableFunctionType::WriteRemoteTransaction(None)),
            ),
            (12, start(Some(11), DurableFunctionType::WriteRemote)),
        ];
        assert!(scan(10, &entries));
    }

    #[test]
    fn foreign_read_remote_is_allowed() {
        let entries = vec![(11, start(None, DurableFunctionType::ReadRemote))];
        assert!(scan(10, &entries));
    }

    #[test]
    fn foreign_write_remote_is_rejected() {
        let entries = vec![(11, start(None, DurableFunctionType::WriteRemote))];
        assert!(!scan(10, &entries));
    }

    #[test]
    fn foreign_batched_scope_is_rejected() {
        let entries = vec![(
            11,
            start(None, DurableFunctionType::WriteRemoteBatched(None)),
        )];
        assert!(!scan(10, &entries));
    }

    #[test]
    fn grandchild_of_foreign_scope_is_rejected() {
        // The intermediate scope (11) is foreign (parent is unrelated 99), so its descendant (12)
        // must not be absorbed into the root scope.
        let entries = vec![
            (
                11,
                start(Some(99), DurableFunctionType::WriteRemoteBatched(None)),
            ),
            (12, start(Some(11), DurableFunctionType::WriteRemote)),
        ];
        assert!(!scan(10, &entries));
    }

    #[test]
    fn end_and_cancelled_markers_are_not_side_effects() {
        let end = OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: idx(11),
            response: None,
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        };
        let cancelled = OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: idx(12),
            partial: None,
            span_finished: None,
        };
        let entries = vec![
            (11, start(Some(10), DurableFunctionType::WriteRemote)),
            (12, end),
            (13, cancelled),
        ];
        assert!(scan(10, &entries));
    }

    #[test]
    fn invocation_finished_marker_is_not_a_foreign_side_effect() {
        let entries = vec![
            (
                11,
                start(
                    Some(10),
                    DurableFunctionType::WriteRemoteBatched(Some(idx(10))),
                ),
            ),
            (12, invocation_finished()),
        ];
        assert!(scan(10, &entries));
    }
}

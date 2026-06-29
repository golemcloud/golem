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
use crate::model::card::{Card, CardId};
use crate::model::component::PluginPriority;
use crate::model::invocation_context::{SpanId, TraceId};
use crate::model::lucene::Query;
use crate::model::oplog::payload::types::{SecretRevealAudit, SerializableDateTime};
use crate::model::oplog::payload::{HostRequestSecretReveal, HostResponseSecretRevealed};
use crate::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginAtomicRegionParams, BeginRemoteTransactionParams, CancelPendingInvocationParams,
    CancelledParams, ChangePersistenceLevelParams, CommittedRemoteTransactionParams, CreateParams,
    CreateResourceParams, DeactivatePluginParams, DropResourceParams, EndAtomicRegionParams,
    EndParams, ErrorParams, ExitedParams, FailedUpdateParams, FinishSpanParams, GrowMemoryParams,
    InterruptedParams, JumpParams, LogParams, NoOpParams, PendingAgentInvocationParams,
    PendingUpdateParams, PreCommitRemoteTransactionParams, PreRollbackRemoteTransactionParams,
    RemoveRetryPolicyParams, RestartParams, RevertParams, RolledBackRemoteTransactionParams,
    SetRetryPolicyParams, SetSpanAttributeParams, SnapshotParams, StartParams, StartSpanParams,
    SuccessfulUpdateParams, SuspendParams,
};
use crate::model::oplog::{
    AgentInitializationParameters, AgentInvocationOutputParameters,
    AgentMethodInvocationParameters, AgentResourceId, JsonSnapshotData, LogLevel,
    MultipartPartData, MultipartSnapshotData, MultipartSnapshotPart, OplogEntry, OplogPayload,
    PersistenceLevel, PluginInstallationDescription, PublicAgentInvocation,
    PublicAgentInvocationResult, PublicAttribute, PublicAttributeValue, PublicDurableFunctionType,
    PublicLocalSpanData, PublicOplogEntry, PublicSnapshotData, PublicSpanData,
    PublicTypedAgentConfigEntry, PublicUpdateDescription, RawSnapshotData,
    SnapshotBasedUpdateParameters, StringAttributeValue,
};
use crate::model::regions::OplogRegion;
use crate::model::{
    AccountId, AgentId, ComponentId, Empty, IdempotencyKey, OplogIndex, Timestamp, TransactionId,
};
use crate::schema::IntoTypedSchemaValue;
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};
use crate::schema::schema_value::{ResultValuePayload, SchemaValue, VariantValuePayload};
use poem_openapi::types::ToJSON;
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, BTreeSet};
use test_r::test;
use uuid::Uuid;

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
        request: Some(typed(
            SchemaType::string(),
            SchemaValue::String("test".to_string()),
        )),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
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
        request: Some(typed(
            SchemaType::variant(vec![vc("none", None), vc("some", Some(SchemaType::u32()))]),
            SchemaValue::Variant(VariantValuePayload {
                case: 0,
                payload: None,
            }),
        )),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
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
        request: Some(typed(
            SchemaType::variant(vec![vc("none", None), vc("some", Some(SchemaType::u32()))]),
            SchemaValue::Variant(VariantValuePayload {
                case: 1,
                payload: Some(Box::new(SchemaValue::U32(42))),
            }),
        )),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
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
        request: Some(request),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
    });

    assert!(entry.matches(&Query::parse("reveal").unwrap()));
    assert!(entry.matches(&Query::parse("request.secret_id.low-bits:291").unwrap()));
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
fn start_span_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::StartSpan(StartSpanParams {
        timestamp: Timestamp::now_utc().rounded(),
        span_id: SpanId::generate(),
        parent_id: Some(SpanId::generate()),
        linked_context: Some(SpanId::generate()),
        attributes: vec![PublicAttribute {
            key: "test-attr".to_string(),
            value: PublicAttributeValue::String(StringAttributeValue {
                value: "test-value".to_string(),
            }),
        }],
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn finish_span_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::FinishSpan(FinishSpanParams {
        timestamp: Timestamp::now_utc().rounded(),
        span_id: SpanId::generate(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn set_span_attribute_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::SetSpanAttribute(SetSpanAttributeParams {
        timestamp: Timestamp::now_utc().rounded(),
        span_id: SpanId::generate(),
        key: "http.method".to_string(),
        value: PublicAttributeValue::String(StringAttributeValue {
            value: "GET".to_string(),
        }),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn change_persistence_level_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ChangePersistenceLevel(ChangePersistenceLevelParams {
        timestamp: Timestamp::now_utc().rounded(),
        persistence_level: PersistenceLevel::Smart,
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
    };

    let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
        entry.clone().try_into().unwrap();
    let decoded = OplogEntry::try_from(proto).unwrap();

    match decoded {
        OplogEntry::Snapshot { active_cards, .. } => {
            assert_eq!(active_cards, vec![card.into()]);
        }
        other => panic!("expected snapshot entry, got {other:?}"),
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
    use crate::model::oplog::host_functions::HostFunctionName;
    use crate::model::oplog::{DurableFunctionType, OplogEntry, ScopeScanState};
    use crate::model::{OplogIndex, Timestamp};
    use test_r::test;

    fn idx(i: u64) -> OplogIndex {
        OplogIndex::from_u64(i)
    }

    fn start(parent: Option<u64>, durable_function_type: DurableFunctionType) -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: parent.map(idx),
            function_name: HostFunctionName::Custom("test".to_string()),
            request: None,
            durable_function_type,
        }
    }

    /// Replays the forward scan that `lookup_oplog_entry_with_condition_and_state` performs,
    /// returning `true` if no entry between the scope `Start` (`root`) and its `End` is a foreign
    /// concurrent side effect (i.e. `for_all_intermediate` holds for all of `entries`).
    fn scan(
        root: u64,
        entries: &[(u64, OplogEntry)],
        persistence_level: crate::model::oplog::PersistenceLevel,
    ) -> bool {
        let mut state = ScopeScanState::new(idx(root), persistence_level);
        let mut ok = true;
        for (i, entry) in entries {
            entry.track_scope_membership(idx(*i), &mut state);
            if !entry.no_concurrent_side_effect(idx(root), &state) {
                ok = false;
            }
        }
        ok
    }

    fn persist_all() -> crate::model::oplog::PersistenceLevel {
        crate::model::oplog::PersistenceLevel::Smart
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
        assert!(scan(10, &entries, persist_all()));
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
        assert!(scan(10, &entries, persist_all()));
    }

    #[test]
    fn foreign_read_remote_is_allowed() {
        let entries = vec![(11, start(None, DurableFunctionType::ReadRemote))];
        assert!(scan(10, &entries, persist_all()));
    }

    #[test]
    fn foreign_write_remote_is_rejected() {
        let entries = vec![(11, start(None, DurableFunctionType::WriteRemote))];
        assert!(!scan(10, &entries, persist_all()));
    }

    #[test]
    fn foreign_batched_scope_is_rejected() {
        let entries = vec![(
            11,
            start(None, DurableFunctionType::WriteRemoteBatched(None)),
        )];
        assert!(!scan(10, &entries, persist_all()));
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
        assert!(!scan(10, &entries, persist_all()));
    }

    #[test]
    fn persist_nothing_ignores_foreign_side_effects() {
        let entries = vec![(11, start(None, DurableFunctionType::WriteRemote))];
        assert!(scan(
            10,
            &entries,
            crate::model::oplog::PersistenceLevel::PersistNothing
        ));
    }

    #[test]
    fn end_and_cancelled_markers_are_not_side_effects() {
        let end = OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: idx(11),
            response: None,
            forced_commit: false,
        };
        let cancelled = OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: idx(12),
            partial: None,
        };
        let entries = vec![
            (11, start(Some(10), DurableFunctionType::WriteRemote)),
            (12, end),
            (13, cancelled),
        ];
        assert!(scan(10, &entries, persist_all()));
    }
}

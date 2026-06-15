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
use crate::model::component::PluginPriority;
use crate::model::invocation_context::{SpanId, TraceId};
use crate::model::lucene::Query;
use crate::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginAtomicRegionParams, BeginRemoteTransactionParams, BeginRemoteWriteParams,
    CancelPendingInvocationParams, ChangePersistenceLevelParams, CommittedRemoteTransactionParams,
    CreateParams, CreateResourceParams, DeactivatePluginParams, DropResourceParams,
    EndAtomicRegionParams, EndRemoteWriteParams, ErrorParams, ExitedParams, FailedUpdateParams,
    FinishSpanParams, GrowMemoryParams, HostCallParams, InterruptedParams, JumpParams, LogParams,
    NoOpParams, PendingAgentInvocationParams, PendingUpdateParams,
    PreCommitRemoteTransactionParams, PreRollbackRemoteTransactionParams, RemoveRetryPolicyParams,
    RestartParams, RevertParams, RolledBackRemoteTransactionParams, SetRetryPolicyParams,
    SetSpanAttributeParams, SnapshotParams, StartSpanParams, SuccessfulUpdateParams, SuspendParams,
};
use crate::model::oplog::{
    AgentInitializationParameters, AgentInvocationOutputParameters,
    AgentMethodInvocationParameters, AgentResourceId, JsonSnapshotData, LogLevel,
    MultipartPartData, MultipartSnapshotData, MultipartSnapshotPart, PersistenceLevel,
    PluginInstallationDescription, PublicAgentInvocation, PublicAgentInvocationResult,
    PublicAttribute, PublicAttributeValue, PublicDurableFunctionType, PublicLocalSpanData,
    PublicOplogEntry, PublicSnapshotData, PublicSpanData, PublicTypedAgentConfigEntry,
    PublicUpdateDescription, RawSnapshotData, SnapshotBasedUpdateParameters, StringAttributeValue,
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
fn host_call_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::HostCall(HostCallParams {
        timestamp: Timestamp::now_utc().rounded(),
        function_name: "test".to_string(),
        request: typed(
            SchemaType::string(),
            SchemaValue::String("test".to_string()),
        ),
        response: typed(
            SchemaType::list(SchemaType::u64()),
            SchemaValue::List {
                elements: vec![SchemaValue::U64(1)],
            },
        ),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn host_call_with_tuple_values_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::HostCall(HostCallParams {
        timestamp: Timestamp::now_utc().rounded(),
        function_name: "golem:rpc/wasm-rpc.{invoke-and-await}".to_string(),
        request: typed(
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
        ),
        response: typed(
            SchemaType::tuple(vec![SchemaType::u64()]),
            SchemaValue::Tuple {
                elements: vec![SchemaValue::U64(5)],
            },
        ),
        durable_function_type: PublicDurableFunctionType::WriteRemote(Empty {}),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn host_call_with_complex_values_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::HostCall(HostCallParams {
        timestamp: Timestamp::now_utc().rounded(),
        function_name: "wasi:keyvalue/store.{get}".to_string(),
        request: typed(
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
        ),
        response: typed(
            SchemaType::result(ResultSpec {
                ok: None,
                err: Some(Box::new(SchemaType::string())),
            }),
            SchemaValue::Result(ResultValuePayload::Err {
                value: Some(Box::new(SchemaValue::String("not found".to_string()))),
            }),
        ),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn matcher_matches_payload_less_variant_case_name() {
    let entry = PublicOplogEntry::HostCall(HostCallParams {
        timestamp: Timestamp::now_utc().rounded(),
        function_name: "test".to_string(),
        request: typed(
            SchemaType::variant(vec![vc("none", None), vc("some", Some(SchemaType::u32()))]),
            SchemaValue::Variant(VariantValuePayload {
                case: 0,
                payload: None,
            }),
        ),
        response: typed(
            SchemaType::tuple(Vec::new()),
            SchemaValue::Tuple { elements: vec![] },
        ),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
    });

    assert!(entry.matches(&Query::parse("none").unwrap()));
    assert!(entry.matches(&Query::parse("request:none").unwrap()));
}

#[test]
fn matcher_matches_variant_payload_under_case_path() {
    let entry = PublicOplogEntry::HostCall(HostCallParams {
        timestamp: Timestamp::now_utc().rounded(),
        function_name: "test".to_string(),
        request: typed(
            SchemaType::variant(vec![vc("none", None), vc("some", Some(SchemaType::u32()))]),
            SchemaValue::Variant(VariantValuePayload {
                case: 1,
                payload: Some(Box::new(SchemaValue::U32(42))),
            }),
        ),
        response: typed(
            SchemaType::tuple(Vec::new()),
            SchemaValue::Tuple { elements: vec![] },
        ),
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
    });

    assert!(entry.matches(&Query::parse("some").unwrap()));
    assert!(entry.matches(&Query::parse("request.some:42").unwrap()));
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
fn begin_remote_write_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::BeginRemoteWrite(BeginRemoteWriteParams {
        timestamp: Timestamp::now_utc().rounded(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn end_remote_write_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::EndRemoteWrite(EndRemoteWriteParams {
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

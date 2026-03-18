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
use crate::model::agent::{ComponentModelElementValue, DataValue, ElementValue, ElementValues};
use crate::model::component::PluginPriority;
use crate::model::invocation_context::{SpanId, TraceId};
use crate::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginAtomicRegionParams, BeginRemoteTransactionParams, BeginRemoteWriteParams,
    CancelPendingInvocationParams, ChangePersistenceLevelParams, ChangeRetryPolicyParams,
    CommittedRemoteTransactionParams, CreateParams, CreateResourceParams, DeactivatePluginParams,
    DropResourceParams, EndAtomicRegionParams, EndRemoteWriteParams, ErrorParams, ExitedParams,
    FailedUpdateParams, FinishSpanParams, GrowMemoryParams, HostCallParams, InterruptedParams,
    JumpParams, LogParams, NoOpParams, PendingAgentInvocationParams, PendingUpdateParams,
    PreCommitRemoteTransactionParams, PreRollbackRemoteTransactionParams, RestartParams,
    RevertParams, RolledBackRemoteTransactionParams, SetSpanAttributeParams, SnapshotParams,
    StartSpanParams, SuccessfulUpdateParams, SuspendParams,
};
use crate::model::oplog::{
    AgentInitializationParameters, AgentInvocationOutputParameters,
    AgentMethodInvocationParameters, AgentResourceId, JsonSnapshotData, LogLevel, PersistenceLevel,
    PluginInstallationDescription, PublicAgentInvocation, PublicAgentInvocationResult,
    PublicAttribute, PublicAttributeValue, PublicDurableFunctionType, PublicLocalSpanData,
    PublicOplogEntry, PublicRetryConfig, PublicSnapshotData, PublicSpanData,
    PublicUpdateDescription, RawSnapshotData, SnapshotBasedUpdateParameters, StringAttributeValue,
};
use crate::model::regions::OplogRegion;
use crate::model::worker::ParsedWorkerAgentConfigEntry;
use crate::model::{
    AccountId, AgentId, ComponentId, Empty, IdempotencyKey, OplogIndex, Timestamp, TransactionId,
};
use golem_wasm::analysis::analysed_type::{
    bool, f64, field, handle, list, option, r#enum, record, result_err, result_ok, s16, s32, str,
    tuple, u64, variant,
};
use golem_wasm::analysis::{AnalysedResourceId, AnalysedResourceMode};
use golem_wasm::{IntoValueAndType, Value, ValueAndType};
use poem_openapi::types::ToJSON;
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, BTreeSet};
use test_r::test;
use uuid::Uuid;

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
        component_revision: ComponentRevision::new(1).unwrap(),
        env: vec![("x".to_string(), "y".to_string())]
            .into_iter()
            .collect(),
        created_by: AccountId::new(),
        config_vars: BTreeMap::from_iter(vec![("A".to_string(), "B".to_string())]),
        local_agent_config: vec![ParsedWorkerAgentConfigEntry {
            path: vec!["foo".to_string(), "bar".to_string()],
            value: 1.into_value_and_type(),
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
        request: ValueAndType {
            value: Value::String("test".to_string()),
            typ: str(),
        },
        response: ValueAndType {
            value: Value::List(vec![Value::U64(1)]),
            typ: list(u64()),
        },
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn host_call_with_handle_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::HostCall(HostCallParams {
        timestamp: Timestamp::now_utc().rounded(),
        function_name: "golem:rpc/wasm-rpc.{invoke-and-await}".to_string(),
        request: ValueAndType {
            value: Value::Handle {
                uri: "urn:worker:component-id/worker-name".to_string(),
                resource_id: 42,
            },
            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
        },
        response: ValueAndType {
            value: Value::Tuple(vec![Value::U64(5)]),
            typ: tuple(vec![u64()]),
        },
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
        request: ValueAndType {
            value: Value::Record(vec![
                Value::String("key".to_string()),
                Value::Option(Some(Box::new(Value::List(vec![
                    Value::U8(1),
                    Value::U8(2),
                ])))),
                Value::Result(Ok(Some(Box::new(Value::Tuple(vec![
                    Value::Bool(true),
                    Value::F64(1.23),
                ]))))),
                Value::Variant {
                    case_idx: 1,
                    case_value: Some(Box::new(Value::S32(-42))),
                },
                Value::Flags(vec![true, false, true]),
                Value::Enum(2),
            ]),
            typ: record(vec![
                field("name", str()),
                field(
                    "data",
                    option(list(golem_wasm::analysis::analysed_type::u8())),
                ),
                field("status", result_ok(tuple(vec![bool(), f64()]))),
                field(
                    "kind",
                    variant(vec![
                        golem_wasm::analysis::analysed_type::case("none", str()),
                        golem_wasm::analysis::analysed_type::case("some", s32()),
                    ]),
                ),
                field(
                    "perms",
                    golem_wasm::analysis::analysed_type::flags(&["read", "write", "exec"]),
                ),
                field("color", r#enum(&["red", "green", "blue"])),
            ]),
        },
        response: ValueAndType {
            value: Value::Result(Err(Some(Box::new(Value::String("not found".to_string()))))),
            typ: result_err(str()),
        },
        durable_function_type: PublicDurableFunctionType::ReadRemote(Empty {}),
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
            function_input: DataValue::Tuple(ElementValues {
                elements: vec![
                    ElementValue::ComponentModel(ComponentModelElementValue {
                        value: ValueAndType {
                            value: Value::String("test".to_string()),
                            typ: str(),
                        },
                    }),
                    ElementValue::ComponentModel(ComponentModelElementValue {
                        value: ValueAndType {
                            value: Value::Record(vec![Value::S16(1), Value::S16(-1)]),
                            typ: record(vec![field("x", s16()), field("y", s16())]),
                        },
                    }),
                ],
            }),
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
            output: DataValue::Tuple(ElementValues {
                elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                    value: ValueAndType {
                        value: Value::Enum(1),
                        typ: r#enum(&["red", "green", "blue"]),
                    },
                })],
            }),
        }),
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
fn change_retry_policy_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ChangeRetryPolicy(ChangeRetryPolicyParams {
        timestamp: Timestamp::now_utc().rounded(),
        new_policy: PublicRetryConfig {
            max_attempts: 10,
            min_delay: std::time::Duration::from_secs(1),
            max_delay: std::time::Duration::from_secs(10),
            multiplier: 2.0,
            max_jitter_factor: Some(0.1),
        },
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
            constructor_parameters: DataValue::Tuple(ElementValues {
                elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                    value: ValueAndType {
                        value: Value::String("test".to_string()),
                        typ: str(),
                    },
                })],
            }),
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
            constructor_parameters: DataValue::Tuple(ElementValues {
                elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                    value: ValueAndType {
                        value: Value::Tuple(vec![]),
                        typ: tuple(vec![]),
                    },
                })],
            }),
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
            function_input: DataValue::Tuple(ElementValues {
                elements: vec![
                    ElementValue::ComponentModel(ComponentModelElementValue {
                        value: ValueAndType {
                            value: Value::String("test".to_string()),
                            typ: str(),
                        },
                    }),
                    ElementValue::ComponentModel(ComponentModelElementValue {
                        value: ValueAndType {
                            value: Value::Record(vec![Value::S16(1), Value::S16(-1)]),
                            typ: record(vec![field("x", s16()), field("y", s16())]),
                        },
                    }),
                ],
            }),
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
fn oplog_entry_type_matches_wit() {
    use crate::model::oplog::OplogEntry;
    use golem_wasm::analysis::wit_parser::{AnalysedTypeResolve, TypeName, TypeOwner};
    use golem_wasm::IntoValue;
    use std::path::PathBuf;

    let wit_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit");
    let mut resolver =
        AnalysedTypeResolve::from_wit_directory(&wit_dir).expect("Failed to parse WIT");

    let wit_type = resolver
        .analysed_type(&TypeName {
            package: Some("golem:api@1.5.0".to_string()),
            owner: TypeOwner::Interface("oplog".to_string()),
            name: Some("oplog-entry".to_string()),
        })
        .expect("Failed to find oplog-entry type in WIT");

    let rust_type = OplogEntry::get_type();

    assert_eq!(
        rust_type, wit_type,
        "OplogEntry::get_type() does not match the WIT oplog-entry definition"
    );
}

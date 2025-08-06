// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use test_r::test;

use crate::model::public_oplog::{
    ChangeRetryPolicyParameters, CreateParameters, DescribeResourceParameters, EndRegionParameters,
    ErrorParameters, ExportedFunctionCompletedParameters, ExportedFunctionInvokedParameters,
    ExportedFunctionParameters, FailedUpdateParameters, GrowMemoryParameters,
    ImportedFunctionInvokedParameters, JumpParameters, LogParameters, PendingUpdateParameters,
    PendingWorkerInvocationParameters, PluginInstallationDescription, PublicAttribute,
    PublicAttributeValue, PublicDurableFunctionType, PublicLocalSpanData, PublicOplogEntry,
    PublicRetryConfig, PublicSpanData, PublicUpdateDescription, PublicWorkerInvocation,
    ResourceParameters, SnapshotBasedUpdateParameters, StringAttributeValue,
    SuccessfulUpdateParameters, TimestampParameter,
};
use crate::model::{
    AccountId, ComponentId, Empty, IdempotencyKey, PluginInstallationId, ProjectId, Timestamp,
    WorkerId,
};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use crate::model::invocation_context::{SpanId, TraceId};
use crate::model::oplog::{LogLevel, OplogIndex, WorkerResourceId};
use crate::model::regions::OplogRegion;
use golem_wasm_ast::analysis::analysed_type::{field, list, r#enum, record, s16, str, u64};
use golem_wasm_rpc::{Value, ValueAndType};
#[cfg(feature = "poem")]
use poem_openapi::types::ToJSON;

fn rounded_ts(ts: Timestamp) -> Timestamp {
    Timestamp::from(ts.to_millis())
}

#[test]
#[cfg(feature = "poem")]
fn create_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Create(CreateParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        worker_id: WorkerId {
            component_id: ComponentId(
                Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
            ),
            worker_name: "test1".to_string(),
        },
        component_version: 1,
        args: vec!["a".to_string(), "b".to_string()],
        env: vec![("x".to_string(), "y".to_string())]
            .into_iter()
            .collect(),
        created_by: AccountId {
            value: "account_id".to_string(),
        },
        wasi_config_vars: BTreeMap::from_iter(vec![("A".to_string(), "B".to_string())]).into(),
        project_id: ProjectId::new_v4(),
        parent: Some(WorkerId {
            component_id: ComponentId(
                Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
            ),
            worker_name: "test2".to_string(),
        }),
        component_size: 100_000_000,
        initial_total_linear_memory_size: 200_000_000,
        initial_active_plugins: BTreeSet::from_iter(vec![PluginInstallationDescription {
            installation_id: PluginInstallationId(
                Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
            ),
            plugin_name: "plugin1".to_string(),
            plugin_version: "1".to_string(),
            parameters: BTreeMap::new(),
            registered: true,
        }]),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn imported_function_invoked_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ImportedFunctionInvoked(ImportedFunctionInvokedParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
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
#[cfg(feature = "poem")]
fn exported_function_invoked_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ExportedFunctionInvoked(ExportedFunctionInvokedParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        function_name: "test".to_string(),
        request: vec![
            ValueAndType {
                value: Value::String("test".to_string()),
                typ: str(),
            },
            ValueAndType {
                value: Value::Record(vec![Value::S16(1), Value::S16(-1)]),
                typ: record(vec![field("x", s16()), field("y", s16())]),
            },
        ],
        idempotency_key: IdempotencyKey::new("idempotency_key".to_string()),
        trace_id: TraceId::generate(),
        trace_states: vec!["a".to_string(), "b".to_string()],
        invocation_context: vec![vec![PublicSpanData::LocalSpan(PublicLocalSpanData {
            span_id: SpanId::generate(),
            start: rounded_ts(Timestamp::now_utc()),
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
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn exported_function_completed_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        response: Some(ValueAndType {
            value: Value::Enum(1),
            typ: r#enum(&["red", "green", "blue"]),
        }),
        consumed_fuel: 100,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn suspend_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Suspend(TimestampParameter {
        timestamp: rounded_ts(Timestamp::now_utc()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn error_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Error(ErrorParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        error: "test".to_string(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn no_op_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::NoOp(TimestampParameter {
        timestamp: rounded_ts(Timestamp::now_utc()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn jump_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Jump(JumpParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
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
#[cfg(feature = "poem")]
fn interrupted_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Interrupted(TimestampParameter {
        timestamp: rounded_ts(Timestamp::now_utc()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn exited_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Exited(TimestampParameter {
        timestamp: rounded_ts(Timestamp::now_utc()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn change_retry_policy_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ChangeRetryPolicy(ChangeRetryPolicyParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
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
#[cfg(feature = "poem")]
fn begin_atomic_region_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::BeginAtomicRegion(TimestampParameter {
        timestamp: rounded_ts(Timestamp::now_utc()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn end_atomic_region_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::EndAtomicRegion(EndRegionParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        begin_index: OplogIndex::from_u64(1),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn begin_remote_write_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::BeginRemoteWrite(TimestampParameter {
        timestamp: rounded_ts(Timestamp::now_utc()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn end_remote_write_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::EndRemoteWrite(EndRegionParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        begin_index: OplogIndex::from_u64(1),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn pending_worker_invocation_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::PendingWorkerInvocation(PendingWorkerInvocationParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        invocation: PublicWorkerInvocation::ExportedFunction(ExportedFunctionParameters {
            idempotency_key: IdempotencyKey::new("idempotency_key".to_string()),
            full_function_name: "test".to_string(),
            function_input: Some(vec![
                ValueAndType {
                    value: Value::String("test".to_string()),
                    typ: str(),
                },
                ValueAndType {
                    value: Value::Record(vec![Value::S16(1), Value::S16(-1)]),
                    typ: record(vec![field("x", s16()), field("y", s16())]),
                },
            ]),
            trace_id: TraceId::generate(),
            trace_states: vec!["a".to_string(), "b".to_string()],
            invocation_context: vec![vec![PublicSpanData::LocalSpan(PublicLocalSpanData {
                span_id: SpanId::generate(),
                start: rounded_ts(Timestamp::now_utc()),
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
#[cfg(feature = "poem")]
fn pending_update_serialization_poem_serde_equivalence_1() {
    let entry = PublicOplogEntry::PendingUpdate(PendingUpdateParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        target_version: 1,
        description: PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
            payload: "test".as_bytes().to_vec(),
        }),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn pending_update_serialization_poem_serde_equivalence_2() {
    let entry = PublicOplogEntry::PendingUpdate(PendingUpdateParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        target_version: 1,
        description: PublicUpdateDescription::Automatic(Empty {}),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn successful_update_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        target_version: 1,
        new_component_size: 100_000_000,
        new_active_plugins: BTreeSet::from_iter(vec![PluginInstallationDescription {
            installation_id: PluginInstallationId(
                Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
            ),
            plugin_name: "plugin1".to_string(),
            plugin_version: "1".to_string(),
            parameters: BTreeMap::new(),
            registered: true,
        }]),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn failed_update_serialization_poem_serde_equivalence_1() {
    let entry = PublicOplogEntry::FailedUpdate(FailedUpdateParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        target_version: 1,
        details: Some("test".to_string()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn failed_update_serialization_poem_serde_equivalence_2() {
    let entry = PublicOplogEntry::FailedUpdate(FailedUpdateParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        target_version: 1,
        details: None,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn grow_memory_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::GrowMemory(GrowMemoryParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        delta: 100_000_000,
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn create_resource_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::CreateResource(ResourceParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        id: WorkerResourceId(100),
        name: "test".to_string(),
        owner: "owner".to_string(),
    });

    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn drop_resource_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::DropResource(ResourceParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        id: WorkerResourceId(100),
        name: "test".to_string(),
        owner: "owner".to_string(),
    });

    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn describe_resource_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::DescribeResource(DescribeResourceParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        id: WorkerResourceId(100),
        resource_name: "test".to_string(),
        resource_params: vec![
            ValueAndType {
                value: Value::String("test".to_string()),
                typ: str(),
            },
            ValueAndType {
                value: Value::Record(vec![Value::S16(1), Value::S16(-1)]),
                typ: record(vec![field("x", s16()), field("y", s16())]),
            },
        ],
        resource_owner: "owner".to_string(),
    });

    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn log_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Log(LogParameters {
        timestamp: rounded_ts(Timestamp::now_utc()),
        level: LogLevel::Stderr,
        context: "test".to_string(),
        message: "test".to_string(),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
#[cfg(feature = "poem")]
fn restart_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::Restart(TimestampParameter {
        timestamp: rounded_ts(Timestamp::now_utc()),
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

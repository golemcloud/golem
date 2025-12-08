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

use crate::model::invocation_context::{SpanId, TraceId};
use crate::model::oplog::public_oplog_entry::{
    BeginAtomicRegionParams, BeginRemoteWriteParams, ChangeRetryPolicyParams, CreateParams,
    CreateResourceParams, DropResourceParams, EndAtomicRegionParams, EndRemoteWriteParams,
    ErrorParams, ExitedParams, ExportedFunctionCompletedParams, ExportedFunctionInvokedParams,
    FailedUpdateParams, GrowMemoryParams, ImportedFunctionInvokedParams, InterruptedParams,
    JumpParams, LogParams, NoOpParams, PendingUpdateParams, PendingWorkerInvocationParams,
    RestartParams, SuccessfulUpdateParams, SuspendParams,
};
use crate::model::oplog::{
    ExportedFunctionParameters, LogLevel, PluginInstallationDescription, PublicAttribute,
    PublicAttributeValue, PublicDurableFunctionType, PublicLocalSpanData, PublicOplogEntry,
    PublicRetryConfig, PublicSpanData, PublicUpdateDescription, PublicWorkerInvocation,
    SnapshotBasedUpdateParameters, StringAttributeValue, WorkerResourceId,
};
use crate::model::regions::OplogRegion;
use crate::model::{
    AccountId, ComponentId, Empty, IdempotencyKey, OplogIndex, PluginPriority, Timestamp, WorkerId,
};
use golem_wasm::analysis::analysed_type::{field, list, r#enum, record, s16, str, u64};
use golem_wasm::{Value, ValueAndType};
use poem_openapi::types::ToJSON;
use std::collections::{BTreeMap, BTreeSet};
use test_r::test;
use uuid::Uuid;

#[test]
fn create_serialization_poem_serde_equivalence() {
    use crate::model::component::ComponentRevision;
    use crate::model::environment::EnvironmentId;

    let entry = PublicOplogEntry::Create(CreateParams {
        timestamp: Timestamp::now_utc().rounded(),
        worker_id: WorkerId {
            component_id: ComponentId(
                Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
            ),
            worker_name: "test1".to_string(),
        },
        component_revision: ComponentRevision(1),
        env: vec![("x".to_string(), "y".to_string())]
            .into_iter()
            .collect(),
        created_by: AccountId::new(),
        wasi_config_vars: BTreeMap::from_iter(vec![("A".to_string(), "B".to_string())]).into(),
        environment_id: EnvironmentId::new(),
        parent: Some(WorkerId {
            component_id: ComponentId(
                Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
            ),
            worker_name: "test2".to_string(),
        }),
        component_size: 100_000_000,
        initial_total_linear_memory_size: 200_000_000,
        initial_active_plugins: BTreeSet::from_iter(vec![PluginInstallationDescription {
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
fn imported_function_invoked_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ImportedFunctionInvoked(ImportedFunctionInvokedParams {
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
fn exported_function_invoked_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ExportedFunctionInvoked(ExportedFunctionInvokedParams {
        timestamp: Timestamp::now_utc().rounded(),
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
    });
    let serialized = entry.to_json_string();
    let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(entry, deserialized);
}

#[test]
fn exported_function_completed_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParams {
        timestamp: Timestamp::now_utc().rounded(),
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
fn pending_worker_invocation_serialization_poem_serde_equivalence() {
    let entry = PublicOplogEntry::PendingWorkerInvocation(PendingWorkerInvocationParams {
        timestamp: Timestamp::now_utc().rounded(),
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
        target_revision: ComponentRevision(1),
        description: PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
            payload: "test".as_bytes().to_vec(),
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
        target_revision: ComponentRevision(1),
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
        target_revision: ComponentRevision(1),
        new_component_size: 100_000_000,
        new_active_plugins: BTreeSet::from_iter(vec![PluginInstallationDescription {
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
        target_revision: ComponentRevision(1),
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
        target_revision: ComponentRevision(1),
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
        id: WorkerResourceId(100),
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
        id: WorkerResourceId(100),
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

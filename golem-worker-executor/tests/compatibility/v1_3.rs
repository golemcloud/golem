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

use crate::compatibility::v1::{backward_compatible, backward_compatible_custom};
use goldenfile::Mint;
use golem_common::base_model::{
    ComponentId, OplogIndex, PluginInstallationId, ProjectId, PromiseId, WorkerId,
};
use golem_common::model::agent::{DataValue, ElementValue, ElementValues};
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextStack, SpanId, TraceId,
};
use golem_common::model::oplog::{
    DurableFunctionType, LogLevel, OplogEntry, OplogPayload, SpanData, UpdateDescription,
    WorkerError, WorkerResourceId,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    AccountId, AgentInstanceDescription, ExportedResourceInstanceDescription, IdempotencyKey,
    OwnedWorkerId, RetryConfig, ScheduledAction, Timestamp, WorkerInvocation,
    WorkerResourceDescription,
};
use golem_common::serialization::deserialize;
use golem_wasm_rpc::wasmtime::ResourceTypeId;
use golem_wasm_rpc::{IntoValueAndType, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Duration;
use test_r::test;
use uuid::Uuid;

#[test]
pub fn scheduled_action() {
    let sa1 = ScheduledAction::CompletePromise {
        account_id: AccountId {
            value: "account_id".to_string(),
        },
        project_id: ProjectId(Uuid::parse_str("296aa41a-ff44-4882-8f34-08b7fe431aa4").unwrap()),
        promise_id: PromiseId {
            worker_id: WorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("4B29BF7C-13F6-4E37-AC03-830B81EAD478").unwrap(),
                ),
                worker_name: "worker_name".to_string(),
            },
            oplog_idx: OplogIndex::from_u64(100),
        },
    };
    let sa2 = ScheduledAction::ArchiveOplog {
        account_id: AccountId {
            value: "account_id".to_string(),
        },
        owned_worker_id: OwnedWorkerId {
            project_id: ProjectId(Uuid::parse_str("296aa41a-ff44-4882-8f34-08b7fe431aa4").unwrap()),
            worker_id: WorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("4B29BF7C-13F6-4E37-AC03-830B81EAD478").unwrap(),
                ),
                worker_name: "worker_name".to_string(),
            },
        },
        last_oplog_index: OplogIndex::from_u64(100),
        next_after: Duration::from_secs(10),
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("scheduled_action_complete_promise", &mut mint, sa1);
    backward_compatible("scheduled_action_archive_oplog", &mut mint, sa2);
}

#[test]
pub fn oplog_entry() {
    // Special differ ignoring the invocation_context field
    fn is_deserializable_ignoring_invocation_context(old: &Path, new: &Path) {
        let old = std::fs::read(old).unwrap();
        let new = std::fs::read(new).unwrap();

        // Both the old and the latest binary can be deserialized
        let mut old_decoded: OplogEntry = deserialize(&old).unwrap();
        let new_decoded: OplogEntry = deserialize(&new).unwrap();

        if let (
            OplogEntry::PendingWorkerInvocation {
                invocation:
                    WorkerInvocation::ExportedFunction {
                        invocation_context: old,
                        ..
                    },
                ..
            },
            OplogEntry::PendingWorkerInvocation {
                invocation:
                    WorkerInvocation::ExportedFunction {
                        invocation_context: new,
                        ..
                    },
                ..
            },
        ) = (&mut old_decoded, &new_decoded)
        {
            *old = new.clone();
        }

        // And they represent the same value
        assert_eq!(old_decoded, new_decoded);
    }

    let oe4 = OplogEntry::ExportedFunctionCompleted {
        timestamp: Timestamp::from(1724701938466),
        response: OplogPayload::Inline(vec![0, 1, 2, 3, 4]),
        consumed_fuel: 12345678910,
    };

    let oe5 = OplogEntry::Suspend {
        timestamp: Timestamp::from(1724701938466),
    };

    let oe6 = OplogEntry::Error {
        timestamp: Timestamp::from(1724701938466),
        error: WorkerError::OutOfMemory,
    };

    let oe7 = OplogEntry::NoOp {
        timestamp: Timestamp::from(1724701938466),
    };

    let oe8 = OplogEntry::Jump {
        timestamp: Timestamp::from(1724701938466),
        jump: OplogRegion {
            start: OplogIndex::from_u64(0),
            end: OplogIndex::from_u64(10),
        },
    };

    let oe9 = OplogEntry::Interrupted {
        timestamp: Timestamp::from(1724701938466),
    };

    let oe10 = OplogEntry::Exited {
        timestamp: Timestamp::from(1724701938466),
    };

    let oe11 = OplogEntry::ChangeRetryPolicy {
        timestamp: Timestamp::from(1724701938466),
        new_policy: RetryConfig::default(),
    };

    let oe12 = OplogEntry::BeginAtomicRegion {
        timestamp: Timestamp::from(1724701938466),
    };

    let oe13 = OplogEntry::EndAtomicRegion {
        timestamp: Timestamp::from(1724701938466),
        begin_index: OplogIndex::from_u64(0),
    };

    let oe14 = OplogEntry::BeginRemoteWrite {
        timestamp: Timestamp::from(1724701938466),
    };

    let oe15 = OplogEntry::EndRemoteWrite {
        timestamp: Timestamp::from(1724701938466),
        begin_index: OplogIndex::from_u64(0),
    };

    let oe16 = OplogEntry::PendingWorkerInvocation {
        timestamp: Timestamp::from(1724701938466),
        invocation: WorkerInvocation::ExportedFunction {
            idempotency_key: IdempotencyKey {
                value: "idempotency_key".to_string(),
            },
            full_function_name: "function-name".to_string(),
            function_input: vec![Value::Bool(true)],
            invocation_context: InvocationContextStack::fresh(),
        },
    };

    let oe17 = OplogEntry::PendingUpdate {
        timestamp: Timestamp::from(1724701938466),
        description: UpdateDescription::Automatic {
            target_version: 100,
        },
    };

    let oe19a = OplogEntry::FailedUpdate {
        timestamp: Timestamp::from(1724701938466),
        target_version: 10,
        details: None,
    };

    let oe19b = OplogEntry::FailedUpdate {
        timestamp: Timestamp::from(1724701938466),
        target_version: 10,
        details: Some("details".to_string()),
    };

    let oe20 = OplogEntry::GrowMemory {
        timestamp: Timestamp::from(1724701938466),
        delta: 100_000_000,
    };

    let oe21 = OplogEntry::CreateResource {
        timestamp: Timestamp::from(1724701938466),
        id: WorkerResourceId(1),
        resource_type_id: ResourceTypeId {
            name: "name".to_string(),
            owner: "owner".to_string(),
        },
    };

    let oe22 = OplogEntry::DropResource {
        timestamp: Timestamp::from(1724701938466),
        id: WorkerResourceId(1),
        resource_type_id: ResourceTypeId {
            name: "name".to_string(),
            owner: "owner".to_string(),
        },
    };

    let oe23 = OplogEntry::DescribeResource {
        timestamp: Timestamp::from(1724701938466),
        id: WorkerResourceId(1),
        resource_type_id: ResourceTypeId {
            name: "name".to_string(),
            owner: "owner".to_string(),
        },
        indexed_resource_parameters: vec!["a".to_string(), "b".to_string()],
    };

    let oe24 = OplogEntry::Log {
        timestamp: Timestamp::from(1724701938466),
        level: LogLevel::Error,
        context: "context".to_string(),
        message: "message".to_string(),
    };

    let oe25 = OplogEntry::Restart {
        timestamp: Timestamp::from(1724701938466),
    };
    let oe26 = OplogEntry::ImportedFunctionInvoked {
        timestamp: Timestamp::from(1724701938466),
        function_name: "test:pkg/iface.{fn}".to_string(),
        request: OplogPayload::Inline(vec![5, 6, 7, 8, 9]),
        response: OplogPayload::Inline(vec![0, 1, 2, 3, 4]),
        durable_function_type: DurableFunctionType::ReadLocal,
    };
    let oe27a = OplogEntry::Create {
        timestamp: Timestamp::from(1724701938466),
        worker_id: WorkerId {
            component_id: ComponentId(
                Uuid::parse_str("4B29BF7C-13F6-4E37-AC03-830B81EAD478").unwrap(),
            ),
            worker_name: "worker_name".to_string(),
        },
        component_version: 0,
        args: vec!["hello".to_string(), "world".to_string()],
        env: vec![
            ("key1".to_string(), "value1".to_string()),
            ("key2".to_string(), "value2".to_string()),
        ],
        wasi_config_vars: BTreeMap::new(),
        project_id: ProjectId(Uuid::parse_str("296aa41a-ff44-4882-8f34-08b7fe431aa4").unwrap()),
        created_by: AccountId {
            value: "account_id".to_string(),
        },
        parent: None,
        component_size: 100_000_000,
        initial_total_linear_memory_size: 100_000_000,
        initial_active_plugins: HashSet::from_iter(vec![
            PluginInstallationId(Uuid::parse_str("E7AA7893-B8F8-4DC7-B3AC-3A9E3472EA18").unwrap()),
            PluginInstallationId(Uuid::parse_str("339ED9E3-9D93-440C-BC07-377F56642ABB").unwrap()),
        ]),
    };
    let oe27b = OplogEntry::Create {
        timestamp: Timestamp::from(1724701938466),
        worker_id: WorkerId {
            component_id: ComponentId(
                Uuid::parse_str("4B29BF7C-13F6-4E37-AC03-830B81EAD478").unwrap(),
            ),
            worker_name: "worker_name".to_string(),
        },
        component_version: 0,
        args: vec!["hello".to_string(), "world".to_string()],
        env: vec![
            ("key1".to_string(), "value1".to_string()),
            ("key2".to_string(), "value2".to_string()),
        ],
        wasi_config_vars: BTreeMap::from([
            ("ckey1".to_string(), "cvalue1".to_string()),
            ("ckey2".to_string(), "cvalue2".to_string()),
        ]),
        project_id: ProjectId(Uuid::parse_str("296aa41a-ff44-4882-8f34-08b7fe431aa4").unwrap()),
        created_by: AccountId {
            value: "account_id".to_string(),
        },
        parent: Some(WorkerId {
            component_id: ComponentId(
                Uuid::parse_str("90BB3957-2C4E-4711-A488-902B7018100F").unwrap(),
            ),
            worker_name: "parent_worker_name".to_string(),
        }),
        component_size: 100_000_000,
        initial_total_linear_memory_size: 100_000_000,
        initial_active_plugins: HashSet::from_iter(vec![
            PluginInstallationId(Uuid::parse_str("E7AA7893-B8F8-4DC7-B3AC-3A9E3472EA18").unwrap()),
            PluginInstallationId(Uuid::parse_str("339ED9E3-9D93-440C-BC07-377F56642ABB").unwrap()),
        ]),
    };
    let oe28 = OplogEntry::SuccessfulUpdate {
        timestamp: Timestamp::from(1724701938466),
        target_version: 10,
        new_component_size: 1234,
        new_active_plugins: HashSet::from_iter(vec![
            PluginInstallationId(Uuid::parse_str("E7AA7893-B8F8-4DC7-B3AC-3A9E3472EA18").unwrap()),
            PluginInstallationId(Uuid::parse_str("339ED9E3-9D93-440C-BC07-377F56642ABB").unwrap()),
        ]),
    };
    let oe29 = OplogEntry::ActivatePlugin {
        timestamp: Timestamp::from(1724701938466),
        plugin: PluginInstallationId(
            Uuid::parse_str("E7AA7893-B8F8-4DC7-B3AC-3A9E3472EA18").unwrap(),
        ),
    };
    let oe30 = OplogEntry::DeactivatePlugin {
        timestamp: Timestamp::from(1724701938466),
        plugin: PluginInstallationId(
            Uuid::parse_str("E7AA7893-B8F8-4DC7-B3AC-3A9E3472EA18").unwrap(),
        ),
    };
    let oe31 = OplogEntry::Revert {
        timestamp: Timestamp::from(1724701938466),
        dropped_region: OplogRegion {
            start: OplogIndex::from_u64(3),
            end: OplogIndex::from_u64(10),
        },
    };

    let oe32 = OplogEntry::CancelPendingInvocation {
        timestamp: Timestamp::from(1724701938466),
        idempotency_key: IdempotencyKey {
            value: "idempotency_key".to_string(),
        },
    };

    let oe33 = OplogEntry::ExportedFunctionInvoked {
        timestamp: Timestamp::from(1724701938466),
        function_name: "test:pkg/iface.{fn}".to_string(),
        request: OplogPayload::Inline(vec![0, 1, 2, 3, 4]),
        idempotency_key: IdempotencyKey {
            value: "id1".to_string(),
        },
        trace_id: TraceId::from_string("4bf92f3577b34da6a3ce929d0e0e4736").unwrap(),
        trace_states: vec!["a=1".to_string(), "b=2".to_string()],
        invocation_context: vec![
            SpanData::LocalSpan {
                span_id: SpanId::from_string("cddd89c618fb7bf3").unwrap(),
                start: Timestamp::from(1724701938466),
                parent_id: Some(SpanId::from_string("00f067aa0ba902b7").unwrap()),
                linked_context: Some(vec![SpanData::LocalSpan {
                    span_id: SpanId::from_string("d0fa4a9110f2dcab").unwrap(),
                    start: Timestamp::from(1724701938466),
                    parent_id: None,
                    linked_context: None,
                    attributes: HashMap::new(),
                    inherited: true,
                }]),
                attributes: HashMap::from_iter(vec![(
                    "key".to_string(),
                    AttributeValue::String("value".to_string()),
                )]),
                inherited: false,
            },
            SpanData::ExternalSpan {
                span_id: SpanId::from_string("00f067aa0ba902b7").unwrap(),
            },
        ],
    };

    let oe34 = OplogEntry::StartSpan {
        timestamp: Timestamp::from(1724701938466),
        span_id: SpanId::from_string("cddd89c618fb7bf3").unwrap(),
        parent_id: Some(SpanId::from_string("00f067aa0ba902b7").unwrap()),
        linked_context_id: Some(SpanId::from_string("d0fa4a9110f2dcab").unwrap()),
        attributes: HashMap::from_iter(vec![(
            "key".to_string(),
            AttributeValue::String("value".to_string()),
        )]),
    };

    let oe35 = OplogEntry::FinishSpan {
        timestamp: Timestamp::from(1724701938466),
        span_id: SpanId::from_string("cddd89c618fb7bf3").unwrap(),
    };

    let oe36 = OplogEntry::SetSpanAttribute {
        timestamp: Timestamp::from(1724701938466),
        span_id: SpanId::from_string("cddd89c618fb7bf3").unwrap(),
        key: "key".to_string(),
        value: AttributeValue::String("value".to_string()),
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("oplog_entry_exported_function_completed", &mut mint, oe4);
    backward_compatible("oplog_entry_suspend", &mut mint, oe5);
    backward_compatible("oplog_entry_error", &mut mint, oe6);
    backward_compatible("oplog_entry_no_op", &mut mint, oe7);
    backward_compatible("oplog_entry_jump", &mut mint, oe8);
    backward_compatible("oplog_entry_interrupted", &mut mint, oe9);
    backward_compatible("oplog_entry_exited", &mut mint, oe10);
    backward_compatible("oplog_entry_change_retry_policy", &mut mint, oe11);
    backward_compatible("oplog_entry_begin_atomic_region", &mut mint, oe12);
    backward_compatible("oplog_entry_end_atomic_region", &mut mint, oe13);
    backward_compatible("oplog_entry_begin_remote_write", &mut mint, oe14);
    backward_compatible("oplog_entry_end_remote_write", &mut mint, oe15);
    backward_compatible_custom(
        "oplog_entry_pending_worker_invocation",
        &mut mint,
        oe16,
        Box::new(is_deserializable_ignoring_invocation_context),
    );
    backward_compatible("oplog_entry_pending_update", &mut mint, oe17);
    backward_compatible("oplog_entry_failed_update_no_details", &mut mint, oe19a);
    backward_compatible("oplog_entry_failed_update_with_details", &mut mint, oe19b);
    backward_compatible("oplog_entry_grow_memory", &mut mint, oe20);
    backward_compatible("oplog_entry_create_resource", &mut mint, oe21);
    backward_compatible("oplog_entry_drop_resource", &mut mint, oe22);
    backward_compatible("oplog_entry_describe_resource", &mut mint, oe23);
    backward_compatible("oplog_entry_log", &mut mint, oe24);
    backward_compatible("oplog_entry_restart", &mut mint, oe25);
    backward_compatible("oplog_entry_import_function_invoked", &mut mint, oe26);
    backward_compatible("oplog_entry_createa", &mut mint, oe27a);
    backward_compatible("oplog_entry_createb", &mut mint, oe27b);
    backward_compatible("oplog_entry_successful_update", &mut mint, oe28);
    backward_compatible("oplog_entry_activate_plugin", &mut mint, oe29);
    backward_compatible("oplog_entry_deactivate_plugin", &mut mint, oe30);
    backward_compatible("oplog_entry_revert", &mut mint, oe31);
    backward_compatible("oplog_entry_cancel_pending_invocation", &mut mint, oe32);
    backward_compatible("oplog_entry_exported_function_invoked_v12", &mut mint, oe33);
    backward_compatible("oplog_entry_start_span", &mut mint, oe34);
    backward_compatible("oplog_entry_finish_span", &mut mint, oe35);
    backward_compatible("oplog_entry_set_span_attribute", &mut mint, oe36);
}

#[test]
pub fn worker_resource_description() {
    let wrd1 =
        WorkerResourceDescription::ExportedResourceInstance(ExportedResourceInstanceDescription {
            created_at: Timestamp::from(1724701938466),
            resource_owner: "owner".to_string(),
            resource_name: "name".to_string(),
            resource_params: None,
        });
    let wrd2 =
        WorkerResourceDescription::ExportedResourceInstance(ExportedResourceInstanceDescription {
            created_at: Timestamp::from(1724701938466),
            resource_owner: "rpc:counters-export/api".to_string(),
            resource_name: "counter".to_string(),
            resource_params: Some(vec!["a".to_string(), "b".to_string()]),
        });
    let wrd3 = WorkerResourceDescription::AgentInstance(AgentInstanceDescription {
        created_at: Timestamp::from(1724701938466),
        agent_parameters: DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel("a".into_value_and_type()),
                ElementValue::ComponentModel(10.into_value_and_type()),
            ],
        }),
    });
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("worker_resource_description", &mut mint, wrd1);
    backward_compatible("worker_resource_description_indexed", &mut mint, wrd2);
    backward_compatible("worker_resource_description_agent", &mut mint, wrd3);
}

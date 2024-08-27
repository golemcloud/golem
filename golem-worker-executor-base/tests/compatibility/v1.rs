// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! This module contains golden tests ensuring that worker related serialized information
//! (such as oplog entries, promises, scheduling, etc.) created by Golem OSS 1.0.0 can be deserialized.
//! Do not regenerate the golden test binaries unless backward compatibility with 1.0 is dropped.
//!
//! The tests are assuming composability of the serializer implementation, so if a given type A has a field of type B,
//! the test for A only contains an example value of B but there exists a separate test that tests the serialization of B.

use bincode::{Decode, Encode};
use goldenfile::Mint;
use golem_common::config::RetryConfig;
use golem_common::model::oplog::{
    IndexedResourceKey, LogLevel, OplogEntry, OplogIndex, OplogPayload, PayloadId,
    TimestampedUpdateDescription, UpdateDescription, WorkerError, WorkerResourceId,
    WrappedFunctionType,
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AccountId, ComponentId, FailedUpdateRecord, IdempotencyKey, OwnedWorkerId, PromiseId,
    ScheduledAction, SuccessfulUpdateRecord, Timestamp, TimestampedWorkerInvocation, WorkerId,
    WorkerInvocation, WorkerResourceDescription, WorkerStatus, WorkerStatusRecord,
};
use golem_common::serialization::{deserialize, serialize};
use golem_wasm_rpc::{Uri, Value, WitValue};
use golem_worker_executor_base::durable_host::http::serialized::{
    SerializableErrorCode, SerializableResponse, SerializableResponseHeaders,
};
use golem_worker_executor_base::durable_host::serialized::{
    SerializableError, SerializableIpAddress, SerializableIpAddresses, SerializableStreamError,
};
use golem_worker_executor_base::durable_host::wasm_rpc::serialized::SerializableInvokeResult;
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::model::InterruptKind;
use golem_worker_executor_base::services::blob_store;
use golem_worker_executor_base::services::promise::RedisPromiseState;
use golem_worker_executor_base::services::rpc::RpcError;
use golem_worker_executor_base::services::worker_proxy::WorkerProxyError;
use std::collections::HashMap;
use std::fmt::Debug;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

fn is_deserializable<T: Encode + Decode + PartialEq + Debug>(old: &Path, new: &Path) {
    let old = std::fs::read(old).unwrap();
    let new = std::fs::read(new).unwrap();

    // Both the old and the latest binary can be deserialized
    let old_decoded: T = deserialize(&old).unwrap();
    let new_decoded: T = deserialize(&new).unwrap();

    // And they represent the same value
    assert_eq!(old_decoded, new_decoded);
}

fn backward_compatible<T: Encode + Decode + PartialEq + Debug + 'static>(
    name: impl AsRef<str>,
    mint: &mut Mint,
    value: T,
) {
    let mut file = mint
        .new_goldenfile_with_differ(
            format!("{}.bin", name.as_ref()),
            Box::new(is_deserializable::<T>),
        )
        .unwrap();
    let encoded = serialize(&value).unwrap();
    file.write_all(&encoded).unwrap();
    file.flush().unwrap();
}

fn is_deserializable_wit_value(old: &Path, new: &Path) {
    let old = std::fs::read(old).unwrap();
    let new = std::fs::read(new).unwrap();

    // Both the old and the latest binary can be deserialized
    let old_decoded: WitValue = deserialize(&old).unwrap();
    let new_decoded: WitValue = deserialize(&new).unwrap();

    let old_value: Value = old_decoded.into();
    let new_value: Value = new_decoded.into();

    // And they represent the same value
    assert_eq!(old_value, new_value);
}

/// Special case for WitValue which does not implement PartialEq at the moment, but can be converted
/// to Value for comparison.
fn backward_compatible_wit_value(name: impl AsRef<str>, mint: &mut Mint, value: WitValue) {
    let mut file = mint
        .new_goldenfile_with_differ(
            format!("{}.bin", name.as_ref()),
            Box::new(is_deserializable_wit_value),
        )
        .unwrap();
    let encoded = serialize(&value).unwrap();
    file.write_all(&encoded).unwrap();
    file.flush().unwrap();
}

#[test]
pub fn worker_status() {
    let ws1 = WorkerStatus::Running;
    let ws2 = WorkerStatus::Idle;
    let ws3 = WorkerStatus::Suspended;
    let ws4 = WorkerStatus::Interrupted;
    let ws5 = WorkerStatus::Retrying;
    let ws6 = WorkerStatus::Failed;
    let ws7 = WorkerStatus::Exited;

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("worker_status_running", &mut mint, ws1);
    backward_compatible("worker_status_idle", &mut mint, ws2);
    backward_compatible("worker_status_suspended", &mut mint, ws3);
    backward_compatible("worker_status_interrupted", &mut mint, ws4);
    backward_compatible("worker_status_retrying", &mut mint, ws5);
    backward_compatible("worker_status_failed", &mut mint, ws6);
    backward_compatible("worker_status_exited", &mut mint, ws7);
}

#[test]
pub fn deleted_regions() {
    let dr1 = DeletedRegions::new();
    let dr2 = DeletedRegions::from_regions(vec![
        OplogRegion {
            start: OplogIndex::from_u64(0),
            end: OplogIndex::from_u64(10),
        },
        OplogRegion {
            start: OplogIndex::from_u64(20),
            end: OplogIndex::from_u64(30),
        },
    ]);

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("deleted_regions_empty", &mut mint, dr1);
    backward_compatible("deleted_regions_nonempty", &mut mint, dr2);
}

#[test]
pub fn retry_config() {
    let rc1 = RetryConfig::default();
    let rc2 = RetryConfig {
        max_attempts: 10,
        min_delay: Duration::from_millis(10),
        max_delay: Duration::from_secs(10),
        multiplier: 1.2,
        max_jitter_factor: None,
    };
    let rc3 = RetryConfig {
        max_attempts: 10,
        min_delay: Duration::from_millis(10),
        max_delay: Duration::from_secs(10),
        multiplier: 1.2,
        max_jitter_factor: Some(0.1),
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("retry_config_default", &mut mint, rc1);
    backward_compatible("retry_config_custom1", &mut mint, rc2);
    backward_compatible("retry_config_custom2", &mut mint, rc3);
}

#[test]
pub fn wasm_rpc_value() {
    let v1 = Value::Bool(true);
    let v2 = Value::U8(1);
    let v3 = Value::U16(12345);
    let v4 = Value::U32(123456789);
    let v5 = Value::U64(12345678901234567890);
    let v6 = Value::S8(-1);
    let v7 = Value::S16(-12345);
    let v8 = Value::S32(-123456789);
    let v9 = Value::S64(-1234567890123456789);
    let v10 = Value::F32(1.234);
    let v11 = Value::F64(1.234567890123456789);
    let v12 = Value::Char('a');
    let v13 = Value::String("hello world".to_string());
    let v14 = Value::List(vec![Value::Bool(true), Value::Bool(false)]);
    let v15 = Value::Tuple(vec![Value::Bool(true), Value::Char('x')]);
    let v16 = Value::Record(vec![
        Value::Bool(true),
        Value::Char('x'),
        Value::List(vec![]),
    ]);
    let v17a = Value::Variant {
        case_idx: 1,
        case_value: Some(Box::new(Value::Record(vec![Value::Option(None)]))),
    };
    let v17b = Value::Variant {
        case_idx: 1,
        case_value: None,
    };
    let v18 = Value::Enum(1);
    let v19 = Value::Flags(vec![true, false, true]);
    let v20a = Value::Option(Some(Box::new(Value::Bool(true))));
    let v20b = Value::Option(None);
    let v21a = Value::Result(Ok(Some(Box::new(Value::Bool(true)))));
    let v21b = Value::Result(Err(Some(Box::new(Value::Bool(true)))));
    let v21c = Value::Result(Ok(None));
    let v21d = Value::Result(Err(None));
    let v22 = Value::Handle {
        uri: Uri {
            value: "uri".to_string(),
        },
        resource_id: 123,
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("wasm_rpc_value_bool", &mut mint, v1);
    backward_compatible("wasm_rpc_value_u8", &mut mint, v2);
    backward_compatible("wasm_rpc_value_u16", &mut mint, v3);
    backward_compatible("wasm_rpc_value_u32", &mut mint, v4);
    backward_compatible("wasm_rpc_value_u64", &mut mint, v5);
    backward_compatible("wasm_rpc_value_s8", &mut mint, v6);
    backward_compatible("wasm_rpc_value_s16", &mut mint, v7);
    backward_compatible("wasm_rpc_value_s32", &mut mint, v8);
    backward_compatible("wasm_rpc_value_s64", &mut mint, v9);
    backward_compatible("wasm_rpc_value_f32", &mut mint, v10);
    backward_compatible("wasm_rpc_value_f64", &mut mint, v11);
    backward_compatible("wasm_rpc_value_char", &mut mint, v12);
    backward_compatible("wasm_rpc_value_string", &mut mint, v13);
    backward_compatible("wasm_rpc_value_list", &mut mint, v14);
    backward_compatible("wasm_rpc_value_tuple", &mut mint, v15);
    backward_compatible("wasm_rpc_value_record", &mut mint, v16);
    backward_compatible("wasm_rpc_value_variant_some", &mut mint, v17a);
    backward_compatible("wasm_rpc_value_variant_none", &mut mint, v17b);
    backward_compatible("wasm_rpc_value_enum", &mut mint, v18);
    backward_compatible("wasm_rpc_value_flags", &mut mint, v19);
    backward_compatible("wasm_rpc_value_option_some", &mut mint, v20a);
    backward_compatible("wasm_rpc_value_option_none", &mut mint, v20b);
    backward_compatible("wasm_rpc_value_result_ok_some", &mut mint, v21a);
    backward_compatible("wasm_rpc_value_result_err_some", &mut mint, v21b);
    backward_compatible("wasm_rpc_value_result_ok_none", &mut mint, v21c);
    backward_compatible("wasm_rpc_value_result_err_none", &mut mint, v21d);
    backward_compatible("wasm_rpc_value_handle", &mut mint, v22);
}

#[test]
pub fn timestamped_worker_invocation() {
    let twi1 = TimestampedWorkerInvocation {
        timestamp: Timestamp::from(1724701938466),
        invocation: WorkerInvocation::ExportedFunction {
            idempotency_key: IdempotencyKey {
                value: "idempotency_key".to_string(),
            },
            full_function_name: "function-name".to_string(),
            function_input: vec![Value::Bool(true)],
        },
    };
    let twi2 = TimestampedWorkerInvocation {
        timestamp: Timestamp::from(1724701938466),
        invocation: WorkerInvocation::ManualUpdate {
            target_version: 100,
        },
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible(
        "timestamped_worker_invocation_exported_function",
        &mut mint,
        twi1,
    );
    backward_compatible(
        "timestamped_worker_invocation_manual_update",
        &mut mint,
        twi2,
    );
}

#[test]
pub fn timestamped_update_description() {
    let tud1 = TimestampedUpdateDescription {
        timestamp: Timestamp::from(1724701938466),
        oplog_index: OplogIndex::from_u64(123),
        description: UpdateDescription::Automatic {
            target_version: 100,
        },
    };
    let tud2 = TimestampedUpdateDescription {
        timestamp: Timestamp::from(1724701938466),
        oplog_index: OplogIndex::from_u64(123),
        description: UpdateDescription::SnapshotBased {
            target_version: 100,
            payload: OplogPayload::Inline(vec![0, 1, 2, 3, 4]),
        },
    };
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("timestamped_update_description_automatic", &mut mint, tud1);
    backward_compatible(
        "timestamped_update_description_snapshot_based",
        &mut mint,
        tud2,
    );
}

#[test]
pub fn successful_update_record() {
    let sur1 = SuccessfulUpdateRecord {
        timestamp: Timestamp::from(1724701938466),
        target_version: 123,
    };
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("successful_update_record", &mut mint, sur1);
}

#[test]
pub fn failed_update_record() {
    let fur1 = FailedUpdateRecord {
        timestamp: Timestamp::from(1724701938466),
        target_version: 123,
        details: None,
    };
    let fur2 = FailedUpdateRecord {
        timestamp: Timestamp::from(1724701938466),
        target_version: 123,
        details: Some("details".to_string()),
    };
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("failed_update_record_no_details", &mut mint, fur1);
    backward_compatible("failed_update_record_with_details", &mut mint, fur2);
}

#[test]
pub fn worker_resource_id() {
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("worker_resource_id", &mut mint, WorkerResourceId(1));
}

#[test]
pub fn oplog_index() {
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("oplog_index", &mut mint, OplogIndex::from_u64(1));
}

#[test]
pub fn idempotency_key() {
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible(
        "idempotency_key",
        &mut mint,
        IdempotencyKey {
            value: "idempotency_key".to_string(),
        },
    );
}

#[test]
pub fn timestamp() {
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("timestamp", &mut mint, Timestamp::from(1724701938466));
}

#[test]
pub fn worker_resource_description() {
    let wrd1 = WorkerResourceDescription {
        created_at: Timestamp::from(1724701938466),
        indexed_resource_key: None,
    };
    let wrd2 = WorkerResourceDescription {
        created_at: Timestamp::from(1724701938466),
        indexed_resource_key: Some(IndexedResourceKey {
            resource_name: "r1".to_string(),
            resource_params: vec!["a".to_string(), "b".to_string()],
        }),
    };
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("worker_resource_description", &mut mint, wrd1);
    backward_compatible("worker_resource_description_indexed", &mut mint, wrd2);
}

#[test]
pub fn oplog_payload() {
    let op1 = OplogPayload::Inline(vec![0, 1, 2, 3, 4]);
    let op2 = OplogPayload::External {
        payload_id: PayloadId(Uuid::parse_str("4B29BF7C-13F6-4E37-AC03-830B81EAD478").unwrap()),
        md5_hash: vec![1, 2, 3, 4],
    };
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("oplog_payload_inline", &mut mint, op1);
    backward_compatible("oplog_payload_external", &mut mint, op2);
}

#[test]
pub fn worker_status_record() {
    let wsr1 = WorkerStatusRecord {
        status: WorkerStatus::Running,
        deleted_regions: Default::default(),
        overridden_retry_config: Some(RetryConfig::default()),
        pending_invocations: vec![TimestampedWorkerInvocation {
            timestamp: Timestamp::from(1724701938466),
            invocation: WorkerInvocation::ManualUpdate {
                target_version: 100,
            },
        }],
        pending_updates: Default::default(),
        failed_updates: vec![FailedUpdateRecord {
            timestamp: Timestamp::from(1724701938466),
            target_version: 123,
            details: None,
        }],
        successful_updates: vec![SuccessfulUpdateRecord {
            timestamp: Timestamp::from(1724701938466),
            target_version: 123,
        }],
        invocation_results: HashMap::from_iter(vec![(
            IdempotencyKey {
                value: "id1".to_string(),
            },
            OplogIndex::from_u64(111),
        )]),
        current_idempotency_key: Some(IdempotencyKey {
            value: "id1".to_string(),
        }),
        component_version: 2,
        component_size: 100_000_000,
        total_linear_memory_size: 500_000_000,
        owned_resources: HashMap::from_iter(vec![(
            WorkerResourceId(1),
            WorkerResourceDescription {
                created_at: Timestamp::from(1724701938466),
                indexed_resource_key: None,
            },
        )]),
        oplog_idx: OplogIndex::from_u64(10000),
    };

    let wsr2 = WorkerStatusRecord {
        status: WorkerStatus::Running,
        deleted_regions: Default::default(),
        overridden_retry_config: Some(RetryConfig::default()),
        pending_invocations: vec![TimestampedWorkerInvocation {
            timestamp: Timestamp::from(1724701938466),
            invocation: WorkerInvocation::ManualUpdate {
                target_version: 100,
            },
        }],
        pending_updates: Default::default(),
        failed_updates: vec![],
        successful_updates: vec![],
        invocation_results: HashMap::from_iter(vec![(
            IdempotencyKey {
                value: "id1".to_string(),
            },
            OplogIndex::from_u64(111),
        )]),
        current_idempotency_key: None,
        component_version: 2,
        component_size: 100_000_000,
        total_linear_memory_size: 500_000_000,
        owned_resources: HashMap::from_iter(vec![(
            WorkerResourceId(1),
            WorkerResourceDescription {
                created_at: Timestamp::from(1724701938466),
                indexed_resource_key: Some(IndexedResourceKey {
                    resource_name: "r1".to_string(),
                    resource_params: vec!["a".to_string(), "b".to_string()],
                }),
            },
        )]),
        oplog_idx: OplogIndex::from_u64(10000),
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("worker_status_record", &mut mint, wsr1);
    backward_compatible("worker_status_record_indexed", &mut mint, wsr2);
}

#[test]
pub fn redis_promise_state() {
    let s1 = RedisPromiseState::Pending;
    let s2 = RedisPromiseState::Complete(vec![]);
    let s3 = RedisPromiseState::Complete(vec![1, 2, 3, 4]);
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("redis_promise_state_pending", &mut mint, s1);
    backward_compatible("redis_promise_state_complete_empty", &mut mint, s2);
    backward_compatible("redis_promise_state_complete_nonempty", &mut mint, s3);
}

#[test]
pub fn account_id() {
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible(
        "account_id",
        &mut mint,
        AccountId {
            value: "account_id".to_string(),
        },
    );
}

#[test]
pub fn component_id() {
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible(
        "component_id",
        &mut mint,
        ComponentId(Uuid::parse_str("4B29BF7C-13F6-4E37-AC03-830B81EAD478").unwrap()),
    );
}

#[test]
pub fn worker_id() {
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible(
        "worker_id",
        &mut mint,
        WorkerId {
            component_id: ComponentId(
                Uuid::parse_str("4B29BF7C-13F6-4E37-AC03-830B81EAD478").unwrap(),
            ),
            worker_name: "worker_name".to_string(),
        },
    );
}

#[test]
pub fn promise_id() {
    let pid1 = PromiseId {
        worker_id: WorkerId {
            component_id: ComponentId(
                Uuid::parse_str("4B29BF7C-13F6-4E37-AC03-830B81EAD478").unwrap(),
            ),
            worker_name: "worker_name".to_string(),
        },
        oplog_idx: OplogIndex::from_u64(100),
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("promise_id", &mut mint, pid1);
}

#[test]
pub fn scheduled_action() {
    let sa1 = ScheduledAction::CompletePromise {
        account_id: AccountId {
            value: "account_id".to_string(),
        },
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
        owned_worker_id: OwnedWorkerId {
            account_id: AccountId {
                value: "account_id".to_string(),
            },
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
pub fn wrapped_function_type() {
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible(
        "wrapped_function_type_read_local",
        &mut mint,
        WrappedFunctionType::ReadLocal,
    );
    backward_compatible(
        "wrapped_function_type_read_remote",
        &mut mint,
        WrappedFunctionType::ReadRemote,
    );
    backward_compatible(
        "wrapped_function_type_write_local",
        &mut mint,
        WrappedFunctionType::WriteLocal,
    );
    backward_compatible(
        "wrapped_function_type_write_remote",
        &mut mint,
        WrappedFunctionType::WriteRemote,
    );
    backward_compatible(
        "wrapped_function_type_write_remote_batched_none",
        &mut mint,
        WrappedFunctionType::WriteRemoteBatched(None),
    );
    backward_compatible(
        "wrapped_function_type_write_remote_batched_some",
        &mut mint,
        WrappedFunctionType::WriteRemoteBatched(Some(OplogIndex::from_u64(100))),
    );
}

#[test]
pub fn worker_error() {
    let we1 = WorkerError::OutOfMemory;
    let we2 = WorkerError::InvalidRequest("invalid request".to_string());
    let we3 = WorkerError::StackOverflow;
    let we4 = WorkerError::Unknown("unknown".to_string());

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("worker_error_out_of_memory", &mut mint, we1);
    backward_compatible("worker_error_invalid_request", &mut mint, we2);
    backward_compatible("worker_error_stack_overflow", &mut mint, we3);
    backward_compatible("worker_error_unknown", &mut mint, we4);
}

#[test]
pub fn log_level() {
    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("log_level_error", &mut mint, LogLevel::Error);
    backward_compatible("log_level_debug", &mut mint, LogLevel::Debug);
    backward_compatible("log_level_warn", &mut mint, LogLevel::Warn);
    backward_compatible("log_level_stderr", &mut mint, LogLevel::Stderr);
    backward_compatible("log_level_info", &mut mint, LogLevel::Info);
    backward_compatible("log_level_stdout", &mut mint, LogLevel::Stdout);
    backward_compatible("log_level_critical", &mut mint, LogLevel::Critical);
    backward_compatible("log_level_trace", &mut mint, LogLevel::Trace);
}

#[test]
pub fn oplog_entry() {
    let oe1a = OplogEntry::Create {
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
        account_id: AccountId {
            value: "account_id".to_string(),
        },
        parent: None,
        component_size: 100_000_000,
        initial_total_linear_memory_size: 100_000_000,
    };
    let oe1b = OplogEntry::Create {
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
        account_id: AccountId {
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
    };

    let oe2 = OplogEntry::ImportedFunctionInvoked {
        timestamp: Timestamp::from(1724701938466),
        function_name: "test:pkg/iface.{fn}".to_string(),
        response: OplogPayload::Inline(vec![0, 1, 2, 3, 4]),
        wrapped_function_type: WrappedFunctionType::ReadLocal,
    };

    let oe3 = OplogEntry::ExportedFunctionInvoked {
        timestamp: Timestamp::from(1724701938466),
        function_name: "test:pkg/iface.{fn}".to_string(),
        request: OplogPayload::Inline(vec![0, 1, 2, 3, 4]),
        idempotency_key: IdempotencyKey {
            value: "id1".to_string(),
        },
    };

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
        },
    };

    let oe17 = OplogEntry::PendingUpdate {
        timestamp: Timestamp::from(1724701938466),
        description: UpdateDescription::Automatic {
            target_version: 100,
        },
    };

    let oe18 = OplogEntry::SuccessfulUpdate {
        timestamp: Timestamp::from(1724701938466),
        target_version: 10,
        new_component_size: 1234,
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
    };

    let oe22 = OplogEntry::DropResource {
        timestamp: Timestamp::from(1724701938466),
        id: WorkerResourceId(1),
    };

    let oe23 = OplogEntry::DescribeResource {
        timestamp: Timestamp::from(1724701938466),
        id: WorkerResourceId(1),
        indexed_resource: IndexedResourceKey {
            resource_name: "r1".to_string(),
            resource_params: vec!["a".to_string(), "b".to_string()],
        },
    };

    let oe24 = OplogEntry::Log {
        timestamp: Timestamp::from(1724701938466),
        level: LogLevel::Error,
        context: "context".to_string(),
        message: "message".to_string(),
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("oplog_entry_create", &mut mint, oe1a);
    backward_compatible("oplog_entry_create_with_parent", &mut mint, oe1b);
    backward_compatible("oplog_entry_imported_function_invoked", &mut mint, oe2);
    backward_compatible("oplog_entry_exported_function_invoked", &mut mint, oe3);
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
    backward_compatible("oplog_entry_pending_worker_invocation", &mut mint, oe16);
    backward_compatible("oplog_entry_pending_update", &mut mint, oe17);
    backward_compatible("oplog_entry_successful_update", &mut mint, oe18);
    backward_compatible("oplog_entry_failed_update_no_details", &mut mint, oe19a);
    backward_compatible("oplog_entry_failed_update_with_details", &mut mint, oe19b);
    backward_compatible("oplog_entry_grow_memory", &mut mint, oe20);
    backward_compatible("oplog_entry_create_resource", &mut mint, oe21);
    backward_compatible("oplog_entry_drop_resource", &mut mint, oe22);
    backward_compatible("oplog_entry_describe_resource", &mut mint, oe23);
    backward_compatible("oplog_entry_log", &mut mint, oe24);
}

#[test]
pub fn blob_store_object_metadata() {
    let om1 = blob_store::ObjectMetadata {
        name: "item".to_string(),
        container: "container".to_string(),
        created_at: 1724701938466,
        size: 500_000_000,
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("blob_store_object_metadata", &mut mint, om1);
}

#[test]
pub fn golem_error() {
    todo!()
}

#[test]
pub fn rpc_error() {
    todo!()
}

#[test]
pub fn worker_proxy_error() {
    todo!()
}

#[test]
pub fn serializable_error() {
    let se1 = SerializableError::FsError { code: 11 };
    let se2 = SerializableError::Generic {
        message: "hello world".to_string(),
    };
    let se3 = SerializableError::Golem {
        error: GolemError::Interrupted {
            kind: InterruptKind::Restart,
        },
    };
    let se4 = SerializableError::SocketError { code: 1 };
    let se5 = SerializableError::Rpc {
        error: RpcError::ProtocolError {
            details: "not working".to_string(),
        },
    };
    let se6 = SerializableError::WorkerProxy {
        error: WorkerProxyError::AlreadyExists("already exists".to_string()),
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("serializable_error_fs_error", &mut mint, se1);
    backward_compatible("serializable_error_generic", &mut mint, se2);
    backward_compatible("serializable_error_golem", &mut mint, se3);
    backward_compatible("serializable_error_socket_error", &mut mint, se4);
    backward_compatible("serializable_error_rpc", &mut mint, se5);
    backward_compatible("serializable_error_worker_proxy", &mut mint, se6);
}

#[test]
pub fn serializable_stream_error() {
    let sse1 = SerializableStreamError::Closed;
    let sse2 = SerializableStreamError::LastOperationFailed(SerializableError::Generic {
        message: "hello world".to_string(),
    });
    let sse3 = SerializableStreamError::Trap(SerializableError::Generic {
        message: "hello world".to_string(),
    });

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("serializable_stream_error_closed", &mut mint, sse1);
    backward_compatible(
        "serializable_stream_error_last_operation_failed",
        &mut mint,
        sse2,
    );
    backward_compatible("serializable_stream_error_trap", &mut mint, sse3);
}

#[test]
pub fn serializable_ip_address() {
    let sia1 = SerializableIpAddress::IPv4 {
        address: [127, 0, 0, 1],
    };
    let sia2 = SerializableIpAddress::IPv6 {
        address: [1, 2, 3, 4, 5, 6, 7, 8],
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("serializable_ip_address_ipv4", &mut mint, sia1);
    backward_compatible("serializable_ip_address_ipv6", &mut mint, sia2);
}

#[test]
pub fn serializable_ip_addresses() {
    let sia1 = SerializableIpAddresses(vec![SerializableIpAddress::IPv4 {
        address: [127, 0, 0, 1],
    }]);

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("serializable_ip_addresses", &mut mint, sia1);
}

#[test]
pub fn wit_value() {
    let wv1: WitValue = Value::Bool(true).into();
    let wv2: WitValue = Value::U8(1).into();
    let wv3: WitValue = Value::U16(12345).into();
    let wv4: WitValue = Value::U32(123456789).into();
    let wv5: WitValue = Value::U64(12345678901234567890).into();
    let wv6: WitValue = Value::S8(-1).into();
    let wv7: WitValue = Value::S16(-12345).into();
    let wv8: WitValue = Value::S32(-123456789).into();
    let wv9: WitValue = Value::S64(-1234567890123456789).into();
    let wv10: WitValue = Value::F32(1.234).into();
    let wv11: WitValue = Value::F64(1.234567890123456789).into();
    let wv12: WitValue = Value::Char('a').into();
    let wv13: WitValue = Value::String("hello world".to_string()).into();
    let wv14: WitValue = Value::List(vec![Value::Bool(true), Value::Bool(false)]).into();
    let wv15: WitValue = Value::Tuple(vec![Value::Bool(true), Value::Char('x')]).into();
    let wv16: WitValue = Value::Record(vec![
        Value::Bool(true),
        Value::Char('x'),
        Value::List(vec![]),
    ])
    .into();
    let wv17a: WitValue = Value::Variant {
        case_idx: 1,
        case_value: Some(Box::new(Value::Record(vec![Value::Option(None)]))),
    }
    .into();
    let wv17b: WitValue = Value::Variant {
        case_idx: 1,
        case_value: None,
    }
    .into();
    let wv18: WitValue = Value::Enum(1).into();
    let wv19: WitValue = Value::Flags(vec![true, false, true]).into();
    let wv20a: WitValue = Value::Option(Some(Box::new(Value::Bool(true)))).into();
    let wv20b: WitValue = Value::Option(None).into();
    let wv21a: WitValue = Value::Result(Ok(Some(Box::new(Value::Bool(true))))).into();
    let wv21b: WitValue = Value::Result(Err(Some(Box::new(Value::Bool(true))))).into();
    let wv21c: WitValue = Value::Result(Ok(None)).into();
    let wv21d: WitValue = Value::Result(Err(None)).into();
    let wv22: WitValue = Value::Handle {
        uri: Uri {
            value: "uri".to_string(),
        },
        resource_id: 123,
    }
    .into();

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible_wit_value("wit_value_bool", &mut mint, wv1);
    backward_compatible_wit_value("wit_value_u8", &mut mint, wv2);
    backward_compatible_wit_value("wit_value_u16", &mut mint, wv3);
    backward_compatible_wit_value("wit_value_u32", &mut mint, wv4);
    backward_compatible_wit_value("wit_value_u64", &mut mint, wv5);
    backward_compatible_wit_value("wit_value_s8", &mut mint, wv6);
    backward_compatible_wit_value("wit_value_s16", &mut mint, wv7);
    backward_compatible_wit_value("wit_value_s32", &mut mint, wv8);
    backward_compatible_wit_value("wit_value_s64", &mut mint, wv9);
    backward_compatible_wit_value("wit_value_f32", &mut mint, wv10);
    backward_compatible_wit_value("wit_value_f64", &mut mint, wv11);
    backward_compatible_wit_value("wit_value_char", &mut mint, wv12);
    backward_compatible_wit_value("wit_value_string", &mut mint, wv13);
    backward_compatible_wit_value("wit_value_list", &mut mint, wv14);
    backward_compatible_wit_value("wit_value_tuple", &mut mint, wv15);
    backward_compatible_wit_value("wit_value_record", &mut mint, wv16);
    backward_compatible_wit_value("wit_value_variant_some", &mut mint, wv17a);
    backward_compatible_wit_value("wit_value_variant_none", &mut mint, wv17b);
    backward_compatible_wit_value("wit_value_enum", &mut mint, wv18);
    backward_compatible_wit_value("wit_value_flags", &mut mint, wv19);
    backward_compatible_wit_value("wit_value_option_some", &mut mint, wv20a);
    backward_compatible_wit_value("wit_value_option_none", &mut mint, wv20b);
    backward_compatible_wit_value("wit_value_result_ok_some", &mut mint, wv21a);
    backward_compatible_wit_value("wit_value_result_err_some", &mut mint, wv21b);
    backward_compatible_wit_value("wit_value_result_ok_none", &mut mint, wv21c);
    backward_compatible_wit_value("wit_value_result_err_none", &mut mint, wv21d);
    backward_compatible_wit_value("wit_value_handle", &mut mint, wv22);
}

#[test]
pub fn serializable_error_code() {
    todo!()
}

#[test]
pub fn serializable_response() {
    let sr1 = SerializableResponse::Pending;
    let sr2 = SerializableResponse::HeadersReceived(SerializableResponseHeaders {
        status: 200,
        headers: HashMap::from_iter(vec![("key".to_string(), vec![0, 1, 2, 3])]),
    });
    let sr3 = SerializableResponse::HttpError(SerializableErrorCode::ConnectionLimitReached);
    let sr4 = SerializableResponse::InternalError(None);
    let sr5 = SerializableResponse::InternalError(Some(SerializableError::Generic {
        message: "hello world".to_string(),
    }));

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("serializable_response_pending", &mut mint, sr1);
    backward_compatible("serializable_response_headers_received", &mut mint, sr2);
    backward_compatible("serializable_response_http_error", &mut mint, sr3);
    backward_compatible("serializable_response_internal_error_none", &mut mint, sr4);
    backward_compatible("serializable_response_internal_error_some", &mut mint, sr5);
}

#[test]
pub fn serializable_invoke_result() {
    let sir1 = SerializableInvokeResult::Pending;
    let sir2 = SerializableInvokeResult::Failed(SerializableError::Generic {
        message: "hello world".to_string(),
    });
    let sir3 = SerializableInvokeResult::Completed(Ok(Value::Bool(true).into()));
    let sir4 = SerializableInvokeResult::Completed(Err(RpcError::Denied {
        details: "not now".to_string(),
    }));

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("serializable_invoke_result_pending", &mut mint, sir1);
    backward_compatible("serializable_invoke_result_failed", &mut mint, sir2);
    backward_compatible("serializable_invoke_result_completed_ok", &mut mint, sir3);
    backward_compatible("serializable_invoke_result_completed_err", &mut mint, sir4);
}

// TODO: proto Val
// TODO: proto TypeAnnotatedValue
// TODO: SerializableFileTimes
// TODO: SerializableErrorCode

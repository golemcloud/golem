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

use crate::compatibility::v1::backward_compatible;
use goldenfile::Mint;
use golem_common::model::oplog::{DurableFunctionType, OplogEntry, OplogPayload};
use golem_common::model::{AccountId, ComponentId, PluginInstallationId, Timestamp, WorkerId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm_ast::analysis::analysed_type::bool;
use golem_wasm_rpc::{Value, ValueAndType};
use golem_worker_executor::durable_host::serialized::SerializableError;
use golem_worker_executor::durable_host::wasm_rpc::serialized::SerializableInvokeResult;
use golem_worker_executor::services::rpc::RpcError;
use std::collections::HashSet;
use test_r::test;
use uuid::Uuid;

#[test]
pub fn golem_error() {
    let g1 = WorkerExecutorError::ShardingNotReady;

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("golem_error_sharding_not_ready", &mut mint, g1);
}

#[test]
pub fn oplog_entry() {
    let oe25 = OplogEntry::Restart {
        timestamp: Timestamp::from(1724701938466),
    };
    let oe26 = OplogEntry::ImportedFunctionInvoked {
        timestamp: Timestamp::from(1724701938466),
        function_name: "test:pkg/iface.{fn}".to_string(),
        request: OplogPayload::Inline(vec![5, 6, 7, 8, 9]),
        response: OplogPayload::Inline(vec![0, 1, 2, 3, 4]),
        wrapped_function_type: DurableFunctionType::ReadLocal,
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
        account_id: AccountId {
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

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("oplog_entry_restart", &mut mint, oe25);
    backward_compatible("oplog_entry_import_function_invoked_v11", &mut mint, oe26);
    backward_compatible("oplog_entry_create_v11a", &mut mint, oe27a);
    backward_compatible("oplog_entry_create_v11b", &mut mint, oe27b);
    backward_compatible("oplog_entry_successful_update_v11", &mut mint, oe28);
    backward_compatible("oplog_entry_activate_plugin_v11", &mut mint, oe29);
    backward_compatible("oplog_entry_deactivate_plugin_v11", &mut mint, oe30);
}

#[test]
#[ignore] // compatibility has been broken in 1.3
pub fn serializable_invoke_result() {
    let sir1 = SerializableInvokeResult::Pending;
    let sir2 = SerializableInvokeResult::Failed(SerializableError::Generic {
        message: "hello world".to_string(),
    });
    let sir3 =
        SerializableInvokeResult::Completed(
            Ok(ValueAndType::new(Value::Bool(true), bool()).into()),
        );
    let sir4 = SerializableInvokeResult::Completed(Err(RpcError::Denied {
        details: "not now".to_string(),
    }));

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("serializable_invoke_result_v11_pending", &mut mint, sir1);
    backward_compatible("serializable_invoke_result_v11_failed", &mut mint, sir2);
    backward_compatible(
        "serializable_invoke_result_v11_completed_ok",
        &mut mint,
        sir3,
    );
    backward_compatible(
        "serializable_invoke_result_v11_completed_err",
        &mut mint,
        sir4,
    );
}

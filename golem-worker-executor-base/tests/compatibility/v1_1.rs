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

use test_r::test;

use crate::compatibility::v1::backward_compatible;
use goldenfile::Mint;
use golem_common::model::oplog::{OplogEntry, OplogPayload, WrappedFunctionType};
use golem_common::model::Timestamp;
use golem_wasm_ast::analysis::analysed_type::bool;
use golem_wasm_rpc::{Value, ValueAndType};
use golem_worker_executor_base::durable_host::serialized::SerializableError;
use golem_worker_executor_base::durable_host::wasm_rpc::serialized::SerializableInvokeResult;
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::services::rpc::RpcError;

#[test]
pub fn golem_error() {
    let g1 = GolemError::ShardingNotReady;

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
        wrapped_function_type: WrappedFunctionType::ReadLocal,
    };

    let mut mint = Mint::new("tests/goldenfiles");
    backward_compatible("oplog_entry_restart", &mut mint, oe25);
    backward_compatible("oplog_entry_import_function_invoked_v11", &mut mint, oe26);
}

#[test]
pub fn serializable_invoke_result() {
    let sir1 = SerializableInvokeResult::Pending;
    let sir2 = SerializableInvokeResult::Failed(SerializableError::Generic {
        message: "hello world".to_string(),
    });
    let sir3 =
        SerializableInvokeResult::Completed(Ok(ValueAndType::new(Value::Bool(true), bool())
            .try_into()
            .unwrap()));
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

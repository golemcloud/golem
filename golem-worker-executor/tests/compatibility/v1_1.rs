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
use golem_common::base_model::ProjectId;
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

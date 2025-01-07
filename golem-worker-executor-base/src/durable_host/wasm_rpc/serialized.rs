// Copyright 2024-2025 Golem Cloud
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

use crate::durable_host::serialized::SerializableError;
use crate::services::rpc::RpcError;
use bincode::{Decode, Encode};
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{ValueAndType, WitValue};

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum SerializableInvokeResultV1 {
    Failed(SerializableError),
    Pending,
    Completed(Result<WitValue, RpcError>),
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum SerializableInvokeResult {
    Failed(SerializableError),
    Pending,
    Completed(Result<TypeAnnotatedValue, RpcError>),
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct SerializableInvokeRequest {
    pub remote_worker_id: WorkerId,
    pub idempotency_key: IdempotencyKey,
    pub function_name: String,
    pub function_params: Vec<ValueAndType>,
}

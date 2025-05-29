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

use crate::durable_host::serialized::{SerializableDateTime, SerializableError};
use crate::services::rpc::RpcError;
use bincode::{Decode, Encode};
use golem_common::model::{IdempotencyKey, ScheduleId, WorkerId};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{ValueAndType, WitValue};
use golem_wasm_rpc_derive::IntoValue;

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

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub struct SerializableInvokeRequest {
    pub remote_worker_id: WorkerId,
    pub idempotency_key: IdempotencyKey,
    pub function_name: String,
    #[wit_field(convert_vec = WitValue)]
    pub function_params: Vec<ValueAndType>,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub struct SerializableScheduleInvocationRequest {
    pub remote_worker_id: WorkerId,
    pub idempotency_key: IdempotencyKey,
    pub function_name: String,
    #[wit_field(convert_vec = WitValue)]
    pub function_params: Vec<ValueAndType>,
    pub datetime: SerializableDateTime,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
#[wit_transparent]
pub struct SerializableScheduleId {
    pub data: Vec<u8>,
}

impl SerializableScheduleId {
    pub fn from_domain(schedule_id: &ScheduleId) -> Self {
        let data = golem_common::serialization::serialize(schedule_id)
            .unwrap()
            .to_vec();
        Self { data }
    }

    pub fn as_domain(&self) -> Result<ScheduleId, String> {
        golem_common::serialization::deserialize(&self.data)
    }
}

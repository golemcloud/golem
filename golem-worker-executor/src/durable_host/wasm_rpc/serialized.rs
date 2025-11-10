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

use desert_rust::BinaryCodec;
use golem_common::model::agent::DataValue;
use golem_common::model::oplog::types::SerializableDateTime;
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_wasm::ValueAndType;
use golem_wasm_derive::IntoValue;

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue)]
#[desert(evolution())]
pub struct EnrichedSerializableInvokeRequest {
    pub remote_worker_id: WorkerId,
    pub remote_agent_type: Option<String>,
    pub remote_agent_parameters: Option<DataValue>,
    pub idempotency_key: IdempotencyKey,
    pub function_name: String,
    pub function_params: Vec<ValueAndType>,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue)]
#[desert(evolution())]
pub struct EnrichedSerializableScheduleInvocationRequest {
    pub remote_worker_id: WorkerId,
    pub remote_agent_type: Option<String>,
    pub remote_agent_parameters: Option<DataValue>,
    pub idempotency_key: IdempotencyKey,
    pub function_name: String,
    pub function_params: Vec<ValueAndType>,
    pub datetime: SerializableDateTime,
}

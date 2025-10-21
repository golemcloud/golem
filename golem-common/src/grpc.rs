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

use crate::model::component::ComponentId;
use crate::model::{IdempotencyKey, PromiseId, WorkerId};
use golem_api_grpc::proto::golem::component;
use golem_api_grpc::proto::golem::worker;
use golem_api_grpc::proto::golem::common::{AccountId as ProtoAccountId};
use crate::model::account::AccountId;

pub fn proto_account_id_string(account_id: &Option<ProtoAccountId>) -> Option<String> {
    (*account_id)
        .and_then(|v| TryInto::<AccountId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_component_id_string(component_id: &Option<component::ComponentId>) -> Option<String> {
    (*component_id)
        .and_then(|v| TryInto::<ComponentId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_worker_id_string(worker_id: &Option<worker::WorkerId>) -> Option<String> {
    worker_id
        .clone()
        .and_then(|v| TryInto::<WorkerId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_idempotency_key_string(
    idempotency_key: &Option<worker::IdempotencyKey>,
) -> Option<String> {
    idempotency_key
        .clone()
        .map(|v| Into::<IdempotencyKey>::into(v).to_string())
}

pub fn proto_promise_id_string(promise_id: &Option<worker::PromiseId>) -> Option<String> {
    promise_id
        .clone()
        .and_then(|v| TryInto::<PromiseId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_invocation_context_parent_worker_id_string(
    invocation_context: &Option<worker::InvocationContext>,
) -> Option<String> {
    proto_worker_id_string(&invocation_context.as_ref().and_then(|c| c.parent.clone()))
}

pub enum ProtoApiDefinitionKind {
    Golem,
    OpenAPI,
}

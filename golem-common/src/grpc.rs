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

use crate::model::{AccountId, ComponentId, IdempotencyKey, PromiseId, WorkerId};

pub fn proto_component_id_string(
    component_id: &Option<golem_api_grpc::proto::golem::component::ComponentId>,
) -> Option<String> {
    component_id
        .clone()
        .and_then(|v| TryInto::<ComponentId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_worker_id_string(
    worker_id: &Option<golem_api_grpc::proto::golem::worker::WorkerId>,
) -> Option<String> {
    worker_id
        .clone()
        .and_then(|v| TryInto::<WorkerId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_idempotency_key_string(
    idempotency_key: &Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
) -> Option<String> {
    idempotency_key
        .clone()
        .map(|v| Into::<IdempotencyKey>::into(v).to_string())
}

pub fn proto_account_id_string(
    account_id: &Option<golem_api_grpc::proto::golem::common::AccountId>,
) -> Option<String> {
    account_id
        .clone()
        .map(|v| Into::<AccountId>::into(v).to_string())
}

pub fn proto_promise_id_string(
    promise_id: &Option<golem_api_grpc::proto::golem::worker::PromiseId>,
) -> Option<String> {
    promise_id
        .clone()
        .and_then(|v| TryInto::<PromiseId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_invocation_context_parent_worker_id_string(
    invocation_context: &Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
) -> Option<String> {
    proto_worker_id_string(&invocation_context.as_ref().and_then(|c| c.parent.clone()))
}

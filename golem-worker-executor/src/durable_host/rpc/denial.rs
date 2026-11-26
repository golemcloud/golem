// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use golem_common::model::IdempotencyKey;
use golem_common::model::InvocationRequestKey;
use golem_common::model::OwnedAgentId;
use golem_common::model::oplog::HostRequestGolemRpcInvoke;
use golem_common::schema::schema_value::SchemaValue;

pub(super) fn denied_rpc_request(
    remote_agent_id: &OwnedAgentId,
    idempotency_key: &IdempotencyKey,
    method_name: String,
) -> HostRequestGolemRpcInvoke {
    HostRequestGolemRpcInvoke {
        remote_agent_id: remote_agent_id.agent_id(),
        request_key: InvocationRequestKey::Internal(idempotency_key.clone()),
        method_name,
        input: SchemaValue::Tuple { elements: vec![] },
        logical_streaming_origin: None,
        remote_agent_type: None,
        remote_agent_parameters: None,
        scope_card: None,
    }
}

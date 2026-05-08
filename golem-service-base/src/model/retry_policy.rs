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

use golem_common::model::environment::EnvironmentId;
use golem_common::model::retry_policy::{RetryPolicyDto, RetryPolicyId, RetryPolicyRevision};
use golem_common::model::{NamedRetryPolicy, UntypedJsonBody};

#[derive(Debug, Clone)]
pub struct StoredRetryPolicy {
    pub id: RetryPolicyId,
    pub environment_id: EnvironmentId,
    pub name: String,
    pub revision: RetryPolicyRevision,
    pub priority: u32,
    pub predicate_json: String,
    pub policy_json: String,
}

impl TryFrom<StoredRetryPolicy> for RetryPolicyDto {
    type Error = String;

    fn try_from(value: StoredRetryPolicy) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            environment_id: value.environment_id,
            name: value.name,
            revision: value.revision,
            priority: value.priority,
            predicate: UntypedJsonBody(
                serde_json::from_str::<serde_json::Value>(&value.predicate_json).map_err(|e| {
                    format!(
                        "Invalid predicate JSON for retry policy '{}': {e}",
                        value.id
                    )
                })?,
            ),
            policy: UntypedJsonBody(
                serde_json::from_str::<serde_json::Value>(&value.policy_json).map_err(|e| {
                    format!("Invalid policy JSON for retry policy '{}': {e}", value.id)
                })?,
            ),
        })
    }
}

impl TryFrom<StoredRetryPolicy> for NamedRetryPolicy {
    type Error = String;

    fn try_from(value: StoredRetryPolicy) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            priority: value.priority,
            predicate: serde_json::from_str(&value.predicate_json).map_err(|e| {
                format!(
                    "Invalid predicate JSON for retry policy '{}': {e}",
                    value.id
                )
            })?,
            policy: serde_json::from_str(&value.policy_json)
                .map_err(|e| format!("Invalid policy JSON for retry policy '{}': {e}", value.id))?,
        })
    }
}

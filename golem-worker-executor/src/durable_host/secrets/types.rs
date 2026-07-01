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

use chrono::{DateTime, Utc};
use golem_common::model::agent_secret::{AgentSecretId, AgentSecretRevision};
use golem_common::schema::schema_value::SecretValuePayload;
use golem_schema::schema::wit::SecretHandleRep;

/// Resource-table entry for a `golem:core/types@2.0.0` `secret` handle.
///
/// The entry contains only stable identity and resolution metadata. Plaintext
/// secret material stays in the registry and is fetched by `(secret-id,
/// pinned-revision)` only when a caller has the `golem:secrets/reveal` import.
#[derive(Clone, Debug)]
pub struct SecretEntry {
    pub secret_id: AgentSecretId,
    pub pinned_revision: AgentSecretRevision,
    pub config_key: Option<Vec<String>>,
    pub resolved_at: DateTime<Utc>,
    pub category: Option<String>,
}

impl SecretEntry {
    pub fn to_snapshot(&self) -> SecretValuePayload {
        SecretValuePayload {
            secret_id: self.secret_id.0,
            config_key: self.config_key.clone(),
            version: self.pinned_revision.get(),
            resolved_at: self.resolved_at,
            category: self.category.clone(),
        }
    }

    pub fn from_snapshot(snapshot: &SecretValuePayload) -> anyhow::Result<Self> {
        Ok(Self {
            secret_id: AgentSecretId(snapshot.secret_id),
            pinned_revision: AgentSecretRevision::new(snapshot.version)?,
            config_key: snapshot.config_key.clone(),
            resolved_at: snapshot.resolved_at,
            category: snapshot.category.clone(),
        })
    }
}

impl From<SecretEntry> for SecretHandleRep {
    fn from(entry: SecretEntry) -> Self {
        SecretHandleRep::new(entry)
    }
}

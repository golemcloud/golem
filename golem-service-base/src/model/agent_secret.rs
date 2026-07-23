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

use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_value::SchemaValue;

/// In-memory representation of an agent secret.
///
/// `secret_type` is a [`SchemaGraph`] (whose `root` is the secret's type
/// and `defs` carry any named composites) and `secret_value` is an optional
/// [`SchemaValue`] bound to that graph. Persisted repo blobs store these
/// schema types directly via `BinaryCodec`.
///
/// The wire formats (REST DTO and gRPC) are schema-native as well, so the
/// conversions below are structural pass-throughs.
#[derive(Debug, Clone)]
pub struct AgentSecret {
    pub id: AgentSecretId,
    pub environment_id: EnvironmentId,
    pub path: CanonicalAgentSecretPath,
    pub revision: AgentSecretRevision,
    pub secret_type: SchemaGraph,
    pub secret_value: Option<SchemaValue>,
}

impl From<AgentSecret> for golem_common::model::agent_secret::AgentSecretDto {
    fn from(value: AgentSecret) -> Self {
        Self {
            id: value.id,
            environment_id: value.environment_id,
            path: value.path,
            revision: value.revision,
            secret_type: value.secret_type,
            secret_value: value.secret_value,
        }
    }
}

impl From<AgentSecret> for golem_api_grpc::proto::golem::registry::AgentSecret {
    fn from(value: AgentSecret) -> Self {
        Self {
            agent_secret_id: Some(value.id.into()),
            environment_id: Some(value.environment_id.into()),
            path: value.path.0,
            revision: value.revision.into(),
            secret_type: Some(value.secret_type.into()),
            secret_value: value.secret_value.map(Into::into),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::AgentSecret> for AgentSecret {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::registry::AgentSecret,
    ) -> Result<Self, Self::Error> {
        debug_assert!(
            CanonicalAgentSecretPath::from_path_in_unknown_casing(&value.path).0 == value.path,
            "agent secret path must be in canonical form"
        );

        let secret_type: SchemaGraph = value
            .secret_type
            .ok_or("Missing secret_type field")?
            .try_into()
            .map_err(|e| format!("Failed to decode secret_type SchemaGraph: {e}"))?;
        let secret_value = value
            .secret_value
            .map(|pv| -> Result<SchemaValue, String> {
                pv.try_into()
                    .map_err(|e| format!("Failed to decode secret_value SchemaValue: {e}"))
            })
            .transpose()?;

        Ok(Self {
            id: value
                .agent_secret_id
                .ok_or("Missing agent_secret_id field")?
                .try_into()?,
            environment_id: value
                .environment_id
                .ok_or("Missing environment_id field")?
                .try_into()?,
            path: CanonicalAgentSecretPath(value.path),
            revision: AgentSecretRevision::try_from(value.revision)?,
            secret_type,
            secret_value,
        })
    }
}

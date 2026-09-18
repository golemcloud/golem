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

use crate::base_model::agent_secret::{
    AgentSecretCreation as DomainAgentSecretCreation, AgentSecretDto as DomainAgentSecretDto,
    AgentSecretId, AgentSecretPath, AgentSecretRevision,
    AgentSecretUpdate as DomainAgentSecretUpdate, CanonicalAgentSecretPath,
};
use crate::base_model::environment::EnvironmentId;
use crate::base_model::optional_field_update::OptionalFieldUpdate;
use crate::schema::{ExternalSchemaValue, SchemaGraph};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(
    feature = "full",
    oai(rename = "AgentSecretCreation", rename_all = "camelCase")
)]
#[serde(rename_all = "camelCase")]
pub struct AgentSecretCreation {
    pub path: AgentSecretPath,
    pub secret_type: SchemaGraph,
    pub secret_value: Option<ExternalSchemaValue>,
}

impl From<AgentSecretCreation> for DomainAgentSecretCreation {
    fn from(value: AgentSecretCreation) -> Self {
        Self {
            path: value.path,
            secret_type: value.secret_type,
            secret_value: value.secret_value.map(ExternalSchemaValue::into_inner),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(
    feature = "full",
    oai(rename = "AgentSecretUpdate", rename_all = "camelCase")
)]
#[serde(rename_all = "camelCase")]
pub struct AgentSecretUpdate {
    pub current_revision: AgentSecretRevision,
    pub secret_value: OptionalFieldUpdate<ExternalSchemaValue>,
}

impl From<AgentSecretUpdate> for DomainAgentSecretUpdate {
    fn from(value: AgentSecretUpdate) -> Self {
        Self {
            current_revision: value.current_revision,
            secret_value: match value.secret_value {
                OptionalFieldUpdate::Set(value) => OptionalFieldUpdate::Set(value.into_inner()),
                OptionalFieldUpdate::Unset => OptionalFieldUpdate::Unset,
                OptionalFieldUpdate::NoChange => OptionalFieldUpdate::NoChange,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(
    feature = "full",
    oai(rename = "AgentSecretDto", rename_all = "camelCase")
)]
#[serde(rename_all = "camelCase")]
pub struct AgentSecretDto {
    pub id: AgentSecretId,
    pub environment_id: EnvironmentId,
    pub path: CanonicalAgentSecretPath,
    pub revision: AgentSecretRevision,
    pub secret_type: SchemaGraph,
    pub secret_value: Option<ExternalSchemaValue>,
}

impl TryFrom<DomainAgentSecretDto> for AgentSecretDto {
    type Error = String;

    fn try_from(value: DomainAgentSecretDto) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            environment_id: value.environment_id,
            path: value.path,
            revision: value.revision,
            secret_type: value.secret_type,
            secret_value: value
                .secret_value
                .map(ExternalSchemaValue::try_from)
                .transpose()?,
        })
    }
}

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
use golem_common::schema::adapters::analysed_type::{
    analysed_type_to_schema_graph, schema_graph_to_analysed_type,
};
use golem_common::schema::adapters::value::{schema_value_to_value, value_to_schema_value};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_value::SchemaValue;
use golem_wasm::ValueAndType;
use golem_wasm::json::ValueAndTypeJsonExtensions;

/// In-memory representation of an agent secret.
///
/// `secret_type` is a [`SchemaGraph`] (whose `root` is the secret's type
/// and `defs` carry any named composites preserved by
/// [`analysed_type_to_schema_graph`]) and `secret_value` is an optional
/// [`SchemaValue`] bound to that graph. Persisted repo blobs store these
/// schema types directly via `BinaryCodec`.
///
/// Wire formats (REST DTO and gRPC) still use the legacy `AnalysedType` /
/// `golem_wasm::Value` pair; conversions happen at the service boundary.
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
        // Render the secret value as legacy `ValueAndType` JSON (numeric
        // `char`, `{"Case": null}` for unit variants, etc.) to match the
        // REST DTO contract.
        let secret_type_legacy = schema_graph_to_analysed_type(&value.secret_type)
            .expect("agent secret schema must be representable as AnalysedType");
        let secret_value_json = value.secret_value.as_ref().map(|sv| {
            let legacy_value =
                schema_value_to_value(&value.secret_type, &value.secret_type.root, sv)
                    .expect("agent secret value must be representable as legacy Value");
            ValueAndType::new(legacy_value, secret_type_legacy.clone())
                .to_json_value()
                .expect("agent secret value must be renderable as JSON")
        });
        Self {
            id: value.id,
            environment_id: value.environment_id,
            path: value.path,
            revision: value.revision,
            secret_type: secret_type_legacy,
            secret_value: secret_value_json,
        }
    }
}

impl From<AgentSecret> for golem_api_grpc::proto::golem::registry::AgentSecret {
    fn from(value: AgentSecret) -> Self {
        let secret_type_legacy = schema_graph_to_analysed_type(&value.secret_type)
            .expect("agent secret schema must be representable as AnalysedType");
        let secret_value_legacy = value.secret_value.as_ref().map(|sv| {
            schema_value_to_value(&value.secret_type, &value.secret_type.root, sv)
                .expect("agent secret value must be representable as legacy Value")
        });
        Self {
            agent_secret_id: Some(value.id.into()),
            environment_id: Some(value.environment_id.into()),
            path: value.path.0,
            revision: value.revision.into(),
            secret_type: Some((&secret_type_legacy).into()),
            secret_value: secret_value_legacy.map(Into::into),
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

        let analysed: golem_wasm::analysis::AnalysedType =
            (&value.secret_type.ok_or("Missing secret_type field")?).try_into()?;
        let secret_type = analysed_type_to_schema_graph(&analysed)
            .map_err(|e| format!("Failed to convert AnalysedType to SchemaGraph: {e}"))?;
        let secret_value = value
            .secret_value
            .map(|pv| -> Result<SchemaValue, String> {
                let legacy: golem_wasm::Value = pv.try_into()?;
                value_to_schema_value(&legacy, &analysed)
                    .map_err(|e| format!("Failed to convert legacy Value to SchemaValue: {e}"))
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

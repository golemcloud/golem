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

use super::optional_field_update::OptionalFieldUpdate;
use crate::base_model::environment::EnvironmentId;
use crate::{declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid};
use golem_wasm::analysis::AnalysedType;

newtype_uuid!(
    AgentSecretId,
    golem_api_grpc::proto::golem::registry::AgentSecretId
);

declare_revision!(AgentSecretRevision);

declare_transparent_newtypes! {
    /// Agent secret path in any casing. All agent secret paths
    /// are converted to the same casing internally to allow easier cross-language use.
    #[cfg_attr(feature = "full", oai(to_header = false))]
    pub struct AgentSecretPath(pub Vec<String>);

    /// Canonical representation of an agent secret path (segments are each camelCase)
    #[derive(Eq, Hash)]
    #[cfg_attr(feature = "full", oai(to_header = false))]
    pub struct CanonicalAgentSecretPath(pub Vec<String>);
}

impl CanonicalAgentSecretPath {
    #[cfg(feature = "full")]
    pub fn from_path_in_unknown_casing(value: &[String]) -> Self {
        use heck::ToLowerCamelCase;
        Self(value.iter().map(|s| s.to_lower_camel_case()).collect())
    }
}

#[cfg(feature = "full")]
impl From<AgentSecretPath> for CanonicalAgentSecretPath {
    fn from(value: AgentSecretPath) -> Self {
        Self::from_path_in_unknown_casing(&value.0)
    }
}

declare_structs! {
    pub struct AgentSecretDto {
        pub id: AgentSecretId,
        pub environment_id: EnvironmentId,
        pub path: CanonicalAgentSecretPath,
        pub revision: AgentSecretRevision,
        pub secret_type: AnalysedType,
        pub secret_value: Option<serde_json::Value>,
    }

    pub struct AgentSecretCreation {
        pub path: AgentSecretPath,
        pub secret_type: AnalysedType,
        pub secret_value: Option<serde_json::Value>,
    }

    pub struct AgentSecretUpdate {
        pub current_revision: AgentSecretRevision,
        pub secret_value: OptionalFieldUpdate<serde_json::Value>
    }
}

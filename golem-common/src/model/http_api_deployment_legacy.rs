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

use super::domain_registration::Domain;
use super::environment::EnvironmentId;
use super::http_api_definition::HttpApiDefinitionName;
use super::http_api_deployment::{HttpApiDeploymentId, HttpApiDeploymentRevision};
use crate::declare_structs;
use crate::model::diff;
use chrono::DateTime;

declare_structs! {
    pub struct LegacyHttpApiDeploymentCreation {
        pub domain: Domain,
        pub api_definitions: Vec<HttpApiDefinitionName>
    }

    pub struct LegacyHttpApiDeploymentUpdate {
        pub current_revision: HttpApiDeploymentRevision,
        pub api_definitions: Option<Vec<HttpApiDefinitionName>>
    }

    pub struct LegacyHttpApiDeployment {
        pub id: HttpApiDeploymentId,
        pub revision: HttpApiDeploymentRevision,
        pub environment_id: EnvironmentId,
        pub domain: Domain,
        pub hash: diff::Hash,
        pub api_definitions: Vec<HttpApiDefinitionName>,
        pub created_at: DateTime<chrono::Utc>,
    }
}

impl LegacyHttpApiDeployment {
    pub fn to_diffable(&self) -> diff::HttpApiDeployment {
        diff::HttpApiDeployment {
            agent_types: self
                .api_definitions
                .iter()
                .map(|def| def.0.clone())
                .collect(),
        }
    }
}

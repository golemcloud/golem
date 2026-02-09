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

use super::security_scheme::SecuritySchemeName;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::diff;
use crate::base_model::domain_registration::Domain;
use crate::base_model::environment::EnvironmentId;
use crate::{declare_revision, declare_structs, newtype_uuid};
use chrono::DateTime;
use std::collections::BTreeMap;

newtype_uuid!(HttpApiDeploymentId);

declare_revision!(HttpApiDeploymentRevision);

declare_structs! {
    #[derive(Default)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct HttpApiDeploymentAgentOptions {
        /// Security scheme to use for all agent methods that require auth.
        /// Failure to provide a security scheme for an agent that requires one will lead to a deployment failure.
        /// If the requested security scheme does not exist in the environment, the route will be disabled at runtime.
        pub security_scheme: Option<SecuritySchemeName>
    }

    pub struct HttpApiDeploymentCreation {
        pub domain: Domain,
        /// webhooks url to use for agents deployed to this domain. Defaults to `/webhooks/` if not provided.
        pub webhooks_url: Option<String>,
        pub agents: BTreeMap<AgentTypeName, HttpApiDeploymentAgentOptions>
    }

    pub struct HttpApiDeploymentUpdate {
        pub current_revision: HttpApiDeploymentRevision,
        pub agents: Option<BTreeMap<AgentTypeName, HttpApiDeploymentAgentOptions>>
    }

    pub struct HttpApiDeployment {
        pub id: HttpApiDeploymentId,
        pub revision: HttpApiDeploymentRevision,
        pub environment_id: EnvironmentId,
        pub domain: Domain,
        pub hash: diff::Hash,
        pub agents: BTreeMap<AgentTypeName, HttpApiDeploymentAgentOptions>,
        pub webhooks_url: String,
        pub created_at: DateTime<chrono::Utc>,
    }
}

impl HttpApiDeploymentCreation {
    pub fn default_webhooks_url() -> String {
        "/webhooks/".to_string()
    }
}

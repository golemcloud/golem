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
use crate::{declare_revision, declare_structs, declare_unions, newtype_uuid};
use chrono::DateTime;
use std::collections::BTreeMap;

newtype_uuid!(HttpApiDeploymentId);

declare_revision!(HttpApiDeploymentRevision);

declare_unions! {
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub enum HttpApiDeploymentAgentSecurity {
        TestSessionHeader(TestSessionHeaderAgentSecurity),
        SecurityScheme(SecuritySchemeAgentSecurity)
    }
}

declare_structs! {
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    /// Header that can be used to provide the oidc session directly to the agent through http apis.
    /// Failure to provide the header will result in a 401 response.
    pub struct TestSessionHeaderAgentSecurity {
        pub header_name: String
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    /// OIDC security scheme in the environment that should be used for the agent.
    /// If the requested security scheme does not exist in the environment, the route will be disabled at runtime.
    pub struct SecuritySchemeAgentSecurity {
        pub security_scheme: SecuritySchemeName
    }

    #[derive(Default)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct HttpApiDeploymentAgentOptions {
        /// Security option to use for all agent methods that require auth.
        /// Failure to provide a security option for an agent that requires one will lead to a deployment failure.
        pub security: Option<HttpApiDeploymentAgentSecurity>
    }

    pub struct HttpApiDeploymentCreation {
        pub domain: Domain,
        pub webhooks_url: String,
        pub agents: BTreeMap<AgentTypeName, HttpApiDeploymentAgentOptions>
    }

    pub struct HttpApiDeploymentUpdate {
        pub current_revision: HttpApiDeploymentRevision,
        pub webhook_url: Option<String>,
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

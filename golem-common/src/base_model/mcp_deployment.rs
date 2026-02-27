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

use crate::base_model::agent::AgentTypeName;
use crate::base_model::diff;
use crate::base_model::domain_registration::Domain;
use crate::base_model::environment::EnvironmentId;
use crate::{declare_revision, declare_structs, newtype_uuid};
use chrono::DateTime;
use std::collections::BTreeMap;

newtype_uuid!(McpDeploymentId);

declare_revision!(McpDeploymentRevision);

declare_structs! {
    #[derive(Default)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub struct McpDeploymentAgentOptions {
        // TODO: MCP agent configuration options coming soon
    }

    pub struct McpDeploymentCreation {
        pub domain: Domain,
        pub agents: BTreeMap<AgentTypeName, McpDeploymentAgentOptions>,
    }

    pub struct McpDeploymentUpdate {
        pub current_revision: McpDeploymentRevision,
        pub domain: Option<Domain>,
        pub agents: Option<BTreeMap<AgentTypeName, McpDeploymentAgentOptions>>,
    }

    pub struct McpDeployment {
        pub id: McpDeploymentId,
        pub revision: McpDeploymentRevision,
        pub environment_id: EnvironmentId,
        pub domain: Domain,
        pub hash: diff::Hash,
        pub agents: BTreeMap<AgentTypeName, McpDeploymentAgentOptions>,
        pub created_at: DateTime<chrono::Utc>,
    }
}

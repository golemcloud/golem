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

use super::owner::{
    EmptyOwnerPattern, PolymorphicAccountOwnerPattern, PolymorphicAgentOwnerPattern,
    PolymorphicComponentOwnerPattern, PolymorphicEmptyOwnerPattern,
    PolymorphicEnvironmentOwnerPattern, PolymorphicToolOwnerPattern,
};
use super::recipient::RecipientPattern;
use super::{
    AgentResourcePattern, BlobResourcePattern, CardResourcePattern, ComponentResourcePattern,
    ConfigResourcePattern, EnvResourcePattern, EnvironmentResourcePattern,
    FilesystemResourcePattern, KvResourcePattern, NetworkResourcePattern, OplogResourcePattern,
    PolymorphicClassPermissionPattern, PolymorphicPermissionPattern, RdbmsResourcePattern,
    SecretResourcePattern, ToolResourcePattern,
};

pub fn default_agent_initial_permission_grants(
    recipient: RecipientPattern,
) -> Vec<PolymorphicPermissionPattern> {
    vec![
        PolymorphicPermissionPattern::Filesystem(PolymorphicClassPermissionPattern {
            owner: PolymorphicAgentOwnerPattern::Agent,
            recipient: recipient.clone(),
            verb: None,
            resource: FilesystemResourcePattern::any(),
        }),
        PolymorphicPermissionPattern::Network(PolymorphicClassPermissionPattern {
            owner: PolymorphicEmptyOwnerPattern::Concrete(EmptyOwnerPattern),
            recipient: recipient.clone(),
            verb: None,
            resource: NetworkResourcePattern::Any,
        }),
        PolymorphicPermissionPattern::Env(PolymorphicClassPermissionPattern {
            owner: PolymorphicAgentOwnerPattern::Agent,
            recipient: recipient.clone(),
            verb: None,
            resource: EnvResourcePattern::Any,
        }),
        PolymorphicPermissionPattern::Oplog(PolymorphicClassPermissionPattern {
            owner: PolymorphicAgentOwnerPattern::Agent,
            recipient: recipient.clone(),
            verb: None,
            resource: OplogResourcePattern::Any,
        }),
        PolymorphicPermissionPattern::Config(PolymorphicClassPermissionPattern {
            owner: PolymorphicAgentOwnerPattern::Agent,
            recipient: recipient.clone(),
            verb: None,
            resource: ConfigResourcePattern::Any,
        }),
        PolymorphicPermissionPattern::Secret(PolymorphicClassPermissionPattern {
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: recipient.clone(),
            verb: None,
            resource: SecretResourcePattern::Any,
        }),
        PolymorphicPermissionPattern::Agent(PolymorphicClassPermissionPattern {
            owner: PolymorphicAgentOwnerPattern::EnvAgents,
            recipient: recipient.clone(),
            verb: None,
            resource: AgentResourcePattern::Any,
        }),
        PolymorphicPermissionPattern::Environment(PolymorphicClassPermissionPattern {
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: recipient.clone(),
            verb: None,
            resource: EnvironmentResourcePattern::Any,
        }),
        PolymorphicPermissionPattern::Component(PolymorphicClassPermissionPattern {
            owner: PolymorphicComponentOwnerPattern::Component,
            recipient: recipient.clone(),
            verb: None,
            resource: ComponentResourcePattern::Any,
        }),
        PolymorphicPermissionPattern::Tool(PolymorphicClassPermissionPattern {
            owner: PolymorphicToolOwnerPattern::EnvTools,
            recipient: recipient.clone(),
            verb: None,
            resource: ToolResourcePattern::any(),
        }),
        PolymorphicPermissionPattern::Kv(PolymorphicClassPermissionPattern {
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: recipient.clone(),
            verb: None,
            resource: KvResourcePattern::any(),
        }),
        PolymorphicPermissionPattern::Blob(PolymorphicClassPermissionPattern {
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: recipient.clone(),
            verb: None,
            resource: BlobResourcePattern::any(),
        }),
        PolymorphicPermissionPattern::Rdbms(PolymorphicClassPermissionPattern {
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: recipient.clone(),
            verb: None,
            resource: RdbmsResourcePattern::any(),
        }),
        PolymorphicPermissionPattern::Card(PolymorphicClassPermissionPattern {
            owner: PolymorphicAccountOwnerPattern::Account,
            recipient,
            verb: None,
            resource: CardResourcePattern::Any,
        }),
    ]
}

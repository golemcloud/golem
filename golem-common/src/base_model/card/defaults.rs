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
    PolymorphicAgentOwnerPattern, PolymorphicComponentOwnerPattern,
    PolymorphicEnvironmentOwnerPattern,
};
use super::recipient::RecipientPattern;
use super::{
    AgentResourcePattern, AgentVerb, ComponentResourcePattern, ComponentVerb, EnvResourcePattern,
    EnvVarName, EnvVerb, EnvironmentResourcePattern, EnvironmentVerb,
    PolymorphicClassPermissionPattern, PolymorphicPermissionPattern,
};

pub fn default_agent_initial_permission_grants(
    recipient: RecipientPattern,
) -> Vec<PolymorphicPermissionPattern> {
    let mut grants = vec![
        PolymorphicPermissionPattern::Environment(PolymorphicClassPermissionPattern {
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: recipient.clone(),
            verb: Some(EnvironmentVerb::View),
            resource: EnvironmentResourcePattern::Any,
        }),
        PolymorphicPermissionPattern::Component(PolymorphicClassPermissionPattern {
            owner: PolymorphicComponentOwnerPattern::Component,
            recipient: recipient.clone(),
            verb: Some(ComponentVerb::View),
            resource: ComponentResourcePattern::Any,
        }),
        agent_permission(
            AgentVerb::View,
            AgentResourcePattern::Any,
            recipient.clone(),
        ),
        agent_permission(
            AgentVerb::Invoke,
            AgentResourcePattern::Any,
            recipient.clone(),
        ),
    ];
    grants.extend(
        [
            "GOLEM_AGENT_ID",
            "GOLEM_AGENT_TYPE",
            "GOLEM_WORKER_NAME",
            "GOLEM_COMPONENT_ID",
            "GOLEM_COMPONENT_REVISION",
        ]
        .into_iter()
        .map(|name| env_permission(name, recipient.clone())),
    );
    grants
}

fn agent_permission(
    verb: AgentVerb,
    resource: AgentResourcePattern,
    recipient: RecipientPattern,
) -> PolymorphicPermissionPattern {
    PolymorphicPermissionPattern::Agent(PolymorphicClassPermissionPattern {
        owner: PolymorphicAgentOwnerPattern::EnvAgents,
        recipient,
        verb: Some(verb),
        resource,
    })
}

fn env_permission(name: &str, recipient: RecipientPattern) -> PolymorphicPermissionPattern {
    PolymorphicPermissionPattern::Env(PolymorphicClassPermissionPattern {
        owner: PolymorphicAgentOwnerPattern::Agent,
        recipient,
        verb: Some(EnvVerb::Read),
        resource: EnvResourcePattern::VarName(EnvVarName(name.to_string())),
    })
}

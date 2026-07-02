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
    AgentResourcePattern, AgentVerb, ComponentResourcePattern, ComponentVerb,
    EnvironmentResourcePattern, EnvironmentVerb, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern,
};

pub fn default_agent_initial_permission_grants(
    recipient: RecipientPattern,
) -> Vec<PolymorphicPermissionPattern> {
    vec![
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
        agent_permission(AgentVerb::View, recipient.clone()),
        agent_permission(AgentVerb::Invoke, recipient.clone()),
        agent_permission(AgentVerb::Resume, recipient.clone()),
        agent_permission(AgentVerb::UpdateRevision, recipient),
    ]
}

fn agent_permission(verb: AgentVerb, recipient: RecipientPattern) -> PolymorphicPermissionPattern {
    PolymorphicPermissionPattern::Agent(PolymorphicClassPermissionPattern {
        owner: PolymorphicAgentOwnerPattern::EnvAgents,
        recipient,
        verb: Some(verb),
        resource: AgentResourcePattern::Any,
    })
}

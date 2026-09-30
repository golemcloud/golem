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

use super::monomorphization::{
    AgentPermissionMonomorphizationContext, resolve_permissions_for_agent_context,
};
use super::owner::{AgentOwnerPattern, PolymorphicAgentOwnerPattern};
use super::recipient::{RecipientOwnerContext, RecipientPattern};
use super::{PermissionPattern, PolymorphicPermissionPattern};
use crate::model::account::AccountEmail;
use crate::model::agent::AgentTypeName;
use crate::model::application::ApplicationName;
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentName;

pub fn permission_envelopes_for_recipient_patterns(
    permissions: &[PolymorphicPermissionPattern],
) -> Result<Vec<PermissionPattern>, String> {
    permission_envelopes_for_recipient_patterns_with_polarity(permissions, true)
}

/// Approximates negative permissions for attenuation validation without widening
/// instance-relative external tool owner denials beyond their runtime scope.
pub fn negative_permission_envelopes_for_recipient_patterns(
    permissions: &[PolymorphicPermissionPattern],
) -> Result<Vec<PermissionPattern>, String> {
    permission_envelopes_for_recipient_patterns_with_polarity(permissions, false)
}

fn permission_envelopes_for_recipient_patterns_with_polarity(
    permissions: &[PolymorphicPermissionPattern],
    positive: bool,
) -> Result<Vec<PermissionPattern>, String> {
    let mut result = Vec::new();
    for permission in permissions {
        let context = recipient_envelope_context(permission.recipient());
        let instance_relative_external_owner = matches!(
            permission.recipient(),
            RecipientPattern::ComponentExternalToolOwner { .. }
        ) && uses_context_agent_owner(permission);
        if !positive && instance_relative_external_owner {
            continue;
        }
        let mut resolved =
            resolve_permissions_for_agent_context(std::slice::from_ref(permission), &context);
        if instance_relative_external_owner {
            resolved.iter_mut().for_each(|permission| {
                widen_agent_owner_to_component(permission, &context);
            });
        }
        result.append(&mut resolved);
    }
    Ok(result)
}

fn uses_context_agent_owner(permission: &PolymorphicPermissionPattern) -> bool {
    (match permission {
        PolymorphicPermissionPattern::Filesystem(pattern) => &pattern.owner,
        PolymorphicPermissionPattern::Env(pattern) => &pattern.owner,
        PolymorphicPermissionPattern::Oplog(pattern) => &pattern.owner,
        PolymorphicPermissionPattern::Config(pattern) => &pattern.owner,
        PolymorphicPermissionPattern::Agent(pattern) => &pattern.owner,
        _ => return false,
    }) == &PolymorphicAgentOwnerPattern::Agent
}

fn widen_agent_owner_to_component(
    permission: &mut PermissionPattern,
    context: &AgentPermissionMonomorphizationContext,
) {
    let component_owner = AgentOwnerPattern::ComponentAgents {
        account: context.account.clone(),
        application: context.application.clone(),
        environment: context.environment.clone(),
        component: context.component.clone(),
    };

    macro_rules! widen {
        ($pattern:expr) => {{ $pattern.owner = component_owner.clone() }};
    }

    match permission {
        PermissionPattern::Filesystem(pattern) => widen!(pattern),
        PermissionPattern::Env(pattern) => widen!(pattern),
        PermissionPattern::Oplog(pattern) => widen!(pattern),
        PermissionPattern::Config(pattern) => widen!(pattern),
        PermissionPattern::Agent(pattern) => widen!(pattern),
        _ => {}
    }
}

fn recipient_envelope_context(
    recipient: &RecipientPattern,
) -> AgentPermissionMonomorphizationContext {
    let wildcard_account = || AccountEmail::new("*");
    let wildcard_application = || ApplicationName("*".to_string());
    let wildcard_environment = || EnvironmentName("*".to_string());
    let wildcard_component = || ComponentName("*".to_string());
    let wildcard_agent_type = || AgentTypeName("*".to_string());

    match recipient {
        RecipientPattern::Any => AgentPermissionMonomorphizationContext {
            account: wildcard_account(),
            application: wildcard_application(),
            environment: wildcard_environment(),
            component: wildcard_component(),
            agent_name: "*".to_string(),
            owner: RecipientOwnerContext::AgentType(wildcard_agent_type()),
        },
        RecipientPattern::Account { account }
        | RecipientPattern::AccountEnvironments { account }
        | RecipientPattern::AccountAgents { account } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: wildcard_application(),
            environment: wildcard_environment(),
            component: wildcard_component(),
            agent_name: "*".to_string(),
            owner: RecipientOwnerContext::AgentType(wildcard_agent_type()),
        },
        RecipientPattern::ApplicationEnvironments {
            account,
            application,
        }
        | RecipientPattern::ApplicationAgents {
            account,
            application,
        } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: application.clone(),
            environment: wildcard_environment(),
            component: wildcard_component(),
            agent_name: "*".to_string(),
            owner: RecipientOwnerContext::AgentType(wildcard_agent_type()),
        },
        RecipientPattern::Environment {
            account,
            application,
            environment,
        }
        | RecipientPattern::EnvironmentAgents {
            account,
            application,
            environment,
        } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: application.clone(),
            environment: environment.clone(),
            component: wildcard_component(),
            agent_name: "*".to_string(),
            owner: RecipientOwnerContext::AgentType(wildcard_agent_type()),
        },
        RecipientPattern::ComponentAgents {
            account,
            application,
            environment,
            component,
        } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: application.clone(),
            environment: environment.clone(),
            component: component.clone(),
            agent_name: "*".to_string(),
            owner: RecipientOwnerContext::AgentType(wildcard_agent_type()),
        },
        RecipientPattern::ComponentExternalToolOwner {
            account,
            application,
            environment,
            component,
        } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: application.clone(),
            environment: environment.clone(),
            component: component.clone(),
            agent_name: "~external-tool-owner".to_string(),
            owner: RecipientOwnerContext::ComponentExternalToolOwner,
        },
        RecipientPattern::Agent {
            account,
            application,
            environment,
            component,
            agent_type,
        } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: application.clone(),
            environment: environment.clone(),
            component: component.clone(),
            agent_name: format!("{}(*)", agent_type.0),
            owner: RecipientOwnerContext::AgentType(agent_type.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::card::{parse_permission, parse_polymorphic_permission};
    use test_r::test;

    const RECIPIENT: &str = "owner@example.com/app/prod/component/~external-tool-owner";
    const RUNTIME_OWNER: &str = "~golem-external-tool-owner-01234567-89ab-cdef-0123-456789abcdef";

    #[test]
    fn external_tool_owner_envelope_uses_component_wide_agent_authority() {
        let permission =
            parse_polymorphic_permission(&format!("filesystem(?agent) @ {RECIPIENT} : read : /**"))
                .unwrap();

        let envelope = permission_envelopes_for_recipient_patterns(&[permission]).unwrap();
        let PermissionPattern::Filesystem(pattern) = &envelope[0] else {
            panic!("expected filesystem permission")
        };
        assert!(matches!(
            pattern.owner,
            AgentOwnerPattern::ComponentAgents { .. }
        ));

        let runtime_target = parse_permission(&format!(
            "filesystem(owner@example.com/app/prod/component/{RUNTIME_OWNER}) @ {RECIPIENT} : read : /**"
        ))
        .unwrap();
        assert!(envelope[0].subsumes(&runtime_target));
    }

    #[test]
    fn negative_external_tool_owner_envelope_omits_instance_relative_denial() {
        let permission =
            parse_polymorphic_permission(&format!("filesystem(?agent) @ {RECIPIENT} : read : /**"))
                .unwrap();

        let envelope = negative_permission_envelopes_for_recipient_patterns(&[permission]).unwrap();

        assert!(envelope.is_empty());
    }

    #[test]
    fn negative_external_tool_owner_envelope_retains_concrete_denial() {
        let permission = parse_polymorphic_permission(&format!(
            "filesystem(owner@example.com/app/prod/component/*) @ {RECIPIENT} : read : /**"
        ))
        .unwrap();

        let envelope = negative_permission_envelopes_for_recipient_patterns(&[permission]).unwrap();

        assert_eq!(envelope.len(), 1);
        let PermissionPattern::Filesystem(pattern) = &envelope[0] else {
            panic!("expected filesystem permission")
        };
        assert!(matches!(
            pattern.owner,
            AgentOwnerPattern::ComponentAgents { .. }
        ));
    }

    #[test]
    fn dummy_external_tool_owner_literal_cannot_authorize_runtime_instance() {
        let dummy = parse_polymorphic_permission(&format!(
            "filesystem(owner@example.com/app/prod/component/~external-tool-owner) @ {RECIPIENT} : read : /**"
        ))
        .unwrap();
        let dummy_envelope = permission_envelopes_for_recipient_patterns(&[dummy]).unwrap();
        let runtime_target = parse_permission(&format!(
            "filesystem(owner@example.com/app/prod/component/{RUNTIME_OWNER}) @ {RECIPIENT} : read : /**"
        ))
        .unwrap();

        assert!(!dummy_envelope[0].subsumes(&runtime_target));
    }

    #[test]
    fn runtime_monomorphization_targets_generated_exact_external_tool_owner() {
        let permission =
            parse_polymorphic_permission(&format!("filesystem(?agent) @ {RECIPIENT} : read : /**"))
                .unwrap();
        let mut context = recipient_envelope_context(permission.recipient());
        context.agent_name = RUNTIME_OWNER.to_string();

        let resolved = resolve_permissions_for_agent_context(&[permission], &context);
        let PermissionPattern::Filesystem(pattern) = &resolved[0] else {
            panic!("expected filesystem permission")
        };
        assert!(matches!(
            &pattern.owner,
            AgentOwnerPattern::Agent { agent, .. }
                if *agent == super::super::owner::AgentOwnerLeafPattern::Agent(RUNTIME_OWNER.to_string())
        ));
    }
}

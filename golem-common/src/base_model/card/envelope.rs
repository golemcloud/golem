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

use super::class::card_permission_classes;
use super::monomorphization::{
    AgentPermissionMonomorphizationContext, resolve_permissions_for_agent_context,
};
use super::owner::*;
use super::recipient::{RecipientOwnerContext, RecipientPattern};
use super::{PermissionPattern, PolymorphicPermissionPattern};
use crate::model::account::AccountEmail;
use crate::model::agent::AgentTypeName;
use crate::model::application::ApplicationName;
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentName;

/// Conservative proof bounds for every holder that can bind a template card.
/// Positives overapproximate authority; only concrete negatives provide proof evidence.
/// These bounds do not replace the template's stored grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateCardEnvelope {
    pub lower_positive: Vec<PermissionPattern>,
    pub lower_negative: Vec<PermissionPattern>,
    pub upper_positive: Vec<PermissionPattern>,
    pub upper_negative: Vec<PermissionPattern>,
}

/// Bounds all instantiations admitted by the card's binding recipients.
/// Every ceiling binds at installation, including ceilings addressed to other recipients.
pub fn permission_card_envelope(
    lower_positive: &[PolymorphicPermissionPattern],
    lower_negative: &[PolymorphicPermissionPattern],
    upper_positive: &[PolymorphicPermissionPattern],
    upper_negative: &[PolymorphicPermissionPattern],
) -> Result<TemplateCardEnvelope, String> {
    let mut ceilings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let bindings = lower_positive
        .iter()
        .chain(upper_positive)
        .chain(upper_negative)
        .map(PolymorphicPermissionPattern::recipient)
        .filter(|recipient| seen.insert(*recipient))
        .collect::<Vec<_>>();
    for permission in upper_positive {
        if let Some(concrete) = permission.as_concrete() {
            ceilings.push(concrete);
        } else {
            for binding in &bindings {
                ceilings.push(permission_envelope(permission, binding));
            }
        }
    }
    Ok(TemplateCardEnvelope {
        lower_positive: permission_envelopes_for_recipient_patterns(lower_positive)?,
        lower_negative: negative_permission_envelopes_for_recipient_patterns(lower_negative)?,
        upper_positive: ceilings,
        upper_negative: negative_permission_envelopes_for_recipient_patterns(upper_negative)?,
    })
}

pub fn permission_envelopes_for_recipient_patterns(
    permissions: &[PolymorphicPermissionPattern],
) -> Result<Vec<PermissionPattern>, String> {
    permission_envelopes_for_recipient_patterns_with_polarity(permissions, true)
}

/// Only structurally concrete denials provide universal proof evidence.
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
        if positive {
            result.push(permission_envelope(permission, permission.recipient()));
        } else if let Some(concrete) = permission.as_concrete() {
            result.push(concrete);
        }
    }
    Ok(result)
}

fn permission_envelope(
    permission: &PolymorphicPermissionPattern,
    binding: &RecipientPattern,
) -> PermissionPattern {
    if let Some(concrete) = permission.as_concrete() {
        return concrete;
    }
    let context = recipient_envelope_context(binding);
    let mut resolved =
        resolve_permissions_for_agent_context(std::slice::from_ref(permission), &context).remove(0);
    if uses_context_agent_owner(permission) {
        widen_context_agent_owner(&mut resolved, &context);
    }
    macro_rules! normalize {
        ($($variant:ident: $class:ty,)+) => {
            match &mut resolved {
                $(PermissionPattern::$variant(p) => p.owner = p.owner.envelope(),)+
            }
        };
    }
    card_permission_classes!(normalize);
    resolved
}

trait EnvelopeOwner: Sized {
    fn envelope(&self) -> Self;
}

impl EnvelopeOwner for EmptyOwnerPattern {
    fn envelope(&self) -> Self {
        self.clone()
    }
}

// Synthetic context wildcards denote regions, not literal owner path segments.
macro_rules! envelope_owner {
    ($owner:ident, $any:ident, $account_variant:ident, $($variant:ident { $account:ident, $($field:ident),+ }),+ $(,)?) => {
        impl EnvelopeOwner for $owner {
            fn envelope(&self) -> Self {
                match self {
                    Self::$any => self.clone(),
                    Self::$account_variant { account } => {
                        if account.as_str() == "*" { Self::$any } else { self.clone() }
                    }
                    $(Self::$variant { $account, $($field),+ } => {
                        let _ = ($($field),+);
                        if $account.as_str() == "*" { return Self::$any; }
                        envelope_owner!(@fields $account_variant, $account; $($field),+);
                        self.clone()
                    })*
                }
            }
        }
    };
    (@fields $account_variant:ident, $account:ident; $application:ident $(, $rest:ident)*) => {
        if $application.0 == "*" { return Self::$account_variant { account: $account.clone() }; }
        envelope_owner!(@environment $account, $application; $($rest),*);
    };
    (@environment $account:ident, $application:ident;) => {};
    (@environment $account:ident, $application:ident; $environment:ident $(, $rest:ident)*) => {
        if $environment.0 == "*" { return Self::application_envelope($account.clone(), $application.clone()); }
        envelope_owner!(@component $account, $application, $environment; $($rest),*);
    };
    (@component $account:ident, $application:ident, $environment:ident;) => {};
    (@component $account:ident, $application:ident, $environment:ident; $component:ident $(, $rest:ident)*) => {
        if $component.0 == "*" { return Self::environment_envelope($account.clone(), $application.clone(), $environment.clone()); }
    };
}

macro_rules! ancestor_envelopes {
    ($owner:ident, $application_variant:ident $(, $environment_variant:ident)?) => {
        impl $owner {
            fn application_envelope(account: AccountEmail, application: ApplicationName) -> Self {
                Self::$application_variant { account, application }
            }
            $(fn environment_envelope(account: AccountEmail, application: ApplicationName, environment: EnvironmentName) -> Self {
                Self::$environment_variant { account, application, environment }
            })?
        }
    };
}

envelope_owner!(
    ApplicationOwnerPattern,
    AnyApplications,
    AccountApplications,
    Application {
        account,
        application
    }
);
envelope_owner!(
    EnvironmentOwnerPattern,
    AnyEnvironments,
    AccountEnvironments,
    ApplicationEnvironments {
        account,
        application
    },
    Environment {
        account,
        application,
        environment
    }
);
envelope_owner!(
    ComponentOwnerPattern,
    AnyComponents,
    AccountComponents,
    ApplicationComponents {
        account,
        application
    },
    EnvironmentComponents {
        account,
        application,
        environment
    },
    Component {
        account,
        application,
        environment,
        component
    }
);
envelope_owner!(
    AgentOwnerPattern,
    AnyAgents,
    AccountAgents,
    ApplicationAgents {
        account,
        application
    },
    EnvironmentAgents {
        account,
        application,
        environment
    },
    ComponentAgents {
        account,
        application,
        environment,
        component
    },
    Agent {
        account,
        application,
        environment,
        component,
        agent
    }
);
envelope_owner!(
    ToolOwnerPattern,
    AnyTools,
    AccountTools,
    ApplicationTools {
        account,
        application
    },
    EnvironmentTools {
        account,
        application,
        environment
    },
    ComponentTools {
        account,
        application,
        environment,
        component
    },
    Tool {
        account,
        application,
        environment,
        component,
        tool
    }
);
ancestor_envelopes!(EnvironmentOwnerPattern, ApplicationEnvironments);
ancestor_envelopes!(
    ComponentOwnerPattern,
    ApplicationComponents,
    EnvironmentComponents
);
ancestor_envelopes!(AgentOwnerPattern, ApplicationAgents, EnvironmentAgents);
ancestor_envelopes!(ToolOwnerPattern, ApplicationTools, EnvironmentTools);

impl EnvelopeOwner for AccountOwnerPattern {
    fn envelope(&self) -> Self {
        match self {
            Self::Account { account } if account.as_str() == "*" => Self::Any,
            _ => self.clone(),
        }
    }
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

fn widen_context_agent_owner(
    permission: &mut PermissionPattern,
    context: &AgentPermissionMonomorphizationContext,
) {
    let owner_envelope = match &context.owner {
        RecipientOwnerContext::AgentType(agent_type) if agent_type.0 != "*" => {
            AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
                agent: AgentOwnerLeafPattern::AgentTypeWildcard(agent_type.clone()),
            }
        }
        _ => AgentOwnerPattern::ComponentAgents {
            account: context.account.clone(),
            application: context.application.clone(),
            environment: context.environment.clone(),
            component: context.component.clone(),
        },
    };

    macro_rules! widen {
        ($pattern:expr) => {{ $pattern.owner = owner_envelope.clone() }};
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
    fn template_envelope_covers_broad_binding_regions() {
        for recipient in [
            "*",
            "owner@example.com/*/*/*/*",
            "owner@example.com/app/*/*/*",
            "owner@example.com/app/prod/*/*",
        ] {
            for (class, owner) in [
                ("agent", "?agent"),
                ("agent", "?component/Other(*)"),
                ("component", "?component"),
                ("environment", "?env"),
                ("application", "?app"),
                ("account", "?account"),
                ("tool", "?component/*"),
                ("network", ""),
            ] {
                let slot = super::super::parse_polymorphic_permission_fields(
                    class,
                    owner,
                    recipient,
                    "*",
                    if class == "account" { "" } else { "*" },
                )
                .unwrap();
                let mut context = recipient_envelope_context(slot.recipient());
                context.account = AccountEmail::new("owner@example.com");
                context.application = ApplicationName("app".to_string());
                context.environment = EnvironmentName("prod".to_string());
                context.component = ComponentName("component".to_string());
                context.agent_name = "Cart(alice)".to_string();
                let actual =
                    resolve_permissions_for_agent_context(std::slice::from_ref(&slot), &context);
                let envelope = permission_card_envelope(&[slot], &[], &[], &[]).unwrap();
                assert!(
                    envelope.lower_positive[0].subsumes(&actual[0]),
                    "{class}({owner}) @ {recipient}"
                );
                let converted = PolymorphicPermissionPattern::from(actual[0].clone());
                assert_eq!(converted.as_concrete(), Some(actual[0].clone()));
            }
        }
    }

    #[test]
    fn template_agent_envelope_is_structural_and_covers_actual_instances() {
        let recipient = "owner@example.com/app/prod/component/Cart";
        let slot =
            parse_polymorphic_permission(&format!("filesystem(?agent) @ {recipient} : read : /**"))
                .unwrap();
        let envelope =
            permission_card_envelope(std::slice::from_ref(&slot), &[], &[], &[]).unwrap();
        let literal = parse_permission(&format!(
            "filesystem(owner@example.com/app/prod/component/Cart%28%2A%29) @ {recipient} : read : /**"
        )).unwrap();
        let PermissionPattern::Filesystem(pattern) = &envelope.lower_positive[0] else {
            panic!()
        };
        assert!(matches!(
            &pattern.owner,
            AgentOwnerPattern::Agent {
                agent: AgentOwnerLeafPattern::AgentTypeWildcard(_),
                ..
            }
        ));
        for name in ["Cart(alice)", "Cart(bob)", "Cart(*)"] {
            let mut context = recipient_envelope_context(slot.recipient());
            context.agent_name = name.to_string();
            let actual =
                resolve_permissions_for_agent_context(std::slice::from_ref(&slot), &context);
            assert!(envelope.lower_positive[0].subsumes(&actual[0]));
            if name != "Cart(*)" {
                assert!(!literal.subsumes(&actual[0]));
            }
        }
        let converted = PolymorphicPermissionPattern::from(literal.clone());
        assert_eq!(converted.as_concrete(), Some(literal));
        assert!(!converted.has_slots());
        assert!(slot.has_slots());
    }

    #[test]
    fn template_ceilings_bind_across_recipients_and_slot_denials_give_no_proof() {
        let first = parse_polymorphic_permission(
            "filesystem(?agent) @ owner@example.com/app/prod/first/Cart : read : /**",
        )
        .unwrap();
        let second = parse_polymorphic_permission(
            "filesystem(?agent) @ owner@example.com/app/prod/second/Cart : read : /**",
        )
        .unwrap();
        let envelope = permission_card_envelope(
            std::slice::from_ref(&first),
            std::slice::from_ref(&first),
            std::slice::from_ref(&second),
            std::slice::from_ref(&first),
        )
        .unwrap();
        assert!(envelope.lower_negative.is_empty());
        assert!(envelope.upper_negative.is_empty());
        let mut context = recipient_envelope_context(first.recipient());
        context.agent_name = "Cart(alice)".to_string();
        let actual = resolve_permissions_for_agent_context(std::slice::from_ref(&second), &context);
        assert!(
            envelope
                .upper_positive
                .iter()
                .any(|p| p.subsumes(&actual[0]))
        );
        assert!(
            envelope
                .upper_positive
                .iter()
                .all(|p| p.recipient() == second.recipient())
        );
        for bounds in [
            permission_card_envelope(
                std::slice::from_ref(&first),
                &[],
                std::slice::from_ref(&second),
                &[],
            ),
            permission_card_envelope(&[], &[], &[first.clone(), second.clone()], &[]),
            permission_card_envelope(
                &[],
                &[],
                std::slice::from_ref(&second),
                std::slice::from_ref(&first),
            ),
        ] {
            assert!(
                bounds
                    .unwrap()
                    .upper_positive
                    .iter()
                    .any(|p| p.subsumes(&actual[0]))
            );
        }
        let own = permission_envelopes_for_recipient_patterns(&[second]).unwrap();
        assert!(!own[0].subsumes(&actual[0]));
        assert!(first.has_slots());
    }

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

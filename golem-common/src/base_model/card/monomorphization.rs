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

use super::class::*;
use super::owner::*;
use super::recipient::RecipientPattern;
use super::{
    Card, CardId, DelegationSurface, EffectiveSurface, PermissionPattern, PolymorphicCard,
    PolymorphicPermissionPattern, ScopeCard, StoredCard,
};
use crate::model::account::AccountEmail;
use crate::model::agent::AgentTypeName;
use crate::model::application::ApplicationName;
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentName;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPermissionMonomorphizationContext {
    pub account: AccountEmail,
    pub application: ApplicationName,
    pub environment: EnvironmentName,
    pub component: ComponentName,
    pub agent_name: String,
    pub agent_type: AgentTypeName,
}

pub fn agent_effective_surface_from_wallet<'a>(
    context: &AgentPermissionMonomorphizationContext,
    wallet_cards: impl IntoIterator<Item = &'a StoredCard>,
) -> EffectiveSurface {
    let holder = agent_recipient_pattern(context);
    let cards = monomorphize_wallet_cards(context, wallet_cards);

    EffectiveSurface::from_cards(&cards, &holder).unwrap_or_default()
}

pub fn agent_effective_surface_from_wallet_and_scope<'a>(
    context: &AgentPermissionMonomorphizationContext,
    wallet_cards: impl IntoIterator<Item = &'a StoredCard>,
    scope_card: Option<&ScopeCard>,
) -> EffectiveSurface {
    let mut surface = agent_effective_surface_from_wallet(context, wallet_cards);
    if let Some(scope_card) = scope_card {
        let holder = agent_recipient_pattern(context);
        if let Ok(mut scope_surface) = EffectiveSurface::from_grants(
            &scope_card.lower_positive,
            &scope_card.lower_negative,
            &scope_card.upper_positive,
            &scope_card.upper_negative,
            &holder,
        ) {
            surface.source_card_ids.push(scope_card.scope_card_id);
            surface.lower.append(&mut scope_surface.lower);
            surface.upper.append(&mut scope_surface.upper);
        }
    }
    surface
}

pub fn agent_delegation_surface_from_wallet<'a>(
    context: &AgentPermissionMonomorphizationContext,
    wallet_cards: impl IntoIterator<Item = &'a StoredCard>,
) -> DelegationSurface {
    DelegationSurface::from_cards(&monomorphize_wallet_cards(context, wallet_cards))
}

fn monomorphize_wallet_cards<'a>(
    context: &AgentPermissionMonomorphizationContext,
    wallet_cards: impl IntoIterator<Item = &'a StoredCard>,
) -> Vec<Card> {
    wallet_cards
        .into_iter()
        .map(|card| match card {
            StoredCard::Concrete(card) => card.clone(),
            StoredCard::Polymorphic(_) => monomorphize_card_for_agent(card, context),
        })
        .collect()
}

fn agent_recipient_pattern(context: &AgentPermissionMonomorphizationContext) -> RecipientPattern {
    RecipientPattern::Agent {
        account: context.account.clone(),
        application: context.application.clone(),
        environment: context.environment.clone(),
        component: context.component.clone(),
        agent_type: context.agent_type.clone(),
    }
}

pub fn monomorphize_card_for_agent(
    card: &StoredCard,
    context: &AgentPermissionMonomorphizationContext,
) -> Card {
    match card {
        StoredCard::Concrete(card) => card.clone(),
        StoredCard::Polymorphic(card) => monomorphize_polymorphic_card_for_agent(card, context),
    }
}

pub fn instantiate_polymorphic_card_for_agent(
    card: &PolymorphicCard,
    context: &AgentPermissionMonomorphizationContext,
    child_card_id: CardId,
    created_at: DateTime<Utc>,
) -> Card {
    Card {
        card_id: child_card_id,
        parent_ids: vec![card.card_id],
        created_at,
        ..monomorphize_polymorphic_card_for_agent(card, context)
    }
}

fn monomorphize_polymorphic_card_for_agent(
    card: &PolymorphicCard,
    context: &AgentPermissionMonomorphizationContext,
) -> Card {
    Card {
        card_id: card.card_id,
        parent_ids: card.parent_ids.clone(),
        lower_positive: resolve_permissions_for_agent_context(&card.lower_positive, context),
        lower_negative: resolve_permissions_for_agent_context(&card.lower_negative, context),
        upper_positive: resolve_permissions_for_agent_context(&card.upper_positive, context),
        upper_negative: resolve_permissions_for_agent_context(&card.upper_negative, context),
        created_at: card.created_at,
        expires_at: card.expires_at,
        system_card: card.system_card,
        managed_by: None,
    }
}

pub fn card_matches_agent_recipient(
    card: &StoredCard,
    context: &AgentPermissionMonomorphizationContext,
) -> bool {
    let holder = agent_recipient_pattern(context);
    monomorphize_card_for_agent(card, context)
        .lower_positive
        .iter()
        .any(|grant| grant.recipient().subsumes(&holder))
}

pub(crate) fn resolve_permissions_for_agent_context(
    permissions: &[PolymorphicPermissionPattern],
    context: &AgentPermissionMonomorphizationContext,
) -> Vec<PermissionPattern> {
    permissions
        .iter()
        .map(|permission| monomorphize_permission(permission, context))
        .collect()
}

macro_rules! mono_permission {
    ($variant:ident, $pattern:expr, $context:expr) => {{
        let pattern = $pattern;
        PermissionPattern::$variant(ClassPermissionPattern {
            owner: pattern.owner.monomorphize($context),
            recipient: pattern.recipient.clone(),
            verb: pattern.verb,
            resource: pattern.resource.clone(),
        })
    }};
}

fn monomorphize_permission(
    permission: &PolymorphicPermissionPattern,
    context: &AgentPermissionMonomorphizationContext,
) -> PermissionPattern {
    match permission {
        PolymorphicPermissionPattern::Filesystem(p) => mono_permission!(Filesystem, p, context),
        PolymorphicPermissionPattern::Network(p) => mono_permission!(Network, p, context),
        PolymorphicPermissionPattern::Env(p) => mono_permission!(Env, p, context),
        PolymorphicPermissionPattern::Oplog(p) => mono_permission!(Oplog, p, context),
        PolymorphicPermissionPattern::Config(p) => mono_permission!(Config, p, context),
        PolymorphicPermissionPattern::Secret(p) => mono_permission!(Secret, p, context),
        PolymorphicPermissionPattern::Agent(p) => mono_permission!(Agent, p, context),
        PolymorphicPermissionPattern::Tool(p) => mono_permission!(Tool, p, context),
        PolymorphicPermissionPattern::Kv(p) => mono_permission!(Kv, p, context),
        PolymorphicPermissionPattern::Blob(p) => mono_permission!(Blob, p, context),
        PolymorphicPermissionPattern::Rdbms(p) => mono_permission!(Rdbms, p, context),
        PolymorphicPermissionPattern::Card(p) => mono_permission!(Card, p, context),
        PolymorphicPermissionPattern::System(p) => mono_permission!(System, p, context),
        PolymorphicPermissionPattern::Plan(p) => mono_permission!(Plan, p, context),
        PolymorphicPermissionPattern::Account(p) => mono_permission!(Account, p, context),
        PolymorphicPermissionPattern::AccountUsage(p) => mono_permission!(AccountUsage, p, context),
        PolymorphicPermissionPattern::AccountToken(p) => mono_permission!(AccountToken, p, context),
        PolymorphicPermissionPattern::AccountPlugin(p) => {
            mono_permission!(AccountPlugin, p, context)
        }
        PolymorphicPermissionPattern::Application(p) => mono_permission!(Application, p, context),
        PolymorphicPermissionPattern::Environment(p) => mono_permission!(Environment, p, context),
        PolymorphicPermissionPattern::EnvironmentPluginGrant(p) => {
            mono_permission!(EnvironmentPluginGrant, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentDomainRegistration(p) => {
            mono_permission!(EnvironmentDomainRegistration, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentSecurityScheme(p) => {
            mono_permission!(EnvironmentSecurityScheme, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentHttpApiDeployment(p) => {
            mono_permission!(EnvironmentHttpApiDeployment, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentMcpDeployment(p) => {
            mono_permission!(EnvironmentMcpDeployment, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentAgentSecret(p) => {
            mono_permission!(EnvironmentAgentSecret, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentResourceDefinition(p) => {
            mono_permission!(EnvironmentResourceDefinition, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentRetryPolicy(p) => {
            mono_permission!(EnvironmentRetryPolicy, p, context)
        }
        PolymorphicPermissionPattern::Component(p) => mono_permission!(Component, p, context),
        PolymorphicPermissionPattern::AccountOauth2Identity(p) => {
            mono_permission!(AccountOauth2Identity, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentInitialFiles(p) => {
            mono_permission!(EnvironmentInitialFiles, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentKvBucket(p) => {
            mono_permission!(EnvironmentKvBucket, p, context)
        }
        PolymorphicPermissionPattern::EnvironmentBlobBucket(p) => {
            mono_permission!(EnvironmentBlobBucket, p, context)
        }
        PolymorphicPermissionPattern::AccountPermissionShare(p) => {
            mono_permission!(AccountPermissionShare, p, context)
        }
    }
}

trait MonomorphizeOwner<T> {
    fn monomorphize(&self, context: &AgentPermissionMonomorphizationContext) -> T;
}

impl MonomorphizeOwner<EmptyOwnerPattern> for PolymorphicEmptyOwnerPattern {
    fn monomorphize(&self, _context: &AgentPermissionMonomorphizationContext) -> EmptyOwnerPattern {
        match self {
            Self::Concrete(owner) => owner.clone(),
        }
    }
}

impl MonomorphizeOwner<AccountOwnerPattern> for PolymorphicAccountOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> AccountOwnerPattern {
        match self {
            Self::Concrete(owner) => owner.clone(),
            Self::Account => AccountOwnerPattern::Account {
                account: context.account.clone(),
            },
        }
    }
}

impl MonomorphizeOwner<ApplicationOwnerPattern> for PolymorphicApplicationOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> ApplicationOwnerPattern {
        match self {
            Self::Concrete(owner) => owner.clone(),
            Self::AccountApplications => ApplicationOwnerPattern::AccountApplications {
                account: context.account.clone(),
            },
            Self::AccountApplication { application } => ApplicationOwnerPattern::Application {
                account: context.account.clone(),
                application: application.clone(),
            },
            Self::App => ApplicationOwnerPattern::Application {
                account: context.account.clone(),
                application: context.application.clone(),
            },
        }
    }
}

impl MonomorphizeOwner<EnvironmentOwnerPattern> for PolymorphicEnvironmentOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> EnvironmentOwnerPattern {
        match self {
            Self::Concrete(owner) => owner.clone(),
            Self::AccountEnvironments => EnvironmentOwnerPattern::AccountEnvironments {
                account: context.account.clone(),
            },
            Self::AccountApplicationEnvironments { application } => {
                EnvironmentOwnerPattern::ApplicationEnvironments {
                    account: context.account.clone(),
                    application: application.clone(),
                }
            }
            Self::AccountEnvironment {
                application,
                environment,
            } => EnvironmentOwnerPattern::Environment {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
            },
            Self::ApplicationEnvironments => EnvironmentOwnerPattern::ApplicationEnvironments {
                account: context.account.clone(),
                application: context.application.clone(),
            },
            Self::ApplicationEnvironment { environment } => EnvironmentOwnerPattern::Environment {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
            },
            Self::Env => EnvironmentOwnerPattern::Environment {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
            },
        }
    }
}

impl MonomorphizeOwner<ComponentOwnerPattern> for PolymorphicComponentOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> ComponentOwnerPattern {
        match self {
            Self::Concrete(owner) => owner.clone(),
            Self::AccountComponents => ComponentOwnerPattern::AccountComponents {
                account: context.account.clone(),
            },
            Self::AccountApplicationComponents { application } => {
                ComponentOwnerPattern::ApplicationComponents {
                    account: context.account.clone(),
                    application: application.clone(),
                }
            }
            Self::AccountEnvironmentComponents {
                application,
                environment,
            } => ComponentOwnerPattern::EnvironmentComponents {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
            },
            Self::AccountComponent {
                application,
                environment,
                component,
            } => ComponentOwnerPattern::Component {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            },
            Self::ApplicationComponents => ComponentOwnerPattern::ApplicationComponents {
                account: context.account.clone(),
                application: context.application.clone(),
            },
            Self::ApplicationEnvironmentComponents { environment } => {
                ComponentOwnerPattern::EnvironmentComponents {
                    account: context.account.clone(),
                    application: context.application.clone(),
                    environment: environment.clone(),
                }
            }
            Self::ApplicationComponent {
                environment,
                component,
            } => ComponentOwnerPattern::Component {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            },
            Self::EnvComponents => ComponentOwnerPattern::EnvironmentComponents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
            },
            Self::EnvComponent { component } => ComponentOwnerPattern::Component {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
            },
            Self::Component => ComponentOwnerPattern::Component {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
            },
        }
    }
}

impl MonomorphizeOwner<AgentOwnerPattern> for PolymorphicAgentOwnerPattern {
    fn monomorphize(&self, context: &AgentPermissionMonomorphizationContext) -> AgentOwnerPattern {
        match self {
            Self::Concrete(owner) => owner.clone(),
            Self::AccountAgents => AgentOwnerPattern::AccountAgents {
                account: context.account.clone(),
            },
            Self::AccountApplicationAgents { application } => {
                AgentOwnerPattern::ApplicationAgents {
                    account: context.account.clone(),
                    application: application.clone(),
                }
            }
            Self::AccountEnvironmentAgents {
                application,
                environment,
            } => AgentOwnerPattern::EnvironmentAgents {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
            },
            Self::AccountComponentAgents {
                application,
                environment,
                component,
            } => AgentOwnerPattern::ComponentAgents {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            },
            Self::AccountAgent {
                application,
                environment,
                component,
                agent,
            } => AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
                agent: agent.clone(),
            },
            Self::ApplicationAgents => AgentOwnerPattern::ApplicationAgents {
                account: context.account.clone(),
                application: context.application.clone(),
            },
            Self::ApplicationEnvironmentAgents { environment } => {
                AgentOwnerPattern::EnvironmentAgents {
                    account: context.account.clone(),
                    application: context.application.clone(),
                    environment: environment.clone(),
                }
            }
            Self::ApplicationComponentAgents {
                environment,
                component,
            } => AgentOwnerPattern::ComponentAgents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            },
            Self::ApplicationAgent {
                environment,
                component,
                agent,
            } => AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
                agent: agent.clone(),
            },
            Self::EnvAgents => AgentOwnerPattern::EnvironmentAgents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
            },
            Self::EnvComponentAgents { component } => AgentOwnerPattern::ComponentAgents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
            },
            Self::EnvAgent { component, agent } => AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
                agent: agent.clone(),
            },
            Self::ComponentAgents => AgentOwnerPattern::ComponentAgents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
            },
            Self::ComponentAgent { agent } => AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
                agent: agent.clone(),
            },
            Self::Agent => AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
                agent: AgentOwnerLeafPattern::Agent(context.agent_name.clone()),
            },
        }
    }
}

impl MonomorphizeOwner<ToolOwnerPattern> for PolymorphicToolOwnerPattern {
    fn monomorphize(&self, context: &AgentPermissionMonomorphizationContext) -> ToolOwnerPattern {
        match self {
            Self::Concrete(owner) => owner.clone(),
            Self::AccountTools => ToolOwnerPattern::AccountTools {
                account: context.account.clone(),
            },
            Self::AccountApplicationTools { application } => ToolOwnerPattern::ApplicationTools {
                account: context.account.clone(),
                application: application.clone(),
            },
            Self::AccountEnvironmentTools {
                application,
                environment,
            } => ToolOwnerPattern::EnvironmentTools {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
            },
            Self::AccountComponentTools {
                application,
                environment,
                component,
            } => ToolOwnerPattern::ComponentTools {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            },
            Self::AccountTool {
                application,
                environment,
                component,
                tool,
            } => ToolOwnerPattern::Tool {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
                tool: tool.clone(),
            },
            Self::ApplicationTools => ToolOwnerPattern::ApplicationTools {
                account: context.account.clone(),
                application: context.application.clone(),
            },
            Self::ApplicationEnvironmentTools { environment } => {
                ToolOwnerPattern::EnvironmentTools {
                    account: context.account.clone(),
                    application: context.application.clone(),
                    environment: environment.clone(),
                }
            }
            Self::ApplicationComponentTools {
                environment,
                component,
            } => ToolOwnerPattern::ComponentTools {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            },
            Self::ApplicationTool {
                environment,
                component,
                tool,
            } => ToolOwnerPattern::Tool {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
                tool: tool.clone(),
            },
            Self::EnvTools => ToolOwnerPattern::EnvironmentTools {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
            },
            Self::EnvComponentTools { component } => ToolOwnerPattern::ComponentTools {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
            },
            Self::EnvTool { component, tool } => ToolOwnerPattern::Tool {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
                tool: tool.clone(),
            },
            Self::ComponentTools => ToolOwnerPattern::ComponentTools {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
            },
            Self::ComponentTool { tool } => ToolOwnerPattern::Tool {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
                tool: tool.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::account::AccountEmail;
    use crate::model::agent::AgentTypeName;
    use crate::model::application::ApplicationName;
    use crate::model::card::recipient::RecipientPattern;
    use crate::model::card::{
        AgentClass, AgentResourcePattern, AgentVerb, CardId, ClassPermissionTarget, ComponentClass,
        ComponentResourcePattern, ComponentVerb, EnvironmentClass, EnvironmentResourcePattern,
        EnvironmentVerb, PermissionTarget, PolymorphicCard, PolymorphicClassPermissionPattern,
        PolymorphicPermissionPattern, StoredCard,
    };
    use crate::model::card::{default_agent_initial_permission_grants, parse_permission};
    use crate::model::component::ComponentName;
    use crate::model::environment::EnvironmentName;

    use test_r::test;

    fn context() -> AgentPermissionMonomorphizationContext {
        AgentPermissionMonomorphizationContext {
            account: AccountEmail::from("owner@example.com"),
            application: ApplicationName::try_from("shop").unwrap(),
            environment: EnvironmentName::try_from("prod").unwrap(),
            component: ComponentName("cart-svc".to_string()),
            agent_name: "Cart(alice)".to_string(),
            agent_type: AgentTypeName("Cart".to_string()),
        }
    }

    fn environment_view_target(owner: &str) -> PermissionTarget {
        environment_target(owner, EnvironmentVerb::View)
    }

    fn environment_target(owner: &str, verb: EnvironmentVerb) -> PermissionTarget {
        PermissionTarget::Environment(ClassPermissionTarget::<EnvironmentClass> {
            verb: Some(verb),
            owner: EnvironmentOwnerPattern::parse(owner).unwrap(),
            resource: EnvironmentResourcePattern::Any,
        })
    }

    fn component_view_target(owner: &str) -> PermissionTarget {
        component_target(owner, ComponentVerb::View)
    }

    fn component_target(owner: &str, verb: ComponentVerb) -> PermissionTarget {
        PermissionTarget::Component(ClassPermissionTarget::<ComponentClass> {
            verb: Some(verb),
            owner: ComponentOwnerPattern::parse(owner).unwrap(),
            resource: ComponentResourcePattern::Any,
        })
    }

    fn agent_target(owner: &str, verb: AgentVerb) -> PermissionTarget {
        PermissionTarget::Agent(ClassPermissionTarget::<AgentClass> {
            verb: Some(verb),
            owner: AgentOwnerPattern::parse(owner).unwrap(),
            resource: AgentResourcePattern::Any,
        })
    }

    #[test]
    fn invocation_scope_card_contributes_lower_and_upper_authority() {
        let context = context();
        let scope_card_id = CardId(uuid::Uuid::from_u128(10));
        let grant = parse_permission(
            "agent(owner@example.com/shop/prod/cart-svc/Cart(*)) @ owner@example.com/shop/prod/cart-svc/Cart : view : *",
        )
        .unwrap();
        let scope_card = ScopeCard {
            scope_card_id,
            root_card_ids: vec![CardId(uuid::Uuid::from_u128(11))],
            lower_positive: vec![grant.clone()],
            lower_negative: Vec::new(),
            upper_positive: vec![grant],
            upper_negative: Vec::new(),
        };

        let without_scope = agent_effective_surface_from_wallet(&context, std::iter::empty());
        let with_scope = agent_effective_surface_from_wallet_and_scope(
            &context,
            std::iter::empty(),
            Some(&scope_card),
        );
        let request = agent_target(
            "owner@example.com/shop/prod/cart-svc/Cart(alice)",
            AgentVerb::View,
        );

        assert!(!without_scope.authorize(&request).unwrap());
        assert!(with_scope.authorize(&request).unwrap());
        assert_eq!(with_scope.source_card_ids, vec![scope_card_id]);
        assert_eq!(with_scope.lower.len(), 1);
        assert_eq!(with_scope.upper.len(), 1);
    }

    #[test]
    fn upper_only_invocation_scope_card_clamps_persistent_authority() {
        let context = context();
        let persistent_grant = parse_permission(
            "agent(owner@example.com/shop/prod/cart-svc/Cart(*)) @ owner@example.com/shop/prod/cart-svc/Cart : view : *",
        )
        .unwrap();
        let scope_ceiling = parse_permission(
            "agent(owner@example.com/shop/prod/cart-svc/Cart(*)) @ owner@example.com/shop/prod/cart-svc/Cart : invoke : *",
        )
        .unwrap();
        let persistent_card = StoredCard::Concrete(Card {
            card_id: CardId(uuid::Uuid::from_u128(20)),
            parent_ids: Vec::new(),
            lower_positive: vec![persistent_grant],
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: chrono::Utc::now(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        });
        let scope_card = ScopeCard {
            scope_card_id: CardId(uuid::Uuid::from_u128(21)),
            root_card_ids: vec![persistent_card.card_id()],
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: vec![scope_ceiling],
            upper_negative: Vec::new(),
        };
        let request = agent_target(
            "owner@example.com/shop/prod/cart-svc/Cart(alice)",
            AgentVerb::View,
        );

        assert!(
            agent_effective_surface_from_wallet(&context, [&persistent_card])
                .authorize(&request)
                .unwrap()
        );
        assert!(
            !agent_effective_surface_from_wallet_and_scope(
                &context,
                [&persistent_card],
                Some(&scope_card),
            )
            .authorize(&request)
            .unwrap()
        );
    }

    #[test]
    fn monomorphizes_holder_relative_agent_initial_card_slots() {
        let context = context();
        let recipient = RecipientPattern::Any;
        let card_id = CardId(uuid::Uuid::from_u128(1));
        let card = PolymorphicCard {
            card_id,
            parent_ids: Vec::new(),
            lower_positive: vec![
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
                PolymorphicPermissionPattern::Agent(PolymorphicClassPermissionPattern {
                    owner: PolymorphicAgentOwnerPattern::Agent,
                    recipient,
                    verb: Some(AgentVerb::View),
                    resource: AgentResourcePattern::Any,
                }),
            ],
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: chrono::Utc::now(),
            expires_at: None,
            system_card: false,
        };
        let stored_card = StoredCard::Polymorphic(card);
        let surface = agent_effective_surface_from_wallet(&context, [&stored_card]);
        let delegation_surface = agent_delegation_surface_from_wallet(&context, [&stored_card]);

        assert_eq!(surface.source_card_ids, vec![card_id]);
        assert_eq!(delegation_surface.cards.len(), 1);
        assert_eq!(delegation_surface.cards[0].source_card_id, Some(card_id));
        assert!(
            delegation_surface.cards[0]
                .lower_positive
                .iter()
                .all(|grant| grant.recipient() == &RecipientPattern::Any)
        );
        assert!(
            surface
                .authorize(&environment_view_target("owner@example.com/shop/prod"))
                .unwrap()
        );
        assert!(
            surface
                .authorize(&component_view_target(
                    "owner@example.com/shop/prod/cart-svc"
                ))
                .unwrap()
        );
        assert!(
            surface
                .authorize(&agent_target(
                    "owner@example.com/shop/prod/cart-svc/Cart(alice)",
                    AgentVerb::View,
                ))
                .unwrap()
        );
        assert!(
            !surface
                .authorize(&agent_target(
                    "owner@example.com/shop/prod/cart-svc/Cart(bob)",
                    AgentVerb::View,
                ))
                .unwrap()
        );
    }

    #[test]
    fn installed_polymorphic_child_has_target_grants_and_source_parent() {
        let context = context();
        let source_card_id = CardId(uuid::Uuid::from_u128(1));
        let source_parent_id = CardId(uuid::Uuid::from_u128(2));
        let child_card_id = CardId(uuid::Uuid::from_u128(3));
        let source_created_at = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let child_created_at = chrono::DateTime::from_timestamp(1_700_000_100, 0).unwrap();
        let expires_at = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let grant = PolymorphicPermissionPattern::Agent(PolymorphicClassPermissionPattern {
            owner: PolymorphicAgentOwnerPattern::Agent,
            recipient: RecipientPattern::Any,
            verb: Some(AgentVerb::View),
            resource: AgentResourcePattern::Any,
        });
        let source = PolymorphicCard {
            card_id: source_card_id,
            parent_ids: vec![source_parent_id],
            lower_positive: vec![grant.clone()],
            lower_negative: vec![grant.clone()],
            upper_positive: vec![grant.clone()],
            upper_negative: vec![grant],
            created_at: source_created_at,
            expires_at: Some(expires_at),
            system_card: false,
        };
        let monomorphized_source =
            monomorphize_card_for_agent(&StoredCard::Polymorphic(source.clone()), &context);

        let child = instantiate_polymorphic_card_for_agent(
            &source,
            &context,
            child_card_id,
            child_created_at,
        );

        assert_eq!(child.card_id, child_card_id);
        assert_eq!(child.parent_ids, vec![source_card_id]);
        assert_eq!(child.lower_positive, monomorphized_source.lower_positive);
        assert_eq!(child.lower_negative, monomorphized_source.lower_negative);
        assert_eq!(child.upper_positive, monomorphized_source.upper_positive);
        assert_eq!(child.upper_negative, monomorphized_source.upper_negative);
        assert_eq!(child.created_at, child_created_at);
        assert_eq!(child.expires_at, source.expires_at);
        assert_eq!(child.system_card, source.system_card);
        assert_eq!(child.managed_by, None);
        assert_eq!(
            child,
            instantiate_polymorphic_card_for_agent(
                &source,
                &context,
                child_card_id,
                child_created_at,
            )
        );
    }

    #[test]
    fn default_agent_initial_permission_is_current_environment_scoped() {
        let context = context();
        let recipient = RecipientPattern::Agent {
            account: context.account.clone(),
            application: context.application.clone(),
            environment: context.environment.clone(),
            component: context.component.clone(),
            agent_type: context.agent_type.clone(),
        };
        let card = PolymorphicCard {
            card_id: CardId::new(),
            parent_ids: Vec::new(),
            lower_positive: default_agent_initial_permission_grants(recipient),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: chrono::Utc::now(),
            expires_at: None,
            system_card: false,
        };
        let surface =
            agent_effective_surface_from_wallet(&context, [&StoredCard::Polymorphic(card)]);

        assert!(
            surface
                .authorize(&environment_view_target("owner@example.com/shop/prod"))
                .unwrap()
        );
        assert!(
            surface
                .authorize(&component_view_target(
                    "owner@example.com/shop/prod/cart-svc"
                ))
                .unwrap()
        );
        assert!(
            !surface
                .authorize(&environment_target(
                    "owner@example.com/shop/prod",
                    EnvironmentVerb::Update
                ))
                .unwrap()
        );
        assert!(
            !surface
                .authorize(&component_target(
                    "owner@example.com/shop/prod/cart-svc",
                    ComponentVerb::Update
                ))
                .unwrap()
        );
        assert!(
            surface
                .authorize(&agent_target(
                    "owner@example.com/shop/prod/cart-svc/Cart",
                    AgentVerb::Invoke,
                ))
                .unwrap()
        );
        assert!(
            surface
                .authorize(&agent_target(
                    "owner@example.com/shop/prod/inventory-svc/Inventory(bob)",
                    AgentVerb::Invoke,
                ))
                .unwrap()
        );
        assert!(
            !surface
                .authorize(&agent_target(
                    "owner@example.com/shop/dev/cart-svc/Cart",
                    AgentVerb::Invoke,
                ))
                .unwrap()
        );
    }
}

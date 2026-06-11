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
use super::{Card, CardId, PermissionPattern, PolymorphicPermissionPattern};
use crate::model::account::AccountEmail;
use crate::model::agent::AgentTypeName;
use crate::model::application::ApplicationName;
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentName;
use chrono::Utc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPermissionMonomorphizationContext {
    pub account: AccountEmail,
    pub application: ApplicationName,
    pub environment: EnvironmentName,
    pub component: ComponentName,
    pub agent_name: String,
    pub agent_type: AgentTypeName,
}

pub fn monomorphize_agent_initial_card(
    lower_positive: &[PolymorphicPermissionPattern],
    lower_negative: &[PolymorphicPermissionPattern],
    upper_positive: &[PolymorphicPermissionPattern],
    upper_negative: &[PolymorphicPermissionPattern],
    context: &AgentPermissionMonomorphizationContext,
) -> Result<Card, String> {
    Ok(Card {
        card_id: CardId(uuid::Uuid::nil()),
        parent_ids: Vec::new(),
        lower_positive: monomorphize_permissions(lower_positive, context)?,
        lower_negative: monomorphize_permissions(lower_negative, context)?,
        upper_positive: monomorphize_permissions(upper_positive, context)?,
        upper_negative: monomorphize_permissions(upper_negative, context)?,
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    })
}

fn monomorphize_permissions(
    permissions: &[PolymorphicPermissionPattern],
    context: &AgentPermissionMonomorphizationContext,
) -> Result<Vec<PermissionPattern>, String> {
    permissions
        .iter()
        .map(|permission| monomorphize_permission(permission, context))
        .collect()
}

macro_rules! mono_permission {
    ($variant:ident, $pattern:expr, $context:expr) => {{
        let pattern = $pattern;
        Ok(PermissionPattern::$variant(ClassPermissionPattern {
            owner: pattern.owner.monomorphize($context)?,
            recipient: pattern.recipient.clone(),
            verb: pattern.verb,
            resource: pattern.resource.clone(),
        }))
    }};
}

fn monomorphize_permission(
    permission: &PolymorphicPermissionPattern,
    context: &AgentPermissionMonomorphizationContext,
) -> Result<PermissionPattern, String> {
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
    fn monomorphize(&self, context: &AgentPermissionMonomorphizationContext) -> Result<T, String>;
}

impl MonomorphizeOwner<EmptyOwnerPattern> for PolymorphicEmptyOwnerPattern {
    fn monomorphize(
        &self,
        _context: &AgentPermissionMonomorphizationContext,
    ) -> Result<EmptyOwnerPattern, String> {
        match self {
            Self::Concrete(owner) => Ok(owner.clone()),
        }
    }
}

impl MonomorphizeOwner<AccountOwnerPattern> for PolymorphicAccountOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> Result<AccountOwnerPattern, String> {
        match self {
            Self::Concrete(owner) => Ok(owner.clone()),
            Self::Account => Ok(AccountOwnerPattern::Account {
                account: context.account.clone(),
            }),
        }
    }
}

impl MonomorphizeOwner<ApplicationOwnerPattern> for PolymorphicApplicationOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> Result<ApplicationOwnerPattern, String> {
        match self {
            Self::Concrete(owner) => Ok(owner.clone()),
            Self::AccountApplications => Ok(ApplicationOwnerPattern::AccountApplications {
                account: context.account.clone(),
            }),
            Self::AccountApplication { application } => Ok(ApplicationOwnerPattern::Application {
                account: context.account.clone(),
                application: application.clone(),
            }),
            Self::App => Ok(ApplicationOwnerPattern::Application {
                account: context.account.clone(),
                application: context.application.clone(),
            }),
        }
    }
}

impl MonomorphizeOwner<EnvironmentOwnerPattern> for PolymorphicEnvironmentOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> Result<EnvironmentOwnerPattern, String> {
        match self {
            Self::Concrete(owner) => Ok(owner.clone()),
            Self::AccountEnvironments => Ok(EnvironmentOwnerPattern::AccountEnvironments {
                account: context.account.clone(),
            }),
            Self::AccountApplicationEnvironments { application } => {
                Ok(EnvironmentOwnerPattern::ApplicationEnvironments {
                    account: context.account.clone(),
                    application: application.clone(),
                })
            }
            Self::AccountEnvironment {
                application,
                environment,
            } => Ok(EnvironmentOwnerPattern::Environment {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
            }),
            Self::ApplicationEnvironments => Ok(EnvironmentOwnerPattern::ApplicationEnvironments {
                account: context.account.clone(),
                application: context.application.clone(),
            }),
            Self::ApplicationEnvironment { environment } => {
                Ok(EnvironmentOwnerPattern::Environment {
                    account: context.account.clone(),
                    application: context.application.clone(),
                    environment: environment.clone(),
                })
            }
            Self::Env => Ok(EnvironmentOwnerPattern::Environment {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
            }),
        }
    }
}

impl MonomorphizeOwner<ComponentOwnerPattern> for PolymorphicComponentOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> Result<ComponentOwnerPattern, String> {
        match self {
            Self::Concrete(owner) => Ok(owner.clone()),
            Self::AccountComponents => Ok(ComponentOwnerPattern::AccountComponents {
                account: context.account.clone(),
            }),
            Self::AccountApplicationComponents { application } => {
                Ok(ComponentOwnerPattern::ApplicationComponents {
                    account: context.account.clone(),
                    application: application.clone(),
                })
            }
            Self::AccountEnvironmentComponents {
                application,
                environment,
            } => Ok(ComponentOwnerPattern::EnvironmentComponents {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
            }),
            Self::AccountComponent {
                application,
                environment,
                component,
            } => Ok(ComponentOwnerPattern::Component {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            }),
            Self::ApplicationComponents => Ok(ComponentOwnerPattern::ApplicationComponents {
                account: context.account.clone(),
                application: context.application.clone(),
            }),
            Self::ApplicationEnvironmentComponents { environment } => {
                Ok(ComponentOwnerPattern::EnvironmentComponents {
                    account: context.account.clone(),
                    application: context.application.clone(),
                    environment: environment.clone(),
                })
            }
            Self::ApplicationComponent {
                environment,
                component,
            } => Ok(ComponentOwnerPattern::Component {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            }),
            Self::EnvComponents => Ok(ComponentOwnerPattern::EnvironmentComponents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
            }),
            Self::EnvComponent { component } => Ok(ComponentOwnerPattern::Component {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
            }),
            Self::Component => Ok(ComponentOwnerPattern::Component {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
            }),
        }
    }
}

impl MonomorphizeOwner<AgentOwnerPattern> for PolymorphicAgentOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> Result<AgentOwnerPattern, String> {
        match self {
            Self::Concrete(owner) => Ok(owner.clone()),
            Self::AccountAgents => Ok(AgentOwnerPattern::AccountAgents {
                account: context.account.clone(),
            }),
            Self::AccountApplicationAgents { application } => {
                Ok(AgentOwnerPattern::ApplicationAgents {
                    account: context.account.clone(),
                    application: application.clone(),
                })
            }
            Self::AccountEnvironmentAgents {
                application,
                environment,
            } => Ok(AgentOwnerPattern::EnvironmentAgents {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
            }),
            Self::AccountComponentAgents {
                application,
                environment,
                component,
            } => Ok(AgentOwnerPattern::ComponentAgents {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            }),
            Self::AccountAgent {
                application,
                environment,
                component,
                agent,
            } => Ok(AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
                agent: agent.clone(),
            }),
            Self::ApplicationAgents => Ok(AgentOwnerPattern::ApplicationAgents {
                account: context.account.clone(),
                application: context.application.clone(),
            }),
            Self::ApplicationEnvironmentAgents { environment } => {
                Ok(AgentOwnerPattern::EnvironmentAgents {
                    account: context.account.clone(),
                    application: context.application.clone(),
                    environment: environment.clone(),
                })
            }
            Self::ApplicationComponentAgents {
                environment,
                component,
            } => Ok(AgentOwnerPattern::ComponentAgents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            }),
            Self::ApplicationAgent {
                environment,
                component,
                agent,
            } => Ok(AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
                agent: agent.clone(),
            }),
            Self::EnvAgents => Ok(AgentOwnerPattern::EnvironmentAgents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
            }),
            Self::EnvComponentAgents { component } => Ok(AgentOwnerPattern::ComponentAgents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
            }),
            Self::EnvAgent { component, agent } => Ok(AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
                agent: agent.clone(),
            }),
            Self::ComponentAgents => Ok(AgentOwnerPattern::ComponentAgents {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
            }),
            Self::ComponentAgent { agent } => Ok(AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
                agent: agent.clone(),
            }),
            Self::Agent => Ok(AgentOwnerPattern::Agent {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
                agent: AgentOwnerLeafPattern::Agent(context.agent_name.clone()),
            }),
        }
    }
}

impl MonomorphizeOwner<ToolOwnerPattern> for PolymorphicToolOwnerPattern {
    fn monomorphize(
        &self,
        context: &AgentPermissionMonomorphizationContext,
    ) -> Result<ToolOwnerPattern, String> {
        match self {
            Self::Concrete(owner) => Ok(owner.clone()),
            Self::AccountTools => Ok(ToolOwnerPattern::AccountTools {
                account: context.account.clone(),
            }),
            Self::AccountApplicationTools { application } => {
                Ok(ToolOwnerPattern::ApplicationTools {
                    account: context.account.clone(),
                    application: application.clone(),
                })
            }
            Self::AccountEnvironmentTools {
                application,
                environment,
            } => Ok(ToolOwnerPattern::EnvironmentTools {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
            }),
            Self::AccountComponentTools {
                application,
                environment,
                component,
            } => Ok(ToolOwnerPattern::ComponentTools {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            }),
            Self::AccountTool {
                application,
                environment,
                component,
                tool,
            } => Ok(ToolOwnerPattern::Tool {
                account: context.account.clone(),
                application: application.clone(),
                environment: environment.clone(),
                component: component.clone(),
                tool: tool.clone(),
            }),
            Self::ApplicationTools => Ok(ToolOwnerPattern::ApplicationTools {
                account: context.account.clone(),
                application: context.application.clone(),
            }),
            Self::ApplicationEnvironmentTools { environment } => {
                Ok(ToolOwnerPattern::EnvironmentTools {
                    account: context.account.clone(),
                    application: context.application.clone(),
                    environment: environment.clone(),
                })
            }
            Self::ApplicationComponentTools {
                environment,
                component,
            } => Ok(ToolOwnerPattern::ComponentTools {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
            }),
            Self::ApplicationTool {
                environment,
                component,
                tool,
            } => Ok(ToolOwnerPattern::Tool {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: environment.clone(),
                component: component.clone(),
                tool: tool.clone(),
            }),
            Self::EnvTools => Ok(ToolOwnerPattern::EnvironmentTools {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
            }),
            Self::EnvComponentTools { component } => Ok(ToolOwnerPattern::ComponentTools {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
            }),
            Self::EnvTool { component, tool } => Ok(ToolOwnerPattern::Tool {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: component.clone(),
                tool: tool.clone(),
            }),
            Self::ComponentTools => Ok(ToolOwnerPattern::ComponentTools {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
            }),
            Self::ComponentTool { tool } => Ok(ToolOwnerPattern::Tool {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
                tool: tool.clone(),
            }),
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
        AgentClass, AgentResourcePattern, AgentVerb, ClassPermissionTarget, ComponentClass,
        ComponentResourcePattern, ComponentVerb, EffectiveSurface, EnvironmentClass,
        EnvironmentResourcePattern, EnvironmentVerb, PermissionTarget,
        PolymorphicClassPermissionPattern, PolymorphicPermissionPattern,
    };
    use crate::model::component::ComponentName;
    use crate::model::component_metadata::AgentInitialPermissionTemplate;
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

    fn holder() -> RecipientPattern {
        RecipientPattern::parse("owner@example.com/shop/prod/cart-svc/Cart(alice)").unwrap()
    }

    fn environment_view_target(owner: &str) -> PermissionTarget {
        PermissionTarget::Environment(ClassPermissionTarget::<EnvironmentClass> {
            verb: Some(EnvironmentVerb::View),
            owner: EnvironmentOwnerPattern::parse(owner).unwrap(),
            resource: EnvironmentResourcePattern::Any,
        })
    }

    fn component_view_target(owner: &str) -> PermissionTarget {
        PermissionTarget::Component(ClassPermissionTarget::<ComponentClass> {
            verb: Some(ComponentVerb::View),
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
    fn monomorphizes_holder_relative_agent_initial_template_slots() {
        let context = context();
        let recipient = RecipientPattern::Any;
        let card = monomorphize_agent_initial_card(
            &[
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
            &[],
            &[],
            &[],
            &context,
        )
        .unwrap();

        assert_eq!(card.card_id, CardId(uuid::Uuid::nil()));
        assert_eq!(card.lower_positive.len(), 3);
        assert!(
            card.lower_positive[0]
                .subsumes_target(&environment_view_target("owner@example.com/shop/prod"))
        );
        assert!(
            card.lower_positive[1].subsumes_target(&component_view_target(
                "owner@example.com/shop/prod/cart-svc"
            ))
        );
        assert!(card.lower_positive[2].subsumes_target(&agent_target(
            "owner@example.com/shop/prod/cart-svc/Cart(alice)",
            AgentVerb::View,
        )));
        assert!(!card.lower_positive[2].subsumes_target(&agent_target(
            "owner@example.com/shop/prod/cart-svc/Cart(bob)",
            AgentVerb::View,
        )));
    }

    #[test]
    fn default_agent_initial_template_is_current_component_scoped() {
        let context = context();
        let template =
            AgentInitialPermissionTemplate::default_for(&context.environment, &context.component);
        let card = monomorphize_agent_initial_card(
            &template.lower_positive,
            &template.lower_negative,
            &template.upper_positive,
            &template.upper_negative,
            &context,
        )
        .unwrap();
        let surface = EffectiveSurface::from_cards(&[card], &holder()).unwrap();

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
                    "owner@example.com/shop/prod/cart-svc/Cart(bob)",
                    AgentVerb::Invoke,
                ))
                .unwrap()
        );
        assert!(
            !surface
                .authorize(&agent_target(
                    "owner@example.com/shop/prod/inventory-svc/Inventory(bob)",
                    AgentVerb::Invoke,
                ))
                .unwrap()
        );
        assert!(
            !surface
                .authorize(&agent_target(
                    "owner@example.com/shop/dev/cart-svc/Cart(bob)",
                    AgentVerb::Invoke,
                ))
                .unwrap()
        );
    }
}

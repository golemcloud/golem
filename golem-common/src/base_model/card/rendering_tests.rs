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

use super::owner::*;
use super::*;
use crate::model::account::AccountEmail;
use crate::model::agent::AgentTypeName;
use crate::model::application::ApplicationName;
use crate::model::auth::TokenId;
use crate::model::card::recipient::RecipientPattern;
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentName;
use crate::model::permission_share::PermissionShareName;
use proptest::collection::vec;
use proptest::prelude::*;
use std::str::FromStr;
use test_r::test;

fn ident() -> BoxedStrategy<String> {
    "[a-z][a-z0-9]{0,8}".prop_map(|s| s).boxed()
}

fn account_email() -> BoxedStrategy<AccountEmail> {
    "[a-z][a-z0-9]{0,6}@[a-z]{3}\\.com"
        .prop_map(AccountEmail::new)
        .boxed()
}

fn application_name() -> BoxedStrategy<ApplicationName> {
    ident().prop_map(ApplicationName).boxed()
}

fn environment_name() -> BoxedStrategy<EnvironmentName> {
    ident().prop_map(EnvironmentName).boxed()
}

fn component_name() -> BoxedStrategy<ComponentName> {
    ident().prop_map(ComponentName).boxed()
}

fn agent_type_name() -> BoxedStrategy<AgentTypeName> {
    "[A-Z][A-Za-z0-9]{0,8}".prop_map(AgentTypeName).boxed()
}

fn agent_leaf() -> BoxedStrategy<AgentOwnerLeafPattern> {
    prop_oneof![
        agent_type_name().prop_map(|name| AgentOwnerLeafPattern::Agent(name.0)),
        agent_type_name().prop_map(AgentOwnerLeafPattern::AgentTypeWildcard),
    ]
    .boxed()
}

fn recipient() -> BoxedStrategy<RecipientPattern> {
    prop_oneof![
        Just(RecipientPattern::Any),
        account_email().prop_map(|account| RecipientPattern::Account { account }),
        account_email().prop_map(|account| RecipientPattern::AccountEnvironments { account }),
        (account_email(), application_name()).prop_map(|(account, application)| {
            RecipientPattern::ApplicationEnvironments {
                account,
                application,
            }
        }),
        (account_email(), application_name(), environment_name()).prop_map(
            |(account, application, environment)| RecipientPattern::Environment {
                account,
                application,
                environment,
            },
        ),
        account_email().prop_map(|account| RecipientPattern::AccountAgents { account }),
        (account_email(), application_name()).prop_map(|(account, application)| {
            RecipientPattern::ApplicationAgents {
                account,
                application,
            }
        }),
        (account_email(), application_name(), environment_name()).prop_map(
            |(account, application, environment)| RecipientPattern::EnvironmentAgents {
                account,
                application,
                environment,
            },
        ),
        (
            account_email(),
            application_name(),
            environment_name(),
            component_name(),
        )
            .prop_map(|(account, application, environment, component)| {
                RecipientPattern::ComponentAgents {
                    account,
                    application,
                    environment,
                    component,
                }
            }),
        (
            account_email(),
            application_name(),
            environment_name(),
            component_name(),
            agent_type_name(),
        )
            .prop_map(
                |(account, application, environment, component, agent_type)| {
                    RecipientPattern::Agent {
                        account,
                        application,
                        environment,
                        component,
                        agent_type,
                    }
                },
            ),
    ]
    .boxed()
}

fn agent_recipient() -> BoxedStrategy<RecipientPattern> {
    (
        account_email(),
        application_name(),
        environment_name(),
        component_name(),
        agent_type_name(),
    )
        .prop_map(
            |(account, application, environment, component, agent_type)| RecipientPattern::Agent {
                account,
                application,
                environment,
                component,
                agent_type,
            },
        )
        .boxed()
}

fn empty_owner() -> BoxedStrategy<PolymorphicEmptyOwnerPattern> {
    Just(PolymorphicEmptyOwnerPattern::Concrete(EmptyOwnerPattern)).boxed()
}

fn account_owner() -> BoxedStrategy<PolymorphicAccountOwnerPattern> {
    prop_oneof![
        Just(PolymorphicAccountOwnerPattern::Concrete(
            AccountOwnerPattern::Any
        )),
        account_email().prop_map(|account| PolymorphicAccountOwnerPattern::Concrete(
            AccountOwnerPattern::Account { account }
        )),
        Just(PolymorphicAccountOwnerPattern::Account),
    ]
    .boxed()
}

fn application_owner() -> BoxedStrategy<PolymorphicApplicationOwnerPattern> {
    prop_oneof![
        Just(PolymorphicApplicationOwnerPattern::Concrete(
            ApplicationOwnerPattern::AnyApplications,
        )),
        account_email().prop_map(|account| PolymorphicApplicationOwnerPattern::Concrete(
            ApplicationOwnerPattern::AccountApplications { account }
        )),
        (account_email(), application_name()).prop_map(|(account, application)| {
            PolymorphicApplicationOwnerPattern::Concrete(ApplicationOwnerPattern::Application {
                account,
                application,
            })
        }),
        Just(PolymorphicApplicationOwnerPattern::AccountApplications),
        application_name().prop_map(|application| {
            PolymorphicApplicationOwnerPattern::AccountApplication { application }
        }),
        Just(PolymorphicApplicationOwnerPattern::App),
    ]
    .boxed()
}

fn environment_owner() -> BoxedStrategy<PolymorphicEnvironmentOwnerPattern> {
    prop_oneof![
        Just(PolymorphicEnvironmentOwnerPattern::Concrete(
            EnvironmentOwnerPattern::AnyEnvironments,
        )),
        account_email().prop_map(|account| PolymorphicEnvironmentOwnerPattern::Concrete(
            EnvironmentOwnerPattern::AccountEnvironments { account }
        )),
        (account_email(), application_name()).prop_map(|(account, application)| {
            PolymorphicEnvironmentOwnerPattern::Concrete(
                EnvironmentOwnerPattern::ApplicationEnvironments {
                    account,
                    application,
                },
            )
        }),
        (account_email(), application_name(), environment_name()).prop_map(
            |(account, application, environment)| {
                PolymorphicEnvironmentOwnerPattern::Concrete(EnvironmentOwnerPattern::Environment {
                    account,
                    application,
                    environment,
                })
            },
        ),
        Just(PolymorphicEnvironmentOwnerPattern::AccountEnvironments),
        (application_name(), environment_name()).prop_map(|(application, environment)| {
            PolymorphicEnvironmentOwnerPattern::AccountEnvironment {
                application,
                environment,
            }
        }),
        Just(PolymorphicEnvironmentOwnerPattern::ApplicationEnvironments),
        environment_name().prop_map(|environment| {
            PolymorphicEnvironmentOwnerPattern::ApplicationEnvironment { environment }
        }),
        Just(PolymorphicEnvironmentOwnerPattern::Env),
    ]
    .boxed()
}

fn component_owner() -> BoxedStrategy<PolymorphicComponentOwnerPattern> {
    prop_oneof![
        Just(PolymorphicComponentOwnerPattern::Concrete(
            ComponentOwnerPattern::AnyComponents,
        )),
        (
            account_email(),
            application_name(),
            environment_name(),
            component_name(),
        )
            .prop_map(|(account, application, environment, component)| {
                PolymorphicComponentOwnerPattern::Concrete(ComponentOwnerPattern::Component {
                    account,
                    application,
                    environment,
                    component,
                })
            }),
        Just(PolymorphicComponentOwnerPattern::AccountComponents),
        (application_name(), environment_name(), component_name()).prop_map(
            |(application, environment, component)| {
                PolymorphicComponentOwnerPattern::AccountComponent {
                    application,
                    environment,
                    component,
                }
            },
        ),
        Just(PolymorphicComponentOwnerPattern::ApplicationComponents),
        (environment_name(), component_name()).prop_map(|(environment, component)| {
            PolymorphicComponentOwnerPattern::ApplicationComponent {
                environment,
                component,
            }
        }),
        Just(PolymorphicComponentOwnerPattern::EnvComponents),
        component_name()
            .prop_map(|component| { PolymorphicComponentOwnerPattern::EnvComponent { component } }),
        Just(PolymorphicComponentOwnerPattern::Component),
    ]
    .boxed()
}

fn agent_owner() -> BoxedStrategy<PolymorphicAgentOwnerPattern> {
    prop_oneof![
        Just(PolymorphicAgentOwnerPattern::Concrete(
            AgentOwnerPattern::AnyAgents,
        )),
        (
            account_email(),
            application_name(),
            environment_name(),
            component_name(),
            agent_leaf(),
        )
            .prop_map(|(account, application, environment, component, agent)| {
                PolymorphicAgentOwnerPattern::Concrete(AgentOwnerPattern::Agent {
                    account,
                    application,
                    environment,
                    component,
                    agent,
                })
            }),
        Just(PolymorphicAgentOwnerPattern::AccountAgents),
        (
            application_name(),
            environment_name(),
            component_name(),
            agent_leaf()
        )
            .prop_map(|(application, environment, component, agent)| {
                PolymorphicAgentOwnerPattern::AccountAgent {
                    application,
                    environment,
                    component,
                    agent,
                }
            },),
        Just(PolymorphicAgentOwnerPattern::ApplicationAgents),
        (environment_name(), component_name(), agent_leaf()).prop_map(
            |(environment, component, agent)| PolymorphicAgentOwnerPattern::ApplicationAgent {
                environment,
                component,
                agent,
            },
        ),
        Just(PolymorphicAgentOwnerPattern::EnvAgents),
        (component_name(), agent_leaf()).prop_map(|(component, agent)| {
            PolymorphicAgentOwnerPattern::EnvAgent { component, agent }
        }),
        Just(PolymorphicAgentOwnerPattern::ComponentAgents),
        agent_leaf().prop_map(|agent| PolymorphicAgentOwnerPattern::ComponentAgent { agent }),
        Just(PolymorphicAgentOwnerPattern::Agent),
    ]
    .boxed()
}

fn tool_owner() -> BoxedStrategy<PolymorphicToolOwnerPattern> {
    prop_oneof![
        Just(PolymorphicToolOwnerPattern::Concrete(
            ToolOwnerPattern::AnyTools
        )),
        Just(PolymorphicToolOwnerPattern::EnvTools),
        component_name()
            .prop_map(|component| PolymorphicToolOwnerPattern::EnvComponentTools { component }),
        (component_name(), ident()).prop_map(|(component, tool)| {
            PolymorphicToolOwnerPattern::EnvTool { component, tool }
        }),
        Just(PolymorphicToolOwnerPattern::ComponentTools),
        ident().prop_map(|tool| PolymorphicToolOwnerPattern::ComponentTool { tool }),
    ]
    .boxed()
}

fn option_verb<T: Copy + std::fmt::Debug + 'static>(values: Vec<T>) -> BoxedStrategy<Option<T>> {
    prop_oneof![Just(None), proptest::sample::select(values).prop_map(Some)].boxed()
}

fn dot_segments<S>(segment: impl Strategy<Value = S> + 'static) -> BoxedStrategy<Vec<S>>
where
    S: std::fmt::Debug + Clone + 'static,
{
    vec(segment, 2..4).boxed()
}

fn slash_segments<S>(segment: impl Strategy<Value = S> + 'static) -> BoxedStrategy<Vec<S>>
where
    S: std::fmt::Debug + Clone + 'static,
{
    vec(segment, 0..4).boxed()
}

fn fs_segment() -> BoxedStrategy<FilesystemPathSegmentPattern> {
    prop_oneof![
        ident().prop_map(FilesystemPathSegmentPattern::Literal),
        Just(FilesystemPathSegmentPattern::Star),
        Just(FilesystemPathSegmentPattern::GlobStar),
    ]
    .boxed()
}

fn config_segment() -> BoxedStrategy<ConfigKeySegmentPattern> {
    prop_oneof![
        ident().prop_map(ConfigKeySegmentPattern::Literal),
        Just(ConfigKeySegmentPattern::Star),
        Just(ConfigKeySegmentPattern::GlobStar),
    ]
    .boxed()
}

fn secret_segment() -> BoxedStrategy<SecretKeySegmentPattern> {
    prop_oneof![
        ident().prop_map(SecretKeySegmentPattern::Literal),
        Just(SecretKeySegmentPattern::Star),
        Just(SecretKeySegmentPattern::GlobStar),
    ]
    .boxed()
}

fn env_agent_secret_segment() -> BoxedStrategy<EnvironmentAgentSecretKeySegmentPattern> {
    prop_oneof![
        ident().prop_map(EnvironmentAgentSecretKeySegmentPattern::Literal),
        Just(EnvironmentAgentSecretKeySegmentPattern::Star),
        Just(EnvironmentAgentSecretKeySegmentPattern::GlobStar),
    ]
    .boxed()
}

fn initial_files_segment() -> BoxedStrategy<EnvironmentInitialFilesPathSegmentPattern> {
    prop_oneof![
        ident().prop_map(EnvironmentInitialFilesPathSegmentPattern::Literal),
        Just(EnvironmentInitialFilesPathSegmentPattern::Star),
        Just(EnvironmentInitialFilesPathSegmentPattern::GlobStar),
    ]
    .boxed()
}

fn class_permission<C>(
    owner: BoxedStrategy<<C::Owner as OwnerPattern>::Polymorphic>,
    verb: BoxedStrategy<Option<C::Verb>>,
    resource: BoxedStrategy<C::Resource>,
) -> BoxedStrategy<PolymorphicPermissionPattern>
where
    C: PermissionClass + 'static,
    <C::Owner as OwnerPattern>::Polymorphic: 'static,
    C::Verb: 'static,
    C::Resource: 'static,
{
    (owner, recipient(), verb, resource)
        .prop_map(|(owner, recipient, verb, resource)| {
            C::into_polymorphic_permission(PolymorphicClassPermissionPattern::<C> {
                owner,
                recipient,
                verb,
                resource,
            })
        })
        .boxed()
}

fn permission_strategy() -> BoxedStrategy<PolymorphicPermissionPattern> {
    prop_oneof![
        class_permission::<FilesystemClass>(
            agent_owner(),
            option_verb(vec![
                FilesystemVerb::Read,
                FilesystemVerb::Write,
                FilesystemVerb::List,
                FilesystemVerb::Stat,
                FilesystemVerb::Delete
            ]),
            slash_segments(fs_segment())
                .prop_map(
                    |segments| FilesystemResourcePattern::Path(FilesystemPathPattern { segments })
                )
                .boxed(),
        ),
        class_permission::<NetworkClass>(
            empty_owner(),
            option_verb(vec![NetworkVerb::Connect]),
            prop_oneof![
                Just(NetworkResourcePattern::Any),
                Just(NetworkResourcePattern::host_port(
                    "api.example.com",
                    PortPattern::Single(443)
                ))
            ]
            .boxed(),
        ),
        class_permission::<EnvClass>(
            agent_owner(),
            option_verb(vec![EnvVerb::Read]),
            prop_oneof![
                Just(EnvResourcePattern::Any),
                ident()
                    .prop_map(EnvVarName)
                    .prop_map(EnvResourcePattern::VarName)
            ]
            .boxed()
        ),
        class_permission::<OplogClass>(
            agent_owner(),
            option_verb(vec![OplogVerb::Read]),
            prop_oneof![
                Just(OplogResourcePattern::Any),
                Just(OplogResourcePattern::Range {
                    start: Some(1),
                    end: Some(10)
                })
            ]
            .boxed()
        ),
        class_permission::<ConfigClass>(
            agent_owner(),
            option_verb(vec![ConfigVerb::Read]),
            prop_oneof![
                Just(ConfigResourcePattern::Any),
                dot_segments(config_segment()).prop_map(|segments| ConfigResourcePattern::Key(
                    ConfigKeyPathPattern { segments }
                ))
            ]
            .boxed()
        ),
        class_permission::<SecretClass>(
            environment_owner(),
            option_verb(vec![SecretVerb::Hold, SecretVerb::Mint, SecretVerb::Reveal]),
            prop_oneof![
                Just(SecretResourcePattern::Any),
                dot_segments(secret_segment()).prop_map(|segments| SecretResourcePattern::Key(
                    SecretKeyPathPattern { segments }
                ))
            ]
            .boxed()
        ),
        class_permission::<AgentClass>(
            agent_owner(),
            option_verb(vec![
                AgentVerb::Invoke,
                AgentVerb::View,
                AgentVerb::Delete,
                AgentVerb::Interrupt,
                AgentVerb::Resume,
                AgentVerb::UpdateRevision,
                AgentVerb::Fork,
                AgentVerb::Revert,
                AgentVerb::CancelInvocation,
                AgentVerb::ActivatePlugin,
                AgentVerb::DeactivatePlugin,
                AgentVerb::Debug
            ]),
            prop_oneof![
                Just(AgentResourcePattern::Any),
                ident()
                    .prop_map(AgentMethodName)
                    .prop_map(AgentResourcePattern::Method),
                any::<u16>().prop_map(|index| AgentResourcePattern::OplogIndex(index.into()))
            ]
            .boxed()
        ),
        class_permission::<ToolClass>(
            tool_owner(),
            option_verb(vec![ToolVerb::Invoke]),
            Just(ToolResourcePattern::AnyInvocation).boxed()
        ),
        class_permission::<KvClass>(
            environment_owner(),
            option_verb(vec![
                KvVerb::Read,
                KvVerb::Write,
                KvVerb::Delete,
                KvVerb::List
            ]),
            ident()
                .prop_map(|store| KvResourcePattern::StoreKey {
                    store,
                    key_pattern: "key-*".to_string()
                })
                .boxed()
        ),
        class_permission::<BlobClass>(
            environment_owner(),
            option_verb(vec![
                BlobVerb::Read,
                BlobVerb::Write,
                BlobVerb::Delete,
                BlobVerb::List
            ]),
            ident()
                .prop_map(|bucket| BlobResourcePattern::BucketKey {
                    bucket,
                    key_pattern: "path/**".to_string()
                })
                .boxed()
        ),
        class_permission::<RdbmsClass>(
            environment_owner(),
            option_verb(vec![RdbmsVerb::Query, RdbmsVerb::Mutate]),
            Just(RdbmsResourcePattern::Table {
                database: "db".to_string(),
                schema: "schema".to_string(),
                table: "table".to_string()
            })
            .boxed()
        ),
        class_permission::<CardClass>(
            account_owner(),
            option_verb(vec![
                CardVerb::Derive,
                CardVerb::Revoke,
                CardVerb::Inspect,
                CardVerb::Install
            ]),
            prop_oneof![
                Just(CardResourcePattern::Any),
                agent_recipient().prop_map(CardResourcePattern::InstallTarget)
            ]
            .boxed()
        ),
        class_permission::<SystemClass>(
            empty_owner(),
            option_verb(vec![
                SystemVerb::CreateAccount,
                SystemVerb::ImpersonateUser,
                SystemVerb::ViewDefaultPlan,
                SystemVerb::ViewAccountSummariesReport,
                SystemVerb::ViewAccountCountsReport
            ]),
            Just(SystemResourcePattern).boxed()
        ),
        class_permission::<PlanClass>(
            empty_owner(),
            option_verb(vec![PlanVerb::View, PlanVerb::Create, PlanVerb::Update]),
            prop_oneof![
                Just(PlanResourcePattern::Any),
                ident()
                    .prop_map(PlanIdentifier)
                    .prop_map(PlanIdPattern::Identifier)
                    .prop_map(PlanResourcePattern::Plan)
            ]
            .boxed()
        ),
        class_permission::<AccountClass>(
            account_owner(),
            option_verb(vec![
                AccountVerb::View,
                AccountVerb::Update,
                AccountVerb::Delete,
                AccountVerb::SetPlan,
                AccountVerb::ViewPlan
            ]),
            Just(AccountResourcePattern).boxed()
        ),
        class_permission::<AccountUsageClass>(
            account_owner(),
            option_verb(vec![AccountUsageVerb::View]),
            Just(AccountUsageResourcePattern).boxed()
        ),
        class_permission::<AccountTokenClass>(
            account_owner(),
            option_verb(vec![
                AccountTokenVerb::View,
                AccountTokenVerb::Create,
                AccountTokenVerb::Delete
            ]),
            prop_oneof![
                Just(AccountTokenResourcePattern::Any),
                Just(TokenId::try_from("550e8400-e29b-41d4-a716-446655440000").unwrap())
                    .prop_map(AccountTokenResourcePattern::Token)
            ]
            .boxed()
        ),
        class_permission::<AccountPluginClass>(
            account_owner(),
            option_verb(vec![
                AccountPluginVerb::View,
                AccountPluginVerb::Register,
                AccountPluginVerb::Delete,
                AccountPluginVerb::Restore
            ]),
            prop_oneof![
                Just(AccountPluginResourcePattern::Any),
                ident()
                    .prop_map(AccountPluginName)
                    .prop_map(AccountPluginResourcePattern::Name)
            ]
            .boxed()
        ),
        class_permission::<ApplicationClass>(
            application_owner(),
            option_verb(vec![
                ApplicationVerb::View,
                ApplicationVerb::Create,
                ApplicationVerb::Update,
                ApplicationVerb::Delete
            ]),
            Just(ApplicationResourcePattern).boxed()
        ),
        class_permission::<EnvironmentClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentVerb::View,
                EnvironmentVerb::Create,
                EnvironmentVerb::Update,
                EnvironmentVerb::Delete,
                EnvironmentVerb::Deploy,
                EnvironmentVerb::Rollback,
                EnvironmentVerb::ViewDeployment,
                EnvironmentVerb::ViewDeploymentPlan,
                EnvironmentVerb::ViewAgentTypes,
                EnvironmentVerb::WriteDeploymentRecord
            ]),
            prop_oneof![
                Just(EnvironmentResourcePattern::Any),
                Just(EnvironmentResourcePattern::Revision { revision: 42 })
            ]
            .boxed()
        ),
        class_permission::<EnvironmentPluginGrantClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentPluginGrantVerb::View,
                EnvironmentPluginGrantVerb::Create,
                EnvironmentPluginGrantVerb::Delete
            ]),
            prop_oneof![
                Just(EnvironmentPluginGrantResourcePattern::Any),
                ident()
                    .prop_map(EnvironmentPluginGrantName)
                    .prop_map(EnvironmentPluginGrantResourcePattern::Name)
            ]
            .boxed()
        ),
        class_permission::<EnvironmentDomainRegistrationClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentDomainRegistrationVerb::View,
                EnvironmentDomainRegistrationVerb::Create,
                EnvironmentDomainRegistrationVerb::Delete
            ]),
            prop_oneof![
                Just(EnvironmentDomainRegistrationResourcePattern::Any),
                vec(ident().prop_map(DomainLabel), 1..4).prop_map(|labels| {
                    EnvironmentDomainRegistrationResourcePattern::Domain(DomainNamePattern {
                        labels,
                    })
                })
            ]
            .boxed()
        ),
        class_permission::<EnvironmentSecuritySchemeClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentSecuritySchemeVerb::View,
                EnvironmentSecuritySchemeVerb::Create,
                EnvironmentSecuritySchemeVerb::Update,
                EnvironmentSecuritySchemeVerb::Delete,
                EnvironmentSecuritySchemeVerb::Restore
            ]),
            prop_oneof![
                Just(EnvironmentSecuritySchemeResourcePattern::Any),
                ident()
                    .prop_map(EnvironmentSecuritySchemeName)
                    .prop_map(EnvironmentSecuritySchemeResourcePattern::Name)
            ]
            .boxed()
        ),
        class_permission::<EnvironmentHttpApiDeploymentClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentHttpApiDeploymentVerb::View,
                EnvironmentHttpApiDeploymentVerb::Create,
                EnvironmentHttpApiDeploymentVerb::Update,
                EnvironmentHttpApiDeploymentVerb::Delete,
                EnvironmentHttpApiDeploymentVerb::Restore
            ]),
            prop_oneof![
                Just(EnvironmentHttpApiDeploymentResourcePattern::Any),
                Just(EnvironmentHttpApiDeploymentResourcePattern::DomainPath {
                    domain: "api".to_string(),
                    path_glob: "/v1/**".to_string()
                })
            ]
            .boxed()
        ),
        class_permission::<EnvironmentMcpDeploymentClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentMcpDeploymentVerb::View,
                EnvironmentMcpDeploymentVerb::Create,
                EnvironmentMcpDeploymentVerb::Update,
                EnvironmentMcpDeploymentVerb::Delete,
                EnvironmentMcpDeploymentVerb::Restore
            ]),
            prop_oneof![
                Just(EnvironmentMcpDeploymentResourcePattern::Any),
                ident()
                    .prop_map(EnvironmentMcpDeploymentName)
                    .prop_map(EnvironmentMcpDeploymentResourcePattern::Name)
            ]
            .boxed()
        ),
        class_permission::<EnvironmentAgentSecretClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentAgentSecretVerb::View,
                EnvironmentAgentSecretVerb::Create,
                EnvironmentAgentSecretVerb::Update,
                EnvironmentAgentSecretVerb::Delete,
                EnvironmentAgentSecretVerb::Restore
            ]),
            prop_oneof![
                Just(EnvironmentAgentSecretResourcePattern::Any),
                dot_segments(env_agent_secret_segment()).prop_map(|segments| {
                    EnvironmentAgentSecretResourcePattern::Key(
                        EnvironmentAgentSecretKeyPathPattern { segments },
                    )
                })
            ]
            .boxed()
        ),
        class_permission::<EnvironmentResourceDefinitionClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentResourceDefinitionVerb::View,
                EnvironmentResourceDefinitionVerb::Create,
                EnvironmentResourceDefinitionVerb::Update,
                EnvironmentResourceDefinitionVerb::Delete,
                EnvironmentResourceDefinitionVerb::Restore
            ]),
            prop_oneof![
                Just(EnvironmentResourceDefinitionResourcePattern::Any),
                ident()
                    .prop_map(EnvironmentResourceDefinitionName)
                    .prop_map(EnvironmentResourceDefinitionResourcePattern::Name)
            ]
            .boxed()
        ),
        class_permission::<EnvironmentRetryPolicyClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentRetryPolicyVerb::View,
                EnvironmentRetryPolicyVerb::Create,
                EnvironmentRetryPolicyVerb::Update,
                EnvironmentRetryPolicyVerb::Delete,
                EnvironmentRetryPolicyVerb::Restore
            ]),
            prop_oneof![
                Just(EnvironmentRetryPolicyResourcePattern::Any),
                ident()
                    .prop_map(EnvironmentRetryPolicyName)
                    .prop_map(EnvironmentRetryPolicyResourcePattern::Name)
            ]
            .boxed()
        ),
        class_permission::<ComponentClass>(
            component_owner(),
            option_verb(vec![
                ComponentVerb::View,
                ComponentVerb::Create,
                ComponentVerb::Update,
                ComponentVerb::Delete
            ]),
            prop_oneof![
                Just(ComponentResourcePattern::Any),
                Just(ComponentResourcePattern::Revision { revision: 42 })
            ]
            .boxed()
        ),
        class_permission::<AccountOauth2IdentityClass>(
            account_owner(),
            option_verb(vec![
                AccountOauth2IdentityVerb::View,
                AccountOauth2IdentityVerb::Link,
                AccountOauth2IdentityVerb::Unlink
            ]),
            prop_oneof![
                Just(AccountOauth2IdentityResourcePattern::Any),
                Just(AccountOauth2IdentityResourcePattern::Identity {
                    provider: "google".to_string(),
                    external_id: "12345".to_string()
                })
            ]
            .boxed()
        ),
        class_permission::<EnvironmentInitialFilesClass>(
            component_owner(),
            option_verb(vec![
                EnvironmentInitialFilesVerb::View,
                EnvironmentInitialFilesVerb::Update,
                EnvironmentInitialFilesVerb::Delete,
                EnvironmentInitialFilesVerb::List
            ]),
            slash_segments(initial_files_segment())
                .prop_map(|segments| EnvironmentInitialFilesResourcePattern::Path(
                    EnvironmentInitialFilesPathPattern { segments }
                ))
                .boxed()
        ),
        class_permission::<EnvironmentKvBucketClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentKvBucketVerb::View,
                EnvironmentKvBucketVerb::Create,
                EnvironmentKvBucketVerb::Delete,
                EnvironmentKvBucketVerb::Clear
            ]),
            prop_oneof![
                Just(EnvironmentKvBucketResourcePattern::Any),
                ident()
                    .prop_map(EnvironmentKvBucketName)
                    .prop_map(EnvironmentKvBucketResourcePattern::Name)
            ]
            .boxed()
        ),
        class_permission::<EnvironmentBlobBucketClass>(
            environment_owner(),
            option_verb(vec![
                EnvironmentBlobBucketVerb::View,
                EnvironmentBlobBucketVerb::Create,
                EnvironmentBlobBucketVerb::Delete,
                EnvironmentBlobBucketVerb::Clear
            ]),
            prop_oneof![
                Just(EnvironmentBlobBucketResourcePattern::Any),
                ident()
                    .prop_map(EnvironmentBlobBucketName)
                    .prop_map(EnvironmentBlobBucketResourcePattern::Name)
            ]
            .boxed()
        ),
        class_permission::<AccountPermissionShareClass>(
            account_owner(),
            option_verb(vec![
                AccountPermissionShareVerb::View,
                AccountPermissionShareVerb::Create,
                AccountPermissionShareVerb::Update,
                AccountPermissionShareVerb::Delete
            ]),
            prop_oneof![
                Just(AccountPermissionShareResourcePattern::Any),
                ident()
                    .prop_map(PermissionShareName)
                    .prop_map(AccountPermissionShareResourcePattern::Name)
            ]
            .boxed()
        ),
    ]
    .boxed()
}

fn golden_grants() -> Vec<&'static str> {
    vec![
        "filesystem(acme/shop/prod/cart/CartAgent(*)) @ acme/shop/prod/cart/CartAgent : read : /data/**",
        "network() @ acme/shop/prod/cart/CartAgent : connect : api.example.com:443",
        "env(acme/shop/prod/cart/CartAgent(*)) @ acme/shop/prod/cart/CartAgent : read : DATABASE_URL",
        "oplog(acme/shop/prod/cart/CartAgent(*)) @ acme/shop/prod/cart/CartAgent : read : start=1:end=10",
        "config(acme/shop/prod/cart/CartAgent(*)) @ acme/shop/prod/cart/CartAgent : read : db.**",
        "secret(acme/shop/prod) @ acme/shop/prod/cart/CartAgent : reveal : api-key",
        "agent(acme/shop/prod/cart/CartAgent(*)) @ acme/shop/prod/cart/CartAgent : invoke : charge",
        "tool(acme/shop/prod/tools/*) @ acme/shop/prod/cart/CartAgent : invoke : *",
        "kv(acme/shop/prod) @ acme/shop/prod/cart/CartAgent : read : store.user-*",
        "blob(acme/shop/prod) @ acme/shop/prod/cart/CartAgent : read : bucket.path/**",
        "rdbms(acme/shop/prod) @ acme/shop/prod/cart/CartAgent : query : orders.public.orders",
        "card(acme) @ acme/shop/prod/cart/CartAgent : derive : *",
        "system() @ acme : create-account :",
        "plan() @ acme : view : plan-a",
        "account(acme) @ acme : view :",
        "account.usage(acme) @ acme : view :",
        "account.token(acme) @ acme : view : 550e8400-e29b-41d4-a716-446655440000",
        "account.plugin(acme) @ acme : view : plugin-a",
        "application(acme/shop) @ acme : view :",
        "environment(acme/shop/prod) @ acme/shop/prod : view :",
        "environment.plugin-grant(acme/shop/prod) @ acme/shop/prod : view : plugin-a",
        "environment.domain-registration(acme/shop/prod) @ acme/shop/prod : view : domain-a",
        "environment.security-scheme(acme/shop/prod) @ acme/shop/prod : view : scheme-a",
        "environment.http-api-deployment(acme/shop/prod) @ acme/shop/prod : view : api./v1/**",
        "environment.mcp-deployment(acme/shop/prod) @ acme/shop/prod : view : mcp-a",
        "environment.agent-secret(acme/shop/prod) @ acme/shop/prod : view : cart.*",
        "environment.resource-definition(acme/shop/prod) @ acme/shop/prod : view : resource-a",
        "environment.retry-policy(acme/shop/prod) @ acme/shop/prod : view : retry-a",
        "component(acme/shop/prod/cart-svc) @ acme/shop/prod : view : @rev=42",
        "account.oauth2-identity(acme) @ acme : view : google/12345",
        "environment.initial-files(acme/shop/prod/cart-svc) @ acme/shop/prod : view : /etc/*",
        "environment.kv-bucket(acme/shop/prod) @ acme/shop/prod : view : bucket-a",
        "environment.blob-bucket(acme/shop/prod) @ acme/shop/prod : view : bucket-a",
        "account.permission-share(acme) @ acme : view : team-access",
    ]
}

#[test]
fn golden_polymorphic_permission_renderer_roundtrips_through_parser() {
    for grant in golden_grants() {
        let parsed = PolymorphicPermissionPattern::from_str(grant).unwrap();
        let rendered = parsed.render().unwrap();
        let reparsed = PolymorphicPermissionPattern::from_str(&rendered).unwrap();
        assert_eq!(reparsed, parsed, "golden grant did not roundtrip: {grant}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn polymorphic_permission_renderer_roundtrips_through_parser(permission in permission_strategy()) {
        let rendered = permission.render().unwrap();
        let reparsed = PolymorphicPermissionPattern::from_str(&rendered).unwrap();

        prop_assert_eq!(reparsed, permission);
    }
}

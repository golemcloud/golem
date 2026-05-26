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

use super::*;
use pretty_assertions::assert_eq;
use test_r::core::{DynamicTestRegistration, TestProperties};
use test_r::{add_test, test, test_gen};

fn parsed_permission(input: &str) -> PermissionPattern {
    parse_pattern_grant(input)
        .expect("grant should parse")
        .permission
}

fn account_owner(account: &str) -> AccountOwnerPattern {
    AccountOwnerPattern::Account {
        account: account.to_string(),
    }
}

fn account_recipient(account: &str) -> AccountRecipientPattern {
    AccountRecipientPattern::Account {
        account: account.to_string(),
    }
}

fn application_owner(account: &str, application: &str) -> ApplicationOwnerPattern {
    ApplicationOwnerPattern::Application {
        account: account.to_string(),
        application: application.to_string(),
    }
}

fn environment_owner(
    account: &str,
    application: &str,
    environment: &str,
) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: account.to_string(),
        application: application.to_string(),
        environment: environment.to_string(),
    }
}

fn environment_recipient(
    account: &str,
    application: &str,
    environment: &str,
) -> EnvironmentRecipientPattern {
    EnvironmentRecipientPattern::Environment {
        account: account.to_string(),
        application: application.to_string(),
        environment: environment.to_string(),
    }
}

fn agent_owner(
    account: &str,
    application: &str,
    environment: &str,
    component: &str,
    agent: AgentOwnerLeafPattern,
) -> AgentOwnerPattern {
    AgentOwnerPattern::Agent {
        account: account.to_string(),
        application: application.to_string(),
        environment: environment.to_string(),
        component: component.to_string(),
        agent,
    }
}

fn agent_recipient(
    account: &str,
    application: &str,
    environment: &str,
    component: &str,
    agent: &str,
) -> AgentRecipientPattern {
    AgentRecipientPattern::Agent {
        account: account.to_string(),
        application: application.to_string(),
        environment: environment.to_string(),
        component: component.to_string(),
        agent: agent.to_string(),
    }
}

fn filesystem_path_data_glob() -> FilesystemResourcePattern {
    FilesystemResourcePattern::Path(FilesystemPathPattern {
        segments: vec![
            FilesystemPathSegmentPattern::Literal("data".to_string()),
            FilesystemPathSegmentPattern::GlobStar,
        ],
    })
}

fn secret_key(segments: Vec<SecretKeySegmentPattern>) -> SecretResourcePattern {
    SecretResourcePattern::Key(SecretKeyPathPattern { segments })
}

fn environment_agent_secret_key(
    segments: Vec<EnvironmentAgentSecretKeySegmentPattern>,
) -> EnvironmentAgentSecretResourcePattern {
    EnvironmentAgentSecretResourcePattern::Key(EnvironmentAgentSecretKeyPathPattern { segments })
}

fn token_id() -> uuid::Uuid {
    uuid::Uuid::from_u128(0x550e8400e29b41d4a716446655440000)
}

#[test_gen]
fn parses_runtime_class_examples_from_spec(r: &mut DynamicTestRegistration) {
    let cases: Vec<(&str, &str, PermissionPattern)> = vec![
        (
            "filesystem_canonical",
            "filesystem(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : /data/**",
            PermissionPattern::Filesystem(FilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Read,
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart",
                    AgentOwnerLeafPattern::Agent("agent".to_string()),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart", "agent"),
                resource: filesystem_path_data_glob(),
            }),
        ),
        (
            "filesystem_email_recipient",
            "filesystem(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ alice@example.com/shop/prod/cart-svc/CartAgent(\"42\") : read : /data/**",
            PermissionPattern::Filesystem(FilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Read,
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    AgentOwnerLeafPattern::Agent("CartAgent(\"42\")".to_string()),
                ),
                recipient: agent_recipient(
                    "alice@example.com",
                    "shop",
                    "prod",
                    "cart-svc",
                    "CartAgent(\"42\")",
                ),
                resource: filesystem_path_data_glob(),
            }),
        ),
        (
            "network",
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal:8080",
            PermissionPattern::Network(NetworkPermissionPattern::Verb {
                verb: NetworkVerb::Connect,
                owner: EmptyOwnerPattern,
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: NetworkResourcePattern::HostPort {
                    host: "api.internal".to_string(),
                    ports: PortPattern::Single(8080),
                },
            }),
        ),
        (
            "network_port_range",
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal:8080-9000",
            PermissionPattern::Network(NetworkPermissionPattern::Verb {
                verb: NetworkVerb::Connect,
                owner: EmptyOwnerPattern,
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: NetworkResourcePattern::HostPort {
                    host: "api.internal".to_string(),
                    ports: PortPattern::Range {
                        start: 8080,
                        end: 9000,
                    },
                },
            }),
        ),
        (
            "env",
            "env(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : HOME",
            PermissionPattern::Env(EnvPermissionPattern::Verb {
                verb: EnvVerb::Read,
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    AgentOwnerLeafPattern::Agent("CartAgent(\"42\")".to_string()),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: EnvResourcePattern::VarName(EnvVarName("HOME".to_string())),
            }),
        ),
        (
            "oplog_any",
            "oplog(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : *",
            PermissionPattern::Oplog(OplogPermissionPattern::Verb {
                verb: OplogVerb::Read,
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    AgentOwnerLeafPattern::Agent("CartAgent(\"42\")".to_string()),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: OplogResourcePattern::Any,
            }),
        ),
        (
            "oplog_range_with_colons",
            "oplog(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : start=1000:end=2000",
            PermissionPattern::Oplog(OplogPermissionPattern::Verb {
                verb: OplogVerb::Read,
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart",
                    AgentOwnerLeafPattern::Agent("agent".to_string()),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart", "agent"),
                resource: OplogResourcePattern::Range {
                    start: Some(1000),
                    end: Some(2000),
                },
            }),
        ),
        (
            "config",
            "config(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : model.retry-count",
            PermissionPattern::Config(ConfigPermissionPattern::Verb {
                verb: ConfigVerb::Read,
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    AgentOwnerLeafPattern::Agent("CartAgent(\"42\")".to_string()),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: ConfigResourcePattern::Key(ConfigKeyPathPattern {
                    segments: vec![
                        ConfigKeySegmentPattern::Literal("model".to_string()),
                        ConfigKeySegmentPattern::Literal("retry-count".to_string()),
                    ],
                }),
            }),
        ),
        (
            "secret_hold",
            "secret(acme/shop/prod) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : hold : cart.api-key",
            PermissionPattern::Secret(SecretPermissionPattern::Verb {
                verb: SecretVerb::Hold,
                owner: environment_owner("acme", "shop", "prod"),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: secret_key(vec![
                    SecretKeySegmentPattern::Literal("cart".to_string()),
                    SecretKeySegmentPattern::Literal("api-key".to_string()),
                ]),
            }),
        ),
        (
            "secret_reveal_agent_type",
            "secret(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : reveal : cart.api-key",
            PermissionPattern::Secret(SecretPermissionPattern::Verb {
                verb: SecretVerb::Reveal,
                owner: environment_owner("acme", "shop", "prod"),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: secret_key(vec![
                    SecretKeySegmentPattern::Literal("cart".to_string()),
                    SecretKeySegmentPattern::Literal("api-key".to_string()),
                ]),
            }),
        ),
        (
            "agent_invoke",
            "agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : add-item",
            PermissionPattern::Agent(AgentPermissionPattern::Verb {
                verb: AgentVerb::Invoke,
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    AgentOwnerLeafPattern::AgentTypeWildcard("ShoppingCart".to_string()),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: AgentResourcePattern::Method(AgentMethodName("add-item".to_string())),
            }),
        ),
        (
            "agent_delete_component",
            "agent(acme/shop/prod/cart-svc/*) @ acme/shop/prod/cart-svc/* : delete : *",
            PermissionPattern::Agent(AgentPermissionPattern::Verb {
                verb: AgentVerb::Delete,
                owner: AgentOwnerPattern::ComponentAgents {
                    account: "acme".to_string(),
                    application: "shop".to_string(),
                    environment: "prod".to_string(),
                    component: "cart-svc".to_string(),
                },
                recipient: AgentRecipientPattern::ComponentAgents {
                    account: "acme".to_string(),
                    application: "shop".to_string(),
                    environment: "prod".to_string(),
                    component: "cart-svc".to_string(),
                },
                resource: AgentResourcePattern::Any,
            }),
        ),
        (
            "tool",
            "tool(acme/shop/prod/cli-tools/grep) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : search",
            PermissionPattern::Tool(ToolPermissionPattern::Verb {
                verb: ToolVerb::Invoke,
                owner: ToolOwnerPattern::Tool {
                    account: "acme".to_string(),
                    application: "shop".to_string(),
                    environment: "prod".to_string(),
                    component: "cli-tools".to_string(),
                    tool: "grep".to_string(),
                },
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: ToolResourcePattern::Invocation(ToolInvocationPattern {
                    command_path: Some(vec![ToolIdentifier("search".to_string())]),
                    args: Vec::new(),
                }),
            }),
        ),
        (
            "kv",
            "kv(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : read : my-store.user-*",
            PermissionPattern::Kv(KvPermissionPattern::Verb {
                verb: KvVerb::Read,
                owner: environment_owner("acme", "shop", "prod"),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: KvResourcePattern::StoreKey {
                    store: "my-store".to_string(),
                    key_pattern: "user-*".to_string(),
                },
            }),
        ),
        (
            "blob",
            "blob(acme/shop/prod) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : my-bucket.models/*.bin",
            PermissionPattern::Blob(BlobPermissionPattern::Verb {
                verb: BlobVerb::Read,
                owner: environment_owner("acme", "shop", "prod"),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: BlobResourcePattern::BucketKey {
                    bucket: "my-bucket".to_string(),
                    key_pattern: "models/*.bin".to_string(),
                },
            }),
        ),
        (
            "rdbms",
            "rdbms(acme/shop/prod) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : query : orders.public.orders",
            PermissionPattern::Rdbms(RdbmsPermissionPattern::Verb {
                verb: RdbmsVerb::Query,
                owner: environment_owner("acme", "shop", "prod"),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: RdbmsResourcePattern::Table {
                    database: "orders".to_string(),
                    schema: "public".to_string(),
                    table: "orders".to_string(),
                },
            }),
        ),
        (
            "card_derive",
            "card(acme) @ acme/shop/prod/cart-svc/ShoppingCart(*) : derive : *",
            PermissionPattern::Card(CardPermissionPattern::Verb {
                verb: CardVerb::Derive,
                owner: account_owner("acme"),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: CardResourcePattern::Any,
            }),
        ),
        (
            "card_install",
            "card(acme) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : install : acme/shop/prod/cart-svc/ShoppingCart(*)",
            PermissionPattern::Card(CardPermissionPattern::Verb {
                verb: CardVerb::Install,
                owner: account_owner("acme"),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: CardResourcePattern::InstallTarget(agent_recipient(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    "ShoppingCart(*)",
                )),
            }),
        ),
        (
            "system",
            "system() @ acme : create-account :",
            PermissionPattern::System(SystemPermissionPattern::Verb {
                verb: SystemVerb::CreateAccount,
                owner: EmptyOwnerPattern,
                recipient: account_recipient("acme"),
                resource: SystemResourcePattern,
            }),
        ),
        (
            "system_email_recipient",
            "system() @ alice@example.com : create-account :",
            PermissionPattern::System(SystemPermissionPattern::Verb {
                verb: SystemVerb::CreateAccount,
                owner: EmptyOwnerPattern,
                recipient: account_recipient("alice@example.com"),
                resource: SystemResourcePattern,
            }),
        ),
        (
            "plan",
            "plan() @ acme : view : plan-a",
            PermissionPattern::Plan(PlanPermissionPattern::Verb {
                verb: PlanVerb::View,
                owner: EmptyOwnerPattern,
                recipient: account_recipient("acme"),
                resource: PlanResourcePattern::Plan(PlanIdPattern::Identifier(PlanIdentifier(
                    "plan-a".to_string(),
                ))),
            }),
        ),
        (
            "account",
            "account(acme) @ acme : view :",
            PermissionPattern::Account(AccountPermissionPattern::Verb {
                verb: AccountVerb::View,
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountResourcePattern,
            }),
        ),
        (
            "account_usage",
            "account.usage(acme) @ acme : view :",
            PermissionPattern::AccountUsage(AccountUsagePermissionPattern::Verb {
                verb: AccountUsageVerb::View,
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountUsageResourcePattern,
            }),
        ),
        (
            "account_token",
            "account.token(acme) @ acme : view : 550e8400-e29b-41d4-a716-446655440000",
            PermissionPattern::AccountToken(AccountTokenPermissionPattern::Verb {
                verb: AccountTokenVerb::View,
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountTokenResourcePattern::Token(token_id()),
            }),
        ),
        (
            "account_token_delete",
            "account.token(acme) @ acme : delete : 550e8400-e29b-41d4-a716-446655440000",
            PermissionPattern::AccountToken(AccountTokenPermissionPattern::Verb {
                verb: AccountTokenVerb::Delete,
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountTokenResourcePattern::Token(token_id()),
            }),
        ),
        (
            "account_plugin",
            "account.plugin(acme) @ acme : view : plugin-a",
            PermissionPattern::AccountPlugin(AccountPluginPermissionPattern::Verb {
                verb: AccountPluginVerb::View,
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountPluginResourcePattern::Name(AccountPluginName(
                    "plugin-a".to_string(),
                )),
            }),
        ),
        (
            "application",
            "application(acme) @ acme : view : shop",
            PermissionPattern::Application(ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::View,
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: ApplicationResourcePattern::Application(ApplicationName(
                    "shop".to_string(),
                )),
            }),
        ),
        (
            "application_create_any",
            "application(acme) @ acme : create : *",
            PermissionPattern::Application(ApplicationPermissionPattern::Verb {
                verb: ApplicationVerb::Create,
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: ApplicationResourcePattern::Any,
            }),
        ),
        (
            "environment",
            "environment(acme/shop) @ acme/shop/prod : view : prod",
            PermissionPattern::Environment(EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::View,
                owner: application_owner("acme", "shop"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentResourcePattern::Environment(EnvironmentName(
                    "prod".to_string(),
                )),
            }),
        ),
        (
            "environment_create_any",
            "environment(acme/shop) @ acme/shop/prod : create : *",
            PermissionPattern::Environment(EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Create,
                owner: application_owner("acme", "shop"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentResourcePattern::Any,
            }),
        ),
        (
            "environment_rollback_revision",
            "environment(acme/shop) @ acme/shop/prod : rollback : prod@rev=42",
            PermissionPattern::Environment(EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Rollback,
                owner: application_owner("acme", "shop"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentResourcePattern::Revision {
                    environment: EnvironmentName("prod".to_string()),
                    revision: 42,
                },
            }),
        ),
        (
            "environment_share",
            "environment.share(acme/shop/prod) @ acme/shop/prod : view : 550e8400-e29b-41d4-a716-446655440000",
            PermissionPattern::EnvironmentShare(EnvironmentSharePermissionPattern::Verb {
                verb: EnvironmentShareVerb::View,
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentShareResourcePattern::Share(token_id()),
            }),
        ),
        (
            "environment_plugin_grant",
            "environment.plugin-grant(acme/shop/prod) @ acme/shop/prod : view : plugin-a",
            PermissionPattern::EnvironmentPluginGrant(
                EnvironmentPluginGrantPermissionPattern::Verb {
                    verb: EnvironmentPluginGrantVerb::View,
                    owner: environment_owner("acme", "shop", "prod"),
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: EnvironmentPluginGrantResourcePattern::Name(
                        EnvironmentPluginGrantName("plugin-a".to_string()),
                    ),
                },
            ),
        ),
        (
            "environment_domain_registration",
            "environment.domain-registration(acme/shop/prod) @ acme/shop/prod : view : domain-a",
            PermissionPattern::EnvironmentDomainRegistration(
                EnvironmentDomainRegistrationPermissionPattern::Verb {
                    verb: EnvironmentDomainRegistrationVerb::View,
                    owner: environment_owner("acme", "shop", "prod"),
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: EnvironmentDomainRegistrationResourcePattern::Domain(
                        DomainNamePattern {
                            labels: vec![DomainLabel("domain-a".to_string())],
                        },
                    ),
                },
            ),
        ),
        (
            "environment_security_scheme",
            "environment.security-scheme(acme/shop/prod) @ acme/shop/prod : view : scheme-a",
            PermissionPattern::EnvironmentSecurityScheme(
                EnvironmentSecuritySchemePermissionPattern::Verb {
                    verb: EnvironmentSecuritySchemeVerb::View,
                    owner: environment_owner("acme", "shop", "prod"),
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: EnvironmentSecuritySchemeResourcePattern::Name(
                        EnvironmentSecuritySchemeName("scheme-a".to_string()),
                    ),
                },
            ),
        ),
        (
            "environment_http_api_deployment",
            "environment.http-api-deployment(acme/shop/prod) @ acme/shop/prod : view : api./v1/**",
            PermissionPattern::EnvironmentHttpApiDeployment(
                EnvironmentHttpApiDeploymentPermissionPattern::Verb {
                    verb: EnvironmentHttpApiDeploymentVerb::View,
                    owner: environment_owner("acme", "shop", "prod"),
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: EnvironmentHttpApiDeploymentResourcePattern::DomainPath {
                        domain: "api".to_string(),
                        path_glob: "/v1/**".to_string(),
                    },
                },
            ),
        ),
        (
            "environment_mcp_deployment",
            "environment.mcp-deployment(acme/shop/prod) @ acme/shop/prod : view : mcp-a",
            PermissionPattern::EnvironmentMcpDeployment(
                EnvironmentMcpDeploymentPermissionPattern::Verb {
                    verb: EnvironmentMcpDeploymentVerb::View,
                    owner: environment_owner("acme", "shop", "prod"),
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: EnvironmentMcpDeploymentResourcePattern::Name(
                        EnvironmentMcpDeploymentName("mcp-a".to_string()),
                    ),
                },
            ),
        ),
        (
            "environment_agent_secret",
            "environment.agent-secret(acme/shop/prod) @ acme/shop/prod : update : cart.*",
            PermissionPattern::EnvironmentAgentSecret(
                EnvironmentAgentSecretPermissionPattern::Verb {
                    verb: EnvironmentAgentSecretVerb::Update,
                    owner: environment_owner("acme", "shop", "prod"),
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: environment_agent_secret_key(vec![
                        EnvironmentAgentSecretKeySegmentPattern::Literal("cart".to_string()),
                        EnvironmentAgentSecretKeySegmentPattern::Star,
                    ]),
                },
            ),
        ),
        (
            "environment_resource_definition",
            "environment.resource-definition(acme/shop/prod) @ acme/shop/prod : view : resource-a",
            PermissionPattern::EnvironmentResourceDefinition(
                EnvironmentResourceDefinitionPermissionPattern::Verb {
                    verb: EnvironmentResourceDefinitionVerb::View,
                    owner: environment_owner("acme", "shop", "prod"),
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: EnvironmentResourceDefinitionResourcePattern::Name(
                        EnvironmentResourceDefinitionName("resource-a".to_string()),
                    ),
                },
            ),
        ),
        (
            "environment_retry_policy",
            "environment.retry-policy(acme/shop/prod) @ acme/shop/prod : view : retry-a",
            PermissionPattern::EnvironmentRetryPolicy(
                EnvironmentRetryPolicyPermissionPattern::Verb {
                    verb: EnvironmentRetryPolicyVerb::View,
                    owner: environment_owner("acme", "shop", "prod"),
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: EnvironmentRetryPolicyResourcePattern::Name(
                        EnvironmentRetryPolicyName("retry-a".to_string()),
                    ),
                },
            ),
        ),
        (
            "component",
            "component(acme/shop/prod) @ acme/shop/prod : view : cart-svc",
            PermissionPattern::Component(ComponentPermissionPattern::Verb {
                verb: ComponentVerb::View,
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: ComponentResourcePattern::Component(ComponentName(
                    "cart-svc".to_string(),
                )),
            }),
        ),
        (
            "component_create_any",
            "component(acme/shop/prod) @ acme/shop/prod : create : *",
            PermissionPattern::Component(ComponentPermissionPattern::Verb {
                verb: ComponentVerb::Create,
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: ComponentResourcePattern::Any,
            }),
        ),
        (
            "account_oauth2_identity",
            "account.oauth2-identity(acme) @ acme : view : google/12345",
            PermissionPattern::AccountOauth2Identity(
                AccountOauth2IdentityPermissionPattern::Verb {
                    verb: AccountOauth2IdentityVerb::View,
                    owner: account_owner("acme"),
                    recipient: account_recipient("acme"),
                    resource: AccountOauth2IdentityResourcePattern::Identity {
                        provider: "google".to_string(),
                        external_id: "12345".to_string(),
                    },
                },
            ),
        ),
        (
            "environment_initial_files",
            "environment.initial-files(acme/shop/prod/cart-svc) @ acme/shop/prod : view : /etc/*",
            PermissionPattern::EnvironmentInitialFiles(
                EnvironmentInitialFilesPermissionPattern::Verb {
                    verb: EnvironmentInitialFilesVerb::View,
                    owner: ComponentOwnerPattern::Component {
                        account: "acme".to_string(),
                        application: "shop".to_string(),
                        environment: "prod".to_string(),
                        component: "cart-svc".to_string(),
                    },
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: EnvironmentInitialFilesResourcePattern::Path(
                        EnvironmentInitialFilesPathPattern {
                            segments: vec![
                                EnvironmentInitialFilesPathSegmentPattern::Literal(
                                    "etc".to_string(),
                                ),
                                EnvironmentInitialFilesPathSegmentPattern::Star,
                            ],
                        },
                    ),
                },
            ),
        ),
        (
            "environment_kv_bucket",
            "environment.kv-bucket(acme/shop/prod) @ acme/shop/prod : view : bucket-a",
            PermissionPattern::EnvironmentKvBucket(EnvironmentKvBucketPermissionPattern::Verb {
                verb: EnvironmentKvBucketVerb::View,
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentKvBucketResourcePattern::Name(EnvironmentKvBucketName(
                    "bucket-a".to_string(),
                )),
            }),
        ),
        (
            "environment_blob_bucket",
            "environment.blob-bucket(acme/shop/prod) @ acme/shop/prod : view : bucket-a",
            PermissionPattern::EnvironmentBlobBucket(
                EnvironmentBlobBucketPermissionPattern::Verb {
                    verb: EnvironmentBlobBucketVerb::View,
                    owner: environment_owner("acme", "shop", "prod"),
                    recipient: environment_recipient("acme", "shop", "prod"),
                    resource: EnvironmentBlobBucketResourcePattern::Name(
                        EnvironmentBlobBucketName("bucket-a".to_string()),
                    ),
                },
            ),
        ),
    ];

    for (name, input, expected) in cases {
        let expected = std::sync::Arc::new(expected);
        add_test!(
            r,
            format!("parses_pattern_grant_{name}"),
            TestProperties::unit_test(),
            || {
                assert_eq!(parsed_permission(input), (*expected).clone());
            }
        );
    }
}

#[test]
fn rejects_unknown_verbs_for_class() {
    let result = parse_pattern_grant(
        "filesystem(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : query : /data/**",
    );

    assert_eq!(
        result,
        Err(CardParseError::UnknownVerb {
            class: "filesystem".to_string(),
            verb: "query".to_string(),
        })
    );
}

#[test]
fn rejects_malformed_grants() {
    assert_eq!(
        parse_pattern_grant("filesystem(acme) : read : /data/**"),
        Err(CardParseError::MissingAtSeparator)
    );
    assert_eq!(
        parse_pattern_grant("filesystem(acme) @ acme : query : /data/**"),
        Err(CardParseError::InvalidOwnerPath {
            class: "filesystem".to_string(),
            owner: "acme".to_string(),
        })
    );
    assert_eq!(
        parse_pattern_grant("system(acme) @ acme : create-account :"),
        Err(CardParseError::InvalidOwnerPath {
            class: "system".to_string(),
            owner: "acme".to_string(),
        })
    );
    assert_eq!(
        parse_pattern_grant("system() @ acme : create-account : not-empty"),
        Err(CardParseError::InvalidResource {
            class: "system".to_string(),
            resource: "not-empty".to_string(),
        })
    );
    assert_eq!(
        parse_pattern_grant(
            "card(acme) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : install : acme/shop"
        ),
        Err(CardParseError::InvalidRecipientPath(
            "acme/shop".to_string()
        ))
    );
    assert_eq!(
        parse_pattern_grant("unknown(acme) @ acme : view :"),
        Err(CardParseError::UnknownClass("unknown".to_string()))
    );
}

#[test]
fn rejects_removed_application_credential_and_restore_forms() {
    let credential_id = "550e8400-e29b-41d4-a716-446655440000";

    assert_eq!(
        parse_pattern_grant("application(acme/shop) @ acme : view :"),
        Err(CardParseError::InvalidOwnerPath {
            class: ApplicationClass::NAME.to_string(),
            owner: "acme/shop".to_string(),
        })
    );
    assert_eq!(
        parse_pattern_grant(&format!(
            "application(acme) @ acme : view-credentials : cred={credential_id}"
        )),
        Err(CardParseError::UnknownVerb {
            class: ApplicationClass::NAME.to_string(),
            verb: "view-credentials".to_string(),
        })
    );
    assert_eq!(
        parse_pattern_grant("account(acme) @ acme : restore :"),
        Err(CardParseError::UnknownVerb {
            class: AccountClass::NAME.to_string(),
            verb: "restore".to_string(),
        })
    );
    assert_eq!(
        parse_pattern_grant("application(acme) @ acme : restore : shop"),
        Err(CardParseError::UnknownVerb {
            class: ApplicationClass::NAME.to_string(),
            verb: "restore".to_string(),
        })
    );
    assert_eq!(
        parse_pattern_grant("environment(acme/shop) @ acme/shop/prod : restore : prod"),
        Err(CardParseError::UnknownVerb {
            class: EnvironmentClass::NAME.to_string(),
            verb: "restore".to_string(),
        })
    );
}

#[test]
fn rejects_recipients_outside_permission_class_scope() {
    assert_eq!(
        parse_pattern_grant("system() @ acme/shop/prod/cart/agent : create-account :"),
        Err(CardParseError::InvalidRecipientPath(
            "acme/shop/prod/cart/agent".to_string()
        ))
    );
    assert_eq!(
        parse_pattern_grant("filesystem(acme/shop/prod/cart/agent) @ acme : read : /data/**"),
        Err(CardParseError::InvalidRecipientPath("acme".to_string()))
    );
    assert_eq!(
        parse_pattern_grant("environment(acme/shop) @ acme/shop/prod/cart/agent : deploy : prod"),
        Err(CardParseError::InvalidRecipientPath(
            "acme/shop/prod/cart/agent".to_string()
        ))
    );
}

#[test]
fn rejects_empty_resource_ids_when_any_resource_is_available() {
    let cases = [
        (
            "agent(acme/shop/prod/cart-svc/*) @ acme/shop/prod/cart-svc/* : delete :",
            AgentClass::NAME,
        ),
        (
            "card(acme) @ acme/shop/prod/cart-svc/ShoppingCart(*) : derive :",
            CardClass::NAME,
        ),
        (
            "tool(acme/shop/prod/cli-tools/grep) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke :",
            ToolClass::NAME,
        ),
        (
            "application(acme) @ acme : create :",
            ApplicationClass::NAME,
        ),
        (
            "environment(acme/shop) @ acme/shop/prod : create :",
            EnvironmentClass::NAME,
        ),
        (
            "component(acme/shop/prod) @ acme/shop/prod : create :",
            ComponentClass::NAME,
        ),
    ];

    for (input, class) in cases {
        assert_eq!(
            parse_pattern_grant(input),
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: String::new(),
            })
        );
    }
}

#[test_gen]
fn parses_polymorphic_pattern_grant_examples_from_spec(r: &mut DynamicTestRegistration) {
    let cases: Vec<(&str, &str, PolymorphicPermissionPattern)> = vec![
        (
            "environment_owner_and_recipient_slots",
            "environment(?env) @ ?env : view : prod",
            PolymorphicPermissionPattern::Environment(
                PolymorphicEnvironmentPermissionPattern::Verb {
                    verb: EnvironmentVerb::View,
                    owner: PolymorphicApplicationOwnerPattern::Env,
                    recipient: PolymorphicEnvironmentRecipientPattern::Environment,
                    resource: EnvironmentResourcePattern::Environment(EnvironmentName(
                        "prod".to_string(),
                    )),
                },
            ),
        ),
        (
            "env_self_slots",
            "env(?self) @ ?self : read : HOME",
            PolymorphicPermissionPattern::Env(PolymorphicEnvPermissionPattern::Verb {
                verb: EnvVerb::Read,
                owner: PolymorphicAgentOwnerPattern::Self_,
                recipient: PolymorphicAgentRecipientPattern::Self_,
                resource: EnvResourcePattern::VarName(EnvVarName("HOME".to_string())),
            }),
        ),
        (
            "secret_monomorphic_resource",
            "secret(?env) @ ?self : reveal : billing.account",
            PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
                verb: SecretVerb::Reveal,
                owner: PolymorphicEnvironmentOwnerPattern::Env,
                recipient: PolymorphicAgentRecipientPattern::Self_,
                resource: secret_key(vec![
                    SecretKeySegmentPattern::Literal("billing".to_string()),
                    SecretKeySegmentPattern::Literal("account".to_string()),
                ]),
            }),
        ),
        (
            "secret_resource_glob",
            "secret(?env) @ ?self : reveal : billing.*",
            PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
                verb: SecretVerb::Reveal,
                owner: PolymorphicEnvironmentOwnerPattern::Env,
                recipient: PolymorphicAgentRecipientPattern::Self_,
                resource: secret_key(vec![
                    SecretKeySegmentPattern::Literal("billing".to_string()),
                    SecretKeySegmentPattern::Star,
                ]),
            }),
        ),
        (
            "secret_environment_agent_recipient_slot",
            "secret(?env) @ ?env/cart-svc/ShoppingCart(*) : hold : cart.api-key",
            PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
                verb: SecretVerb::Hold,
                owner: PolymorphicEnvironmentOwnerPattern::Env,
                recipient: PolymorphicAgentRecipientPattern::EnvironmentAgent {
                    component: "cart-svc".to_string(),
                    agent: "ShoppingCart(*)".to_string(),
                },
                resource: secret_key(vec![
                    SecretKeySegmentPattern::Literal("cart".to_string()),
                    SecretKeySegmentPattern::Literal("api-key".to_string()),
                ]),
            }),
        ),
        (
            "secret_concrete_recipient",
            "secret(?env) @ acme/shop/prod/cart-svc/ShoppingCart(*) : hold : cart.api-key",
            PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
                verb: SecretVerb::Hold,
                owner: PolymorphicEnvironmentOwnerPattern::Env,
                recipient: PolymorphicAgentRecipientPattern::Concrete(agent_recipient(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    "ShoppingCart(*)",
                )),
                resource: secret_key(vec![
                    SecretKeySegmentPattern::Literal("cart".to_string()),
                    SecretKeySegmentPattern::Literal("api-key".to_string()),
                ]),
            }),
        ),
        (
            "agent_env_owner_template",
            "agent(?env/payment-svc/PaymentAgent(*)) @ ?self : invoke : charge",
            PolymorphicPermissionPattern::Agent(PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::Invoke,
                owner: PolymorphicAgentOwnerPattern::EnvAgent {
                    component: "payment-svc".to_string(),
                    agent: AgentOwnerLeafPattern::AgentTypeWildcard("PaymentAgent".to_string()),
                },
                recipient: PolymorphicAgentRecipientPattern::Self_,
                resource: AgentResourcePattern::Method(AgentMethodName("charge".to_string())),
            }),
        ),
    ];

    for (name, input, expected) in cases {
        let expected = std::sync::Arc::new(expected);
        add_test!(
            r,
            format!("parses_polymorphic_pattern_grant_{name}"),
            TestProperties::unit_test(),
            || {
                let grant = parse_polymorphic_pattern_grant(input).expect(input);
                assert_eq!(grant.permission, (*expected).clone());
            }
        );
    }
}

#[test]
fn empty_resource_classes_reject_polymorphic_resource_slots() {
    assert_eq!(
        parse_polymorphic_pattern_grant("account(acme) @ ?account : view : ?resource"),
        Err(CardParseError::InvalidResource {
            class: AccountClass::NAME.to_string(),
            resource: "?resource".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_pattern_grant("system() @ ?account : create-account : ?resource"),
        Err(CardParseError::InvalidResource {
            class: SystemClass::NAME.to_string(),
            resource: "?resource".to_string(),
        })
    );
}

#[test]
fn rejects_polymorphic_resource_slots_and_templates() {
    assert_eq!(
        parse_polymorphic_pattern_grant("env(?self) @ ?self : read : ?env_var"),
        Err(CardParseError::InvalidResource {
            class: EnvClass::NAME.to_string(),
            resource: "?env_var".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_pattern_grant("card(acme) @ ?self : install : ?self"),
        Err(CardParseError::InvalidResource {
            class: CardClass::NAME.to_string(),
            resource: "?self".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_pattern_grant("secret(?env) @ ?self : reveal : secret.?self"),
        Err(CardParseError::InvalidResource {
            class: SecretClass::NAME.to_string(),
            resource: "secret.?self".to_string(),
        })
    );
}

#[test]
fn rejects_undeclared_polymorphic_owner_slots() {
    assert_eq!(
        parse_polymorphic_pattern_grant("account(?account) @ ?account : view :"),
        Err(CardParseError::InvalidOwnerPath {
            class: AccountClass::NAME.to_string(),
            owner: "?account".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_pattern_grant("application(?app) @ ?account : view :"),
        Err(CardParseError::InvalidOwnerPath {
            class: ApplicationClass::NAME.to_string(),
            owner: "?app".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_pattern_grant("component(?component) @ ?env : view : cart-svc"),
        Err(CardParseError::InvalidOwnerPath {
            class: ComponentClass::NAME.to_string(),
            owner: "?component".to_string(),
        })
    );
}

#[test]
fn rejects_polymorphic_owner_slots_with_wrong_scope() {
    assert_eq!(
        parse_polymorphic_pattern_grant("filesystem(?env) @ ?self : read : /data/**"),
        Err(CardParseError::InvalidOwnerPath {
            class: FilesystemClass::NAME.to_string(),
            owner: "?env".to_string(),
        })
    );
}

#[test]
fn rejects_undeclared_polymorphic_recipient_slots() {
    assert_eq!(
        parse_polymorphic_pattern_grant("secret(?env) @ ?account : reveal : billing.*"),
        Err(CardParseError::InvalidRecipientPath("?account".to_string()))
    );
    assert_eq!(
        parse_polymorphic_pattern_grant("secret(?env) @ ?env : reveal : billing.*"),
        Err(CardParseError::InvalidRecipientPath("?env".to_string()))
    );
}

#[test]
fn concrete_parser_rejects_slot_variables() {
    let result = parse_pattern_grant("secret(?env) @ ?self : reveal : billing.*");

    assert_eq!(
        result,
        Err(CardParseError::SlotVariableInConcreteGrant(
            "?env".to_string()
        ))
    );
}

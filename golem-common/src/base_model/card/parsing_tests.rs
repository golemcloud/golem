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
use crate::model::auth::TokenId;
use crate::model::card::owner::{
    AccountOwnerPattern, AgentOwnerLeafPattern, AgentOwnerPattern, ApplicationOwnerPattern,
    ComponentOwnerPattern, EmptyOwnerPattern, EnvironmentOwnerPattern,
    PolymorphicAccountOwnerPattern, PolymorphicAgentOwnerPattern,
    PolymorphicApplicationOwnerPattern, PolymorphicEmptyOwnerPattern,
    PolymorphicEnvironmentOwnerPattern, PolymorphicToolOwnerPattern, ToolOwnerPattern,
};
use crate::model::card::recipient::{
    PolymorphicAgentRecipientPattern, PolymorphicEnvironmentRecipientPattern,
    PolymorphicRecipientPattern, RecipientPattern,
};
use crate::model::permission_share::PermissionShareName;
use RecipientPattern as AccountRecipientPattern;
use RecipientPattern as AgentRecipientPattern;
use RecipientPattern as EnvironmentRecipientPattern;
use pretty_assertions::assert_eq;
use test_r::core::{DynamicTestRegistration, TestProperties};
use test_r::{add_test, test, test_gen};

fn account_email(account: &str) -> crate::model::account::AccountEmail {
    crate::model::account::AccountEmail::new(account)
}

fn application_name(application: &str) -> crate::model::application::ApplicationName {
    crate::model::application::ApplicationName(application.to_string())
}

fn environment_name(environment: &str) -> crate::model::environment::EnvironmentName {
    crate::model::environment::EnvironmentName(environment.to_string())
}

fn component_name(component: &str) -> crate::model::component::ComponentName {
    crate::model::component::ComponentName(component.to_string())
}

fn agent_type_name(agent_type: &str) -> crate::model::agent::AgentTypeName {
    crate::model::agent::AgentTypeName(agent_type.to_string())
}

fn parsed_permission(input: &str) -> PermissionPattern {
    parse_permission(input).expect("permission should parse")
}

fn account_owner(account: &str) -> AccountOwnerPattern {
    AccountOwnerPattern::Account {
        account: account_email(account),
    }
}

fn account_recipient(account: &str) -> AccountRecipientPattern {
    AccountRecipientPattern::Account {
        account: account.to_string(),
    }
}

fn application_owner(account: &str, application: &str) -> ApplicationOwnerPattern {
    ApplicationOwnerPattern::Application {
        account: account_email(account),
        application: application_name(application),
    }
}

fn environment_owner(
    account: &str,
    application: &str,
    environment: &str,
) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: account_email(account),
        application: application_name(application),
        environment: environment_name(environment),
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
        account: account_email(account),
        application: application_name(application),
        environment: environment_name(environment),
        component: component_name(component),
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

fn fixed_uuid() -> uuid::Uuid {
    uuid::Uuid::from_u128(0x550e8400e29b41d4a716446655440000)
}

fn token_id() -> TokenId {
    TokenId(fixed_uuid())
}

#[test_gen]
fn parses_runtime_class_examples_from_spec(r: &mut DynamicTestRegistration) {
    let cases: Vec<(&str, &str, PermissionPattern)> = vec![
        (
            "filesystem_canonical",
            "filesystem(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : /data/**",
            PermissionPattern::Filesystem(ClassPermissionPattern::<FilesystemClass> {
                verb: Some(FilesystemVerb::Read),
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
            PermissionPattern::Filesystem(ClassPermissionPattern::<FilesystemClass> {
                verb: Some(FilesystemVerb::Read),
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
            "filesystem_any_verb",
            "filesystem(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : * : /data/**",
            PermissionPattern::Filesystem(ClassPermissionPattern::<FilesystemClass> {
                verb: None,
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
            "filesystem_root",
            "filesystem(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : /",
            PermissionPattern::Filesystem(ClassPermissionPattern::<FilesystemClass> {
                verb: Some(FilesystemVerb::Read),
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart",
                    AgentOwnerLeafPattern::Agent("agent".to_string()),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart", "agent"),
                resource: FilesystemResourcePattern::Path(FilesystemPathPattern {
                    segments: vec![],
                }),
            }),
        ),
        (
            "filesystem_segment_wildcards",
            "filesystem(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : /data/*/**",
            PermissionPattern::Filesystem(ClassPermissionPattern::<FilesystemClass> {
                verb: Some(FilesystemVerb::Read),
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart",
                    AgentOwnerLeafPattern::Agent("agent".to_string()),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart", "agent"),
                resource: FilesystemResourcePattern::Path(FilesystemPathPattern {
                    segments: vec![
                        FilesystemPathSegmentPattern::Literal("data".to_string()),
                        FilesystemPathSegmentPattern::Star,
                        FilesystemPathSegmentPattern::GlobStar,
                    ],
                }),
            }),
        ),
        (
            "network",
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal:8080",
            PermissionPattern::Network(ClassPermissionPattern::<NetworkClass> {
                verb: Some(NetworkVerb::Connect),
                owner: EmptyOwnerPattern,
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: NetworkResourcePattern::HostPort {
                    host: "api.internal".to_string(),
                    ports: PortPattern::Single(8080),
                },
            }),
        ),
        (
            "network_any_resource",
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : *",
            PermissionPattern::Network(ClassPermissionPattern::<NetworkClass> {
                verb: Some(NetworkVerb::Connect),
                owner: EmptyOwnerPattern,
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: NetworkResourcePattern::Any,
            }),
        ),
        (
            "network_host_any_port",
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal",
            PermissionPattern::Network(ClassPermissionPattern::<NetworkClass> {
                verb: Some(NetworkVerb::Connect),
                owner: EmptyOwnerPattern,
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: NetworkResourcePattern::HostPort {
                    host: "api.internal".to_string(),
                    ports: PortPattern::Any,
                },
            }),
        ),
        (
            "network_port_range",
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal:8080-9000",
            PermissionPattern::Network(ClassPermissionPattern::<NetworkClass> {
                verb: Some(NetworkVerb::Connect),
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
            "network_host_segment_wildcard",
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : *.internal:443",
            PermissionPattern::Network(ClassPermissionPattern::<NetworkClass> {
                verb: Some(NetworkVerb::Connect),
                owner: EmptyOwnerPattern,
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: NetworkResourcePattern::HostPort {
                    host: "*.internal".to_string(),
                    ports: PortPattern::Single(443),
                },
            }),
        ),
        (
            "network_any_host_single_port",
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : *:443",
            PermissionPattern::Network(ClassPermissionPattern::<NetworkClass> {
                verb: Some(NetworkVerb::Connect),
                owner: EmptyOwnerPattern,
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "CartAgent(\"42\")"),
                resource: NetworkResourcePattern::HostPort {
                    host: "*".to_string(),
                    ports: PortPattern::Single(443),
                },
            }),
        ),
        (
            "env",
            "env(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : HOME",
            PermissionPattern::Env(ClassPermissionPattern::<EnvClass> {
                verb: Some(EnvVerb::Read),
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
            PermissionPattern::Oplog(ClassPermissionPattern::<OplogClass> {
                verb: Some(OplogVerb::Read),
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
            PermissionPattern::Oplog(ClassPermissionPattern::<OplogClass> {
                verb: Some(OplogVerb::Read),
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
            PermissionPattern::Config(ClassPermissionPattern::<ConfigClass> {
                verb: Some(ConfigVerb::Read),
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
            PermissionPattern::Secret(ClassPermissionPattern::<SecretClass> {
                verb: Some(SecretVerb::Hold),
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
            PermissionPattern::Secret(ClassPermissionPattern::<SecretClass> {
                verb: Some(SecretVerb::Reveal),
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
            PermissionPattern::Agent(ClassPermissionPattern::<AgentClass> {
                verb: Some(AgentVerb::Invoke),
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    AgentOwnerLeafPattern::AgentTypeWildcard(agent_type_name("ShoppingCart")),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: AgentResourcePattern::Method(AgentMethodName("add-item".to_string())),
            }),
        ),
        (
            "agent_delete_component",
            "agent(acme/shop/prod/cart-svc/*) @ acme/shop/prod/cart-svc/* : delete : *",
            PermissionPattern::Agent(ClassPermissionPattern::<AgentClass> {
                verb: Some(AgentVerb::Delete),
                owner: AgentOwnerPattern::ComponentAgents {
                    account: account_email("acme"),
                    application: application_name("shop"),
                    environment: environment_name("prod"),
                    component: component_name("cart-svc"),
                },
                recipient: AgentRecipientPattern::Agent {
                    account: "acme".to_string(),
                    application: "shop".to_string(),
                    environment: "prod".to_string(),
                    component: "cart-svc".to_string(),
                    agent: "*".to_string(),
                },
                resource: AgentResourcePattern::Any,
            }),
        ),
        (
            "agent_cancel_invocation_uuid",
            "agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme/shop/prod/cart-svc/ShoppingCart(*) : cancel-invocation : 550e8400-e29b-41d4-a716-446655440000",
            PermissionPattern::Agent(ClassPermissionPattern::<AgentClass> {
                verb: Some(AgentVerb::CancelInvocation),
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    AgentOwnerLeafPattern::AgentTypeWildcard(agent_type_name("ShoppingCart")),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: AgentResourcePattern::InvocationId(AgentInvocationIdPattern::Uuid(
                    fixed_uuid(),
                )),
            }),
        ),
        (
            "agent_revert_oplog_index",
            "agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme/shop/prod/cart-svc/ShoppingCart(*) : revert : 42",
            PermissionPattern::Agent(ClassPermissionPattern::<AgentClass> {
                verb: Some(AgentVerb::Revert),
                owner: agent_owner(
                    "acme",
                    "shop",
                    "prod",
                    "cart-svc",
                    AgentOwnerLeafPattern::AgentTypeWildcard(agent_type_name("ShoppingCart")),
                ),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: AgentResourcePattern::OplogIndex(42),
            }),
        ),
        (
            "tool",
            "tool(acme/shop/prod/cli-tools/grep) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : search",
            PermissionPattern::Tool(ClassPermissionPattern::<ToolClass> {
                verb: Some(ToolVerb::Invoke),
                owner: ToolOwnerPattern::Tool {
                    account: account_email("acme"),
                    application: application_name("shop"),
                    environment: environment_name("prod"),
                    component: component_name("cli-tools"),
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
            "tool_with_flags_and_args",
            "tool(acme/shop/prod/cli-tools/grep) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : search.files --pattern=* --path=src/** -in README.md",
            PermissionPattern::Tool(ClassPermissionPattern::<ToolClass> {
                verb: Some(ToolVerb::Invoke),
                owner: ToolOwnerPattern::Tool {
                    account: account_email("acme"),
                    application: application_name("shop"),
                    environment: environment_name("prod"),
                    component: component_name("cli-tools"),
                    tool: "grep".to_string(),
                },
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: ToolResourcePattern::Invocation(ToolInvocationPattern {
                    command_path: Some(vec![
                        ToolIdentifier("search".to_string()),
                        ToolIdentifier("files".to_string()),
                    ]),
                    args: vec![
                        ToolArgPattern::LongFlag {
                            name: ToolIdentifier("pattern".to_string()),
                            value: Some(ToolValuePattern::Star),
                        },
                        ToolArgPattern::LongFlag {
                            name: ToolIdentifier("path".to_string()),
                            value: Some(ToolValuePattern::Literal(ToolValueLiteral(
                                "src/**".to_string(),
                            ))),
                        },
                        ToolArgPattern::ShortFlags {
                            flags: vec!['i', 'n'],
                            value: None,
                        },
                        ToolArgPattern::Positional(ToolValuePattern::Literal(ToolValueLiteral(
                            "README.md".to_string(),
                        ))),
                    ],
                }),
            }),
        ),
        (
            "kv",
            "kv(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : read : my-store.user-*",
            PermissionPattern::Kv(ClassPermissionPattern::<KvClass> {
                verb: Some(KvVerb::Read),
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
            PermissionPattern::Blob(ClassPermissionPattern::<BlobClass> {
                verb: Some(BlobVerb::Read),
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
            PermissionPattern::Rdbms(ClassPermissionPattern::<RdbmsClass> {
                verb: Some(RdbmsVerb::Query),
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
            PermissionPattern::Card(ClassPermissionPattern::<CardClass> {
                verb: Some(CardVerb::Derive),
                owner: account_owner("acme"),
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: CardResourcePattern::Any,
            }),
        ),
        (
            "card_install",
            "card(acme) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : install : acme/shop/prod/cart-svc/ShoppingCart(*)",
            PermissionPattern::Card(ClassPermissionPattern::<CardClass> {
                verb: Some(CardVerb::Install),
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
            PermissionPattern::System(ClassPermissionPattern::<SystemClass> {
                verb: Some(SystemVerb::CreateAccount),
                owner: EmptyOwnerPattern,
                recipient: account_recipient("acme"),
                resource: SystemResourcePattern,
            }),
        ),
        (
            "system_email_recipient",
            "system() @ alice@example.com : create-account :",
            PermissionPattern::System(ClassPermissionPattern::<SystemClass> {
                verb: Some(SystemVerb::CreateAccount),
                owner: EmptyOwnerPattern,
                recipient: account_recipient("alice@example.com"),
                resource: SystemResourcePattern,
            }),
        ),
        (
            "plan",
            "plan() @ acme : view : plan-a",
            PermissionPattern::Plan(ClassPermissionPattern::<PlanClass> {
                verb: Some(PlanVerb::View),
                owner: EmptyOwnerPattern,
                recipient: account_recipient("acme"),
                resource: PlanResourcePattern::Plan(PlanIdPattern::Identifier(PlanIdentifier(
                    "plan-a".to_string(),
                ))),
            }),
        ),
        (
            "plan_any",
            "plan() @ acme : create : *",
            PermissionPattern::Plan(ClassPermissionPattern::<PlanClass> {
                verb: Some(PlanVerb::Create),
                owner: EmptyOwnerPattern,
                recipient: account_recipient("acme"),
                resource: PlanResourcePattern::Any,
            }),
        ),
        (
            "account",
            "account(acme) @ acme : view :",
            PermissionPattern::Account(ClassPermissionPattern::<AccountClass> {
                verb: Some(AccountVerb::View),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountResourcePattern,
            }),
        ),
        (
            "account_view_plan",
            "account(acme) @ acme : view-plan :",
            PermissionPattern::Account(ClassPermissionPattern::<AccountClass> {
                verb: Some(AccountVerb::ViewPlan),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountResourcePattern,
            }),
        ),
        (
            "account_usage",
            "account.usage(acme) @ acme : view :",
            PermissionPattern::AccountUsage(ClassPermissionPattern::<AccountUsageClass> {
                verb: Some(AccountUsageVerb::View),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountUsageResourcePattern,
            }),
        ),
        (
            "account_token",
            "account.token(acme) @ acme : view : 550e8400-e29b-41d4-a716-446655440000",
            PermissionPattern::AccountToken(ClassPermissionPattern::<AccountTokenClass> {
                verb: Some(AccountTokenVerb::View),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountTokenResourcePattern::Token(token_id()),
            }),
        ),
        (
            "account_token_delete",
            "account.token(acme) @ acme : delete : 550e8400-e29b-41d4-a716-446655440000",
            PermissionPattern::AccountToken(ClassPermissionPattern::<AccountTokenClass> {
                verb: Some(AccountTokenVerb::Delete),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountTokenResourcePattern::Token(token_id()),
            }),
        ),
        (
            "account_token_create_any",
            "account.token(acme) @ acme : create : *",
            PermissionPattern::AccountToken(ClassPermissionPattern::<AccountTokenClass> {
                verb: Some(AccountTokenVerb::Create),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountTokenResourcePattern::Any,
            }),
        ),
        (
            "account_plugin",
            "account.plugin(acme) @ acme : view : plugin-a",
            PermissionPattern::AccountPlugin(ClassPermissionPattern::<AccountPluginClass> {
                verb: Some(AccountPluginVerb::View),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountPluginResourcePattern::Name(AccountPluginName(
                    "plugin-a".to_string(),
                )),
            }),
        ),
        (
            "account_permission_share",
            "account.permission-share(acme) @ acme : update : team-access",
            PermissionPattern::AccountPermissionShare(ClassPermissionPattern::<
                AccountPermissionShareClass,
            > {
                verb: Some(AccountPermissionShareVerb::Update),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountPermissionShareResourcePattern::Name(PermissionShareName(
                    "team-access".to_string(),
                )),
            }),
        ),
        (
            "account_permission_share_create_any",
            "account.permission-share(acme) @ acme : create : *",
            PermissionPattern::AccountPermissionShare(ClassPermissionPattern::<
                AccountPermissionShareClass,
            > {
                verb: Some(AccountPermissionShareVerb::Create),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountPermissionShareResourcePattern::Any,
            }),
        ),
        (
            "application",
            "application(acme) @ acme : view : shop",
            PermissionPattern::Application(ClassPermissionPattern::<ApplicationClass> {
                verb: Some(ApplicationVerb::View),
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
            PermissionPattern::Application(ClassPermissionPattern::<ApplicationClass> {
                verb: Some(ApplicationVerb::Create),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: ApplicationResourcePattern::Any,
            }),
        ),
        (
            "environment",
            "environment(acme/shop) @ acme/shop/prod : view : prod",
            PermissionPattern::Environment(ClassPermissionPattern::<EnvironmentClass> {
                verb: Some(EnvironmentVerb::View),
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
            PermissionPattern::Environment(ClassPermissionPattern::<EnvironmentClass> {
                verb: Some(EnvironmentVerb::Create),
                owner: application_owner("acme", "shop"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentResourcePattern::Any,
            }),
        ),
        (
            "environment_rollback_revision",
            "environment(acme/shop) @ acme/shop/prod : rollback : prod@rev=42",
            PermissionPattern::Environment(ClassPermissionPattern::<EnvironmentClass> {
                verb: Some(EnvironmentVerb::Rollback),
                owner: application_owner("acme", "shop"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentResourcePattern::Revision {
                    environment: EnvironmentName("prod".to_string()),
                    revision: 42,
                },
            }),
        ),
        (
            "environment_plugin_grant",
            "environment.plugin-grant(acme/shop/prod) @ acme/shop/prod : view : plugin-a",
            PermissionPattern::EnvironmentPluginGrant(ClassPermissionPattern::<
                EnvironmentPluginGrantClass,
            > {
                verb: Some(EnvironmentPluginGrantVerb::View),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentPluginGrantResourcePattern::Name(EnvironmentPluginGrantName(
                    "plugin-a".to_string(),
                )),
            }),
        ),
        (
            "environment_domain_registration",
            "environment.domain-registration(acme/shop/prod) @ acme/shop/prod : view : domain-a",
            PermissionPattern::EnvironmentDomainRegistration(ClassPermissionPattern::<
                EnvironmentDomainRegistrationClass,
            > {
                verb: Some(EnvironmentDomainRegistrationVerb::View),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentDomainRegistrationResourcePattern::Domain(DomainNamePattern {
                    labels: vec![DomainLabel("domain-a".to_string())],
                }),
            }),
        ),
        (
            "environment_security_scheme",
            "environment.security-scheme(acme/shop/prod) @ acme/shop/prod : view : scheme-a",
            PermissionPattern::EnvironmentSecurityScheme(ClassPermissionPattern::<
                EnvironmentSecuritySchemeClass,
            > {
                verb: Some(EnvironmentSecuritySchemeVerb::View),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentSecuritySchemeResourcePattern::Name(
                    EnvironmentSecuritySchemeName("scheme-a".to_string()),
                ),
            }),
        ),
        (
            "environment_http_api_deployment",
            "environment.http-api-deployment(acme/shop/prod) @ acme/shop/prod : view : api./v1/**",
            PermissionPattern::EnvironmentHttpApiDeployment(ClassPermissionPattern::<
                EnvironmentHttpApiDeploymentClass,
            > {
                verb: Some(EnvironmentHttpApiDeploymentVerb::View),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentHttpApiDeploymentResourcePattern::DomainPath {
                    domain: "api".to_string(),
                    path_glob: "/v1/**".to_string(),
                },
            }),
        ),
        (
            "environment_mcp_deployment",
            "environment.mcp-deployment(acme/shop/prod) @ acme/shop/prod : view : mcp.example.com",
            PermissionPattern::EnvironmentMcpDeployment(ClassPermissionPattern::<
                EnvironmentMcpDeploymentClass,
            > {
                verb: Some(EnvironmentMcpDeploymentVerb::View),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentMcpDeploymentResourcePattern::Name(
                    EnvironmentMcpDeploymentName("mcp.example.com".to_string()),
                ),
            }),
        ),
        (
            "environment_agent_secret",
            "environment.agent-secret(acme/shop/prod) @ acme/shop/prod : update : cart.*",
            PermissionPattern::EnvironmentAgentSecret(ClassPermissionPattern::<
                EnvironmentAgentSecretClass,
            > {
                verb: Some(EnvironmentAgentSecretVerb::Update),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: environment_agent_secret_key(vec![
                    EnvironmentAgentSecretKeySegmentPattern::Literal("cart".to_string()),
                    EnvironmentAgentSecretKeySegmentPattern::Star,
                ]),
            }),
        ),
        (
            "environment_resource_definition",
            "environment.resource-definition(acme/shop/prod) @ acme/shop/prod : view : resource-a",
            PermissionPattern::EnvironmentResourceDefinition(ClassPermissionPattern::<
                EnvironmentResourceDefinitionClass,
            > {
                verb: Some(EnvironmentResourceDefinitionVerb::View),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentResourceDefinitionResourcePattern::Name(
                    EnvironmentResourceDefinitionName("resource-a".to_string()),
                ),
            }),
        ),
        (
            "environment_retry_policy",
            "environment.retry-policy(acme/shop/prod) @ acme/shop/prod : view : retry-a",
            PermissionPattern::EnvironmentRetryPolicy(ClassPermissionPattern::<
                EnvironmentRetryPolicyClass,
            > {
                verb: Some(EnvironmentRetryPolicyVerb::View),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentRetryPolicyResourcePattern::Name(EnvironmentRetryPolicyName(
                    "retry-a".to_string(),
                )),
            }),
        ),
        (
            "component",
            "component(acme/shop/prod) @ acme/shop/prod : view : cart-svc",
            PermissionPattern::Component(ClassPermissionPattern::<ComponentClass> {
                verb: Some(ComponentVerb::View),
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
            PermissionPattern::Component(ClassPermissionPattern::<ComponentClass> {
                verb: Some(ComponentVerb::Create),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: ComponentResourcePattern::Any,
            }),
        ),
        (
            "component_revision",
            "component(acme/shop/prod) @ acme/shop/prod : view : cart-svc@rev=42",
            PermissionPattern::Component(ClassPermissionPattern::<ComponentClass> {
                verb: Some(ComponentVerb::View),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: ComponentResourcePattern::Revision {
                    component: ComponentName("cart-svc".to_string()),
                    revision: 42,
                },
            }),
        ),
        (
            "account_oauth2_identity",
            "account.oauth2-identity(acme) @ acme : view : google/12345",
            PermissionPattern::AccountOauth2Identity(ClassPermissionPattern::<
                AccountOauth2IdentityClass,
            > {
                verb: Some(AccountOauth2IdentityVerb::View),
                owner: account_owner("acme"),
                recipient: account_recipient("acme"),
                resource: AccountOauth2IdentityResourcePattern::Identity {
                    provider: "google".to_string(),
                    external_id: "12345".to_string(),
                },
            }),
        ),
        (
            "environment_initial_files",
            "environment.initial-files(acme/shop/prod/cart-svc) @ acme/shop/prod : view : /etc/*",
            PermissionPattern::EnvironmentInitialFiles(ClassPermissionPattern::<
                EnvironmentInitialFilesClass,
            > {
                verb: Some(EnvironmentInitialFilesVerb::View),
                owner: ComponentOwnerPattern::Component {
                    account: account_email("acme"),
                    application: application_name("shop"),
                    environment: environment_name("prod"),
                    component: component_name("cart-svc"),
                },
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentInitialFilesResourcePattern::Path(
                    EnvironmentInitialFilesPathPattern {
                        segments: vec![
                            EnvironmentInitialFilesPathSegmentPattern::Literal("etc".to_string()),
                            EnvironmentInitialFilesPathSegmentPattern::Star,
                        ],
                    },
                ),
            }),
        ),
        (
            "environment_kv_bucket",
            "environment.kv-bucket(acme/shop/prod) @ acme/shop/prod : view : bucket-a",
            PermissionPattern::EnvironmentKvBucket(ClassPermissionPattern::<
                EnvironmentKvBucketClass,
            > {
                verb: Some(EnvironmentKvBucketVerb::View),
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
            PermissionPattern::EnvironmentBlobBucket(ClassPermissionPattern::<
                EnvironmentBlobBucketClass,
            > {
                verb: Some(EnvironmentBlobBucketVerb::View),
                owner: environment_owner("acme", "shop", "prod"),
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentBlobBucketResourcePattern::Name(EnvironmentBlobBucketName(
                    "bucket-a".to_string(),
                )),
            }),
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
    let result = parse_permission(
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
    assert!(matches!(
        parse_permission("filesystem(acme) : read : /data/**"),
        Err(CardParseError::Malformed(_))
    ));
    assert!(matches!(
        parse_permission("system( @ acme : create-account :"),
        Err(CardParseError::Malformed(_))
    ));
    assert!(matches!(
        parse_permission("application(acme @ acme : view : shop"),
        Err(CardParseError::Malformed(_))
    ));
    assert_eq!(
        parse_permission("filesystem(acme) @ acme : query : /data/**"),
        Err(CardParseError::InvalidOwnerPath {
            class: "filesystem".to_string(),
            owner: "acme".to_string(),
        })
    );
    assert_eq!(
        parse_permission("system(acme) @ acme : create-account :"),
        Err(CardParseError::InvalidOwnerPath {
            class: "system".to_string(),
            owner: "acme".to_string(),
        })
    );
    assert_eq!(
        parse_permission("system() @ acme : create-account : not-empty"),
        Err(CardParseError::InvalidResource {
            class: "system".to_string(),
            resource: "not-empty".to_string(),
        })
    );
    assert_eq!(
        parse_permission(
            "card(acme) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : install : acme/shop"
        ),
        Err(CardParseError::InvalidRecipientPath(
            "acme/shop".to_string()
        ))
    );
    assert_eq!(
        parse_permission("unknown(acme) @ acme : view :"),
        Err(CardParseError::UnknownClass("unknown".to_string()))
    );
}

#[test]
fn rejects_removed_application_credential_and_restore_forms() {
    let credential_id = "550e8400-e29b-41d4-a716-446655440000";

    assert_eq!(
        parse_permission("application(acme/shop) @ acme : view :"),
        Err(CardParseError::InvalidOwnerPath {
            class: ApplicationClass::NAME.to_string(),
            owner: "acme/shop".to_string(),
        })
    );
    assert_eq!(
        parse_permission(&format!(
            "application(acme) @ acme : view-credentials : cred={credential_id}"
        )),
        Err(CardParseError::UnknownVerb {
            class: ApplicationClass::NAME.to_string(),
            verb: "view-credentials".to_string(),
        })
    );
    assert_eq!(
        parse_permission("account(acme) @ acme : restore :"),
        Err(CardParseError::UnknownVerb {
            class: AccountClass::NAME.to_string(),
            verb: "restore".to_string(),
        })
    );
    assert_eq!(
        parse_permission("application(acme) @ acme : restore : shop"),
        Err(CardParseError::UnknownVerb {
            class: ApplicationClass::NAME.to_string(),
            verb: "restore".to_string(),
        })
    );
    assert_eq!(
        parse_permission("environment(acme/shop) @ acme/shop/prod : restore : prod"),
        Err(CardParseError::UnknownVerb {
            class: EnvironmentClass::NAME.to_string(),
            verb: "restore".to_string(),
        })
    );
}

#[test]
fn rejects_recipient_depths_without_holder_kind() {
    assert_eq!(
        parse_permission("system() @ acme/shop : create-account :"),
        Err(CardParseError::InvalidRecipientPath(
            "acme/shop".to_string()
        ))
    );
    assert_eq!(
        parse_permission(
            "filesystem(acme/shop/prod/cart/agent) @ acme/shop/prod/cart : read : /data/**"
        ),
        Err(CardParseError::InvalidRecipientPath(
            "acme/shop/prod/cart".to_string()
        ))
    );
}

#[test]
fn rejects_invalid_network_resource_patterns() {
    for resource in [
        ":8080",
        "api..internal",
        ".api.internal",
        "api.internal.",
        "api.*ternal:8080",
        "api.internal:*",
        "api.internal:abc",
        "api.internal:",
        "api.internal:9000-8000",
        "api.internal:8080-9000-1",
    ] {
        assert_eq!(
            parse_permission(&format!(
                "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : {resource}"
            )),
            Err(CardParseError::InvalidResource {
                class: NetworkClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        );
    }
}

#[test]
fn rejects_invalid_filesystem_resource_patterns() {
    for resource in [
        "data/**",
        "/data//file",
        "/data/",
        "/data/***",
        "/data/fi*le",
    ] {
        assert_eq!(
            parse_permission(&format!(
                "filesystem(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : {resource}"
            )),
            Err(CardParseError::InvalidResource {
                class: FilesystemClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        );
    }
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
            parse_permission(input),
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
            "environment_polymorphic_owner_concrete_environment_recipient",
            "environment(?env) @ acme/shop/prod : view : prod",
            PolymorphicPermissionPattern::Environment(PolymorphicClassPermissionPattern::<
                EnvironmentClass,
            > {
                verb: Some(EnvironmentVerb::View),
                owner: PolymorphicApplicationOwnerPattern::Env,
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: EnvironmentResourcePattern::Environment(EnvironmentName(
                    "prod".to_string(),
                )),
            }),
        ),
        (
            "component_polymorphic_owner_concrete_environment_recipient",
            "component(?env) @ acme/shop/prod : view : cart-svc@rev=42",
            PolymorphicPermissionPattern::Component(PolymorphicClassPermissionPattern::<
                ComponentClass,
            > {
                verb: Some(ComponentVerb::View),
                owner: PolymorphicEnvironmentOwnerPattern::Env,
                recipient: environment_recipient("acme", "shop", "prod"),
                resource: ComponentResourcePattern::Revision {
                    component: ComponentName("cart-svc".to_string()),
                    revision: 42,
                },
            }),
        ),
        (
            "env_self_owner_concrete_agent_recipient",
            "env(?self) @ acme/shop/prod/cart-svc/ShoppingCart(*) : read : HOME",
            PolymorphicPermissionPattern::Env(PolymorphicClassPermissionPattern::<EnvClass> {
                verb: Some(EnvVerb::Read),
                owner: PolymorphicAgentOwnerPattern::Self_,
                recipient: agent_recipient("acme", "shop", "prod", "cart-svc", "ShoppingCart(*)"),
                resource: EnvResourcePattern::VarName(EnvVarName("HOME".to_string())),
            }),
        ),
        (
            "secret_polymorphic_owner_concrete_agent_recipient",
            "secret(?env) @ acme/shop/prod/cart-svc/ShoppingCart(*) : reveal : billing.account",
            PolymorphicPermissionPattern::Secret(
                PolymorphicClassPermissionPattern::<SecretClass> {
                    verb: Some(SecretVerb::Reveal),
                    owner: PolymorphicEnvironmentOwnerPattern::Env,
                    recipient: agent_recipient(
                        "acme",
                        "shop",
                        "prod",
                        "cart-svc",
                        "ShoppingCart(*)",
                    ),
                    resource: secret_key(vec![
                        SecretKeySegmentPattern::Literal("billing".to_string()),
                        SecretKeySegmentPattern::Literal("account".to_string()),
                    ]),
                },
            ),
        ),
        (
            "agent_env_owner_template_concrete_agent_recipient",
            "agent(?env/payment-svc/PaymentAgent(*)) @ acme/shop/prod/payment-svc/PaymentAgent(*) : invoke : charge",
            PolymorphicPermissionPattern::Agent(PolymorphicClassPermissionPattern::<AgentClass> {
                verb: Some(AgentVerb::Invoke),
                owner: PolymorphicAgentOwnerPattern::EnvAgent {
                    component: component_name("payment-svc"),
                    agent: AgentOwnerLeafPattern::AgentTypeWildcard(agent_type_name(
                        "PaymentAgent",
                    )),
                },
                recipient: agent_recipient(
                    "acme",
                    "shop",
                    "prod",
                    "payment-svc",
                    "PaymentAgent(*)",
                ),
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
                assert_eq!(parse_polymorphic_permission(input), Ok((*expected).clone()));
            }
        );
    }
}

#[test_gen]
fn parses_polymorphic_manifest_pattern_grant_examples_from_spec(r: &mut DynamicTestRegistration) {
    let cases: Vec<(&str, &str, PolymorphicManifestPermissionPattern)> = vec![
        (
            "account_recipient_slot",
            "account(acme) @ ?account : view :",
            PolymorphicManifestPermissionPattern::Account(
                PolymorphicManifestClassPermissionPattern::<AccountClass> {
                    verb: Some(AccountVerb::View),
                    owner: PolymorphicAccountOwnerPattern::Concrete(account_owner("acme")),
                    recipient: PolymorphicRecipientPattern::Account,
                    resource: AccountResourcePattern,
                },
            ),
        ),
        (
            "system_recipient_slot",
            "system() @ ?account : create-account :",
            PolymorphicManifestPermissionPattern::System(
                PolymorphicManifestClassPermissionPattern::<SystemClass> {
                    verb: Some(SystemVerb::CreateAccount),
                    owner: PolymorphicEmptyOwnerPattern::Concrete(EmptyOwnerPattern),
                    recipient: PolymorphicRecipientPattern::Account,
                    resource: SystemResourcePattern,
                },
            ),
        ),
        (
            "application_any_verb_and_resource",
            "application(acme) @ ?account : * : *",
            PolymorphicManifestPermissionPattern::Application(
                PolymorphicManifestClassPermissionPattern::<ApplicationClass> {
                    verb: None,
                    owner: PolymorphicAccountOwnerPattern::Concrete(account_owner("acme")),
                    recipient: PolymorphicRecipientPattern::Account,
                    resource: ApplicationResourcePattern::Any,
                },
            ),
        ),
        (
            "environment_owner_and_recipient_slots",
            "environment(?env) @ ?env : view : prod",
            PolymorphicManifestPermissionPattern::Environment(
                PolymorphicManifestClassPermissionPattern::<EnvironmentClass> {
                    verb: Some(EnvironmentVerb::View),
                    owner: PolymorphicApplicationOwnerPattern::Env,
                    recipient: PolymorphicRecipientPattern::Environment(
                        PolymorphicEnvironmentRecipientPattern::Environment,
                    ),
                    resource: EnvironmentResourcePattern::Environment(EnvironmentName(
                        "prod".to_string(),
                    )),
                },
            ),
        ),
        (
            "environment_account_recipient_slot",
            "environment(?env) @ ?account/*/* : create : *",
            PolymorphicManifestPermissionPattern::Environment(
                PolymorphicManifestClassPermissionPattern::<EnvironmentClass> {
                    verb: Some(EnvironmentVerb::Create),
                    owner: PolymorphicApplicationOwnerPattern::Env,
                    recipient: PolymorphicRecipientPattern::Environment(
                        PolymorphicEnvironmentRecipientPattern::AccountEnvironments,
                    ),
                    resource: EnvironmentResourcePattern::Any,
                },
            ),
        ),
        (
            "component_env_owner_slot_with_revision_resource",
            "component(?env) @ ?env : view : cart-svc@rev=42",
            PolymorphicManifestPermissionPattern::Component(
                PolymorphicManifestClassPermissionPattern::<ComponentClass> {
                    verb: Some(ComponentVerb::View),
                    owner: PolymorphicEnvironmentOwnerPattern::Env,
                    recipient: PolymorphicRecipientPattern::Environment(
                        PolymorphicEnvironmentRecipientPattern::Environment,
                    ),
                    resource: ComponentResourcePattern::Revision {
                        component: ComponentName("cart-svc".to_string()),
                        revision: 42,
                    },
                },
            ),
        ),
        (
            "env_self_slots",
            "env(?self) @ ?self : read : HOME",
            PolymorphicManifestPermissionPattern::Env(PolymorphicManifestClassPermissionPattern::<
                EnvClass,
            > {
                verb: Some(EnvVerb::Read),
                owner: PolymorphicAgentOwnerPattern::Self_,
                recipient: PolymorphicRecipientPattern::Agent(
                    PolymorphicAgentRecipientPattern::Self_,
                ),
                resource: EnvResourcePattern::VarName(EnvVarName("HOME".to_string())),
            }),
        ),
        (
            "secret_monomorphic_resource",
            "secret(?env) @ ?self : reveal : billing.account",
            PolymorphicManifestPermissionPattern::Secret(
                PolymorphicManifestClassPermissionPattern::<SecretClass> {
                    verb: Some(SecretVerb::Reveal),
                    owner: PolymorphicEnvironmentOwnerPattern::Env,
                    recipient: PolymorphicRecipientPattern::Agent(
                        PolymorphicAgentRecipientPattern::Self_,
                    ),
                    resource: secret_key(vec![
                        SecretKeySegmentPattern::Literal("billing".to_string()),
                        SecretKeySegmentPattern::Literal("account".to_string()),
                    ]),
                },
            ),
        ),
        (
            "secret_resource_glob",
            "secret(?env) @ ?self : reveal : billing.*",
            PolymorphicManifestPermissionPattern::Secret(
                PolymorphicManifestClassPermissionPattern::<SecretClass> {
                    verb: Some(SecretVerb::Reveal),
                    owner: PolymorphicEnvironmentOwnerPattern::Env,
                    recipient: PolymorphicRecipientPattern::Agent(
                        PolymorphicAgentRecipientPattern::Self_,
                    ),
                    resource: secret_key(vec![
                        SecretKeySegmentPattern::Literal("billing".to_string()),
                        SecretKeySegmentPattern::Star,
                    ]),
                },
            ),
        ),
        (
            "secret_environment_agent_recipient_slot",
            "secret(?env) @ ?env/cart-svc/ShoppingCart(*) : hold : cart.api-key",
            PolymorphicManifestPermissionPattern::Secret(
                PolymorphicManifestClassPermissionPattern::<SecretClass> {
                    verb: Some(SecretVerb::Hold),
                    owner: PolymorphicEnvironmentOwnerPattern::Env,
                    recipient: PolymorphicRecipientPattern::Agent(
                        PolymorphicAgentRecipientPattern::EnvironmentAgent {
                            component: "cart-svc".to_string(),
                            agent: "ShoppingCart(*)".to_string(),
                        },
                    ),
                    resource: secret_key(vec![
                        SecretKeySegmentPattern::Literal("cart".to_string()),
                        SecretKeySegmentPattern::Literal("api-key".to_string()),
                    ]),
                },
            ),
        ),
        (
            "secret_concrete_recipient",
            "secret(?env) @ acme/shop/prod/cart-svc/ShoppingCart(*) : hold : cart.api-key",
            PolymorphicManifestPermissionPattern::Secret(
                PolymorphicManifestClassPermissionPattern::<SecretClass> {
                    verb: Some(SecretVerb::Hold),
                    owner: PolymorphicEnvironmentOwnerPattern::Env,
                    recipient: PolymorphicRecipientPattern::Concrete(agent_recipient(
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
                },
            ),
        ),
        (
            "agent_env_owner_template",
            "agent(?env/payment-svc/PaymentAgent(*)) @ ?self : invoke : charge",
            PolymorphicManifestPermissionPattern::Agent(
                PolymorphicManifestClassPermissionPattern::<AgentClass> {
                    verb: Some(AgentVerb::Invoke),
                    owner: PolymorphicAgentOwnerPattern::EnvAgent {
                        component: component_name("payment-svc"),
                        agent: AgentOwnerLeafPattern::AgentTypeWildcard(agent_type_name(
                            "PaymentAgent",
                        )),
                    },
                    recipient: PolymorphicRecipientPattern::Agent(
                        PolymorphicAgentRecipientPattern::Self_,
                    ),
                    resource: AgentResourcePattern::Method(AgentMethodName("charge".to_string())),
                },
            ),
        ),
        (
            "agent_env_component_agents_owner_and_recipient_slots",
            "agent(?env/payment-svc/*) @ ?component/* : * : *",
            PolymorphicManifestPermissionPattern::Agent(
                PolymorphicManifestClassPermissionPattern::<AgentClass> {
                    verb: None,
                    owner: PolymorphicAgentOwnerPattern::EnvComponentAgents {
                        component: component_name("payment-svc"),
                    },
                    recipient: PolymorphicRecipientPattern::Agent(
                        PolymorphicAgentRecipientPattern::ComponentAgents,
                    ),
                    resource: AgentResourcePattern::Any,
                },
            ),
        ),
        (
            "agent_component_agent_recipient_slot",
            "agent(?env/payment-svc/PaymentAgent(*)) @ ?component/PaymentAgent(*) : invoke : charge",
            PolymorphicManifestPermissionPattern::Agent(
                PolymorphicManifestClassPermissionPattern::<AgentClass> {
                    verb: Some(AgentVerb::Invoke),
                    owner: PolymorphicAgentOwnerPattern::EnvAgent {
                        component: component_name("payment-svc"),
                        agent: AgentOwnerLeafPattern::AgentTypeWildcard(agent_type_name(
                            "PaymentAgent",
                        )),
                    },
                    recipient: PolymorphicRecipientPattern::Agent(
                        PolymorphicAgentRecipientPattern::ComponentAgent {
                            agent: "PaymentAgent(*)".to_string(),
                        },
                    ),
                    resource: AgentResourcePattern::Method(AgentMethodName("charge".to_string())),
                },
            ),
        ),
        (
            "tool_env_component_tools_owner",
            "tool(?env/cli-tools/*) @ ?self : invoke : *",
            PolymorphicManifestPermissionPattern::Tool(
                PolymorphicManifestClassPermissionPattern::<ToolClass> {
                    verb: Some(ToolVerb::Invoke),
                    owner: PolymorphicToolOwnerPattern::EnvComponentTools {
                        component: component_name("cli-tools"),
                    },
                    recipient: PolymorphicRecipientPattern::Agent(
                        PolymorphicAgentRecipientPattern::Self_,
                    ),
                    resource: ToolResourcePattern::AnyInvocation,
                },
            ),
        ),
    ];

    for (name, input, expected) in cases {
        let expected = std::sync::Arc::new(expected);
        add_test!(
            r,
            format!("parses_polymorphic_manifest_pattern_grant_{name}"),
            TestProperties::unit_test(),
            || {
                assert_eq!(
                    parse_polymorphic_manifest_permission(input),
                    Ok((*expected).clone())
                );
            }
        );
    }
}

#[test]
fn empty_resource_classes_reject_polymorphic_resource_slots() {
    assert_eq!(
        parse_polymorphic_manifest_permission("account(acme) @ ?account : view : ?resource"),
        Err(CardParseError::InvalidResource {
            class: AccountClass::NAME.to_string(),
            resource: "?resource".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_manifest_permission("system() @ ?account : create-account : ?resource"),
        Err(CardParseError::InvalidResource {
            class: SystemClass::NAME.to_string(),
            resource: "?resource".to_string(),
        })
    );
}

#[test]
fn rejects_polymorphic_resource_slots_and_templates() {
    assert_eq!(
        parse_polymorphic_manifest_permission("env(?self) @ ?self : read : ?env_var"),
        Err(CardParseError::InvalidResource {
            class: EnvClass::NAME.to_string(),
            resource: "?env_var".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_manifest_permission("card(acme) @ ?self : install : ?self"),
        Err(CardParseError::InvalidResource {
            class: CardClass::NAME.to_string(),
            resource: "?self".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_manifest_permission("secret(?env) @ ?self : reveal : secret.?self"),
        Err(CardParseError::InvalidResource {
            class: SecretClass::NAME.to_string(),
            resource: "secret.?self".to_string(),
        })
    );
}

#[test]
fn rejects_undeclared_polymorphic_owner_slots() {
    assert_eq!(
        parse_polymorphic_permission("account(?account) @ ?account : view :"),
        Err(CardParseError::InvalidOwnerPath {
            class: AccountClass::NAME.to_string(),
            owner: "?account".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_permission("application(?app) @ ?account : view :"),
        Err(CardParseError::InvalidOwnerPath {
            class: ApplicationClass::NAME.to_string(),
            owner: "?app".to_string(),
        })
    );

    assert_eq!(
        parse_polymorphic_permission("component(?component) @ ?env : view : cart-svc"),
        Err(CardParseError::InvalidOwnerPath {
            class: ComponentClass::NAME.to_string(),
            owner: "?component".to_string(),
        })
    );
}

#[test]
fn rejects_polymorphic_owner_slots_with_wrong_scope() {
    assert_eq!(
        parse_polymorphic_permission("filesystem(?env) @ ?self : read : /data/**"),
        Err(CardParseError::InvalidOwnerPath {
            class: FilesystemClass::NAME.to_string(),
            owner: "?env".to_string(),
        })
    );
}

#[test]
fn rejects_undeclared_polymorphic_recipient_slots() {
    assert_eq!(
        parse_polymorphic_permission("secret(?env) @ ?user : reveal : billing.*"),
        Err(CardParseError::InvalidRecipientPath("?user".to_string()))
    );
    assert_eq!(
        parse_polymorphic_permission("secret(?env) @ ?app : reveal : billing.*"),
        Err(CardParseError::InvalidRecipientPath("?app".to_string()))
    );
    assert_eq!(
        parse_polymorphic_permission("secret(?env) @ ?self : reveal : billing.*"),
        Err(CardParseError::InvalidRecipientPath("?self".to_string()))
    );
}

#[test]
fn concrete_parser_rejects_slot_variables() {
    let result = parse_permission("secret(?env) @ ?self : reveal : billing.*");

    assert_eq!(
        result,
        Err(CardParseError::SlotVariableInConcreteGrant(
            "?env".to_string()
        ))
    );
}

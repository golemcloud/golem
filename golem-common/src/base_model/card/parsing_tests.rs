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

#[test]
fn parses_canonical_pattern_grant() {
    let grant = parse_pattern_grant(
        "filesystem(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : /data/**",
    )
    .unwrap();

    assert_eq!(
        grant.permission,
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Verb {
            verb: FilesystemVerb::Read,
            owner: AgentOwnerPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart".to_string(),
                agent: AgentOwnerLeafPattern::Agent("agent".to_string()),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart".to_string(),
                agent: "agent".to_string(),
            },
            resource: FilesystemResourcePattern::Path(FilesystemPathPattern {
                segments: vec![
                    FilesystemPathSegmentPattern::Literal("data".to_string()),
                    FilesystemPathSegmentPattern::GlobStar,
                ],
            }),
        })
    );
}

#[test]
fn parses_email_account_recipient() {
    let grant = parse_pattern_grant("system() @ alice@example.com : create-account :").unwrap();

    assert_eq!(
        grant.permission,
        PermissionPattern::System(SystemPermissionPattern::Verb {
            verb: SystemVerb::CreateAccount,
            owner: EmptyOwnerPattern,
            recipient: AccountRecipientPattern::Account {
                account: "alice@example.com".to_string()
            },
            resource: SystemResourcePattern,
        })
    );
}

#[test]
fn parses_email_recipient_inside_agent_scope() {
    let grant = parse_pattern_grant(
        "filesystem(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ alice@example.com/shop/prod/cart-svc/CartAgent(\"42\") : read : /data/**",
    )
    .unwrap();

    assert_eq!(
        grant.permission,
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Verb {
            verb: FilesystemVerb::Read,
            owner: AgentOwnerPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: AgentOwnerLeafPattern::Agent("CartAgent(\"42\")".to_string()),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "alice@example.com".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "CartAgent(\"42\")".to_string(),
            },
            resource: FilesystemResourcePattern::Path(FilesystemPathPattern {
                segments: vec![
                    FilesystemPathSegmentPattern::Literal("data".to_string()),
                    FilesystemPathSegmentPattern::GlobStar,
                ],
            }),
        })
    );
}

#[test]
fn parses_resource_ids_with_colons() {
    let grant = parse_pattern_grant(
        "oplog(acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : start=1000:end=2000",
    )
    .unwrap();

    assert_eq!(
        grant.permission,
        PermissionPattern::Oplog(OplogPermissionPattern::Verb {
            verb: OplogVerb::Read,
            owner: AgentOwnerPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart".to_string(),
                agent: AgentOwnerLeafPattern::Agent("agent".to_string()),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart".to_string(),
                agent: "agent".to_string(),
            },
            resource: OplogResourcePattern::Range {
                start: Some(1000),
                end: Some(2000)
            },
        })
    );
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

#[test_gen]
fn generate_declared_permission_class_parser_tests(r: &mut DynamicTestRegistration) {
    let cases = [
        (
            "filesystem(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : /data/**",
            "filesystem",
        ),
        (
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal:8080",
            "network",
        ),
        (
            "env(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : HOME",
            "env",
        ),
        (
            "oplog(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : *",
            "oplog",
        ),
        (
            "config(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : model.retry-count",
            "config",
        ),
        (
            "secret(acme/shop/prod) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : hold : cart.api-key",
            "secret",
        ),
        (
            "agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : invoke : add-item",
            "agent",
        ),
        (
            "tool(acme/shop/prod/cli-tools/grep) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : invoke : search",
            "tool",
        ),
        (
            "kv(acme/shop/prod) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : my-store.user-*",
            "kv",
        ),
        (
            "blob(acme/shop/prod) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : my-bucket.models/*.bin",
            "blob",
        ),
        (
            "rdbms(acme/shop/prod) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : query : orders.public.orders",
            "rdbms",
        ),
        (
            "card(acme) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : derive :",
            "card",
        ),
        ("system() @ acme : create-account :", "system"),
        ("plan() @ acme : view : plan-a", "plan"),
        ("account(acme) @ acme : view :", "account"),
        ("account.usage(acme) @ acme : view :", "account.usage"),
        (
            "account.token(acme) @ acme : view : 550e8400-e29b-41d4-a716-446655440000",
            "account.token",
        ),
        (
            "account.plugin(acme) @ acme : view : plugin-a",
            "account.plugin",
        ),
        ("application(acme) @ acme : view : shop", "application"),
        (
            "environment(acme/shop) @ acme/shop/prod : view : prod",
            "environment",
        ),
        (
            "environment.share(acme/shop/prod) @ acme/shop/prod : view : 550e8400-e29b-41d4-a716-446655440000",
            "environment.share",
        ),
        (
            "environment.plugin-grant(acme/shop/prod) @ acme/shop/prod : view : plugin-a",
            "environment.plugin-grant",
        ),
        (
            "environment.domain-registration(acme/shop/prod) @ acme/shop/prod : view : domain-a",
            "environment.domain-registration",
        ),
        (
            "environment.security-scheme(acme/shop/prod) @ acme/shop/prod : view : scheme-a",
            "environment.security-scheme",
        ),
        (
            "environment.http-api-deployment(acme/shop/prod) @ acme/shop/prod : view : api./v1/**",
            "environment.http-api-deployment",
        ),
        (
            "environment.mcp-deployment(acme/shop/prod) @ acme/shop/prod : view : mcp-a",
            "environment.mcp-deployment",
        ),
        (
            "environment.agent-secret(acme/shop/prod) @ acme/shop/prod : update : cart.*",
            "environment.agent-secret",
        ),
        (
            "environment.resource-definition(acme/shop/prod) @ acme/shop/prod : view : resource-a",
            "environment.resource-definition",
        ),
        (
            "environment.retry-policy(acme/shop/prod) @ acme/shop/prod : view : retry-a",
            "environment.retry-policy",
        ),
        (
            "component(acme/shop/prod) @ acme/shop/prod : view : cart-svc",
            "component",
        ),
        (
            "account.oauth2-identity(acme) @ acme : view : google/12345",
            "account.oauth2-identity",
        ),
        (
            "environment.initial-files(acme/shop/prod/cart-svc) @ acme/shop/prod : view : /etc/*",
            "environment.initial-files",
        ),
        (
            "environment.kv-bucket(acme/shop/prod) @ acme/shop/prod : view : bucket-a",
            "environment.kv-bucket",
        ),
        (
            "environment.blob-bucket(acme/shop/prod) @ acme/shop/prod : view : bucket-a",
            "environment.blob-bucket",
        ),
    ];

    for (input, class_name) in cases {
        add_test!(
            r,
            format!("parses_declared_permission_class_{}", test_name(class_name)),
            TestProperties::unit_test(),
            || {
                let grant = parse_pattern_grant(input).expect(input);
                assert_eq!(grant.permission.class_name(), class_name);
            }
        );
    }
}

#[test]
fn parses_runtime_class_examples_from_spec() {
    assert_eq!(
        parsed_permission(
            "filesystem(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : /data/**"
        ),
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Verb {
            verb: FilesystemVerb::Read,
            owner: AgentOwnerPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: AgentOwnerLeafPattern::Agent("CartAgent(\"42\")".to_string()),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "CartAgent(\"42\")".to_string(),
            },
            resource: FilesystemResourcePattern::Path(FilesystemPathPattern {
                segments: vec![
                    FilesystemPathSegmentPattern::Literal("data".to_string()),
                    FilesystemPathSegmentPattern::GlobStar,
                ],
            }),
        })
    );

    assert_eq!(
        parsed_permission(
            "network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal:8080-9000"
        ),
        PermissionPattern::Network(NetworkPermissionPattern::Verb {
            verb: NetworkVerb::Connect,
            owner: EmptyOwnerPattern,
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "CartAgent(\"42\")".to_string(),
            },
            resource: NetworkResourcePattern::HostPort {
                host: "api.internal".to_string(),
                ports: PortPattern::Range {
                    start: 8080,
                    end: 9000,
                },
            },
        })
    );

    assert_eq!(
        parsed_permission(
            "env(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : HOME"
        ),
        PermissionPattern::Env(EnvPermissionPattern::Verb {
            verb: EnvVerb::Read,
            owner: AgentOwnerPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: AgentOwnerLeafPattern::Agent("CartAgent(\"42\")".to_string()),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "CartAgent(\"42\")".to_string(),
            },
            resource: EnvResourcePattern::VarName(EnvVarName("HOME".to_string())),
        })
    );

    assert_eq!(
        parsed_permission(
            "secret(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : reveal : cart.api-key"
        ),
        PermissionPattern::Secret(SecretPermissionPattern::Verb {
            verb: SecretVerb::Reveal,
            owner: EnvironmentOwnerPattern::Environment {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "ShoppingCart(*)".to_string(),
            },
            resource: SecretResourcePattern::Key(SecretKeyPathPattern {
                segments: vec![
                    SecretKeySegmentPattern::Literal("cart".to_string()),
                    SecretKeySegmentPattern::Literal("api-key".to_string()),
                ],
            }),
        })
    );

    assert_eq!(
        parsed_permission(
            "kv(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : read : my-store.user-*"
        ),
        PermissionPattern::Kv(KvPermissionPattern::Verb {
            verb: KvVerb::Read,
            owner: EnvironmentOwnerPattern::Environment {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "ShoppingCart(*)".to_string(),
            },
            resource: KvResourcePattern::StoreKey {
                store: "my-store".to_string(),
                key_pattern: "user-*".to_string(),
            },
        })
    );
}

#[test]
fn parses_agent_tool_and_card_examples_from_spec() {
    assert_eq!(
        parsed_permission(
            "agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : add-item"
        ),
        PermissionPattern::Agent(AgentPermissionPattern::Verb {
            verb: AgentVerb::Invoke,
            owner: AgentOwnerPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: AgentOwnerLeafPattern::AgentTypeWildcard("ShoppingCart".to_string()),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "ShoppingCart(*)".to_string(),
            },
            resource: AgentResourcePattern::Method(AgentMethodName("add-item".to_string())),
        })
    );

    assert_eq!(
        parsed_permission(
            "agent(acme/shop/prod/cart-svc/*) @ acme/shop/prod/cart-svc/* : delete :"
        ),
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
            resource: AgentResourcePattern::Empty,
        })
    );

    assert_eq!(
        parsed_permission(
            "tool(acme/shop/prod/cli-tools/grep) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : search"
        ),
        PermissionPattern::Tool(ToolPermissionPattern::Verb {
            verb: ToolVerb::Invoke,
            owner: ToolOwnerPattern::Tool {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cli-tools".to_string(),
                tool: "grep".to_string(),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "ShoppingCart(*)".to_string(),
            },
            resource: ToolResourcePattern::Invocation(ToolInvocationPattern {
                command_path: Some(vec![ToolIdentifier("search".to_string())]),
                args: Vec::new(),
            }),
        })
    );

    assert_eq!(
        parsed_permission("card(acme) @ acme/shop/prod/cart-svc/ShoppingCart(*) : derive :"),
        PermissionPattern::Card(CardPermissionPattern::Verb {
            verb: CardVerb::Derive,
            owner: AccountOwnerPattern::Account {
                account: "acme".to_string(),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "ShoppingCart(*)".to_string(),
            },
            resource: CardResourcePattern::Empty,
        })
    );

    assert_eq!(
        parsed_permission(
            "card(acme) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : install : acme/shop/prod/cart-svc/ShoppingCart(*)"
        ),
        PermissionPattern::Card(CardPermissionPattern::Verb {
            verb: CardVerb::Install,
            owner: AccountOwnerPattern::Account {
                account: "acme".to_string(),
            },
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "CartAgent(\"42\")".to_string(),
            },
            resource: CardResourcePattern::InstallTarget(AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "ShoppingCart(*)".to_string(),
            }),
        })
    );
}

#[test]
fn parses_admin_class_examples() {
    assert_eq!(
        parsed_permission("system() @ acme : create-account :"),
        PermissionPattern::System(SystemPermissionPattern::Verb {
            verb: SystemVerb::CreateAccount,
            owner: EmptyOwnerPattern,
            recipient: AccountRecipientPattern::Account {
                account: "acme".to_string(),
            },
            resource: SystemResourcePattern,
        })
    );

    assert_eq!(
        parsed_permission("plan() @ acme : view : *"),
        PermissionPattern::Plan(PlanPermissionPattern::Verb {
            verb: PlanVerb::View,
            owner: EmptyOwnerPattern,
            recipient: AccountRecipientPattern::Account {
                account: "acme".to_string(),
            },
            resource: PlanResourcePattern::Any,
        })
    );

    assert_eq!(
        parsed_permission("account(acme) @ acme : set-plan :"),
        PermissionPattern::Account(AccountPermissionPattern::Verb {
            verb: AccountVerb::SetPlan,
            owner: AccountOwnerPattern::Account {
                account: "acme".to_string(),
            },
            recipient: AccountRecipientPattern::Account {
                account: "acme".to_string(),
            },
            resource: AccountResourcePattern,
        })
    );

    assert_eq!(
        parsed_permission("environment(acme/shop) @ acme/shop/prod : deploy : prod"),
        PermissionPattern::Environment(EnvironmentPermissionPattern::Verb {
            verb: EnvironmentVerb::Deploy,
            owner: ApplicationOwnerPattern::Application {
                account: "acme".to_string(),
                application: "shop".to_string(),
            },
            recipient: EnvironmentRecipientPattern::Environment {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
            },
            resource: EnvironmentResourcePattern::Environment(EnvironmentName("prod".to_string())),
        })
    );

    assert_eq!(
        parsed_permission(
            "environment.agent-secret(acme/shop/prod) @ acme/shop/prod : update : cart.*"
        ),
        PermissionPattern::EnvironmentAgentSecret(EnvironmentAgentSecretPermissionPattern::Verb {
            verb: EnvironmentAgentSecretVerb::Update,
            owner: EnvironmentOwnerPattern::Environment {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
            },
            recipient: EnvironmentRecipientPattern::Environment {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
            },
            resource: EnvironmentAgentSecretResourcePattern::Key(
                EnvironmentAgentSecretKeyPathPattern {
                    segments: vec![
                        EnvironmentAgentSecretKeySegmentPattern::Literal("cart".to_string()),
                        EnvironmentAgentSecretKeySegmentPattern::Star,
                    ],
                }
            ),
        })
    );
}

#[test]
fn parses_spec_specific_resource_shapes() {
    let credential_id = "550e8400-e29b-41d4-a716-446655440000";
    assert_eq!(
        parsed_permission("application(acme) @ acme : view : shop"),
        PermissionPattern::Application(ApplicationPermissionPattern::Verb {
            verb: ApplicationVerb::View,
            owner: AccountOwnerPattern::Account {
                account: "acme".to_string(),
            },
            recipient: AccountRecipientPattern::Account {
                account: "acme".to_string(),
            },
            resource: ApplicationResourcePattern::Application(ApplicationName("shop".to_string())),
        })
    );

    assert_eq!(
        parsed_permission("environment(acme/shop) @ acme/shop/prod : rollback : prod@rev=42"),
        PermissionPattern::Environment(EnvironmentPermissionPattern::Verb {
            verb: EnvironmentVerb::Rollback,
            owner: ApplicationOwnerPattern::Application {
                account: "acme".to_string(),
                application: "shop".to_string(),
            },
            recipient: EnvironmentRecipientPattern::Environment {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
            },
            resource: EnvironmentResourcePattern::Revision {
                environment: EnvironmentName("prod".to_string()),
                revision: 42,
            },
        })
    );

    assert_eq!(
        parsed_permission("component(acme/shop/prod) @ acme/shop/prod : view : cart-svc"),
        PermissionPattern::Component(ComponentPermissionPattern::Verb {
            verb: ComponentVerb::View,
            owner: EnvironmentOwnerPattern::Environment {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
            },
            recipient: EnvironmentRecipientPattern::Environment {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
            },
            resource: ComponentResourcePattern::Component(ComponentName("cart-svc".to_string())),
        })
    );

    assert_eq!(
        parsed_permission(&format!(
            "account.token(acme) @ acme : delete : {credential_id}"
        )),
        PermissionPattern::AccountToken(AccountTokenPermissionPattern::Verb {
            verb: AccountTokenVerb::Delete,
            owner: AccountOwnerPattern::Account {
                account: "acme".to_string(),
            },
            recipient: AccountRecipientPattern::Account {
                account: "acme".to_string(),
            },
            resource: AccountTokenResourcePattern::Token(uuid::Uuid::from_u128(
                0x550e8400e29b41d4a716446655440000,
            )),
        })
    );
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
fn polymorphic_pattern_grants_keep_resources_monomorphic() {
    let grant =
        parse_polymorphic_pattern_grant("secret(?env) @ ?self : reveal : billing.account").unwrap();

    assert_eq!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
            verb: SecretVerb::Reveal,
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: PolymorphicAgentRecipientPattern::Self_,
            resource: SecretResourcePattern::Key(SecretKeyPathPattern {
                segments: vec![
                    SecretKeySegmentPattern::Literal("billing".to_string()),
                    SecretKeySegmentPattern::Literal("account".to_string()),
                ],
            }),
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
fn parses_polymorphic_recipient_templates_and_concrete_paths() {
    let grant = parse_polymorphic_pattern_grant(
        "secret(?env) @ ?env/cart-svc/ShoppingCart(*) : hold : cart.api-key",
    )
    .unwrap();

    assert_eq!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
            verb: SecretVerb::Hold,
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: PolymorphicAgentRecipientPattern::EnvironmentAgent {
                component: "cart-svc".to_string(),
                agent: "ShoppingCart(*)".to_string(),
            },
            resource: SecretResourcePattern::Key(SecretKeyPathPattern {
                segments: vec![
                    SecretKeySegmentPattern::Literal("cart".to_string()),
                    SecretKeySegmentPattern::Literal("api-key".to_string()),
                ],
            }),
        })
    );

    let grant = parse_polymorphic_pattern_grant(
        "secret(?env) @ acme/shop/prod/cart-svc/ShoppingCart(*) : hold : cart.api-key",
    )
    .unwrap();

    assert_eq!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
            verb: SecretVerb::Hold,
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: PolymorphicAgentRecipientPattern::Concrete(AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart-svc".to_string(),
                agent: "ShoppingCart(*)".to_string(),
            }),
            resource: SecretResourcePattern::Key(SecretKeyPathPattern {
                segments: vec![
                    SecretKeySegmentPattern::Literal("cart".to_string()),
                    SecretKeySegmentPattern::Literal("api-key".to_string()),
                ],
            }),
        })
    );
}

#[test_gen]
fn generate_polymorphic_owner_slot_parser_tests(r: &mut DynamicTestRegistration) {
    let cases = [
        ("environment(?env) @ ?env : view : prod", "environment"),
        ("env(?self) @ ?self : read : HOME", "self"),
    ];

    for (input, slot) in cases {
        add_test!(
            r,
            format!("parses_polymorphic_owner_slot_{slot}"),
            TestProperties::unit_test(),
            || {
                parse_polymorphic_pattern_grant(input).unwrap();
            }
        );
    }
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
fn parses_polymorphic_owner_templates() {
    let grant = parse_polymorphic_pattern_grant(
        "agent(?env/payment-svc/PaymentAgent(*)) @ ?self : invoke : charge",
    )
    .unwrap();

    assert_eq!(
        grant.permission,
        PolymorphicPermissionPattern::Agent(PolymorphicAgentPermissionPattern::Verb {
            verb: AgentVerb::Invoke,
            owner: PolymorphicAgentOwnerPattern::EnvAgent {
                component: "payment-svc".to_string(),
                agent: AgentOwnerLeafPattern::AgentTypeWildcard("PaymentAgent".to_string()),
            },
            recipient: PolymorphicAgentRecipientPattern::Self_,
            resource: AgentResourcePattern::Method(AgentMethodName("charge".to_string())),
        })
    );
}

#[test]
fn parses_only_declared_polymorphic_recipient_slots() {
    let grant = parse_polymorphic_pattern_grant("environment(?env) @ ?env : view : prod").unwrap();

    assert_eq!(
        grant.permission,
        PolymorphicPermissionPattern::Environment(PolymorphicEnvironmentPermissionPattern::Verb {
            verb: EnvironmentVerb::View,
            owner: PolymorphicApplicationOwnerPattern::Env,
            recipient: PolymorphicEnvironmentRecipientPattern::Environment,
            resource: EnvironmentResourcePattern::Environment(EnvironmentName("prod".to_string())),
        })
    );

    let grant =
        parse_polymorphic_pattern_grant("secret(?env) @ ?self : reveal : billing.*").unwrap();
    assert_eq!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
            verb: SecretVerb::Reveal,
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: PolymorphicAgentRecipientPattern::Self_,
            resource: SecretResourcePattern::Key(SecretKeyPathPattern {
                segments: vec![
                    SecretKeySegmentPattern::Literal("billing".to_string()),
                    SecretKeySegmentPattern::Star,
                ],
            }),
        })
    );

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

fn test_name(value: &str) -> String {
    value
        .trim_start_matches('?')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

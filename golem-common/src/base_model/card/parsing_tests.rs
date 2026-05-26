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
use pretty_assertions::{assert_eq, assert_matches};
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
            owner: AgentOwnerPattern::new("acme/shop/prod/cart/agent"),
            recipient: AgentRecipientPattern::Agent {
                account: "acme".to_string(),
                application: "shop".to_string(),
                environment: "prod".to_string(),
                component: "cart".to_string(),
                agent: "agent".to_string(),
            },
            resource: FilesystemResourcePattern::Path(
                FilesystemPathPattern::parse("/data/**").unwrap()
            ),
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

    assert_matches!(
        grant.permission,
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Verb { verb: FilesystemVerb::Read,
            recipient: AgentRecipientPattern::Agent { account, application, environment, component, agent },
            resource: FilesystemResourcePattern::Path(path),
            ..
        }) if account == "alice@example.com" && application == "shop" && environment == "prod" && component == "cart-svc" && agent == "CartAgent(\"42\")" && path == FilesystemPathPattern::parse("/data/**").unwrap()
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
            owner: AgentOwnerPattern::new("acme/shop/prod/cart/agent"),
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

    assert_matches!(
        result,
        Err(CardParseError::UnknownVerb { class, verb }) if class == "filesystem" && verb == "query"
    );
}

#[test]
fn rejects_malformed_grants() {
    assert_matches!(
        parse_pattern_grant("filesystem(acme) : read : /data/**"),
        Err(CardParseError::MissingAtSeparator)
    );
    assert_matches!(
        parse_pattern_grant("filesystem(acme) @ acme : query : /data/**"),
        Err(CardParseError::InvalidOwnerPath { class, owner })
            if class == "filesystem" && owner == "acme"
    );
    assert_matches!(
        parse_pattern_grant("system(acme) @ acme : create-account :"),
        Err(CardParseError::InvalidOwnerPath { class, owner })
            if class == "system" && owner == "acme"
    );
    assert_matches!(
        parse_pattern_grant("system() @ acme : create-account : not-empty"),
        Err(CardParseError::InvalidResource { class, resource })
            if class == "system" && resource == "not-empty"
    );
    assert_matches!(
        parse_pattern_grant("card(acme) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : install : acme/shop"),
        Err(CardParseError::InvalidRecipientPath(path)) if path == "acme/shop"
    );
    assert_matches!(
        parse_pattern_grant("unknown(acme) @ acme : view :"),
        Err(CardParseError::UnknownClass(class)) if class == "unknown"
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
        ("application(acme/shop) @ acme : view :", "application"),
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
    assert_matches!(
        parsed_permission(
            "filesystem(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : /data/**"
        ),
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Verb { verb: FilesystemVerb::Read,
            owner,
            resource: FilesystemResourcePattern::Path(path),
            ..
        }) if owner == AgentOwnerPattern::new("acme/shop/prod/cart-svc/CartAgent(\"42\")") && path == FilesystemPathPattern::parse("/data/**").unwrap()
    );

    assert_matches!(
        parsed_permission("network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal:8080-9000"),
        PermissionPattern::Network(NetworkPermissionPattern::Verb { verb: NetworkVerb::Connect,
            owner: EmptyOwnerPattern,
            resource: NetworkResourcePattern::HostPort { host, ports },
            ..
        }) if host == "api.internal" && matches!(ports, PortPattern::Range { start: 8080, end: 9000 })
    );

    assert_matches!(
        parsed_permission("env(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : HOME"),
        PermissionPattern::Env(EnvPermissionPattern::Verb { verb: EnvVerb::Read,
            resource: EnvResourcePattern::VarName(EnvVarName(name)),
            ..
        }) if name == "HOME"
    );

    assert_matches!(
        parsed_permission("secret(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : reveal : cart.api-key"),
        PermissionPattern::Secret(SecretPermissionPattern::Verb { verb: SecretVerb::Reveal,
            owner,
            resource: SecretResourcePattern::Key(key),
            ..
        }) if owner == EnvironmentOwnerPattern::new("acme/shop/prod") && key == SecretKeyPathPattern::parse("cart.api-key").unwrap()
    );

    assert_matches!(
        parsed_permission("kv(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : read : my-store.user-*"),
        PermissionPattern::Kv(KvPermissionPattern::Verb { verb: KvVerb::Read,
            resource: KvResourcePattern::StoreKey { store, key_pattern },
            ..
        }) if store == "my-store" && key_pattern == "user-*"
    );
}

#[test]
fn parses_agent_tool_and_card_examples_from_spec() {
    assert_matches!(
        parsed_permission("agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : add-item"),
        PermissionPattern::Agent(AgentPermissionPattern::Verb { verb: AgentVerb::Invoke,
            owner,
            resource: AgentResourcePattern::Method(AgentMethodName(method)),
            ..
        }) if owner == AgentOwnerPattern::new("acme/shop/prod/cart-svc/ShoppingCart(*)") && method == "add-item"
    );

    assert_matches!(
        parsed_permission(
            "agent(acme/shop/prod/cart-svc/*) @ acme/shop/prod/cart-svc/* : delete :"
        ),
        PermissionPattern::Agent(AgentPermissionPattern::Verb {
            verb: AgentVerb::Delete,
            resource: AgentResourcePattern::Empty,
            ..
        })
    );

    assert_matches!(
        parsed_permission("tool(acme/shop/prod/cli-tools/grep) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : search"),
        PermissionPattern::Tool(ToolPermissionPattern::Verb { verb: ToolVerb::Invoke,
            owner,
            resource: ToolResourcePattern::Invocation(command),
            ..
        }) if owner == ToolOwnerPattern::new("acme/shop/prod/cli-tools/grep") && command.command_path == Some(vec![ToolIdentifier("search".to_string())])
    );

    assert_matches!(
        parsed_permission("card(acme) @ acme/shop/prod/cart-svc/ShoppingCart(*) : derive :"),
        PermissionPattern::Card(CardPermissionPattern::Verb { verb: CardVerb::Derive,
            owner,
            resource: CardResourcePattern::Empty,
            ..
        }) if owner == AccountOwnerPattern::new("acme")
    );

    assert_matches!(
        parsed_permission(
            "card(acme) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : install : acme/shop/prod/cart-svc/ShoppingCart(*)"
        ),
        PermissionPattern::Card(CardPermissionPattern::Verb {
            verb: CardVerb::Install,
            resource: CardResourcePattern::InstallTarget(AgentRecipientPattern::Agent { .. }),
            ..
        })
    );
}

#[test]
fn parses_admin_class_examples() {
    assert_matches!(
        parsed_permission("system() @ acme : create-account :"),
        PermissionPattern::System(SystemPermissionPattern::Verb {
            verb: SystemVerb::CreateAccount,
            owner: EmptyOwnerPattern,
            resource: SystemResourcePattern,
            ..
        })
    );

    assert_matches!(
        parsed_permission("plan() @ acme : view : *"),
        PermissionPattern::Plan(PlanPermissionPattern::Verb {
            verb: PlanVerb::View,
            owner: EmptyOwnerPattern,
            resource: PlanResourcePattern::Any,
            ..
        })
    );

    assert_matches!(
        parsed_permission("account(acme) @ acme : set-plan :"),
        PermissionPattern::Account(AccountPermissionPattern::Verb { verb: AccountVerb::SetPlan,
            owner,
            resource: AccountResourcePattern,
            ..
        }) if owner == AccountOwnerPattern::new("acme")
    );

    assert_matches!(
        parsed_permission("environment(acme/shop) @ acme/shop/prod : deploy : prod"),
        PermissionPattern::Environment(EnvironmentPermissionPattern::Verb { verb: EnvironmentVerb::Deploy,
            owner,
            resource: EnvironmentResourcePattern::Environment(environment),
            ..
        }) if owner == ApplicationOwnerPattern::new("acme/shop") && environment == EnvironmentName("prod".to_string())
    );

    assert_matches!(
        parsed_permission("environment.agent-secret(acme/shop/prod) @ acme/shop/prod : update : cart.*"),
        PermissionPattern::EnvironmentAgentSecret(EnvironmentAgentSecretPermissionPattern::Verb { verb: EnvironmentAgentSecretVerb::Update,
            resource: EnvironmentAgentSecretResourcePattern::Key(path),
            ..
        }) if path == EnvironmentAgentSecretKeyPathPattern::parse("cart.*").unwrap()
    );
}

#[test]
fn parses_spec_specific_resource_shapes() {
    let credential_id = "550e8400-e29b-41d4-a716-446655440000";
    assert_matches!(
        parsed_permission(&format!(
            "application(acme/shop) @ acme : view-credentials : cred={credential_id}"
        )),
        PermissionPattern::Application(ApplicationPermissionPattern::Verb {
            resource: ApplicationResourcePattern::Credential(id),
            ..
        }) if id.to_string() == credential_id
    );

    assert_matches!(
        parsed_permission("environment(acme/shop) @ acme/shop/prod : rollback : prod@rev=42"),
        PermissionPattern::Environment(EnvironmentPermissionPattern::Verb {
            resource: EnvironmentResourcePattern::Revision { environment, revision: 42 },
            ..
        }) if environment == EnvironmentName("prod".to_string())
    );

    assert_matches!(
        parsed_permission("component(acme/shop/prod) @ acme/shop/prod : view : cart-svc"),
        PermissionPattern::Component(ComponentPermissionPattern::Verb {
            resource: ComponentResourcePattern::Component(component),
            ..
        }) if component == ComponentName("cart-svc".to_string())
    );

    assert_matches!(
        parsed_permission(&format!("account.token(acme) @ acme : delete : {credential_id}")),
        PermissionPattern::AccountToken(AccountTokenPermissionPattern::Verb {
            resource: AccountTokenResourcePattern::Token(id),
            ..
        }) if id.to_string() == credential_id
    );
}

#[test]
fn empty_resource_classes_reject_polymorphic_resource_slots() {
    assert_matches!(
        parse_polymorphic_pattern_grant("account(acme) @ ?account : view : ?resource"),
        Err(CardParseError::InvalidResource { class, resource })
            if class == AccountClass::NAME && resource == "?resource"
    );

    assert_matches!(
        parse_polymorphic_pattern_grant("system() @ ?account : create-account : ?resource"),
        Err(CardParseError::InvalidResource { class, resource })
            if class == SystemClass::NAME && resource == "?resource"
    );
}

#[test]
fn polymorphic_pattern_grants_keep_resources_monomorphic() {
    let grant =
        parse_polymorphic_pattern_grant("secret(?env) @ ?self : reveal : billing.account").unwrap();

    assert_matches!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb { verb: SecretVerb::Reveal,
            owner: PolymorphicEnvironmentOwnerPattern::Env,
            recipient: PolymorphicAgentRecipientPattern::Self_,
            resource: SecretResourcePattern::Key(resource),
            ..
        }) if resource == SecretKeyPathPattern::parse("billing.account").unwrap()
    );
}

#[test]
fn rejects_polymorphic_resource_slots_and_templates() {
    assert_matches!(
        parse_polymorphic_pattern_grant("env(?self) @ ?self : read : ?env_var"),
        Err(CardParseError::InvalidResource { class, resource })
            if class == EnvClass::NAME && resource == "?env_var"
    );

    assert_matches!(
        parse_polymorphic_pattern_grant("card(acme) @ ?self : install : ?self"),
        Err(CardParseError::InvalidResource { class, resource })
            if class == CardClass::NAME && resource == "?self"
    );

    assert_matches!(
        parse_polymorphic_pattern_grant("secret(?env) @ ?self : reveal : secret.?self"),
        Err(CardParseError::InvalidResource { class, resource })
            if class == SecretClass::NAME && resource == "secret.?self"
    );
}

#[test]
fn parses_polymorphic_recipient_templates_and_concrete_paths() {
    let grant = parse_polymorphic_pattern_grant(
        "secret(?env) @ ?env/cart-svc/ShoppingCart(*) : hold : cart.api-key",
    )
    .unwrap();

    assert_matches!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb { verb: SecretVerb::Hold,
            recipient: PolymorphicAgentRecipientPattern::EnvironmentAgent { component, agent },
            ..
        }) if component == "cart-svc" && agent == "ShoppingCart(*)"
    );

    let grant = parse_polymorphic_pattern_grant(
        "secret(?env) @ acme/shop/prod/cart-svc/ShoppingCart(*) : hold : cart.api-key",
    )
    .unwrap();

    assert_matches!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
            verb: SecretVerb::Hold,
            recipient: PolymorphicAgentRecipientPattern::Concrete(
                AgentRecipientPattern::Agent { .. }
            ),
            ..
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
    assert_matches!(
        parse_polymorphic_pattern_grant("account(?account) @ ?account : view :"),
        Err(CardParseError::InvalidOwnerPath { class, owner })
            if class == AccountClass::NAME && owner == "?account"
    );

    assert_matches!(
        parse_polymorphic_pattern_grant("application(?app) @ ?account : view :"),
        Err(CardParseError::InvalidOwnerPath { class, owner })
            if class == ApplicationClass::NAME && owner == "?app"
    );

    assert_matches!(
        parse_polymorphic_pattern_grant("component(?component) @ ?env : view : cart-svc"),
        Err(CardParseError::InvalidOwnerPath { class, owner })
            if class == ComponentClass::NAME && owner == "?component"
    );
}

#[test]
fn rejects_polymorphic_owner_slots_with_wrong_scope() {
    assert_matches!(
        parse_polymorphic_pattern_grant("filesystem(?env) @ ?self : read : /data/**"),
        Err(CardParseError::InvalidOwnerPath { class, owner })
            if class == FilesystemClass::NAME && owner == "?env"
    );
}

#[test]
fn parses_polymorphic_owner_templates() {
    let grant = parse_polymorphic_pattern_grant(
        "agent(?env/payment-svc/PaymentAgent(*)) @ ?self : invoke : charge",
    )
    .unwrap();

    assert_matches!(
        grant.permission,
        PolymorphicPermissionPattern::Agent(PolymorphicAgentPermissionPattern::Verb { verb: AgentVerb::Invoke,
            owner: PolymorphicAgentOwnerPattern::EnvAgent { component, agent },
            ..
        }) if component == "payment-svc" && agent == AgentOwnerLeafPattern::AgentTypeWildcard("PaymentAgent".to_string())
    );
}

#[test]
fn parses_only_declared_polymorphic_recipient_slots() {
    let grant = parse_polymorphic_pattern_grant("environment(?env) @ ?env : view : prod").unwrap();

    assert_matches!(
        grant.permission,
        PolymorphicPermissionPattern::Environment(PolymorphicEnvironmentPermissionPattern::Verb {
            verb: EnvironmentVerb::View,
            recipient: PolymorphicEnvironmentRecipientPattern::Environment,
            ..
        })
    );

    let grant =
        parse_polymorphic_pattern_grant("secret(?env) @ ?self : reveal : billing.*").unwrap();
    assert_matches!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Verb {
            recipient: PolymorphicAgentRecipientPattern::Self_,
            ..
        })
    );

    assert_matches!(
        parse_polymorphic_pattern_grant("secret(?env) @ ?account : reveal : billing.*"),
        Err(CardParseError::InvalidRecipientPath(path)) if path == "?account"
    );
    assert_matches!(
        parse_polymorphic_pattern_grant("secret(?env) @ ?env : reveal : billing.*"),
        Err(CardParseError::InvalidRecipientPath(path)) if path == "?env"
    );
}

#[test]
fn concrete_parser_rejects_slot_variables() {
    let result = parse_pattern_grant("secret(?env) @ ?self : reveal : billing.*");

    assert_matches!(
        result,
        Err(CardParseError::SlotVariableInConcreteGrant(value)) if value == "?env"
    );
}

fn test_name(value: &str) -> String {
    value
        .trim_start_matches('?')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

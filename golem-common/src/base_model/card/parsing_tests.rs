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
        grant.owner,
        OwnerPathPattern("acme/shop/prod/cart/agent".to_string())
    );
    assert_eq!(
        grant.recipient,
        RecipientPathPattern::parse("acme/shop/prod/cart/agent").unwrap()
    );
    assert_eq!(
        grant.permission,
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Read(
            GlobResourcePattern::Glob("/data/**".to_string())
        ))
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
        PermissionPattern::Oplog(OplogPermissionPattern::Read(OplogResourcePattern::Range {
            start: Some(1000),
            end: Some(2000)
        }))
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
        Err(CardParseError::UnknownVerb { class, verb }) if class == "filesystem" && verb == "query"
    );
    assert_matches!(
        parse_pattern_grant("system() @ acme : create-account : not-empty"),
        Err(CardParseError::InvalidResource { class, resource })
            if class == "system" && resource == "not-empty"
    );
    assert_matches!(
        parse_pattern_grant("card(acme) @ acme : install : agent(*)"),
        Err(CardParseError::InvalidRecipientPath(path)) if path == "agent(*)"
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
            "filesystem(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme : read : /data/**",
            PermissionPattern::Filesystem(FilesystemPermissionPattern::Read(
                GlobResourcePattern::Glob("/data/**".to_string()),
            )),
        ),
        (
            "network() @ acme : connect : api.internal:8080",
            PermissionPattern::Network(NetworkPermissionPattern::Connect(
                NetworkResourcePattern::HostPort {
                    host: "api.internal".to_string(),
                    ports: PortPattern::Single(8080),
                },
            )),
        ),
        (
            "env(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme : read : HOME",
            PermissionPattern::Env(EnvPermissionPattern::Read(
                IdentifierResourcePattern::Exact("HOME".to_string()),
            )),
        ),
        (
            "oplog(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme : read : *",
            PermissionPattern::Oplog(OplogPermissionPattern::Read(OplogResourcePattern::Any)),
        ),
        (
            "config(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme : read : model.retry-count",
            PermissionPattern::Config(ConfigPermissionPattern::Read(GlobResourcePattern::Exact(
                "model.retry-count".to_string(),
            ))),
        ),
        (
            "secret(acme/shop/prod) @ acme : hold : cart.api-key",
            PermissionPattern::Secret(SecretPermissionPattern::Hold(GlobResourcePattern::Exact(
                "cart.api-key".to_string(),
            ))),
        ),
        (
            "agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme : invoke : add-item",
            PermissionPattern::Agent(AgentPermissionPattern::Invoke(
                AgentResourcePattern::Method("add-item".to_string()),
            )),
        ),
        (
            "tool(acme/shop/prod/cli-tools/grep) @ acme : invoke : search",
            PermissionPattern::Tool(ToolPermissionPattern::Invoke(ToolResourcePattern::Command(
                "search".to_string(),
            ))),
        ),
        (
            "kv(acme/shop/prod) @ acme : read : my-store.user-*",
            PermissionPattern::Kv(KvPermissionPattern::Read(GlobResourcePattern::Glob(
                "my-store.user-*".to_string(),
            ))),
        ),
        (
            "blob(acme/shop/prod) @ acme : read : my-bucket.models/*.bin",
            PermissionPattern::Blob(BlobPermissionPattern::Read(GlobResourcePattern::Glob(
                "my-bucket.models/*.bin".to_string(),
            ))),
        ),
        (
            "rdbms(acme/shop/prod) @ acme : query : orders.public.orders",
            PermissionPattern::Rdbms(RdbmsPermissionPattern::Query(GlobResourcePattern::Exact(
                "orders.public.orders".to_string(),
            ))),
        ),
        (
            "card(acme) @ acme : derive :",
            PermissionPattern::Card(CardPermissionPattern::Derive(CardResourcePattern::Empty)),
        ),
        (
            "system() @ acme : create-account :",
            PermissionPattern::System(SystemPermissionPattern::CreateAccount(EmptyResourcePattern)),
        ),
        (
            "plan() @ acme : view : plan-a",
            PermissionPattern::Plan(PlanPermissionPattern::View(
                IdentifierResourcePattern::Exact("plan-a".to_string()),
            )),
        ),
        (
            "account(acme) @ acme : view :",
            PermissionPattern::Account(AccountPermissionPattern::View(EmptyResourcePattern)),
        ),
        (
            "account.usage(acme) @ acme : view :",
            PermissionPattern::AccountUsage(AccountUsagePermissionPattern::View(
                EmptyResourcePattern,
            )),
        ),
        (
            "account.token(acme) @ acme : create : token-a",
            PermissionPattern::AccountToken(AccountTokenPermissionPattern::Create(
                IdentifierResourcePattern::Exact("token-a".to_string()),
            )),
        ),
        (
            "account.plugin(acme) @ acme : view : plugin-a",
            PermissionPattern::AccountPlugin(AccountPluginPermissionPattern::View(
                IdentifierResourcePattern::Exact("plugin-a".to_string()),
            )),
        ),
        (
            "application(acme/shop) @ acme : view :",
            PermissionPattern::Application(ApplicationPermissionPattern::View(
                EmptyResourcePattern,
            )),
        ),
        (
            "environment(acme/shop/prod) @ acme : view :",
            PermissionPattern::Environment(EnvironmentPermissionPattern::View(
                EmptyResourcePattern,
            )),
        ),
        (
            "environment.share(acme/shop/prod) @ acme : view : share-a",
            PermissionPattern::EnvironmentShare(EnvironmentSharePermissionPattern::View(
                IdentifierResourcePattern::Exact("share-a".to_string()),
            )),
        ),
        (
            "environment.plugin-grant(acme/shop/prod) @ acme : view : plugin-a",
            PermissionPattern::EnvironmentPluginGrant(
                EnvironmentPluginGrantPermissionPattern::View(IdentifierResourcePattern::Exact(
                    "plugin-a".to_string(),
                )),
            ),
        ),
        (
            "environment.domain-registration(acme/shop/prod) @ acme : view : domain-a",
            PermissionPattern::EnvironmentDomainRegistration(
                EnvironmentDomainRegistrationPermissionPattern::View(
                    IdentifierResourcePattern::Exact("domain-a".to_string()),
                ),
            ),
        ),
        (
            "environment.security-scheme(acme/shop/prod) @ acme : view : scheme-a",
            PermissionPattern::EnvironmentSecurityScheme(
                EnvironmentSecuritySchemePermissionPattern::View(IdentifierResourcePattern::Exact(
                    "scheme-a".to_string(),
                )),
            ),
        ),
        (
            "environment.http-api-deployment(acme/shop/prod) @ acme : view : api-a",
            PermissionPattern::EnvironmentHttpApiDeployment(
                EnvironmentHttpApiDeploymentPermissionPattern::View(
                    IdentifierResourcePattern::Exact("api-a".to_string()),
                ),
            ),
        ),
        (
            "environment.mcp-deployment(acme/shop/prod) @ acme : view : mcp-a",
            PermissionPattern::EnvironmentMcpDeployment(
                EnvironmentMcpDeploymentPermissionPattern::View(IdentifierResourcePattern::Exact(
                    "mcp-a".to_string(),
                )),
            ),
        ),
        (
            "environment.agent-secret(acme/shop/prod) @ acme : update : cart.*",
            PermissionPattern::EnvironmentAgentSecret(
                EnvironmentAgentSecretPermissionPattern::Update(GlobResourcePattern::Glob(
                    "cart.*".to_string(),
                )),
            ),
        ),
        (
            "environment.resource-definition(acme/shop/prod) @ acme : view : resource-a",
            PermissionPattern::EnvironmentResourceDefinition(
                EnvironmentResourceDefinitionPermissionPattern::View(
                    IdentifierResourcePattern::Exact("resource-a".to_string()),
                ),
            ),
        ),
        (
            "environment.retry-policy(acme/shop/prod) @ acme : view : retry-a",
            PermissionPattern::EnvironmentRetryPolicy(
                EnvironmentRetryPolicyPermissionPattern::View(IdentifierResourcePattern::Exact(
                    "retry-a".to_string(),
                )),
            ),
        ),
        (
            "component(acme/shop/prod/cart-svc) @ acme : view :",
            PermissionPattern::Component(ComponentPermissionPattern::View(EmptyResourcePattern)),
        ),
        (
            "account.oauth2-identity(acme) @ acme : view : identity-a",
            PermissionPattern::AccountOauth2Identity(AccountOauth2IdentityPermissionPattern::View(
                IdentifierResourcePattern::Exact("identity-a".to_string()),
            )),
        ),
        (
            "environment.initial-files(acme/shop/prod/cart-svc) @ acme : view : /etc/*",
            PermissionPattern::EnvironmentInitialFiles(
                EnvironmentInitialFilesPermissionPattern::View(GlobResourcePattern::Glob(
                    "/etc/*".to_string(),
                )),
            ),
        ),
        (
            "environment.kv-bucket(acme/shop/prod) @ acme : view : bucket-a",
            PermissionPattern::EnvironmentKvBucket(EnvironmentKvBucketPermissionPattern::View(
                IdentifierResourcePattern::Exact("bucket-a".to_string()),
            )),
        ),
        (
            "environment.blob-bucket(acme/shop/prod) @ acme : view : bucket-a",
            PermissionPattern::EnvironmentBlobBucket(EnvironmentBlobBucketPermissionPattern::View(
                IdentifierResourcePattern::Exact("bucket-a".to_string()),
            )),
        ),
    ];

    for (input, expected) in cases {
        let class_name = expected.class_name();
        let expected = std::sync::Arc::new(expected);
        add_test!(
            r,
            format!("parses_declared_permission_class_{}", test_name(class_name)),
            TestProperties::unit_test(),
            || {
                let grant = parse_pattern_grant(input).expect(input);
                assert_eq!(grant.permission, (*expected).clone());
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
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Read(GlobResourcePattern::Glob(path)))
            if path == "/data/**"
    );

    assert_matches!(
        parsed_permission(
            "filesystem(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : * : /data/**"
        ),
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Any(GlobResourcePattern::Glob(path)))
            if path == "/data/**"
    );

    assert_matches!(
        parsed_permission("network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : *.openai.com"),
        PermissionPattern::Network(NetworkPermissionPattern::Connect(NetworkResourcePattern::HostPort { host, ports }))
            if host == "*.openai.com" && matches!(ports, PortPattern::Any)
    );

    assert_matches!(
        parsed_permission("network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal:8080-9000"),
        PermissionPattern::Network(NetworkPermissionPattern::Connect(NetworkResourcePattern::HostPort { host, ports }))
            if host == "api.internal" && matches!(ports, PortPattern::Range { start: 8080, end: 9000 })
    );

    assert_matches!(
        parsed_permission("env(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : HOME"),
        PermissionPattern::Env(EnvPermissionPattern::Read(IdentifierResourcePattern::Exact(name)))
            if name == "HOME"
    );

    assert_matches!(
        parsed_permission(
            "oplog(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : *"
        ),
        PermissionPattern::Oplog(OplogPermissionPattern::Read(OplogResourcePattern::Any))
    );

    assert_matches!(
        parsed_permission("secret(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : reveal : cart.api-key"),
        PermissionPattern::Secret(SecretPermissionPattern::Reveal(GlobResourcePattern::Exact(key)))
            if key == "cart.api-key"
    );

    assert_matches!(
        parsed_permission("kv(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : read : my-store.user-*"),
        PermissionPattern::Kv(KvPermissionPattern::Read(GlobResourcePattern::Glob(key)))
            if key == "my-store.user-*"
    );

    assert_matches!(
        parsed_permission("blob(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : read : my-bucket.models/*.bin"),
        PermissionPattern::Blob(BlobPermissionPattern::Read(GlobResourcePattern::Glob(key)))
            if key == "my-bucket.models/*.bin"
    );
}

#[test]
fn parses_agent_tool_and_card_examples_from_spec() {
    assert_matches!(
        parsed_permission("agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : add-item"),
        PermissionPattern::Agent(AgentPermissionPattern::Invoke(AgentResourcePattern::Method(method)))
            if method == "add-item"
    );

    assert_matches!(
        parsed_permission(
            "agent(acme/shop/prod/cart-svc/*) @ acme/shop/prod/cart-svc/* : delete :"
        ),
        PermissionPattern::Agent(AgentPermissionPattern::Delete(AgentResourcePattern::Empty))
    );

    assert_matches!(
        parsed_permission("tool(acme/shop/prod/cli-tools/grep) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : search"),
        PermissionPattern::Tool(ToolPermissionPattern::Invoke(ToolResourcePattern::Command(command)))
            if command == "search"
    );

    assert_matches!(
        parsed_permission("card(acme) @ acme/shop/prod/cart-svc/ShoppingCart(*) : derive :"),
        PermissionPattern::Card(CardPermissionPattern::Derive(CardResourcePattern::Empty))
    );

    assert_matches!(
        parsed_permission("card(acme) @ acme : install : acme/shop/prod/cart-svc/ShoppingCart(*)"),
        PermissionPattern::Card(CardPermissionPattern::Install(
            CardResourcePattern::InstallTarget(RecipientPathPattern::Agent { .. })
        ))
    );
}

#[test]
fn parses_admin_class_examples() {
    assert_matches!(
        parsed_permission("system() @ acme : create-account :"),
        PermissionPattern::System(SystemPermissionPattern::CreateAccount(EmptyResourcePattern))
    );

    assert_matches!(
        parsed_permission("plan() @ acme : view : *"),
        PermissionPattern::Plan(PlanPermissionPattern::View(IdentifierResourcePattern::Any))
    );

    assert_matches!(
        parsed_permission("account(acme) @ acme : set-plan :"),
        PermissionPattern::Account(AccountPermissionPattern::SetPlan(EmptyResourcePattern))
    );

    assert_matches!(
        parsed_permission("environment(acme/shop/prod) @ acme/shop/prod : deploy :"),
        PermissionPattern::Environment(EnvironmentPermissionPattern::Deploy(EmptyResourcePattern))
    );

    assert_matches!(
        parsed_permission("environment.agent-secret(acme/shop/prod) @ acme/shop/prod : update : cart.*"),
        PermissionPattern::EnvironmentAgentSecret(EnvironmentAgentSecretPermissionPattern::Update(GlobResourcePattern::Glob(path)))
            if path == "cart.*"
    );
}

#[test]
fn parses_polymorphic_pattern_grant_with_slot_paths() {
    let grant =
        parse_polymorphic_pattern_grant("secret(?env) @ ?self : reveal : billing.?account.*")
            .unwrap();

    assert_eq!(
        grant.owner,
        PolymorphicOwnerPathPattern::Slot(SlotVariable("env".to_string()))
    );
    assert_eq!(
        grant.recipient,
        PolymorphicRecipientPathPattern::Slot(SlotVariable("self".to_string()))
    );
    assert_matches!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Reveal(
            PolymorphicGlobResourcePattern::Template(resource)
        )) if resource == "billing.?account.*"
    );
}

#[test]
fn parses_polymorphic_resource_slots_and_templates() {
    let env = parse_polymorphic_pattern_grant("env(?self) @ ?self : read : ?env_var").unwrap();
    assert_matches!(
        env.permission,
        PolymorphicPermissionPattern::Env(PolymorphicEnvPermissionPattern::Read(
            PolymorphicIdentifierResourcePattern::Slot(SlotVariable(name))
        )) if name == "env_var"
    );

    let card =
        parse_polymorphic_pattern_grant("card(?account) @ ?account : install : ?self").unwrap();
    assert_matches!(
        card.permission,
        PolymorphicPermissionPattern::Card(PolymorphicCardPermissionPattern::Install(
            PolymorphicCardResourcePattern::Slot(SlotVariable(name))
        )) if name == "self"
    );

    let card =
        parse_polymorphic_pattern_grant("card(?account) @ ?account : install : acme/shop/?env/*/*")
            .unwrap();
    assert_matches!(
        card.permission,
        PolymorphicPermissionPattern::Card(PolymorphicCardPermissionPattern::Install(
            PolymorphicCardResourcePattern::Template(target)
        )) if target == "acme/shop/?env/*/*"
    );
}

#[test]
fn parses_polymorphic_recipient_templates_and_concrete_paths() {
    let grant = parse_polymorphic_pattern_grant(
        "secret(?env) @ ?env/*/ShoppingCart(*) : hold : cart.api-key",
    )
    .unwrap();

    assert_eq!(
        grant.recipient,
        PolymorphicRecipientPathPattern::Template("?env/*/ShoppingCart(*)".to_string())
    );

    let grant = parse_polymorphic_pattern_grant(
        "secret(?env) @ acme/shop/prod/cart-svc/ShoppingCart(*) : hold : cart.api-key",
    )
    .unwrap();

    assert_eq!(
        grant.recipient,
        PolymorphicRecipientPathPattern::Concrete(
            RecipientPathPattern::parse("acme/shop/prod/cart-svc/ShoppingCart(*)").unwrap()
        )
    );
}

#[test_gen]
fn generate_hierarchy_slot_parser_tests(r: &mut DynamicTestRegistration) {
    for slot in ["?account", "?app", "?env", "?component", "?self"] {
        add_test!(
            r,
            format!(
                "parses_hierarchy_slot_{}_in_all_polymorphic_positions",
                test_name(slot)
            ),
            TestProperties::unit_test(),
            || {
                let grant = parse_polymorphic_pattern_grant(&format!(
                    "secret({slot}) @ {slot} : reveal : secret.{slot}"
                ))
                .unwrap();
                let name = slot.trim_start_matches('?').to_string();

                assert_eq!(
                    grant.owner,
                    PolymorphicOwnerPathPattern::Slot(SlotVariable(name.clone()))
                );
                assert_eq!(
                    grant.recipient,
                    PolymorphicRecipientPathPattern::Slot(SlotVariable(name.clone()))
                );
                assert_matches!(
                    grant.permission,
                    PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Reveal(
                        PolymorphicGlobResourcePattern::Template(resource)
                    )) if resource == format!("secret.{slot}")
                );
            }
        );
    }
}

#[test]
fn parses_polymorphic_owner_templates() {
    let grant =
        parse_polymorphic_pattern_grant("agent(?env/*/PaymentAgent(*)) @ ?self : invoke : charge")
            .unwrap();

    assert_eq!(
        grant.owner,
        PolymorphicOwnerPathPattern::Template("?env/*/PaymentAgent(*)".to_string())
    );
}

fn test_name(value: &str) -> String {
    value
        .trim_start_matches('?')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[test]
fn concrete_parser_rejects_slot_variables() {
    let result = parse_pattern_grant("secret(?env) @ ?self : reveal : billing.*");

    assert_matches!(
        result,
        Err(CardParseError::SlotVariableInConcreteGrant(value)) if value == "?env"
    );
}

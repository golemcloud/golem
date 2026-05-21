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
use test_r::test;

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

#[test]
fn concrete_parser_rejects_slot_variables() {
    let result = parse_pattern_grant("secret(?env) @ ?self : reveal : billing.*");

    assert_matches!(
        result,
        Err(CardParseError::SlotVariableInConcreteGrant(value)) if value == "?env"
    );
}

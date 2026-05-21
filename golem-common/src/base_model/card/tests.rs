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
use chrono::Utc;
use test_r::test;
use uuid::Uuid;

fn fs(owner: &str, recipient: &str, resource: GlobResourcePattern) -> PatternGrant {
    PatternGrant {
        owner: OwnerPathPattern(owner.to_string()),
        recipient: RecipientPathPattern::parse(recipient).unwrap(),
        permission: PermissionPattern::Filesystem(FilesystemPermissionPattern::Read(resource)),
    }
}

fn card(lower_positive: Vec<PatternGrant>, upper_positive: Vec<PatternGrant>) -> Card {
    Card {
        card_id: Uuid::new_v4(),
        parent_ids: Vec::new(),
        lower_positive,
        lower_negative: Vec::new(),
        upper_positive,
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        polymorphic: false,
    }
}

#[test]
fn owner_truncation_subsumes_trailing_segments() {
    let broad = OwnerPathPattern("acme/shop".to_string());
    let narrow = OwnerPathPattern("acme/shop/prod/cart/agent".to_string());

    assert!(broad.subsumes(&narrow).unwrap());
    assert!(!narrow.subsumes(&broad).unwrap());
}

#[test]
fn recipient_depths_are_validated() {
    let valid = RecipientPathPattern::parse("acme/shop/prod/cart/agent").unwrap();

    assert!(RecipientPathPattern::parse("acme/shop/prod").is_ok());
    assert!(RecipientPathPattern::parse("acme/shop").is_err());
    assert!(RecipientPathPattern::parse("*/shop/prod").is_err());
    assert!(RecipientPathPattern::parse("*/shop/prod/cart/agent").is_err());
    assert!(RecipientPathPattern::parse("acme/*/prod").is_err());
    assert!(RecipientPathPattern::parse("acme/shop/*/cart/agent").is_err());
    assert!(RecipientPathPattern::parse("acme/shop/prod/*/*").is_ok());
    assert!(RecipientPathPattern::parse("agent(*)").is_err());
    assert!(
        RecipientPathPattern::parse("acme/shop/prod/cart/agent")
            .unwrap()
            .subsumes(&valid)
            .unwrap()
    );
}

#[test]
fn environment_path_pattern_requires_environment_depth() {
    let pattern = EnvironmentPathPattern::parse("acme/shop/prod").unwrap();

    assert_eq!(
        pattern.segments(),
        vec![
            PathSegmentPattern::Exact("acme".to_string()),
            PathSegmentPattern::Exact("shop".to_string()),
            PathSegmentPattern::Exact("prod".to_string())
        ]
    );
    assert!(EnvironmentPathPattern::parse("acme/shop").is_err());
    assert!(EnvironmentPathPattern::parse("acme/shop/prod/cart").is_err());
    assert!(EnvironmentPathPattern::parse("acme/*/prod").is_err());
    assert!(EnvironmentPathPattern::parse("acme/shop/*").is_ok());
}

#[test]
fn glob_resource_subsumes_concrete_resource() {
    let broad = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Glob("/data/**".to_string()),
    );
    let narrow = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Exact("/data/item.json".to_string()),
    );

    assert!(broad.subsumes(&narrow).unwrap());
    assert!(!narrow.subsumes(&broad).unwrap());
}

#[test]
fn effective_surface_requires_lower_and_all_upper_bounds() {
    let holder = RecipientPathPattern::parse("acme/shop/prod/cart/agent").unwrap();
    let read_all = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Glob("/data/**".to_string()),
    );
    let read_secret = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Exact("/data/secret.txt".to_string()),
    );
    let read_public = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Exact("/data/public.txt".to_string()),
    );

    let lower = card(vec![read_all], Vec::new());
    let ceiling = card(Vec::new(), vec![read_public.clone()]);
    let surface = EffectiveSurface::from_cards(&[lower, ceiling], &holder).unwrap();

    assert!(surface.authorize(&read_public).unwrap());
    assert!(!surface.authorize(&read_secret).unwrap());
}

#[test]
fn derivation_must_be_subsumed_by_parent_union() {
    let holder = RecipientPathPattern::parse("acme/shop/prod/cart/agent").unwrap();
    let parent_grant = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Glob("/data/**".to_string()),
    );
    let child_grant = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Exact("/data/file.txt".to_string()),
    );
    let denied_child = fs(
        "other/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Exact("/data/file.txt".to_string()),
    );

    let parent = card(vec![parent_grant], Vec::new());

    assert!(
        EffectiveSurface::validates_derivation(
            std::slice::from_ref(&parent),
            &holder,
            std::slice::from_ref(&child_grant),
            &[]
        )
        .is_ok()
    );
    assert!(
        EffectiveSurface::validates_derivation(&[parent], &holder, &[denied_child], &[]).is_err()
    );
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

    assert!(matches!(
        result,
        Err(CardParseError::UnknownVerb { class, verb }) if class == "filesystem" && verb == "query"
    ));
}

fn parsed_permission(input: &str) -> PermissionPattern {
    parse_pattern_grant(input)
        .expect("grant should parse")
        .permission
}

#[test]
fn parses_runtime_class_examples_from_spec() {
    assert!(matches!(
        parsed_permission(
            "filesystem(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : /data/**"
        ),
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Read(GlobResourcePattern::Glob(path)))
            if path == "/data/**"
    ));

    assert!(matches!(
        parsed_permission(
            "filesystem(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : * : /data/**"
        ),
        PermissionPattern::Filesystem(FilesystemPermissionPattern::Any(GlobResourcePattern::Glob(path)))
            if path == "/data/**"
    ));

    assert!(matches!(
        parsed_permission("network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : *.openai.com"),
        PermissionPattern::Network(NetworkPermissionPattern::Connect(NetworkResourcePattern::HostPort { host, ports }))
            if host == "*.openai.com" && matches!(ports, PortPattern::Any)
    ));

    assert!(matches!(
        parsed_permission("network() @ acme/shop/prod/cart-svc/CartAgent(\"42\") : connect : api.internal:8080-9000"),
        PermissionPattern::Network(NetworkPermissionPattern::Connect(NetworkResourcePattern::HostPort { host, ports }))
            if host == "api.internal" && matches!(ports, PortPattern::Range { start: 8080, end: 9000 })
    ));

    assert!(matches!(
        parsed_permission("env(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : HOME"),
        PermissionPattern::Env(EnvPermissionPattern::Read(IdentifierResourcePattern::Exact(name)))
            if name == "HOME"
    ));

    assert!(matches!(
        parsed_permission(
            "oplog(acme/shop/prod/cart-svc/CartAgent(\"42\")) @ acme/shop/prod/cart-svc/CartAgent(\"42\") : read : *"
        ),
        PermissionPattern::Oplog(OplogPermissionPattern::Read(OplogResourcePattern::Any))
    ));

    assert!(matches!(
        parsed_permission("secret(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : reveal : cart.api-key"),
        PermissionPattern::Secret(SecretPermissionPattern::Reveal(GlobResourcePattern::Exact(key)))
            if key == "cart.api-key"
    ));

    assert!(matches!(
        parsed_permission("kv(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : read : my-store.user-*"),
        PermissionPattern::Kv(KvPermissionPattern::Read(GlobResourcePattern::Glob(key)))
            if key == "my-store.user-*"
    ));

    assert!(matches!(
        parsed_permission("blob(acme/shop/prod) @ acme/shop/prod/cart-svc/ShoppingCart(*) : read : my-bucket.models/*.bin"),
        PermissionPattern::Blob(BlobPermissionPattern::Read(GlobResourcePattern::Glob(key)))
            if key == "my-bucket.models/*.bin"
    ));
}

#[test]
fn parses_agent_tool_and_card_examples_from_spec() {
    assert!(matches!(
        parsed_permission("agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : add-item"),
        PermissionPattern::Agent(AgentPermissionPattern::Invoke(AgentResourcePattern::Method(method)))
            if method == "add-item"
    ));

    assert!(matches!(
        parsed_permission(
            "agent(acme/shop/prod/cart-svc/*) @ acme/shop/prod/cart-svc/* : delete :"
        ),
        PermissionPattern::Agent(AgentPermissionPattern::Delete(AgentResourcePattern::Empty))
    ));

    assert!(matches!(
        parsed_permission("tool(acme/shop/prod/cli-tools/grep) @ acme/shop/prod/cart-svc/ShoppingCart(*) : invoke : search"),
        PermissionPattern::Tool(ToolPermissionPattern::Invoke(ToolResourcePattern::Command(command)))
            if command == "search"
    ));

    assert!(matches!(
        parsed_permission("card(acme) @ acme/shop/prod/cart-svc/ShoppingCart(*) : derive :"),
        PermissionPattern::Card(CardPermissionPattern::Derive(CardResourcePattern::Empty))
    ));

    assert!(matches!(
        parsed_permission("card(acme) @ acme : install : acme/shop/prod/cart-svc/ShoppingCart(*)"),
        PermissionPattern::Card(CardPermissionPattern::Install(
            CardResourcePattern::InstallTarget(RecipientPathPattern::Agent { .. })
        ))
    ));
}

#[test]
fn parses_admin_class_examples() {
    assert!(matches!(
        parsed_permission("system() @ acme : create-account :"),
        PermissionPattern::System(SystemPermissionPattern::CreateAccount(EmptyResourcePattern))
    ));

    assert!(matches!(
        parsed_permission("plan() @ acme : view : *"),
        PermissionPattern::Plan(PlanPermissionPattern::View(IdentifierResourcePattern::Any))
    ));

    assert!(matches!(
        parsed_permission("account(acme) @ acme : set-plan :"),
        PermissionPattern::Account(AccountPermissionPattern::SetPlan(EmptyResourcePattern))
    ));

    assert!(matches!(
        parsed_permission("environment(acme/shop/prod) @ acme/shop/prod : deploy :"),
        PermissionPattern::Environment(EnvironmentPermissionPattern::Deploy(EmptyResourcePattern))
    ));

    assert!(matches!(
        parsed_permission("environment.agent-secret(acme/shop/prod) @ acme/shop/prod : update : cart.*"),
        PermissionPattern::EnvironmentAgentSecret(EnvironmentAgentSecretPermissionPattern::Update(GlobResourcePattern::Glob(path)))
            if path == "cart.*"
    ));
}

#[test]
fn parses_polymorphic_pattern_grant_with_slot_paths() {
    let grant =
        parse_polymorphic_pattern_grant("secret(?env) @ ?self : reveal : billing.?account.*")
            .unwrap();

    assert_eq!(grant.owner, PolymorphicOwnerPathPattern("?env".to_string()));
    assert_eq!(
        grant.recipient,
        PolymorphicRecipientPathPattern("?self".to_string())
    );
    assert!(matches!(
        grant.permission,
        PolymorphicPermissionPattern::Secret(PolymorphicSecretPermissionPattern::Reveal(
            PolymorphicGlobResourcePattern::Template(resource)
        )) if resource == "billing.?account.*"
    ));
}

#[test]
fn parses_polymorphic_resource_slots_and_templates() {
    let env = parse_polymorphic_pattern_grant("env(?self) @ ?self : read : ?env_var").unwrap();
    assert!(matches!(
        env.permission,
        PolymorphicPermissionPattern::Env(PolymorphicEnvPermissionPattern::Read(
            PolymorphicIdentifierResourcePattern::Slot(SlotVariable(name))
        )) if name == "env_var"
    ));

    let card =
        parse_polymorphic_pattern_grant("card(?account) @ ?account : install : ?self").unwrap();
    assert!(matches!(
        card.permission,
        PolymorphicPermissionPattern::Card(PolymorphicCardPermissionPattern::Install(
            PolymorphicCardResourcePattern::Slot(SlotVariable(name))
        )) if name == "self"
    ));

    let card =
        parse_polymorphic_pattern_grant("card(?account) @ ?account : install : acme/shop/?env/*/*")
            .unwrap();
    assert!(matches!(
        card.permission,
        PolymorphicPermissionPattern::Card(PolymorphicCardPermissionPattern::Install(
            PolymorphicCardResourcePattern::Template(target)
        )) if target == "acme/shop/?env/*/*"
    ));
}

#[test]
fn concrete_parser_rejects_slot_variables() {
    let result = parse_pattern_grant("secret(?env) @ ?self : reveal : billing.*");

    assert!(matches!(
        result,
        Err(CardParseError::SlotVariableInConcreteGrant(value)) if value == "?env"
    ));
}

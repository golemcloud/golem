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
    AccountOwnerPattern, AgentOwnerPattern, ApplicationOwnerPattern, ComponentOwnerPattern,
    EmptyOwnerPattern, EnvironmentOwnerPattern, OwnerPattern, ToolOwnerPattern,
};
use crate::model::card::recipient::RecipientPattern;
use RecipientPattern as AccountRecipientPattern;
use RecipientPattern as AgentRecipientPattern;
use RecipientPattern as EnvironmentRecipientPattern;
use chrono::Utc;
use pretty_assertions::assert_matches;
use test_r::core::{DynamicTestRegistration, TestProperties};
use test_r::{add_test, test, test_gen};
use uuid::Uuid;

fn fs(owner: &str, recipient: &str, resource: FilesystemResourcePattern) -> PermissionPattern {
    PermissionPattern::Filesystem(ClassPermissionPattern::<FilesystemClass> {
        verb: Some(FilesystemVerb::Read),
        owner: AgentOwnerPattern::parse(owner).unwrap(),
        recipient: AgentRecipientPattern::parse(recipient).unwrap(),
        resource,
    })
}

fn fs_target(owner: &str, resource: FilesystemResourcePattern) -> PermissionTarget {
    PermissionTarget::Filesystem(ClassPermissionTarget::<FilesystemClass> {
        verb: Some(FilesystemVerb::Read),
        owner: AgentOwnerPattern::parse(owner).unwrap(),
        resource,
    })
}

fn fs_path(segments: Vec<FilesystemPathSegmentPattern>) -> FilesystemResourcePattern {
    FilesystemResourcePattern::Path(FilesystemPathPattern { segments })
}

fn fs_lit(value: &str) -> FilesystemPathSegmentPattern {
    FilesystemPathSegmentPattern::Literal(value.to_string())
}

fn fs_permission(permission: ClassPermissionPattern<FilesystemClass>) -> PermissionPattern {
    PermissionPattern::Filesystem(permission)
}

fn network(recipient: &str, resource: NetworkResourcePattern) -> PermissionPattern {
    PermissionPattern::Network(ClassPermissionPattern::<NetworkClass> {
        verb: Some(NetworkVerb::Connect),
        owner: EmptyOwnerPattern,
        recipient: AgentRecipientPattern::parse(recipient).unwrap(),
        resource,
    })
}

fn oplog_read(resource: OplogResourcePattern) -> PermissionPattern {
    PermissionPattern::Oplog(ClassPermissionPattern::<OplogClass> {
        verb: Some(OplogVerb::Read),
        owner: AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        recipient: AgentRecipientPattern::parse("acme/*/*/*/*").unwrap(),
        resource,
    })
}

fn fixed_uuid() -> Uuid {
    Uuid::from_u128(0x550e8400e29b41d4a716446655440000)
}

fn fixed_token_id() -> TokenId {
    TokenId(fixed_uuid())
}

fn card(lower_positive: Vec<PermissionPattern>, upper_positive: Vec<PermissionPattern>) -> Card {
    Card {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive,
        lower_negative: Vec::new(),
        upper_positive,
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    }
}

#[test]
fn owner_wildcards_subsume_segments() {
    let broad = AgentOwnerPattern::parse("acme/shop/prod/*/*").unwrap();
    let narrow = AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap();

    assert!(broad.subsumes(&narrow));
    assert!(!narrow.subsumes(&broad));
}

#[test_gen]
fn generate_owner_subsumption_tests(r: &mut DynamicTestRegistration) {
    let cases = [
        ("acme/shop/prod/*/*", "acme/shop/prod/cart/agent", true),
        ("acme/shop/*/*/*", "acme/shop/prod/cart/agent", true),
        ("*/*/*/*/*", "acme/shop/prod/cart/agent", true),
        (
            "acme/shop/prod/cart/agent",
            "acme/shop/prod/cart/agent",
            true,
        ),
        ("acme/shop/prod/cart/agent", "acme/shop/prod/cart/*", false),
        (
            "acme/shop/prod/cart/agent",
            "other/shop/prod/cart/agent",
            false,
        ),
        (
            "acme/shop/prod/cart/CartAgent(*)",
            "acme/shop/prod/cart/CartAgent(\"42\")",
            true,
        ),
        (
            "acme/shop/prod/cart/CartAgent(\"42\")",
            "acme/shop/prod/cart/CartAgent(*)",
            false,
        ),
        (
            "acme/shop/prod/cart/ShoppingCart(*)",
            "acme/shop/prod/cart/ShoppingCart(\"42\")",
            true,
        ),
        (
            "acme/shop/prod/cart/ShoppingCart(*)",
            "acme/shop/prod/cart/Order(\"42\")",
            false,
        ),
        ("acme/shop/prod/cart/*", "acme/shop/prod/other/agent", false),
        ("acme/*/*/*/*", "other/shop/prod/cart/agent", false),
    ];

    for (left, right, expected) in cases {
        add_test!(
            r,
            format!(
                "owner_subsumption_{}_{}_{}",
                test_name(left),
                if expected {
                    "subsumes"
                } else {
                    "does_not_subsume"
                },
                test_name(right)
            ),
            TestProperties::unit_test(),
            || {
                let left = AgentOwnerPattern::parse(left).unwrap();
                let right = AgentOwnerPattern::parse(right).unwrap();

                assert_eq!(left.subsumes(&right), expected);
            }
        );
    }
}

#[test]
fn invalid_owner_paths_fail_subsumption() {
    assert_matches!(
        AgentOwnerPattern::parse("acme//prod/cart/agent"),
        Err(path) if path == "acme//prod/cart/agent"
    );
    assert_matches!(AgentOwnerPattern::parse("acme/*/prod/*/*"), Err(_));
    assert_matches!(AgentOwnerPattern::parse("*/shop/prod/cart/agent"), Err(_));
}

#[test]
fn recipient_patterns_subsume_only_matching_holder_subtrees() {
    let account = AccountRecipientPattern::parse("acme").unwrap();
    let account_environments = EnvironmentRecipientPattern::parse("acme/*/*").unwrap();
    let environment = EnvironmentRecipientPattern::parse("acme/shop/prod").unwrap();
    let account_agents = AgentRecipientPattern::parse("acme/*/*/*/*").unwrap();
    let application_agents = AgentRecipientPattern::parse("acme/shop/*/*/*").unwrap();
    let agent_type = AgentRecipientPattern::parse("acme/shop/prod/cart-svc/*").unwrap();
    let agent = AgentRecipientPattern::parse("acme/shop/prod/cart-svc/ShoppingCart").unwrap();
    let other_agent =
        AgentRecipientPattern::parse("other/shop/prod/cart-svc/ShoppingCart").unwrap();

    assert!(account.subsumes(&agent));
    assert!(account.subsumes(&environment));
    assert!(account_environments.subsumes(&environment));
    assert!(account_environments.subsumes(&agent));
    assert!(environment.subsumes(&agent));
    assert!(account_agents.subsumes(&agent));
    assert!(application_agents.subsumes(&agent));
    assert!(!account_agents.subsumes(&environment));
    assert!(agent_type.subsumes(&agent));
    assert!(!agent.subsumes(&agent_type));
    assert!(!account.subsumes(&other_agent));
}

#[test_gen]
fn generate_recipient_subsumption_scope_tests(r: &mut DynamicTestRegistration) {
    let environment_cases = [
        ("*", "acme/shop/prod", true),
        ("acme/*/*", "acme/shop/prod", true),
        ("acme/*/*", "other/shop/prod", false),
        ("acme/shop/*", "acme/shop/prod", true),
        (
            "acme/shop/prod",
            "acme/shop/prod/cart-svc/ShoppingCart(\"42\")",
            true,
        ),
        ("acme/shop/prod", "acme/shop/*", false),
    ];

    for (left, right, expected) in environment_cases {
        add_test!(
            r,
            format!(
                "environment_recipient_subsumption_{}_{}_{}",
                test_name(left),
                if expected {
                    "subsumes"
                } else {
                    "does_not_subsume"
                },
                test_name(right)
            ),
            TestProperties::unit_test(),
            || {
                let left = EnvironmentRecipientPattern::parse(left).unwrap();
                let right = EnvironmentRecipientPattern::parse(right).unwrap();

                assert_eq!(left.subsumes(&right), expected);
            }
        );
    }

    let agent_cases = [
        ("*", "acme/shop/prod/cart-svc/ShoppingCart(\"42\")", true),
        (
            "acme/*/*/*/*",
            "acme/shop/prod/cart-svc/ShoppingCart(\"42\")",
            true,
        ),
        (
            "acme/*/*/*/*",
            "other/shop/prod/cart-svc/ShoppingCart(\"42\")",
            false,
        ),
        ("acme/shop/*/*/*", "acme/shop/prod/*/*", true),
        ("acme/shop/prod/*/*", "acme/shop/*/*/*", false),
        ("acme/shop/prod/*/*", "acme/shop/prod/cart-svc/*", true),
        ("acme/shop/prod/cart-svc/*", "acme/shop/prod/*/*", false),
        (
            "acme/shop/prod/cart-svc/*",
            "acme/shop/prod/cart-svc/ShoppingCart(\"42\")",
            true,
        ),
        (
            "acme/shop/prod/cart-svc/ShoppingCart(\"42\")",
            "acme/shop/prod/cart-svc/*",
            false,
        ),
    ];

    for (left, right, expected) in agent_cases {
        add_test!(
            r,
            format!(
                "agent_recipient_subsumption_{}_{}_{}",
                test_name(left),
                if expected {
                    "subsumes"
                } else {
                    "does_not_subsume"
                },
                test_name(right)
            ),
            TestProperties::unit_test(),
            || {
                let left = AgentRecipientPattern::parse(left).unwrap();
                let right = AgentRecipientPattern::parse(right).unwrap();

                assert_eq!(left.subsumes(&right), expected);
            }
        );
    }
}

#[test_gen]
fn generate_recipient_matching_tests(r: &mut DynamicTestRegistration) {
    let cases = [
        ("*", true),
        ("acme", true),
        ("acme/*/*", true),
        ("acme/shop/*", true),
        ("acme/shop/prod", true),
        ("acme/*/*/*/*", true),
        ("acme/shop/*/*/*", true),
        ("acme/shop/prod/cart-svc/*", true),
        ("acme/shop/prod/other-svc/*", false),
        ("other", false),
    ];

    for (recipient, expected) in cases {
        add_test!(
            r,
            format!(
                "recipient_matching_{}_{}",
                test_name(recipient),
                if expected {
                    "subsumes_holder"
                } else {
                    "does_not_subsume_holder"
                }
            ),
            TestProperties::unit_test(),
            || {
                let holder = "acme/shop/prod/cart-svc/ShoppingCart(\"42\")";

                assert_eq!(recipient_subsumes_holder(recipient, holder), expected);
            }
        );
    }
}

fn recipient_subsumes_holder(recipient: &str, holder: &str) -> bool {
    parse_recipient(recipient).subsumes(&parse_recipient(holder))
}

fn parse_recipient(value: &str) -> RecipientPattern {
    AgentRecipientPattern::parse(value)
        .or_else(|_| EnvironmentRecipientPattern::parse(value))
        .or_else(|_| AccountRecipientPattern::parse(value))
        .unwrap()
}

#[test]
fn glob_resource_subsumes_concrete_resource() {
    let broad = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let narrow = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), fs_lit("item.json")]),
    );

    assert!(broad.subsumes(&narrow));
    assert!(!narrow.subsumes(&broad));
}

#[test_gen]
fn generate_glob_resource_subsumption_tests(r: &mut DynamicTestRegistration) {
    let cases = [
        (
            "any_subsumes_exact",
            FilesystemResourcePattern::any(),
            fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
            true,
        ),
        (
            "double_star_glob_subsumes_exact_prefix",
            fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
            fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
            true,
        ),
        (
            "double_star_glob_subsumes_deep_prefix",
            fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
            fs_path(vec![fs_lit("data"), fs_lit("x"), fs_lit("y"), fs_lit("z")]),
            true,
        ),
        (
            "double_star_glob_subsumes_zero_segments",
            fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
            fs_path(vec![fs_lit("data")]),
            true,
        ),
        (
            "infix_double_star_subsumes_deep_suffix",
            fs_path(vec![
                fs_lit("data"),
                FilesystemPathSegmentPattern::GlobStar,
                fs_lit("secret.txt"),
            ]),
            fs_path(vec![
                fs_lit("data"),
                fs_lit("x"),
                fs_lit("y"),
                fs_lit("secret.txt"),
            ]),
            true,
        ),
        (
            "star_glob_subsumes_exact_prefix",
            fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::Star]),
            fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
            true,
        ),
        (
            "star_glob_does_not_subsume_deep_prefix",
            fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::Star]),
            fs_path(vec![fs_lit("data"), fs_lit("x"), fs_lit("file.txt")]),
            false,
        ),
        (
            "exact_subsumes_same_exact",
            fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
            fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
            true,
        ),
        (
            "exact_does_not_subsume_glob",
            fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
            fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
            false,
        ),
        (
            "wrong_glob_prefix_does_not_subsume_exact",
            fs_path(vec![
                fs_lit("private"),
                FilesystemPathSegmentPattern::GlobStar,
            ]),
            fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
            false,
        ),
    ];

    for (name, left, right, expected) in cases {
        let left = std::sync::Arc::new(left);
        let right = std::sync::Arc::new(right);
        add_test!(
            r,
            format!("glob_resource_subsumption_{name}"),
            TestProperties::unit_test(),
            || {
                let left = fs("acme/shop/prod/cart/agent", "acme/*/*/*/*", (*left).clone());
                let right = fs(
                    "acme/shop/prod/cart/agent",
                    "acme/*/*/*/*",
                    (*right).clone(),
                );

                assert_eq!(left.subsumes(&right), expected);
            }
        );
    }
}

#[test_gen]
fn generate_domain_resource_subsumption_tests(r: &mut DynamicTestRegistration) {
    let application_cases = [(
        "application_unit_subsumes_unit",
        ApplicationResourcePattern,
        ApplicationResourcePattern,
        true,
    )];

    for (name, left, right, expected) in application_cases {
        let left = std::sync::Arc::new(left);
        let right = std::sync::Arc::new(right);
        add_test!(r, name, TestProperties::unit_test(), || {
            assert_eq!(left.as_ref().subsumes(right.as_ref()), expected);
        });
    }

    let environment_cases = [
        (
            "environment_any_subsumes_revision",
            EnvironmentResourcePattern::Any,
            EnvironmentResourcePattern::Revision { revision: 42 },
            true,
        ),
        (
            "environment_revision_does_not_subsume_any",
            EnvironmentResourcePattern::Revision { revision: 42 },
            EnvironmentResourcePattern::Any,
            false,
        ),
        (
            "environment_revision_requires_same_revision",
            EnvironmentResourcePattern::Revision { revision: 42 },
            EnvironmentResourcePattern::Revision { revision: 43 },
            false,
        ),
    ];

    for (name, left, right, expected) in environment_cases {
        let left = std::sync::Arc::new(left);
        let right = std::sync::Arc::new(right);
        add_test!(r, name, TestProperties::unit_test(), || {
            assert_eq!(left.as_ref().subsumes(right.as_ref()), expected);
        });
    }

    let component_cases = [
        (
            "component_any_subsumes_revision",
            ComponentResourcePattern::Any,
            ComponentResourcePattern::Revision { revision: 42 },
            true,
        ),
        (
            "component_revision_does_not_subsume_any",
            ComponentResourcePattern::Revision { revision: 42 },
            ComponentResourcePattern::Any,
            false,
        ),
        (
            "component_revision_requires_same_revision",
            ComponentResourcePattern::Revision { revision: 42 },
            ComponentResourcePattern::Revision { revision: 43 },
            false,
        ),
    ];

    for (name, left, right, expected) in component_cases {
        let left = std::sync::Arc::new(left);
        let right = std::sync::Arc::new(right);
        add_test!(r, name, TestProperties::unit_test(), || {
            assert_eq!(left.as_ref().subsumes(right.as_ref()), expected);
        });
    }

    let agent_cases = [
        (
            "agent_any_subsumes_method",
            AgentResourcePattern::Any,
            AgentResourcePattern::Method(AgentMethodName("charge".to_string())),
            true,
        ),
        (
            "agent_method_does_not_subsume_any",
            AgentResourcePattern::Method(AgentMethodName("charge".to_string())),
            AgentResourcePattern::Any,
            false,
        ),
        (
            "agent_invocation_uuid_requires_same_uuid",
            AgentResourcePattern::InvocationId(AgentInvocationIdPattern::Uuid(fixed_uuid())),
            AgentResourcePattern::InvocationId(AgentInvocationIdPattern::Uuid(Uuid::nil())),
            false,
        ),
        (
            "agent_oplog_index_requires_same_index",
            AgentResourcePattern::OplogIndex(42),
            AgentResourcePattern::OplogIndex(42),
            true,
        ),
    ];

    for (name, left, right, expected) in agent_cases {
        let left = std::sync::Arc::new(left);
        let right = std::sync::Arc::new(right);
        add_test!(r, name, TestProperties::unit_test(), || {
            assert_eq!(left.as_ref().subsumes(right.as_ref()), expected);
        });
    }

    let tool_cases = [
        (
            "tool_any_subsumes_invocation",
            ToolResourcePattern::AnyInvocation,
            ToolResourcePattern::Invocation(ToolInvocationPattern {
                command_path: Some(vec![ToolIdentifier("search".to_string())]),
                args: Vec::new(),
            }),
            true,
        ),
        (
            "tool_invocation_does_not_subsume_any",
            ToolResourcePattern::Invocation(ToolInvocationPattern {
                command_path: Some(vec![ToolIdentifier("search".to_string())]),
                args: Vec::new(),
            }),
            ToolResourcePattern::AnyInvocation,
            false,
        ),
        (
            "tool_invocation_requires_exact_args",
            ToolResourcePattern::Invocation(ToolInvocationPattern {
                command_path: Some(vec![ToolIdentifier("search".to_string())]),
                args: vec![ToolArgPattern::LongFlag {
                    name: ToolIdentifier("pattern".to_string()),
                    value: Some(ToolValuePattern::Star),
                }],
            }),
            ToolResourcePattern::Invocation(ToolInvocationPattern {
                command_path: Some(vec![ToolIdentifier("search".to_string())]),
                args: vec![ToolArgPattern::LongFlag {
                    name: ToolIdentifier("pattern".to_string()),
                    value: Some(ToolValuePattern::Literal(ToolValueLiteral(
                        "cart".to_string(),
                    ))),
                }],
            }),
            false,
        ),
    ];

    for (name, left, right, expected) in tool_cases {
        let left = std::sync::Arc::new(left);
        let right = std::sync::Arc::new(right);
        add_test!(r, name, TestProperties::unit_test(), || {
            assert_eq!(left.as_ref().subsumes(right.as_ref()), expected);
        });
    }

    let plan_cases = [
        (
            "plan_any_subsumes_named",
            PlanResourcePattern::Any,
            PlanResourcePattern::Plan(PlanIdPattern::Identifier(PlanIdentifier(
                "plan-a".to_string(),
            ))),
            true,
        ),
        (
            "plan_named_does_not_subsume_any",
            PlanResourcePattern::Plan(PlanIdPattern::Identifier(PlanIdentifier(
                "plan-a".to_string(),
            ))),
            PlanResourcePattern::Any,
            false,
        ),
    ];

    for (name, left, right, expected) in plan_cases {
        let left = std::sync::Arc::new(left);
        let right = std::sync::Arc::new(right);
        add_test!(r, name, TestProperties::unit_test(), || {
            assert_eq!(left.as_ref().subsumes(right.as_ref()), expected);
        });
    }

    let account_token_cases = [
        (
            "account_token_any_subsumes_token",
            AccountTokenResourcePattern::Any,
            AccountTokenResourcePattern::Token(fixed_token_id()),
            true,
        ),
        (
            "account_token_token_does_not_subsume_any",
            AccountTokenResourcePattern::Token(fixed_token_id()),
            AccountTokenResourcePattern::Any,
            false,
        ),
    ];

    for (name, left, right, expected) in account_token_cases {
        let left = std::sync::Arc::new(left);
        let right = std::sync::Arc::new(right);
        add_test!(r, name, TestProperties::unit_test(), || {
            assert_eq!(left.as_ref().subsumes(right.as_ref()), expected);
        });
    }

    let card_cases = [
        (
            "card_any_subsumes_install_target",
            CardResourcePattern::Any,
            CardResourcePattern::InstallTarget(
                AgentRecipientPattern::parse("acme/shop/prod/cart-svc/ShoppingCart").unwrap(),
            ),
            true,
        ),
        (
            "card_install_target_subsumes_narrower_target",
            CardResourcePattern::InstallTarget(
                AgentRecipientPattern::parse("acme/shop/prod/cart-svc/*").unwrap(),
            ),
            CardResourcePattern::InstallTarget(
                AgentRecipientPattern::parse("acme/shop/prod/cart-svc/ShoppingCart").unwrap(),
            ),
            true,
        ),
        (
            "card_install_target_does_not_subsume_any",
            CardResourcePattern::InstallTarget(
                AgentRecipientPattern::parse("acme/shop/prod/cart-svc/*").unwrap(),
            ),
            CardResourcePattern::Any,
            false,
        ),
    ];

    for (name, left, right, expected) in card_cases {
        let left = std::sync::Arc::new(left);
        let right = std::sync::Arc::new(right);
        add_test!(r, name, TestProperties::unit_test(), || {
            assert_eq!(left.as_ref().subsumes(right.as_ref()), expected);
        });
    }
}

#[test]
fn verb_wildcard_subsumes_class_verbs_only() {
    let any_filesystem = fs_permission(ClassPermissionPattern::<FilesystemClass> {
        verb: None,
        owner: AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        recipient: AgentRecipientPattern::parse("acme/*/*/*/*").unwrap(),
        resource: fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    });
    let read_file = fs_permission(ClassPermissionPattern::<FilesystemClass> {
        verb: Some(FilesystemVerb::Read),
        owner: AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        recipient: AgentRecipientPattern::parse("acme/*/*/*/*").unwrap(),
        resource: fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
    });
    let write_file = fs_permission(ClassPermissionPattern::<FilesystemClass> {
        verb: Some(FilesystemVerb::Write),
        owner: AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        recipient: AgentRecipientPattern::parse("acme/*/*/*/*").unwrap(),
        resource: fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
    });

    assert!(any_filesystem.subsumes(&read_file));
    assert!(any_filesystem.subsumes(&write_file));
    assert!(!read_file.subsumes(&write_file));
}

#[test]
fn network_resource_subsumption_checks_host_and_ports() {
    let port_range = network(
        "acme/*/*/*/*",
        NetworkResourcePattern::HostPort {
            host: "api.internal".to_string(),
            ports: PortPattern::range(8000, 9000),
        },
    );
    let port_single = network(
        "acme/*/*/*/*",
        NetworkResourcePattern::HostPort {
            host: "api.internal".to_string(),
            ports: PortPattern::single(8080),
        },
    );
    let wrong_host = network(
        "acme/*/*/*/*",
        NetworkResourcePattern::HostPort {
            host: "other.internal".to_string(),
            ports: PortPattern::single(8080),
        },
    );
    let wildcard_host = network(
        "acme/*/*/*/*",
        NetworkResourcePattern::HostPort {
            host: "*.internal".to_string(),
            ports: PortPattern::single(8080),
        },
    );
    let deeper_host = network(
        "acme/*/*/*/*",
        NetworkResourcePattern::HostPort {
            host: "api.us.internal".to_string(),
            ports: PortPattern::single(8080),
        },
    );

    assert!(port_range.subsumes(&port_single));
    assert!(!port_single.subsumes(&port_range));
    assert!(!port_range.subsumes(&wrong_host));
    assert!(wildcard_host.subsumes(&port_single));
    assert!(!wildcard_host.subsumes(&deeper_host));
}

#[test]
fn oplog_ranges_subsume_inner_ranges() {
    let broad = oplog_read(OplogResourcePattern::range(Some(100), Some(500)));
    let narrow = oplog_read(OplogResourcePattern::range(Some(200), Some(300)));

    assert!(broad.subsumes(&narrow));
    assert!(!narrow.subsumes(&broad));
}

#[test]
fn subsumption_requires_same_permission_class() {
    let filesystem = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let network = network(
        "acme/shop/prod/cart/agent",
        NetworkResourcePattern::HostPort {
            host: "api.internal".to_string(),
            ports: PortPattern::Any,
        },
    );

    assert!(!filesystem.subsumes(&network));
    assert!(!network.subsumes(&filesystem));
}

#[test]
fn derivation_must_be_subsumed_by_parent_union() {
    let parent_grant = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_grant = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
    );
    let denied_child = fs(
        "other/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
    );

    let parent = card(vec![parent_grant], Vec::new());
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    assert!(
        parent_surface
            .validate_attenuation(std::slice::from_ref(&child_grant), &[], &[], &[],)
            .is_ok()
    );
    assert_matches!(
        parent_surface.validate_attenuation(std::slice::from_ref(&denied_child), &[], &[], &[],),
        Err(CardAlgebraError::LowerBoundTooBroad { .. })
    );
}

#[test]
fn derivation_checks_upper_bounds_against_parent_upper_surface() {
    let parent_upper = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_upper = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
    );
    let too_broad_child_upper = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("other"), fs_lit("file.txt")]),
    );
    let parent = card(Vec::new(), vec![parent_upper]);
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    assert!(
        parent_surface
            .validate_attenuation(&[], &[], std::slice::from_ref(&child_upper), &[],)
            .is_ok()
    );
    assert_matches!(
        parent_surface.validate_attenuation(
            &[],
            &[],
            std::slice::from_ref(&too_broad_child_upper),
            &[],
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
    assert_matches!(
        parent_surface.validate_attenuation(&[], &[], &[], &[]),
        Err(CardAlgebraError::UpperBoundTooBroad { grant: None })
    );
}

#[test]
fn attenuation_rejects_upper_positive_for_disjoint_recipient() {
    let holder = "acme/shop/prod/cart/agent";
    let parent_upper = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_upper = fs(
        holder,
        "acme/shop/prod/cart/child",
        fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
    );
    let parent = card(Vec::new(), vec![parent_upper]);
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    assert_matches!(
        parent_surface.validate_attenuation(&[], &[], std::slice::from_ref(&child_upper), &[],),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
}

#[test]
fn attenuation_rejects_narrowing_upper_positive_recipient() {
    let holder = "acme/shop/prod/cart/agent";
    let other_recipient = "acme/shop/prod/cart/other";
    let parent_upper = fs(
        holder,
        "*",
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let child_upper = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let lower_grant = fs(
        holder,
        other_recipient,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let parent_ceiling = card(Vec::new(), vec![parent_upper]);
    let companion = card(vec![lower_grant], Vec::new());
    let child_ceiling = card(Vec::new(), vec![child_upper.clone()]);
    let recipient = RecipientPattern::parse(other_recipient).unwrap();
    let parent_surface =
        EffectiveSurface::from_cards(&[parent_ceiling.clone(), companion.clone()], &recipient)
            .unwrap();
    let child_surface =
        EffectiveSurface::from_cards(&[child_ceiling, companion], &recipient).unwrap();
    let secret = fs_target(holder, fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]));

    assert!(!parent_surface.authorize(&secret).unwrap());
    assert!(child_surface.authorize(&secret).unwrap());

    let delegation_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent_ceiling));
    assert_matches!(
        delegation_surface.validate_attenuation(&[], &[], std::slice::from_ref(&child_upper), &[],),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
}

#[test]
fn attenuation_accepts_equivalent_disjoint_recipient_ceilings() {
    let holder = "acme/shop/prod/cart/agent";
    let upper_a = fs(
        holder,
        "acme/shop/prod/cart/a",
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let upper_b = fs(
        holder,
        "acme/shop/prod/cart/b",
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let parent_a = card(Vec::new(), vec![upper_a.clone()]);
    let parent_b = card(Vec::new(), vec![upper_b.clone()]);
    let parent_surface = DelegationSurface::from_cards(&[parent_a, parent_b]);

    let result = parent_surface.validate_attenuation(&[], &[], &[upper_a, upper_b], &[]);

    assert!(
        result.is_ok(),
        "retaining the same holder-local ceilings must be accepted: {result:?}"
    );
}

#[test]
fn attenuation_accepts_lower_grant_where_retained_ceiling_is_locally_top() {
    let holder = "acme/shop/prod/cart/agent";
    let recipient_a = "acme/shop/prod/cart/a";
    let recipient_b = "acme/shop/prod/cart/b";
    let parent_lower = fs(
        holder,
        "*",
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_lower = fs(
        holder,
        recipient_b,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let retained_upper = fs(
        holder,
        recipient_a,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let parent = card(vec![parent_lower], vec![retained_upper.clone()]);
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    let result = parent_surface.validate_attenuation(
        std::slice::from_ref(&child_lower),
        &[],
        std::slice::from_ref(&retained_upper),
        &[],
    );

    assert!(
        result.is_ok(),
        "the unchanged ceiling is implicit top for the lower grant's disjoint recipient: {result:?}"
    );
}

#[test]
fn attenuation_rejects_dropped_positive_ceiling_when_child_has_lower_grant() {
    let holder = "acme/shop/prod/cart/agent";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let read_public = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let read_secret = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let bounded_parent = card(vec![read_public.clone()], vec![read_public.clone()]);
    let companion = card(vec![read_secret], Vec::new());
    let parent_cards = [bounded_parent, companion.clone()];
    let parent_surface = EffectiveSurface::from_cards(&parent_cards, &recipient).unwrap();
    let parent_delegation_surface = DelegationSurface::from_cards(&parent_cards);

    let child = card(vec![read_public.clone()], Vec::new());
    let child_surface = EffectiveSurface::from_cards(&[child, companion], &recipient).unwrap();
    let secret_target = fs_target(holder, fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]));

    assert!(!parent_surface.authorize(&secret_target).unwrap());
    assert!(child_surface.authorize(&secret_target).unwrap());
    assert_matches!(
        parent_delegation_surface.validate_attenuation(
            std::slice::from_ref(&read_public),
            &[],
            &[],
            &[],
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
}

#[test]
fn attenuation_preserves_parent_denials_and_accepts_additions_on_both_bounds() {
    let holder = "acme/shop/prod/cart/agent";
    let parent_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("file.txt")]),
    );
    let added_negative = fs(
        holder,
        holder,
        fs_path(vec![
            fs_lit("other"),
            FilesystemPathSegmentPattern::GlobStar,
        ]),
    );
    let inherited_negative = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mut parent = card(vec![parent_grant.clone()], vec![parent_grant]);
    parent.lower_negative = vec![inherited_negative.clone()];
    parent.upper_negative = vec![inherited_negative.clone()];
    let parent_id = parent.card_id;
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));
    let resulting_negative = vec![inherited_negative, added_negative];

    assert_matches!(
        parent_surface.validate_attenuation(
            std::slice::from_ref(&child_grant),
            std::slice::from_ref(&resulting_negative[1]),
            &[],
            &[],
        ),
        Err(CardAlgebraError::LowerBoundTooBroad { .. })
    );
    assert_matches!(
        parent_surface.validate_attenuation(
            &[],
            &resulting_negative,
            std::slice::from_ref(&child_grant),
            std::slice::from_ref(&resulting_negative[1]),
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );

    let witness = parent_surface
        .validate_attenuation(
            std::slice::from_ref(&child_grant),
            &resulting_negative,
            std::slice::from_ref(&child_grant),
            &resulting_negative,
        )
        .unwrap();

    assert_eq!(witness, vec![parent_id]);
}

#[test]
fn attenuation_witness_excludes_grantless_lower_denial_parent() {
    let holder = "acme/shop/prod/cart/agent";
    let parent_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let inherited_negative = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let granting_parent = card(vec![parent_grant], Vec::new());
    let granting_parent_id = granting_parent.card_id;
    let mut denial_parent = card(Vec::new(), Vec::new());
    denial_parent.lower_negative = vec![inherited_negative.clone()];
    let parent_surface = DelegationSurface::from_cards(&[granting_parent, denial_parent]);

    let witness = parent_surface
        .validate_attenuation(std::slice::from_ref(&child_grant), &[], &[], &[])
        .unwrap();

    assert_eq!(witness, vec![granting_parent_id]);
}

#[test]
fn attenuation_does_not_preserve_a_parent_denial_with_an_unrelated_recipient() {
    let holder = "acme/shop/prod/cart/agent";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let parent_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let inherited_negative = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let unrelated_recipient_negative = fs(
        holder,
        "other/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mut parent = card(vec![parent_grant.clone()], vec![parent_grant]);
    parent.lower_negative = vec![inherited_negative.clone()];
    parent.upper_negative = vec![inherited_negative.clone()];
    let parent_delegation_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));
    let parent_surface =
        EffectiveSurface::from_cards(std::slice::from_ref(&parent), &recipient).unwrap();
    let mut child = card(vec![child_grant.clone()], vec![child_grant.clone()]);
    child.lower_negative = vec![unrelated_recipient_negative.clone()];
    child.upper_negative = vec![unrelated_recipient_negative.clone()];
    let child_surface =
        EffectiveSurface::from_cards(std::slice::from_ref(&child), &recipient).unwrap();
    let denied_target = fs_target(holder, fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]));

    assert!(!parent_surface.authorize(&denied_target).unwrap());
    assert!(child_surface.authorize(&denied_target).unwrap());

    assert_matches!(
        parent_delegation_surface.validate_attenuation(
            std::slice::from_ref(&child_grant),
            std::slice::from_ref(&unrelated_recipient_negative),
            &[],
            &[],
        ),
        Err(CardAlgebraError::LowerBoundTooBroad { .. })
    );
    assert_matches!(
        parent_delegation_surface.validate_attenuation(
            &[],
            std::slice::from_ref(&inherited_negative),
            std::slice::from_ref(&child_grant),
            std::slice::from_ref(&unrelated_recipient_negative),
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
}

#[test]
fn attenuation_keeps_inherited_denials_scoped_to_the_parent_recipient() {
    let holder = "acme/shop/prod/cart/agent";
    let child_recipient = "acme/shop/prod/cart/child";
    let parent_grant = fs(
        holder,
        "*",
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_grant = fs(
        holder,
        child_recipient,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let inherited_negative = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mut parent = card(vec![parent_grant], Vec::new());
    parent.lower_negative = vec![inherited_negative.clone()];
    parent.upper_negative = vec![inherited_negative.clone()];
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    let lower_result = parent_surface.validate_attenuation(
        std::slice::from_ref(&child_grant),
        std::slice::from_ref(&inherited_negative),
        &[],
        std::slice::from_ref(&inherited_negative),
    );
    let upper_result = parent_surface.validate_attenuation(
        &[],
        std::slice::from_ref(&inherited_negative),
        std::slice::from_ref(&child_grant),
        std::slice::from_ref(&inherited_negative),
    );

    assert!(
        lower_result.is_ok() && upper_result.is_ok(),
        "lower: {lower_result:?}, upper: {upper_result:?}"
    );
}

#[test]
fn attenuation_rejects_narrowing_an_inherited_upper_denial_recipient() {
    let holder = "acme/shop/prod/cart/agent";
    let parent_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let inherited_negative = fs(
        holder,
        "*",
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let narrowed_negative = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mut parent = card(Vec::new(), vec![parent_grant.clone()]);
    parent.upper_negative = vec![inherited_negative];
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    assert_matches!(
        parent_surface.validate_attenuation(
            &[],
            &[],
            std::slice::from_ref(&parent_grant),
            std::slice::from_ref(&narrowed_negative),
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
}

#[test]
fn attenuation_rejects_omitting_selected_parent_denials_for_other_recipients() {
    let holder = "acme/shop/prod/cart/agent";
    let other_recipient = "acme/shop/prod/cart/other";
    let parent_grant = fs(
        holder,
        "*",
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let inherited_negative = fs(
        holder,
        other_recipient,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mut parent = card(vec![parent_grant.clone()], Vec::new());
    parent.lower_negative = vec![inherited_negative];
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    assert_matches!(
        parent_surface.validate_attenuation(std::slice::from_ref(&parent_grant), &[], &[], &[],),
        Err(CardAlgebraError::LowerBoundTooBroad { .. })
    );
}

#[test]
fn attenuation_rejects_upper_denial_scoped_away_from_lower_recipient() {
    let holder = "acme/shop/prod/cart/agent";
    let lower_recipient = "acme/shop/prod/cart/child";
    let unrelated_upper_recipient = "acme/shop/prod/cart/other";
    let parent_grant = fs(
        holder,
        "*",
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let inherited_negative = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mut parent = card(vec![parent_grant], Vec::new());
    parent.upper_negative = vec![inherited_negative];
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));
    let parent_effective_surface = EffectiveSurface::from_cards(
        std::slice::from_ref(&parent),
        &RecipientPattern::parse(holder).unwrap(),
    )
    .unwrap();

    let child_lower = fs(
        holder,
        lower_recipient,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_upper = fs(
        holder,
        unrelated_upper_recipient,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let child_negative = fs(
        holder,
        unrelated_upper_recipient,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mut child = card(vec![child_lower.clone()], vec![child_upper.clone()]);
    child.upper_negative = vec![child_negative.clone()];
    let child_surface = EffectiveSurface::from_cards(
        std::slice::from_ref(&child),
        &RecipientPattern::parse(lower_recipient).unwrap(),
    )
    .unwrap();
    let secret = fs_target(holder, fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]));

    assert!(!parent_effective_surface.authorize(&secret).unwrap());
    assert!(child_surface.authorize(&secret).unwrap());
    assert_matches!(
        parent_surface.validate_attenuation(
            std::slice::from_ref(&child_lower),
            &[],
            std::slice::from_ref(&child_upper),
            std::slice::from_ref(&child_negative),
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
}

#[test]
fn attenuation_accepts_retained_negative_only_ceiling_without_lower_grants() {
    let holder = "acme/shop/prod/cart/agent";
    let inherited_negative = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let retained_negative = fs(
        holder,
        "*",
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mut parent = card(Vec::new(), Vec::new());
    parent.upper_negative = vec![inherited_negative];
    let parent_id = parent.card_id;
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    let witness = parent_surface
        .validate_attenuation(&[], &[], &[], std::slice::from_ref(&retained_negative))
        .unwrap();

    assert_eq!(witness, vec![parent_id]);
}

#[test]
fn attenuation_rejects_negative_only_child_ceiling_that_drops_parent_positive_ceiling() {
    let holder = "acme/shop/prod/cart/agent";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let parent_upper = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let deny_parent_upper = fs(holder, holder, FilesystemResourcePattern::any());
    let network_grant = network(holder, NetworkResourcePattern::Any);
    let network_target = network_grant.to_target();
    let parent = card(Vec::new(), vec![parent_upper]);
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));
    let companion = card(vec![network_grant], Vec::new());
    let parent_effective_surface =
        EffectiveSurface::from_cards(&[parent, companion.clone()], &recipient).unwrap();
    let mut child_ceiling = card(Vec::new(), Vec::new());
    child_ceiling.upper_negative = vec![deny_parent_upper.clone()];
    let child_effective_surface =
        EffectiveSurface::from_cards(&[child_ceiling, companion], &recipient).unwrap();

    assert!(!parent_effective_surface.authorize(&network_target).unwrap());
    assert!(child_effective_surface.authorize(&network_target).unwrap());

    assert_matches!(
        parent_surface.validate_attenuation(
            &[],
            &[],
            &[],
            std::slice::from_ref(&deny_parent_upper),
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
}

#[test]
fn attenuation_requires_retaining_positive_ceiling_even_when_negative_denies_it() {
    let holder = "acme/shop/prod/cart/agent";
    let parent_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let deny_everything = fs(holder, holder, FilesystemResourcePattern::any());
    let parent = card(vec![parent_grant.clone()], vec![parent_grant.clone()]);
    let parent_id = parent.card_id;
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    assert_matches!(
        parent_surface.validate_attenuation(
            std::slice::from_ref(&child_grant),
            &[],
            &[],
            std::slice::from_ref(&deny_everything),
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );

    let witness = parent_surface
        .validate_attenuation(
            std::slice::from_ref(&child_grant),
            &[],
            std::slice::from_ref(&parent_grant),
            std::slice::from_ref(&deny_everything),
        )
        .unwrap();

    assert_eq!(witness, vec![parent_id]);
}

#[test]
fn attenuation_accepts_identical_fully_denied_parent_ceiling() {
    let holder = "acme/shop/prod/cart/agent";
    let retained_positive = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let mut parent = card(Vec::new(), vec![retained_positive.clone()]);
    parent.upper_negative = vec![retained_positive.clone()];
    let parent_id = parent.card_id;
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    let witness = parent_surface
        .validate_attenuation(
            &[],
            &[],
            std::slice::from_ref(&retained_positive),
            std::slice::from_ref(&retained_positive),
        )
        .unwrap();

    assert_eq!(witness, vec![parent_id]);
}

#[test]
fn attenuation_accepts_identical_fully_denied_parent_lower_bound() {
    let holder = "acme/shop/prod/cart/agent";
    let retained_positive = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let mut parent = card(vec![retained_positive.clone()], Vec::new());
    parent.lower_negative = vec![retained_positive.clone()];
    let parent_id = parent.card_id;
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    let witness = parent_surface
        .validate_attenuation(
            std::slice::from_ref(&retained_positive),
            std::slice::from_ref(&retained_positive),
            &[],
            &[],
        )
        .unwrap();

    assert_eq!(witness, vec![parent_id]);
}

#[test]
fn attenuation_accepts_fully_denied_ceiling_against_implicit_and_explicit_top() {
    macro_rules! grant {
        ($variant:ident, $owner:expr, $resource:expr) => {
            PermissionPattern::$variant(ClassPermissionPattern {
                verb: None,
                owner: $owner,
                recipient: RecipientPattern::Any,
                resource: $resource,
            })
        };
    }

    let universal = vec![
        grant!(
            Filesystem,
            AgentOwnerPattern::AnyAgents,
            FilesystemResourcePattern::any()
        ),
        grant!(Network, EmptyOwnerPattern, NetworkResourcePattern::Any),
        grant!(Env, AgentOwnerPattern::AnyAgents, EnvResourcePattern::Any),
        grant!(
            Oplog,
            AgentOwnerPattern::AnyAgents,
            OplogResourcePattern::Any
        ),
        grant!(
            Config,
            AgentOwnerPattern::AnyAgents,
            ConfigResourcePattern::Any
        ),
        grant!(
            Secret,
            EnvironmentOwnerPattern::AnyEnvironments,
            SecretResourcePattern::Any
        ),
        grant!(
            Agent,
            AgentOwnerPattern::AnyAgents,
            AgentResourcePattern::Any
        ),
        grant!(Tool, ToolOwnerPattern::AnyTools, ToolResourcePattern::any()),
        grant!(
            Kv,
            EnvironmentOwnerPattern::AnyEnvironments,
            KvResourcePattern::any()
        ),
        grant!(
            Blob,
            EnvironmentOwnerPattern::AnyEnvironments,
            BlobResourcePattern::any()
        ),
        grant!(
            Rdbms,
            EnvironmentOwnerPattern::AnyEnvironments,
            RdbmsResourcePattern::any()
        ),
        grant!(Card, AccountOwnerPattern::Any, CardResourcePattern::Any),
        grant!(System, EmptyOwnerPattern, SystemResourcePattern),
        grant!(Plan, EmptyOwnerPattern, PlanResourcePattern::Any),
        grant!(Account, AccountOwnerPattern::Any, AccountResourcePattern),
        grant!(
            AccountUsage,
            AccountOwnerPattern::Any,
            AccountUsageResourcePattern
        ),
        grant!(
            AccountToken,
            AccountOwnerPattern::Any,
            AccountTokenResourcePattern::Any
        ),
        grant!(
            AccountPlugin,
            AccountOwnerPattern::Any,
            AccountPluginResourcePattern::Any
        ),
        grant!(
            Application,
            ApplicationOwnerPattern::AnyApplications,
            ApplicationResourcePattern
        ),
        grant!(
            Environment,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentResourcePattern::Any
        ),
        grant!(
            EnvironmentPluginGrant,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentPluginGrantResourcePattern::Any
        ),
        grant!(
            EnvironmentDomainRegistration,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentDomainRegistrationResourcePattern::Any
        ),
        grant!(
            EnvironmentSecurityScheme,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentSecuritySchemeResourcePattern::Any
        ),
        grant!(
            EnvironmentHttpApiDeployment,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentHttpApiDeploymentResourcePattern::Any
        ),
        grant!(
            EnvironmentMcpDeployment,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentMcpDeploymentResourcePattern::Any
        ),
        grant!(
            EnvironmentAgentSecret,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentAgentSecretResourcePattern::Any
        ),
        grant!(
            EnvironmentResourceDefinition,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentResourceDefinitionResourcePattern::Any
        ),
        grant!(
            EnvironmentRetryPolicy,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentRetryPolicyResourcePattern::Any
        ),
        grant!(
            Component,
            ComponentOwnerPattern::AnyComponents,
            ComponentResourcePattern::Any
        ),
        grant!(
            AccountOauth2Identity,
            AccountOwnerPattern::Any,
            AccountOauth2IdentityResourcePattern::Any
        ),
        grant!(
            EnvironmentInitialFiles,
            ComponentOwnerPattern::AnyComponents,
            EnvironmentInitialFilesResourcePattern::any()
        ),
        grant!(
            EnvironmentKvBucket,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentKvBucketResourcePattern::Any
        ),
        grant!(
            EnvironmentBlobBucket,
            EnvironmentOwnerPattern::AnyEnvironments,
            EnvironmentBlobBucketResourcePattern::Any
        ),
        grant!(
            AccountPermissionShare,
            AccountOwnerPattern::Any,
            AccountPermissionShareResourcePattern::Any
        ),
    ];
    assert_eq!(universal.len(), 34);

    let implicit_top = card(Vec::new(), Vec::new());
    let implicit_top_surface = DelegationSurface::from_cards(std::slice::from_ref(&implicit_top));
    assert!(
        implicit_top_surface
            .validate_attenuation(&[], &[], &[], &universal)
            .is_ok(),
        "the implicit top representation accepts the same empty child ceiling"
    );

    let explicit_top = card(Vec::new(), universal.clone());
    let surface = DelegationSurface::from_cards(std::slice::from_ref(&explicit_top));

    let result = surface.validate_attenuation(&[], &[], &universal, &universal);

    assert!(
        result.is_ok(),
        "a retained child ceiling denying every permission is a subset of an explicit top ceiling: {result:?}"
    );
}

#[test]
fn attenuation_accepts_exactly_retained_negative_only_ceiling() {
    let holder = "acme/shop/prod/cart/agent";
    let inherited_negative = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mut parent = card(Vec::new(), Vec::new());
    parent.upper_negative = vec![inherited_negative.clone()];
    let parent_id = parent.card_id;
    let parent_surface = DelegationSurface::from_cards(std::slice::from_ref(&parent));

    let witness = parent_surface
        .validate_attenuation(&[], &[], &[], std::slice::from_ref(&inherited_negative))
        .unwrap();

    assert_eq!(witness, vec![parent_id]);
}

#[test]
fn attenuation_rejects_negative_only_ceiling_retained_for_another_recipient() {
    let holder = "acme/shop/prod/cart/agent";
    let other_recipient = "acme/shop/prod/cart/other";
    let inherited_negative = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let mismatched_negative = fs(
        holder,
        other_recipient,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let lower_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );

    let mut parent_ceiling = card(Vec::new(), Vec::new());
    parent_ceiling.upper_negative = vec![inherited_negative];
    let companion = card(vec![lower_grant], Vec::new());
    let recipient = RecipientPattern::parse(holder).unwrap();
    let parent_surface =
        EffectiveSurface::from_cards(&[parent_ceiling.clone(), companion.clone()], &recipient)
            .unwrap();

    let mut child_ceiling = card(Vec::new(), Vec::new());
    child_ceiling.upper_negative = vec![mismatched_negative.clone()];
    let child_surface =
        EffectiveSurface::from_cards(&[child_ceiling, companion], &recipient).unwrap();
    let secret = fs_target(holder, fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]));

    assert!(!parent_surface.authorize(&secret).unwrap());
    assert!(child_surface.authorize(&secret).unwrap());

    let parent_ceiling_surface =
        DelegationSurface::from_cards(std::slice::from_ref(&parent_ceiling));
    assert_matches!(
        parent_ceiling_surface.validate_attenuation(
            &[],
            &[],
            &[],
            std::slice::from_ref(&mismatched_negative),
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
}

#[test]
fn wallet_derivation_parent_selection_chooses_highest_qualifying_card_id() {
    let holder = "acme/shop/prod/cart/agent";
    let parent_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let child_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let lower_id = CardId(Uuid::from_u128(1));
    let higher_id = CardId(Uuid::from_u128(2));
    let mut lower = card(vec![parent_grant.clone()], Vec::new());
    lower.card_id = lower_id;
    let mut higher = card(vec![parent_grant], Vec::new());
    higher.card_id = higher_id;

    for cards in [
        vec![lower.clone(), higher.clone()],
        vec![higher.clone(), lower.clone()],
    ] {
        assert_eq!(
            DelegationSurface::from_cards(&cards)
                .select_wallet_derivation_parent(std::slice::from_ref(&child_grant), &[], &[], &[],)
                .unwrap(),
            WalletDerivationParent::Single(higher_id)
        );
    }
}

#[test]
fn wallet_derivation_parent_selection_detects_multi_source_lower_union() {
    let holder = "acme/shop/prod/cart/agent";
    let first_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("first.txt")]),
    );
    let second_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("second.txt")]),
    );
    let parents = [
        card(vec![first_grant.clone()], Vec::new()),
        card(vec![second_grant.clone()], Vec::new()),
    ];

    assert_eq!(
        DelegationSurface::from_cards(&parents)
            .select_wallet_derivation_parent(&[first_grant, second_grant], &[], &[], &[],)
            .unwrap(),
        WalletDerivationParent::MultipleRequired
    );
}

#[test]
fn wallet_derivation_parent_selection_rejects_empty_wallet() {
    assert_eq!(
        DelegationSurface::default()
            .select_wallet_derivation_parent(&[], &[], &[], &[])
            .unwrap(),
        WalletDerivationParent::NotPermitted
    );
}

#[test]
fn wallet_derivation_parent_selection_preserves_combined_attenuation_failure() {
    let holder = "acme/shop/prod/cart/agent";
    let available_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("available.txt")]),
    );
    let unavailable_grant = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("unavailable.txt")]),
    );
    let surface = DelegationSurface::from_cards(&[card(vec![available_grant], Vec::new())]);

    assert_matches!(
        surface.select_wallet_derivation_parent(
            std::slice::from_ref(&unavailable_grant),
            &[],
            &[],
            &[],
        ),
        Err(CardAlgebraError::LowerBoundTooBroad { grant }) if *grant == unavailable_grant
    );
}

#[test]
fn wallet_derivation_parent_selection_applies_upper_intersection_after_lower_union() {
    let holder = "acme/shop/prod/cart/agent";
    let first_lower = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("first.txt")]),
    );
    let second_lower = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("second.txt")]),
    );
    let broad_upper = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
    );
    let narrow_upper = fs(
        holder,
        holder,
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let parents = [
        card(vec![first_lower.clone()], vec![broad_upper.clone()]),
        card(vec![second_lower.clone()], vec![narrow_upper]),
    ];

    assert_matches!(
        DelegationSurface::from_cards(&parents).select_wallet_derivation_parent(
            &[first_lower, second_lower],
            &[],
            std::slice::from_ref(&broad_upper),
            &[],
        ),
        Err(CardAlgebraError::UpperBoundTooBroad { .. })
    );
}

#[test]
fn negative_grants_override_positive_grants() {
    let public = fs_target(
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), fs_lit("public.txt")]),
    );
    let secret = fs_target(
        "acme/shop/prod/cart/agent",
        fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
    );
    let surface = GrantSurface {
        positive: vec![fs_target(
            "acme/shop/prod/cart/agent",
            fs_path(vec![fs_lit("data"), FilesystemPathSegmentPattern::GlobStar]),
        )],
        negative: vec![fs_target(
            "acme/shop/prod/cart/agent",
            fs_path(vec![fs_lit("data"), fs_lit("secret.txt")]),
        )],
    };

    assert!(surface.allows(&public).unwrap());
    assert!(!surface.allows(&secret).unwrap());
}

fn test_name(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

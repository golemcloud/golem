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
use crate::model::card::owner::{AgentOwnerPattern, EmptyOwnerPattern, OwnerPattern};
use crate::model::card::recipient::RecipientPattern;
use RecipientPattern as AccountRecipientPattern;
use RecipientPattern as AgentRecipientPattern;
use RecipientPattern as EnvironmentRecipientPattern;
use chrono::Utc;
use pretty_assertions::assert_matches;
use test_r::core::{DynamicTestRegistration, TestProperties};
use test_r::{add_test, test, test_gen};
use uuid::Uuid;

fn fs(owner: &str, recipient: &str, resource: FilesystemResourcePattern) -> PatternGrant {
    PatternGrant::filesystem_read_pattern(
        AgentOwnerPattern::parse(owner).unwrap(),
        AgentRecipientPattern::parse(recipient).unwrap(),
        resource,
    )
}

fn fs_permission(permission: FilesystemPermissionPattern) -> PatternGrant {
    PatternGrant::new(PermissionPattern::Filesystem(permission))
}

fn network(recipient: &str, resource: NetworkResourcePattern) -> PatternGrant {
    PatternGrant::new(PermissionPattern::Network(NetworkPermissionPattern::Verb {
        verb: NetworkVerb::Connect,
        owner: EmptyOwnerPattern,
        recipient: AgentRecipientPattern::parse(recipient).unwrap(),
        resource,
    }))
}

fn fixed_uuid() -> Uuid {
    Uuid::from_u128(0x550e8400e29b41d4a716446655440000)
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
    let agent =
        AgentRecipientPattern::parse("acme/shop/prod/cart-svc/ShoppingCart(\"42\")").unwrap();

    assert!(account.matches_holder("acme/shop/prod/cart-svc/ShoppingCart(\"42\")"));
    assert!(account.matches_holder("acme/shop/prod"));
    assert!(account_environments.subsumes(&environment));
    assert!(account_environments.matches_holder("acme/shop/prod/cart-svc/ShoppingCart(\"42\")"));
    assert!(environment.matches_holder("acme/shop/prod/cart-svc/ShoppingCart(\"42\")"));
    assert!(account_agents.subsumes(&agent));
    assert!(application_agents.subsumes(&agent));
    assert!(!account_agents.matches_holder("acme/shop/prod"));
    assert!(agent_type.subsumes(&agent));
    assert!(!agent.subsumes(&agent_type));
    assert!(!account.matches_holder("other/shop/prod/cart-svc/ShoppingCart(\"42\")"));
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
                    "matches_holder"
                } else {
                    "does_not_match_holder"
                }
            ),
            TestProperties::unit_test(),
            || {
                let holder = "acme/shop/prod/cart-svc/ShoppingCart(\"42\")";

                assert_eq!(recipient_matches_holder(recipient, holder), expected);
            }
        );
    }
}

fn recipient_matches_holder(recipient: &str, holder: &str) -> bool {
    AgentRecipientPattern::parse(recipient)
        .map(|recipient| recipient.matches_holder(holder))
        .or_else(|_| {
            EnvironmentRecipientPattern::parse(recipient)
                .map(|recipient| recipient.matches_holder(holder))
        })
        .or_else(|_| {
            AccountRecipientPattern::parse(recipient)
                .map(|recipient| recipient.matches_holder(holder))
        })
        .unwrap()
}

#[test]
fn glob_resource_subsumes_concrete_resource() {
    let broad = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::glob("/data/**"),
    );
    let narrow = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::exact("/data/item.json"),
    );

    assert!(broad.subsumes(&narrow).unwrap());
    assert!(!narrow.subsumes(&broad).unwrap());
}

#[test_gen]
fn generate_glob_resource_subsumption_tests(r: &mut DynamicTestRegistration) {
    let cases = [
        (
            "any_subsumes_exact",
            FilesystemResourcePattern::any(),
            FilesystemResourcePattern::exact("/data/file.txt"),
            true,
        ),
        (
            "double_star_glob_subsumes_exact_prefix",
            FilesystemResourcePattern::glob("/data/**"),
            FilesystemResourcePattern::exact("/data/file.txt"),
            true,
        ),
        (
            "star_glob_subsumes_exact_prefix",
            FilesystemResourcePattern::glob("/data/*"),
            FilesystemResourcePattern::exact("/data/file.txt"),
            true,
        ),
        (
            "exact_subsumes_same_exact",
            FilesystemResourcePattern::exact("/data/file.txt"),
            FilesystemResourcePattern::exact("/data/file.txt"),
            true,
        ),
        (
            "exact_does_not_subsume_glob",
            FilesystemResourcePattern::exact("/data/file.txt"),
            FilesystemResourcePattern::glob("/data/**"),
            false,
        ),
        (
            "wrong_glob_prefix_does_not_subsume_exact",
            FilesystemResourcePattern::glob("/private/**"),
            FilesystemResourcePattern::exact("/data/file.txt"),
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

                assert_eq!(left.subsumes(&right).unwrap(), expected);
            }
        );
    }
}

#[test_gen]
fn generate_domain_resource_subsumption_tests(r: &mut DynamicTestRegistration) {
    let application_cases = [
        (
            "application_any_subsumes_named",
            ApplicationResourcePattern::Any,
            ApplicationResourcePattern::Application(ApplicationName("shop".to_string())),
            true,
        ),
        (
            "application_named_does_not_subsume_any",
            ApplicationResourcePattern::Application(ApplicationName("shop".to_string())),
            ApplicationResourcePattern::Any,
            false,
        ),
        (
            "application_named_requires_same_name",
            ApplicationResourcePattern::Application(ApplicationName("shop".to_string())),
            ApplicationResourcePattern::Application(ApplicationName("admin".to_string())),
            false,
        ),
    ];

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
            EnvironmentResourcePattern::Revision {
                environment: EnvironmentName("prod".to_string()),
                revision: 42,
            },
            true,
        ),
        (
            "environment_name_subsumes_own_revision",
            EnvironmentResourcePattern::Environment(EnvironmentName("prod".to_string())),
            EnvironmentResourcePattern::Revision {
                environment: EnvironmentName("prod".to_string()),
                revision: 42,
            },
            true,
        ),
        (
            "environment_revision_does_not_subsume_name",
            EnvironmentResourcePattern::Revision {
                environment: EnvironmentName("prod".to_string()),
                revision: 42,
            },
            EnvironmentResourcePattern::Environment(EnvironmentName("prod".to_string())),
            false,
        ),
        (
            "environment_revision_requires_same_revision",
            EnvironmentResourcePattern::Revision {
                environment: EnvironmentName("prod".to_string()),
                revision: 42,
            },
            EnvironmentResourcePattern::Revision {
                environment: EnvironmentName("prod".to_string()),
                revision: 43,
            },
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
            ComponentResourcePattern::Revision {
                component: ComponentName("cart-svc".to_string()),
                revision: 42,
            },
            true,
        ),
        (
            "component_name_subsumes_own_revision",
            ComponentResourcePattern::Component(ComponentName("cart-svc".to_string())),
            ComponentResourcePattern::Revision {
                component: ComponentName("cart-svc".to_string()),
                revision: 42,
            },
            true,
        ),
        (
            "component_revision_does_not_subsume_name",
            ComponentResourcePattern::Revision {
                component: ComponentName("cart-svc".to_string()),
                revision: 42,
            },
            ComponentResourcePattern::Component(ComponentName("cart-svc".to_string())),
            false,
        ),
        (
            "component_name_requires_same_name",
            ComponentResourcePattern::Component(ComponentName("cart-svc".to_string())),
            ComponentResourcePattern::Revision {
                component: ComponentName("checkout-svc".to_string()),
                revision: 42,
            },
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
            AccountTokenResourcePattern::Token(fixed_uuid()),
            true,
        ),
        (
            "account_token_token_does_not_subsume_any",
            AccountTokenResourcePattern::Token(fixed_uuid()),
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
                AgentRecipientPattern::parse("acme/shop/prod/cart-svc/ShoppingCart(*)").unwrap(),
            ),
            true,
        ),
        (
            "card_install_target_subsumes_narrower_target",
            CardResourcePattern::InstallTarget(
                AgentRecipientPattern::parse("acme/shop/prod/cart-svc/*").unwrap(),
            ),
            CardResourcePattern::InstallTarget(
                AgentRecipientPattern::parse("acme/shop/prod/cart-svc/ShoppingCart(*)").unwrap(),
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
    let any_filesystem = fs_permission(FilesystemPermissionPattern::Any {
        owner: AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        recipient: AgentRecipientPattern::parse("acme/*/*/*/*").unwrap(),
        resource: FilesystemResourcePattern::glob("/data/**"),
    });
    let read_file = fs_permission(FilesystemPermissionPattern::Verb {
        verb: FilesystemVerb::Read,
        owner: AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        recipient: AgentRecipientPattern::parse("acme/*/*/*/*").unwrap(),
        resource: FilesystemResourcePattern::exact("/data/file.txt"),
    });
    let write_file = fs_permission(FilesystemPermissionPattern::Verb {
        verb: FilesystemVerb::Write,
        owner: AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        recipient: AgentRecipientPattern::parse("acme/*/*/*/*").unwrap(),
        resource: FilesystemResourcePattern::exact("/data/file.txt"),
    });

    assert!(any_filesystem.subsumes(&read_file).unwrap());
    assert!(any_filesystem.subsumes(&write_file).unwrap());
    assert!(!read_file.subsumes(&write_file).unwrap());
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

    assert!(port_range.subsumes(&port_single).unwrap());
    assert!(!port_single.subsumes(&port_range).unwrap());
    assert!(!port_range.subsumes(&wrong_host).unwrap());
}

#[test]
fn oplog_ranges_subsume_inner_ranges() {
    let broad = PatternGrant::oplog_read(
        AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        AgentRecipientPattern::parse("acme/*/*/*/*").unwrap(),
        OplogResourcePattern::range(Some(100), Some(500)),
    );
    let narrow = PatternGrant::oplog_read(
        AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        AgentRecipientPattern::parse("acme/*/*/*/*").unwrap(),
        OplogResourcePattern::range(Some(200), Some(300)),
    );

    assert!(broad.subsumes(&narrow).unwrap());
    assert!(!narrow.subsumes(&broad).unwrap());
}

#[test]
fn subsumption_requires_same_permission_class() {
    let filesystem = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::glob("/data/**"),
    );
    let network = network(
        "acme/shop/prod/cart/agent",
        NetworkResourcePattern::HostPort {
            host: "api.internal".to_string(),
            ports: PortPattern::Any,
        },
    );

    assert!(!filesystem.subsumes(&network).unwrap());
    assert!(!network.subsumes(&filesystem).unwrap());
}

#[test]
fn derivation_must_be_subsumed_by_parent_union() {
    let holder = "acme/shop/prod/cart/agent";
    let parent_grant = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::glob("/data/**"),
    );
    let child_grant = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::exact("/data/file.txt"),
    );
    let denied_child = fs(
        "other/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::exact("/data/file.txt"),
    );

    let parent = card(vec![parent_grant], Vec::new());

    assert!(
        EffectiveSurface::validates_derivation(
            std::slice::from_ref(&parent),
            holder,
            std::slice::from_ref(&child_grant),
            &[]
        )
        .is_ok()
    );
    assert_matches!(
        EffectiveSurface::validates_derivation(&[parent], holder, &[denied_child], &[]),
        Err(CardAlgebraError::DerivationNotSubsumed { .. })
    );
}

#[test]
fn derivation_checks_upper_bounds_against_parent_upper_surface() {
    let holder = "acme/shop/prod/cart/agent";
    let parent_upper = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::glob("/data/**"),
    );
    let child_upper = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::exact("/data/file.txt"),
    );
    let too_broad_child_upper = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::exact("/other/file.txt"),
    );
    let parent = card(Vec::new(), vec![parent_upper]);

    assert!(
        EffectiveSurface::validates_derivation(
            std::slice::from_ref(&parent),
            holder,
            &[],
            std::slice::from_ref(&child_upper),
        )
        .is_ok()
    );
    assert_matches!(
        EffectiveSurface::validates_derivation(&[parent], holder, &[], &[too_broad_child_upper]),
        Err(CardAlgebraError::DerivationNotSubsumed { .. })
    );
}

#[test]
fn negative_grants_override_positive_grants() {
    let allowed = fs(
        "acme/shop/prod/cart/agent",
        "acme/*/*/*/*",
        FilesystemResourcePattern::glob("/data/**"),
    );
    let denied = fs(
        "acme/shop/prod/cart/agent",
        "acme/*/*/*/*",
        FilesystemResourcePattern::exact("/data/secret.txt"),
    );
    let public = fs(
        "acme/shop/prod/cart/agent",
        "acme/*/*/*/*",
        FilesystemResourcePattern::exact("/data/public.txt"),
    );
    let secret = fs(
        "acme/shop/prod/cart/agent",
        "acme/*/*/*/*",
        FilesystemResourcePattern::exact("/data/secret.txt"),
    );
    let surface = GrantSurface {
        positive: vec![allowed],
        negative: vec![denied],
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

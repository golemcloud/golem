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
use pretty_assertions::assert_matches;
use test_r::core::{DynamicTestRegistration, TestProperties};
use test_r::{add_test, test, test_gen};
use uuid::Uuid;

fn fs(owner: &str, recipient: &str, resource: GlobResourcePattern) -> PatternGrant {
    fs_permission(
        owner,
        recipient,
        FilesystemPermissionPattern::Read(resource),
    )
}

fn fs_permission(
    owner: &str,
    recipient: &str,
    permission: FilesystemPermissionPattern,
) -> PatternGrant {
    PatternGrant {
        owner: OwnerPathPattern(owner.to_string()),
        recipient: RecipientPathPattern::parse(recipient).unwrap(),
        permission: PermissionPattern::Filesystem(permission),
    }
}

fn network(recipient: &str, resource: NetworkResourcePattern) -> PatternGrant {
    PatternGrant {
        owner: OwnerPathPattern(String::new()),
        recipient: RecipientPathPattern::parse(recipient).unwrap(),
        permission: PermissionPattern::Network(NetworkPermissionPattern::Connect(resource)),
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

#[test_gen]
fn generate_owner_subsumption_tests(r: &mut DynamicTestRegistration) {
    let cases = [
        ("acme", "acme/shop/prod/cart/agent", true),
        ("acme/shop", "acme/shop/prod/cart/agent", true),
        ("acme/*/prod", "acme/shop/prod", true),
        ("*/shop/prod", "acme/shop/prod", true),
        ("acme/shop/prod", "acme/shop/prod", true),
        ("acme/shop/prod/cart/agent", "acme/shop", false),
        ("acme/shop/prod", "other/shop/prod", false),
        ("acme/shop/prod", "acme/*/prod", false),
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
                let left = OwnerPathPattern(left.to_string());
                let right = OwnerPathPattern(right.to_string());

                assert_eq!(left.subsumes(&right).unwrap(), expected);
            }
        );
    }
}

#[test]
fn invalid_owner_paths_fail_subsumption() {
    let invalid = OwnerPathPattern("acme//prod".to_string());
    let valid = OwnerPathPattern("acme/shop/prod".to_string());

    assert_matches!(
        invalid.subsumes(&valid),
        Err(CardAlgebraError::InvalidOwnerPath(path)) if path == "acme//prod"
    );
}

#[test]
fn recipient_patterns_subsume_only_matching_holder_subtrees() {
    let account = RecipientPathPattern::parse("acme").unwrap();
    let environment = RecipientPathPattern::parse("acme/shop/prod").unwrap();
    let agent_type = RecipientPathPattern::parse("acme/shop/prod/cart-svc/*").unwrap();
    let agent =
        RecipientPathPattern::parse("acme/shop/prod/cart-svc/ShoppingCart(\"42\")").unwrap();
    let other =
        RecipientPathPattern::parse("other/shop/prod/cart-svc/ShoppingCart(\"42\")").unwrap();

    assert!(account.subsumes(&agent));
    assert!(environment.subsumes(&agent));
    assert!(agent_type.subsumes(&agent));
    assert!(!agent.subsumes(&agent_type));
    assert!(!account.subsumes(&other));
}

#[test_gen]
fn generate_recipient_matching_tests(r: &mut DynamicTestRegistration) {
    let cases = [
        ("*", true),
        ("acme", true),
        ("acme/shop/prod", true),
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
                let holder =
                    RecipientPathPattern::parse("acme/shop/prod/cart-svc/ShoppingCart(\"42\")")
                        .unwrap();
                let recipient = RecipientPathPattern::parse(recipient).unwrap();

                assert_eq!(recipient.matches_holder(&holder), expected);
            }
        );
    }
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

#[test_gen]
fn generate_glob_resource_subsumption_tests(r: &mut DynamicTestRegistration) {
    let cases = [
        (
            "any_subsumes_exact",
            GlobResourcePattern::Any,
            GlobResourcePattern::Exact("/data/file.txt".to_string()),
            true,
        ),
        (
            "double_star_glob_subsumes_exact_prefix",
            GlobResourcePattern::Glob("/data/**".to_string()),
            GlobResourcePattern::Exact("/data/file.txt".to_string()),
            true,
        ),
        (
            "star_glob_subsumes_exact_prefix",
            GlobResourcePattern::Glob("/data/*".to_string()),
            GlobResourcePattern::Exact("/data/file.txt".to_string()),
            true,
        ),
        (
            "exact_subsumes_same_exact",
            GlobResourcePattern::Exact("/data/file.txt".to_string()),
            GlobResourcePattern::Exact("/data/file.txt".to_string()),
            true,
        ),
        (
            "exact_does_not_subsume_glob",
            GlobResourcePattern::Exact("/data/file.txt".to_string()),
            GlobResourcePattern::Glob("/data/**".to_string()),
            false,
        ),
        (
            "wrong_glob_prefix_does_not_subsume_exact",
            GlobResourcePattern::Glob("/private/**".to_string()),
            GlobResourcePattern::Exact("/data/file.txt".to_string()),
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
                let left = fs("acme/shop/prod/cart/agent", "acme", (*left).clone());
                let right = fs("acme/shop/prod/cart/agent", "acme", (*right).clone());

                assert_eq!(left.subsumes(&right).unwrap(), expected);
            }
        );
    }
}

#[test]
fn verb_wildcard_subsumes_class_verbs_only() {
    let any_filesystem = fs_permission(
        "acme/shop/prod/cart/agent",
        "acme",
        FilesystemPermissionPattern::Any(GlobResourcePattern::Glob("/data/**".to_string())),
    );
    let read_file = fs_permission(
        "acme/shop/prod/cart/agent",
        "acme",
        FilesystemPermissionPattern::Read(GlobResourcePattern::Exact("/data/file.txt".to_string())),
    );
    let write_file = fs_permission(
        "acme/shop/prod/cart/agent",
        "acme",
        FilesystemPermissionPattern::Write(GlobResourcePattern::Exact(
            "/data/file.txt".to_string(),
        )),
    );

    assert!(any_filesystem.subsumes(&read_file).unwrap());
    assert!(any_filesystem.subsumes(&write_file).unwrap());
    assert!(!read_file.subsumes(&write_file).unwrap());
}

#[test]
fn network_resource_subsumption_checks_host_and_ports() {
    let port_range = network(
        "acme",
        NetworkResourcePattern::HostPort {
            host: "api.internal".to_string(),
            ports: PortPattern::Range {
                start: 8000,
                end: 9000,
            },
        },
    );
    let port_single = network(
        "acme",
        NetworkResourcePattern::HostPort {
            host: "api.internal".to_string(),
            ports: PortPattern::Single(8080),
        },
    );
    let wrong_host = network(
        "acme",
        NetworkResourcePattern::HostPort {
            host: "other.internal".to_string(),
            ports: PortPattern::Single(8080),
        },
    );

    assert!(port_range.subsumes(&port_single).unwrap());
    assert!(!port_single.subsumes(&port_range).unwrap());
    assert!(!port_range.subsumes(&wrong_host).unwrap());
}

#[test]
fn oplog_ranges_subsume_inner_ranges() {
    let broad = PatternGrant {
        owner: OwnerPathPattern("acme/shop/prod/cart/agent".to_string()),
        recipient: RecipientPathPattern::parse("acme").unwrap(),
        permission: PermissionPattern::Oplog(OplogPermissionPattern::Read(
            OplogResourcePattern::Range {
                start: Some(100),
                end: Some(500),
            },
        )),
    };
    let narrow = PatternGrant {
        owner: OwnerPathPattern("acme/shop/prod/cart/agent".to_string()),
        recipient: RecipientPathPattern::parse("acme").unwrap(),
        permission: PermissionPattern::Oplog(OplogPermissionPattern::Read(
            OplogResourcePattern::Range {
                start: Some(200),
                end: Some(300),
            },
        )),
    };

    assert!(broad.subsumes(&narrow).unwrap());
    assert!(!narrow.subsumes(&broad).unwrap());
}

#[test]
fn subsumption_requires_same_permission_class() {
    let filesystem = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Glob("/data/**".to_string()),
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
    assert_matches!(
        EffectiveSurface::validates_derivation(&[parent], &holder, &[denied_child], &[]),
        Err(CardAlgebraError::DerivationNotSubsumed { .. })
    );
}

#[test]
fn derivation_checks_upper_bounds_against_parent_upper_surface() {
    let holder = RecipientPathPattern::parse("acme/shop/prod/cart/agent").unwrap();
    let parent_upper = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Glob("/data/**".to_string()),
    );
    let child_upper = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Exact("/data/file.txt".to_string()),
    );
    let too_broad_child_upper = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::Exact("/other/file.txt".to_string()),
    );
    let parent = card(Vec::new(), vec![parent_upper]);

    assert!(
        EffectiveSurface::validates_derivation(
            std::slice::from_ref(&parent),
            &holder,
            &[],
            std::slice::from_ref(&child_upper),
        )
        .is_ok()
    );
    assert_matches!(
        EffectiveSurface::validates_derivation(&[parent], &holder, &[], &[too_broad_child_upper]),
        Err(CardAlgebraError::DerivationNotSubsumed { .. })
    );
}

#[test]
fn negative_grants_override_positive_grants() {
    let allowed = fs(
        "acme/shop/prod/cart/agent",
        "acme",
        GlobResourcePattern::Glob("/data/**".to_string()),
    );
    let denied = fs(
        "acme/shop/prod/cart/agent",
        "acme",
        GlobResourcePattern::Exact("/data/secret.txt".to_string()),
    );
    let public = fs(
        "acme/shop/prod/cart/agent",
        "acme",
        GlobResourcePattern::Exact("/data/public.txt".to_string()),
    );
    let secret = fs(
        "acme/shop/prod/cart/agent",
        "acme",
        GlobResourcePattern::Exact("/data/secret.txt".to_string()),
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

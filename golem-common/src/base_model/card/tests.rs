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

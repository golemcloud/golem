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
use test_r::test;
use uuid::Uuid;

fn fs(owner: &str, recipient: &str, resource: GlobResourcePattern) -> PatternGrant {
    PatternGrant {
        owner: OwnerPathPattern(owner.to_string()),
        recipient: RecipientPathPattern::parse(recipient).unwrap(),
        permission: PermissionPattern::Filesystem(FilesystemPermissionPattern::Read(resource)),
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

#[test]
fn recipient_patterns_subsume_only_matching_holder_subtrees() {
    let account = RecipientPathPattern::parse("acme").unwrap();
    let environment = RecipientPathPattern::parse("acme/shop/prod").unwrap();
    let agent_type = RecipientPathPattern::parse("acme/shop/prod/cart-svc/*").unwrap();
    let agent =
        RecipientPathPattern::parse("acme/shop/prod/cart-svc/ShoppingCart(\"42\")").unwrap();
    let other =
        RecipientPathPattern::parse("other/shop/prod/cart-svc/ShoppingCart(\"42\")").unwrap();

    assert!(account.subsumes(&agent).unwrap());
    assert!(environment.subsumes(&agent).unwrap());
    assert!(agent_type.subsumes(&agent).unwrap());
    assert!(!agent.subsumes(&agent_type).unwrap());
    assert!(!account.subsumes(&other).unwrap());
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
fn derivation_api_is_monomorphic_only() {
    let _signature: fn(
        &[Card],
        &RecipientPathPattern,
        &[PatternGrant],
        &[PatternGrant],
    ) -> Result<(), CardAlgebraError> = EffectiveSurface::validates_derivation;

    let _polymorphic_grant = PolymorphicPatternGrant {
        owner: PolymorphicOwnerPathPattern::Slot(SlotVariable("self".to_string())),
        recipient: PolymorphicRecipientPathPattern::Slot(SlotVariable("self".to_string())),
        permission: PolymorphicPermissionPattern::Filesystem(
            PolymorphicFilesystemPermissionPattern::Read(PolymorphicGlobResourcePattern::Concrete(
                GlobResourcePattern::Glob("/data/**".to_string()),
            )),
        ),
    };

    // Polymorphic grants intentionally have no subsumption/derivation API. They must first be
    // monomorphized into PatternGrant values before the algebra can be applied.
}

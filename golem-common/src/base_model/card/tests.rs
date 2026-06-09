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
use crate::model::card::owner::{AgentOwnerPattern, EnvironmentOwnerPattern};
use crate::model::card::recipient::RecipientPattern;
use RecipientPattern as AccountRecipientPattern;
use RecipientPattern as AgentRecipientPattern;
use RecipientPattern as EnvironmentRecipientPattern;
use chrono::Utc;
use test_r::test;

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

fn secret_reveal(
    owner: &str,
    recipient: &str,
    resource: SecretResourcePattern,
) -> PermissionPattern {
    PermissionPattern::Secret(ClassPermissionPattern::<SecretClass> {
        verb: Some(SecretVerb::Reveal),
        owner: EnvironmentOwnerPattern::parse(owner).unwrap(),
        recipient: AgentRecipientPattern::parse(recipient).unwrap(),
        resource,
    })
}

fn secret_reveal_target(owner: &str, resource: SecretResourcePattern) -> PermissionTarget {
    PermissionTarget::Secret(ClassPermissionTarget::<SecretClass> {
        verb: Some(SecretVerb::Reveal),
        owner: EnvironmentOwnerPattern::parse(owner).unwrap(),
        resource,
    })
}

fn card(lower_positive: Vec<PermissionPattern>, upper_positive: Vec<PermissionPattern>) -> Card {
    card_with_bounds(lower_positive, Vec::new(), upper_positive, Vec::new())
}

fn card_with_bounds(
    lower_positive: Vec<PermissionPattern>,
    lower_negative: Vec<PermissionPattern>,
    upper_positive: Vec<PermissionPattern>,
    upper_negative: Vec<PermissionPattern>,
) -> Card {
    Card {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive,
        lower_negative,
        upper_positive,
        upper_negative,
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    }
}

#[test]
fn recipient_depths_are_validated() {
    let valid = AgentRecipientPattern::parse("acme/shop/prod/cart/agent").unwrap();

    assert!(AccountRecipientPattern::parse("acme").is_ok());
    assert!(EnvironmentRecipientPattern::parse("acme/shop/prod").is_ok());
    assert!(EnvironmentRecipientPattern::parse("acme/*/*").is_ok());
    assert!(EnvironmentRecipientPattern::parse("acme/shop/*").is_ok());
    assert!(EnvironmentRecipientPattern::parse("acme/shop").is_err());
    assert!(EnvironmentRecipientPattern::parse("*/shop/prod").is_err());
    assert!(AgentRecipientPattern::parse("*/shop/prod/cart/agent").is_err());
    assert!(EnvironmentRecipientPattern::parse("acme/*/prod").is_err());
    assert!(AgentRecipientPattern::parse("acme/shop/*/cart/agent").is_err());
    assert!(AgentRecipientPattern::parse("acme/*/*/*/*").is_ok());
    assert!(AgentRecipientPattern::parse("acme/shop/*/*/*").is_ok());
    assert!(AgentRecipientPattern::parse("acme/shop/prod/*/*").is_ok());
    assert!(AgentRecipientPattern::parse("acme/shop/prod/cart/*").is_ok());
    assert!(AgentRecipientPattern::parse("agent(*)").is_err());
    assert!(
        AgentRecipientPattern::parse("acme/shop/prod/cart/agent")
            .unwrap()
            .subsumes(&valid)
    );
}

#[test]
fn effective_surface_requires_lower_and_all_upper_bounds() {
    let holder = "acme/shop/prod/cart/agent";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let read_all = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::GlobStar,
            ],
        }),
    );
    let read_secret_target = fs_target(
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::Literal("secret.txt".to_string()),
            ],
        }),
    );
    let read_public = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::Literal("public.txt".to_string()),
            ],
        }),
    );
    let read_public_target = fs_target(
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::Literal("public.txt".to_string()),
            ],
        }),
    );

    let lower = card(vec![read_all], Vec::new());
    let lower_card_id = lower.card_id;
    let ceiling = card(Vec::new(), vec![read_public.clone()]);
    let ceiling_card_id = ceiling.card_id;
    let surface = EffectiveSurface::from_cards(&[lower, ceiling], &recipient).unwrap();

    assert_eq!(
        surface.source_card_ids,
        vec![lower_card_id, ceiling_card_id]
    );
    assert!(surface.authorize(&read_public_target).unwrap());
    assert!(!surface.authorize(&read_secret_target).unwrap());
}

#[test]
fn upper_negative_without_positive_is_unrestricted_except_denials() {
    let holder = "acme/shop/prod/cart/agent";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let read_tmp = fs(
        holder,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("tmp".to_string()),
                FilesystemPathSegmentPattern::Literal("file.txt".to_string()),
            ],
        }),
    );
    let read_tmp_target = fs_target(
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("tmp".to_string()),
                FilesystemPathSegmentPattern::Literal("file.txt".to_string()),
            ],
        }),
    );
    let reveal_secret = secret_reveal("acme/shop/prod", holder, SecretResourcePattern::Any);
    let reveal_secret_target = secret_reveal_target("acme/shop/prod", SecretResourcePattern::Any);

    let lower = card(vec![read_tmp.clone(), reveal_secret.clone()], Vec::new());
    let ceiling = card_with_bounds(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![reveal_secret.clone()],
    );
    let surface = EffectiveSurface::from_cards(&[lower, ceiling.clone()], &recipient).unwrap();
    let ceiling_surface =
        EffectiveSurface::from_cards(std::slice::from_ref(&ceiling), &recipient).unwrap();

    assert!(surface.authorize(&read_tmp_target).unwrap());
    assert!(!surface.authorize(&reveal_secret_target).unwrap());
    assert!(
        ceiling_surface
            .validates_derivation(&[], std::slice::from_ref(&read_tmp))
            .is_ok()
    );
    assert!(matches!(
        ceiling_surface.validates_derivation(&[], std::slice::from_ref(&reveal_secret)),
        Err(CardAlgebraError::DerivationNotSubsumed { .. })
    ));
}

#[test]
fn lower_negative_is_scoped_to_its_card() {
    let holder = "acme/shop/prod/cart/agent";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let read_all = fs(
        holder,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::GlobStar,
            ],
        }),
    );
    let read_secret = fs(
        holder,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::Literal("secret.txt".to_string()),
            ],
        }),
    );
    let read_secret_target = fs_target(
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::Literal("secret.txt".to_string()),
            ],
        }),
    );

    let deny_in_one_card = card_with_bounds(
        vec![read_all],
        vec![read_secret.clone()],
        Vec::new(),
        Vec::new(),
    );
    let allow_in_another_card = card(vec![read_secret.clone()], Vec::new());
    let surface =
        EffectiveSurface::from_cards(&[deny_in_one_card, allow_in_another_card], &recipient)
            .unwrap();

    assert!(surface.authorize(&read_secret_target).unwrap());
}

#[test]
fn derivation_upper_bound_uses_parent_ceiling_intersection() {
    let holder = "acme/shop/prod/cart/agent";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let read_all = fs(
        holder,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::GlobStar,
            ],
        }),
    );
    let read_secret = fs(
        holder,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::Literal("secret.txt".to_string()),
            ],
        }),
    );

    let parent_allow = card(Vec::new(), vec![read_all]);
    let parent_deny = card_with_bounds(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![read_secret.clone()],
    );
    let parent_surface =
        EffectiveSurface::from_cards(&[parent_allow, parent_deny], &recipient).unwrap();

    assert!(matches!(
        parent_surface.validates_derivation(&[], std::slice::from_ref(&read_secret)),
        Err(CardAlgebraError::DerivationNotSubsumed { .. })
    ));
}

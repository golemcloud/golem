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
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::PolymorphicAccountOwnerPattern;
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    AccountCardHolder, AgentPermissionMonomorphizationContext, ApplicationCardHolder,
    PolymorphicCard, PolymorphicClassPermissionPattern, card_matches_agent_recipient,
};
use golem_common::model::component::ComponentName;
use golem_common::model::environment::EnvironmentName;
use test_r::test;

fn valid_filesystem_grant() -> permissions_types::PatternGrant {
    permissions_types::PatternGrant {
        class: "filesystem".to_string(),
        owner: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        verb: "read".to_string(),
        resource_id: "/data/**".to_string(),
    }
}

fn scope_card(card_id: CardId, root_card_ids: Vec<CardId>) -> ScopeCard {
    let grant = parse_pattern_grant(&valid_filesystem_grant()).unwrap();
    ScopeCard {
        scope_card_id: card_id,
        root_card_ids,
        lower_positive: vec![grant.clone()],
        lower_negative: vec![grant.clone()],
        upper_positive: vec![grant.clone()],
        upper_negative: vec![grant],
        expires_at: None,
    }
}

fn comparable_grants(
    grants: &[permissions_types::PatternGrant],
) -> Vec<(String, String, String, String, String)> {
    grants
        .iter()
        .map(|grant| {
            (
                grant.class.clone(),
                grant.owner.clone(),
                grant.recipient.clone(),
                grant.verb.clone(),
                grant.resource_id.clone(),
            )
        })
        .collect()
}

#[test]
fn scope_and_persistent_cards_share_the_metadata_and_inspection_shape() {
    let card_id = CardId(Uuid::from_u128(10));
    let parent_ids = vec![CardId(Uuid::from_u128(1)), CardId(Uuid::from_u128(2))];
    let scope = scope_card(card_id, parent_ids.clone());
    let persistent = StoredCard::Concrete(Card {
        card_id,
        parent_ids,
        lower_positive: scope.lower_positive.clone(),
        lower_negative: scope.lower_negative.clone(),
        upper_positive: scope.upper_positive.clone(),
        upper_negative: scope.upper_negative.clone(),
        created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    });
    let persistent = ResolvedPermissionCard::Persistent(persistent);
    let scope = ResolvedPermissionCard::Scope(scope);

    assert_eq!(scope.card_id(), persistent.card_id());
    assert_eq!(scope.parent_ids(), persistent.parent_ids());
    assert_eq!(scope.expires_at(), persistent.expires_at());

    let persistent_view = card_view(&persistent).unwrap();
    let scope_view = card_view(&scope).unwrap();
    assert_eq!(
        comparable_grants(&scope_view.lower_positive),
        comparable_grants(&persistent_view.lower_positive)
    );
    assert_eq!(
        comparable_grants(&scope_view.lower_negative),
        comparable_grants(&persistent_view.lower_negative)
    );
    assert_eq!(
        comparable_grants(&scope_view.upper_positive),
        comparable_grants(&persistent_view.upper_positive)
    );
    assert_eq!(
        comparable_grants(&scope_view.upper_negative),
        comparable_grants(&persistent_view.upper_negative)
    );
}

#[test]
fn persistent_operations_reject_scope_cards() {
    let scope = scope_card(CardId(Uuid::from_u128(10)), Vec::new());

    for operation in ["derive", "revoke-card", "install-card"] {
        assert!(matches!(
            ResolvedPermissionCard::Scope(scope.clone()).into_persistent(operation),
            Err(permissions_types::PermissionError::NotPermitted(message))
                if message == format!("{operation} does not accept scope cards")
        ));
    }
}

#[test]
fn scope_cards_cannot_use_persistent_snapshot_transport() {
    let entry = PermissionCardEntry::from_scope(
        scope_card(
            CardId(Uuid::from_u128(10)),
            vec![CardId(Uuid::from_u128(1))],
        ),
        IdempotencyKey::fresh(),
    );

    let error = entry.into_snapshot().unwrap_err();
    assert!(error.to_string().contains(
        "scope-card handles cannot be serialized as persistent permission-card snapshots"
    ));
}

#[test]
fn scope_card_handles_are_owned_by_the_minting_invocation() {
    let owner = IdempotencyKey::new("owner".to_string());
    let card = scope_card(
        CardId(Uuid::from_u128(10)),
        vec![CardId(Uuid::from_u128(1))],
    );

    assert_eq!(
        resolve_owned_scope_card(Some((card.clone(), Some(owner.clone()))), &owner).unwrap(),
        card
    );
    assert!(
        resolve_owned_scope_card(
            Some((card.clone(), Some(owner.clone()))),
            &IdempotencyKey::new("different-invocation".to_string()),
        )
        .is_err()
    );
    assert!(
        resolve_owned_scope_card(Some((card, None)), &owner).is_err(),
        "a retired scope card must not be accepted, even in the invocation that minted it"
    );
}

#[test]
fn persistent_cards_are_not_accepted_as_scope_card_arguments() {
    assert!(
        resolve_owned_scope_card(None, &IdempotencyKey::new("current-invocation".to_string()),)
            .is_err()
    );
}

fn derive_authorization_surface() -> EffectiveSurface {
    let recipient =
        RecipientPattern::parse("alice@example.com/shop/prod/cart-svc/CartAgent").unwrap();
    let grant = parse_pattern_grant(&permissions_types::PatternGrant {
        class: "card".to_string(),
        owner: "alice@example.com".to_string(),
        recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        verb: "derive".to_string(),
        resource_id: "*".to_string(),
    })
    .unwrap();
    let card = Card {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive: vec![grant],
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    };

    EffectiveSurface::from_cards(&[card], &recipient).unwrap()
}

fn install_authority_card(card_id: CardId, target: &RecipientPattern) -> StoredCard {
    let grant = parse_pattern_grant(&permissions_types::PatternGrant {
        class: "card".to_string(),
        owner: "alice@example.com".to_string(),
        recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        verb: "install".to_string(),
        resource_id: target.render(),
    })
    .unwrap();
    StoredCard::Concrete(Card {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: vec![grant],
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    })
}

#[test]
fn account_and_application_install_targets_are_rejected_at_the_wit_host_boundary() {
    let unsupported_targets = [
        permissions_types::Holder::Account(Uuid::new_v4().into()),
        permissions_types::Holder::App(Uuid::new_v4().into()),
    ];

    for target in unsupported_targets {
        let error = require_agent_install_target(target).unwrap_err();
        let permissions_types::PermissionError::NotPermitted(message) = error else {
            panic!("unsupported install target returned {error:?}")
        };
        assert_eq!(message, UNSUPPORTED_INSTALL_TARGET_ERROR);
    }
}

#[test]
fn replay_recovery_rejects_account_and_application_transfer_targets() {
    let unsupported_targets = [
        CardHolder::Account(AccountCardHolder {
            account_id: Uuid::new_v4(),
        }),
        CardHolder::Application(ApplicationCardHolder {
            application_id: Uuid::new_v4(),
        }),
    ];

    for target in unsupported_targets {
        let transfer_id = Uuid::new_v4();
        let error = pending_transfer_target_agent_id(transfer_id, &target).unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "pending transfer {transfer_id} targets an unsupported non-agent holder"
            )),
            "unexpected replay recovery error: {error}"
        );
    }
}

#[test]
fn install_card_authority_is_scoped_to_the_concrete_target() {
    let account = AccountEmail::from("alice@example.com");
    let caller = RecipientPattern::parse("alice@example.com/shop/prod/cart-svc/CartAgent").unwrap();
    let target =
        RecipientPattern::parse("alice@example.com/shop/prod/worker-svc/WorkerAgent").unwrap();
    let other_target =
        RecipientPattern::parse("alice@example.com/shop/prod/other-svc/OtherAgent").unwrap();
    let authority = install_authority_card(CardId::new(), &target);
    let surface = EffectiveSurface::from_cards(
        &[monomorphize_card_for_agent(
            &authority,
            &wallet_derivation_context(),
        )],
        &caller,
    )
    .unwrap();

    assert!(
        ensure_install_permission_in_surface(&surface, &account, CardId::new(), &target,).is_ok()
    );
    assert!(matches!(
        ensure_install_permission_in_surface(&surface, &account, CardId::new(), &other_target,),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
}

#[test]
fn install_card_cannot_authorize_its_own_installation() {
    let account = AccountEmail::from("alice@example.com");
    let caller = RecipientPattern::parse("alice@example.com/shop/prod/cart-svc/CartAgent").unwrap();
    let target =
        RecipientPattern::parse("alice@example.com/shop/prod/worker-svc/WorkerAgent").unwrap();
    let transferred_card_id = CardId::new();
    let transferred = install_authority_card(transferred_card_id, &target);
    let card = monomorphize_card_for_agent(&transferred, &wallet_derivation_context());
    let surface = EffectiveSurface::from_cards(&[card], &caller).unwrap();

    assert!(matches!(
        ensure_install_permission_in_surface(&surface, &account, transferred_card_id, &target,),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
}

#[test]
fn install_card_recipients_must_cover_the_target() {
    let context = wallet_derivation_context();
    let matching_recipient = RecipientPattern::parse("alice@example.com/shop/prod/*/*").unwrap();
    let mismatching_recipient =
        RecipientPattern::parse("alice@example.com/shop/staging/*/*").unwrap();
    let matching = wallet_parent(
        1,
        &permissions_types::PatternGrant {
            recipient: matching_recipient.render(),
            ..valid_filesystem_grant()
        },
    );
    let mismatching = wallet_parent(
        2,
        &permissions_types::PatternGrant {
            recipient: mismatching_recipient.render(),
            ..valid_filesystem_grant()
        },
    );
    let matching_upper_positive = match matching.clone() {
        StoredCard::Concrete(mut card) => {
            card.upper_positive = std::mem::take(&mut card.lower_positive);
            StoredCard::Concrete(card)
        }
        StoredCard::Polymorphic(_) => unreachable!(),
    };
    let matching_upper_negative = match matching.clone() {
        StoredCard::Concrete(mut card) => {
            card.upper_negative = std::mem::take(&mut card.lower_positive);
            StoredCard::Concrete(card)
        }
        StoredCard::Polymorphic(_) => unreachable!(),
    };
    let mismatching_upper_only = match mismatching.clone() {
        StoredCard::Concrete(mut card) => {
            card.upper_positive = std::mem::take(&mut card.lower_positive);
            StoredCard::Concrete(card)
        }
        StoredCard::Polymorphic(_) => unreachable!(),
    };
    let matching_negative_only = StoredCard::Concrete(Card {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: vec![
            parse_pattern_grant(&permissions_types::PatternGrant {
                recipient: matching_recipient.render(),
                ..valid_filesystem_grant()
            })
            .unwrap(),
        ],
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    });
    let polymorphic = StoredCard::Polymorphic(PolymorphicCard {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive: vec![PolymorphicPermissionPattern::Card(
            PolymorphicClassPermissionPattern {
                verb: Some(CardVerb::Inspect),
                owner: PolymorphicAccountOwnerPattern::Account,
                recipient: matching_recipient,
                resource: CardResourcePattern::Any,
            },
        )],
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
    });

    assert!(card_matches_agent_recipient(&matching, &context));
    assert!(card_matches_agent_recipient(
        &matching_upper_positive,
        &context
    ));
    assert!(card_matches_agent_recipient(
        &matching_upper_negative,
        &context
    ));
    assert!(card_matches_agent_recipient(&polymorphic, &context));
    assert!(!card_matches_agent_recipient(&mismatching, &context));
    assert!(!card_matches_agent_recipient(
        &mismatching_upper_only,
        &context
    ));
    assert!(!card_matches_agent_recipient(
        &matching_negative_only,
        &context
    ));
}

#[test]
fn installed_child_request_matches_registry_canonicalization() {
    let requested = StoredCard::Concrete(Card {
        card_id: CardId::new(),
        parent_ids: vec![CardId::new()],
        lower_positive: vec![parse_pattern_grant(&valid_filesystem_grant()).unwrap()],
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap(),
        expires_at: Some(DateTime::from_timestamp(1_800_000_000, 987_654_321).unwrap()),
        system_card: false,
        managed_by: None,
    });
    let provenance = CardManagedByRuntimeDerived {
        environment_id: golem_common::model::environment::EnvironmentId(Uuid::new_v4()),
        agent_id: AgentId {
            component_id: golem_common::model::component::ComponentId(Uuid::new_v4()),
            agent_id: "source-agent".to_string(),
        },
        invocation_key: IdempotencyKey::fresh(),
        oplog_index: OplogIndex::from_u64(42),
    };
    let mut canonical = requested.clone();
    if let StoredCard::Concrete(canonical_card) = &mut canonical {
        canonical_card.created_at = DateTime::from_timestamp(1_700_000_000, 123_456_000).unwrap();
        canonical_card.expires_at =
            Some(DateTime::from_timestamp(1_800_000_000, 987_654_000).unwrap());
        canonical_card.managed_by = Some(CardManagedBy::RuntimeDerived(provenance.clone()));
    }

    assert!(installed_card_matches_request(
        &canonical,
        &requested,
        Some(&provenance),
    ));

    if let StoredCard::Concrete(canonical_card) = &mut canonical {
        canonical_card.parent_ids.push(CardId::new());
    }
    assert!(!installed_card_matches_request(
        &canonical,
        &requested,
        Some(&provenance),
    ));
}

#[test]
fn derive_requires_the_global_card_derive_permission() {
    let account = AccountEmail::from("alice@example.com");
    let surface = derive_authorization_surface();

    assert!(
        ensure_card_permission_in_surface(
            &surface,
            &account,
            CardVerb::Derive,
            CardResourcePattern::Any,
        )
        .is_ok()
    );
    assert!(matches!(
        ensure_card_permission_in_surface(
            &EffectiveSurface::default(),
            &account,
            CardVerb::Derive,
            CardResourcePattern::Any,
        ),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
    assert!(matches!(
        ensure_card_permission_in_surface(
            &surface,
            &account,
            CardVerb::Revoke,
            CardResourcePattern::Any,
        ),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
}

#[test]
fn incomplete_wallet_derivation_requires_its_parent_to_still_be_current() {
    let account = AccountEmail::from("alice@example.com");
    let surface = derive_authorization_surface();
    let parent_id = CardId::new();

    assert!(
        ensure_persistent_derivation_repair_authority(
            &surface,
            &account,
            parent_id,
            PersistentDerivationSource::Wallet,
            true,
        )
        .is_ok()
    );
    assert!(matches!(
        ensure_persistent_derivation_repair_authority(
            &surface,
            &account,
            parent_id,
            PersistentDerivationSource::Wallet,
            false,
        ),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
    assert!(
        ensure_persistent_derivation_repair_authority(
            &surface,
            &account,
            parent_id,
            PersistentDerivationSource::ExplicitCard,
            false,
        )
        .is_ok()
    );
    assert!(
        ensure_persistent_derivation_repair_authority(
            &surface,
            &account,
            parent_id,
            PersistentDerivationSource::ExplicitTemplateCard,
            true,
        )
        .is_ok()
    );
    assert!(matches!(
        ensure_persistent_derivation_repair_authority(
            &surface,
            &account,
            parent_id,
            PersistentDerivationSource::ExplicitTemplateCard,
            false,
        ),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
}

#[test]
fn revoke_requires_possession_of_the_target_or_a_live_ancestor() {
    let target = wallet_parent(3, &valid_filesystem_grant());
    let ancestor = wallet_parent(2, &valid_filesystem_grant());
    let unrelated = wallet_parent(4, &valid_filesystem_grant());
    let target_and_ancestors = [
        CardId(Uuid::from_u128(1)),
        ancestor.card_id(),
        target.card_id(),
    ];

    assert!(
        ensure_revoke_authority(
            &target_and_ancestors,
            std::slice::from_ref(&target.card_id()),
        )
        .is_ok()
    );
    assert!(
        ensure_revoke_authority(
            &target_and_ancestors,
            std::slice::from_ref(&ancestor.card_id()),
        )
        .is_ok()
    );
    assert!(matches!(
        ensure_revoke_authority(
            &target_and_ancestors,
            std::slice::from_ref(&unrelated.card_id()),
        ),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
    assert!(matches!(
        ensure_revoke_authority(&target_and_ancestors, &[]),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
}

#[test]
fn attenuation_errors_preserve_the_failed_bound() {
    let grant = parse_pattern_grant(&valid_filesystem_grant()).unwrap();
    let rendered = grant.render().unwrap();

    assert!(matches!(
        permission_error_from_algebra(CardAlgebraError::LowerBoundTooBroad {
            grant: Box::new(grant.clone()),
        }),
        permissions_types::PermissionError::LowerBoundTooBroad(message)
            if message == rendered
    ));
    assert!(matches!(
        permission_error_from_algebra(CardAlgebraError::UpperBoundTooBroad {
            grant: Some(Box::new(grant)),
        }),
        permissions_types::PermissionError::UpperBoundTooBroad(message)
            if message == rendered
    ));
    assert!(matches!(
        permission_error_from_algebra(CardAlgebraError::UpperBoundTooBroad { grant: None }),
        permissions_types::PermissionError::UpperBoundTooBroad(message)
            if message == "upper-bound grant is too broad"
    ));
}

fn concrete_parent(grant: &permissions_types::PatternGrant) -> Card {
    let positive = parse_pattern_grant(grant).unwrap();
    let mut lower_negative = grant.clone();
    lower_negative.resource_id = "/data/private/**".to_string();
    let mut upper_negative = grant.clone();
    upper_negative.resource_id = "/data/secret/**".to_string();

    Card {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive: vec![positive.clone()],
        lower_negative: vec![parse_pattern_grant(&lower_negative).unwrap()],
        upper_positive: vec![positive],
        upper_negative: vec![parse_pattern_grant(&upper_negative).unwrap()],
        created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    }
}

fn wallet_derivation_context() -> AgentPermissionMonomorphizationContext {
    AgentPermissionMonomorphizationContext {
        account: AccountEmail::from("alice@example.com"),
        application: ApplicationName::try_from("shop").unwrap(),
        environment: EnvironmentName::try_from("prod").unwrap(),
        component: ComponentName("cart-svc".to_string()),
        agent_name: "cart-1".to_string(),
        owner: RecipientOwnerContext::AgentType(AgentTypeName("CartAgent".to_string())),
    }
}

fn wallet_parent(card_id: u128, grant: &permissions_types::PatternGrant) -> StoredCard {
    StoredCard::Concrete(Card {
        card_id: CardId(Uuid::from_u128(card_id)),
        parent_ids: Vec::new(),
        lower_positive: vec![parse_pattern_grant(grant).unwrap()],
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    })
}

fn scope_derivation_parent(
    card_id: u128,
    lower_positive: &[permissions_types::PatternGrant],
    upper_positive: &[permissions_types::PatternGrant],
) -> ScopeDerivationParent {
    ScopeDerivationParent::persistent(&Card {
        card_id: CardId(Uuid::from_u128(card_id)),
        parent_ids: Vec::new(),
        lower_positive: parse_grants(lower_positive).unwrap(),
        lower_negative: Vec::new(),
        upper_positive: parse_grants(upper_positive).unwrap(),
        upper_negative: Vec::new(),
        created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    })
}

fn grant_for_resource(resource_id: &str) -> permissions_types::PatternGrant {
    permissions_types::PatternGrant {
        resource_id: resource_id.to_string(),
        ..valid_filesystem_grant()
    }
}

#[test]
fn scope_derivation_validates_both_attenuation_bounds() {
    let parent_grant = valid_filesystem_grant();
    let child_grant = grant_for_resource("/data/public/**");
    let broader_grant = grant_for_resource("/**");
    let parent = scope_derivation_parent(
        1,
        std::slice::from_ref(&parent_grant),
        std::slice::from_ref(&parent_grant),
    );

    let accepted = parse_derived_grant_sets(
        std::slice::from_ref(&child_grant),
        &[],
        std::slice::from_ref(&child_grant),
        &[],
    )
    .unwrap();
    assert!(build_scope_card(CardId::new(), std::slice::from_ref(&parent), accepted).is_ok());

    let broad_lower = parse_derived_grant_sets(
        std::slice::from_ref(&broader_grant),
        &[],
        std::slice::from_ref(&child_grant),
        &[],
    )
    .unwrap();
    assert!(matches!(
        build_scope_card(CardId::new(), std::slice::from_ref(&parent), broad_lower,),
        Err(permissions_types::PermissionError::LowerBoundTooBroad(_))
    ));

    let broad_upper = parse_derived_grant_sets(
        std::slice::from_ref(&child_grant),
        &[],
        std::slice::from_ref(&broader_grant),
        &[],
    )
    .unwrap();
    assert!(matches!(
        build_scope_card(CardId::new(), &[parent], broad_upper),
        Err(permissions_types::PermissionError::UpperBoundTooBroad(_))
    ));
}

#[test]
fn scope_derivation_preserves_inherited_negatives_on_both_bounds() {
    let parent_positive = valid_filesystem_grant();
    let child_positive = grant_for_resource("/data/public/**");
    let inherited_negative = grant_for_resource("/data/private/**");
    let additional_negative = grant_for_resource("/data/public/restricted/**");
    let mut parent = scope_derivation_parent(
        1,
        std::slice::from_ref(&parent_positive),
        std::slice::from_ref(&parent_positive),
    );
    parent.delegation.lower_negative =
        parse_grants(std::slice::from_ref(&inherited_negative)).unwrap();
    parent.delegation.upper_negative =
        parse_grants(std::slice::from_ref(&inherited_negative)).unwrap();

    let missing_lower_negative = parse_derived_grant_sets(
        std::slice::from_ref(&child_positive),
        &[],
        std::slice::from_ref(&child_positive),
        std::slice::from_ref(&inherited_negative),
    )
    .unwrap();
    assert!(matches!(
        build_scope_card(
            CardId::new(),
            std::slice::from_ref(&parent),
            missing_lower_negative,
        ),
        Err(permissions_types::PermissionError::LowerBoundTooBroad(_))
    ));

    let missing_upper_negative = parse_derived_grant_sets(
        std::slice::from_ref(&child_positive),
        std::slice::from_ref(&inherited_negative),
        std::slice::from_ref(&child_positive),
        &[],
    )
    .unwrap();
    assert!(matches!(
        build_scope_card(
            CardId::new(),
            std::slice::from_ref(&parent),
            missing_upper_negative,
        ),
        Err(permissions_types::PermissionError::UpperBoundTooBroad(_))
    ));

    let retained_and_added = [inherited_negative, additional_negative];
    let attenuated = parse_derived_grant_sets(
        std::slice::from_ref(&child_positive),
        &retained_and_added,
        std::slice::from_ref(&child_positive),
        &retained_and_added,
    )
    .unwrap();
    assert!(build_scope_card(CardId::new(), std::slice::from_ref(&parent), attenuated).is_ok());
}

#[test]
fn scope_derivation_combines_positive_authority_from_multiple_parents() {
    let first_grant = grant_for_resource("/data/first/**");
    let second_grant = grant_for_resource("/data/second/**");
    let first = scope_derivation_parent(1, std::slice::from_ref(&first_grant), &[]);
    let second = scope_derivation_parent(2, std::slice::from_ref(&second_grant), &[]);
    let grants = parse_derived_grant_sets(&[first_grant, second_grant], &[], &[], &[]).unwrap();

    let card = build_scope_card(CardId::new(), &[first, second], grants).unwrap();

    assert_eq!(
        card.root_card_ids,
        vec![CardId(Uuid::from_u128(1)), CardId(Uuid::from_u128(2))]
    );
    assert_eq!(card.lower_positive.len(), 2);
}

#[test]
fn scope_card_expires_with_its_earliest_expiring_parent() {
    let grant = valid_filesystem_grant();
    let at = |seconds| DateTime::from_timestamp(seconds, 0).unwrap();
    let mut later = scope_derivation_parent(1, std::slice::from_ref(&grant), &[]);
    later.expires_at = Some(at(1_900_000_000));
    let earlier_scope = ScopeCard {
        scope_card_id: CardId(Uuid::from_u128(10)),
        root_card_ids: vec![CardId(Uuid::from_u128(2))],
        lower_positive: parse_grants(std::slice::from_ref(&grant)).unwrap(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        expires_at: Some(at(1_800_000_000)),
    };
    let unbounded = scope_derivation_parent(3, std::slice::from_ref(&grant), &[]);
    let grants = || parse_derived_grant_sets(std::slice::from_ref(&grant), &[], &[], &[]).unwrap();

    let card = build_scope_card(
        CardId::new(),
        &[
            later.clone(),
            ScopeDerivationParent::scope(&earlier_scope),
            unbounded.clone(),
        ],
        grants(),
    )
    .unwrap();
    assert_eq!(card.expires_at, Some(at(1_800_000_000)));

    let card = build_scope_card(CardId::new(), &[unbounded.clone(), later], grants()).unwrap();
    assert_eq!(card.expires_at, Some(at(1_900_000_000)));

    let card = build_scope_card(CardId::new(), &[unbounded], grants()).unwrap();
    assert_eq!(card.expires_at, None);
}

#[test]
fn scope_derivation_unions_and_deduplicates_roots_across_diamond_parents() {
    let grant = valid_filesystem_grant();
    let first = ScopeCard {
        scope_card_id: CardId(Uuid::from_u128(10)),
        root_card_ids: vec![CardId(Uuid::from_u128(1)), CardId(Uuid::from_u128(2))],
        lower_positive: parse_grants(std::slice::from_ref(&grant)).unwrap(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        expires_at: None,
    };
    let second = ScopeCard {
        scope_card_id: CardId(Uuid::from_u128(11)),
        root_card_ids: vec![CardId(Uuid::from_u128(2)), CardId(Uuid::from_u128(3))],
        ..first.clone()
    };
    let grants = parse_derived_grant_sets(&[grant], &[], &[], &[]).unwrap();

    let card = build_scope_card(
        CardId(Uuid::from_u128(12)),
        &[
            ScopeDerivationParent::scope(&first),
            ScopeDerivationParent::scope(&second),
        ],
        grants,
    )
    .unwrap();

    assert_eq!(
        card.root_card_ids,
        vec![
            CardId(Uuid::from_u128(1)),
            CardId(Uuid::from_u128(2)),
            CardId(Uuid::from_u128(3)),
        ]
    );
}

#[test]
fn scope_card_parent_propagates_roots_and_allows_a_pure_ceiling() {
    let grant = valid_filesystem_grant();
    let parent = scope_derivation_parent(1, std::slice::from_ref(&grant), &[]);
    let first = build_scope_card(
        CardId(Uuid::from_u128(10)),
        &[parent],
        parse_derived_grant_sets(std::slice::from_ref(&grant), &[], &[], &[]).unwrap(),
    )
    .unwrap();
    let ceiling = grant_for_resource("/data/public/**");

    let second = build_scope_card(
        CardId(Uuid::from_u128(11)),
        &[ScopeDerivationParent::scope(&first)],
        parse_derived_grant_sets(&[], &[], &[ceiling], &[]).unwrap(),
    )
    .unwrap();

    assert!(second.lower_positive.is_empty());
    assert_eq!(second.root_card_ids, first.root_card_ids);
}

#[test]
fn explicit_derivation_resolves_a_wallet_template_parent_for_the_caller() {
    let template_grant =
        |grant: &str| golem_common::model::card::parse_polymorphic_permission(grant).unwrap();
    let staging_write = template_grant(
        "filesystem(?agent) @ alice@example.com/shop/staging/cart-svc/CartAgent : write : /**",
    );
    let template = StoredCard::Polymorphic(PolymorphicCard {
        card_id: CardId(Uuid::from_u128(7)),
        parent_ids: Vec::new(),
        lower_positive: vec![
            template_grant(
                "filesystem(?agent) @ alice@example.com/shop/prod/cart-svc/CartAgent : read : /data/**",
            ),
            staging_write.clone(),
            template_grant("secret(?env) @ alice@example.com/shop/prod/*/* : reveal : billing.*"),
        ],
        lower_negative: Vec::new(),
        upper_positive: vec![staging_write],
        upper_negative: Vec::new(),
        created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
    });
    let wallet = [template.clone()];

    let parent = wallet_parent_for_agent(&wallet, &template, &wallet_derivation_context()).unwrap();
    assert_eq!(parent.card_id, template.card_id());
    let resolved = parent
        .lower_positive
        .iter()
        .map(|grant| concrete_pattern_grant(grant).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        resolved.len(),
        2,
        "a grant for other holders must not be resolved against the caller: {resolved:?}"
    );
    assert!(resolved.iter().all(|grant| grant.verb != "write"));
    assert!(resolved.iter().any(|grant| grant.class == "secret"));
    assert_eq!(parent.upper_positive.len(), 1, "ceilings must be kept");

    let StoredCard::Polymorphic(polymorphic) = &template else {
        unreachable!()
    };
    let installed = instantiate_polymorphic_card_for_agent(
        polymorphic,
        &wallet_derivation_context(),
        CardId(Uuid::from_u128(8)),
        DateTime::from_timestamp(1_700_000_001, 0).unwrap(),
    );
    assert_eq!(
        installed.lower_positive.len(),
        2,
        "installing a template must not resolve grants for other holders against the target"
    );

    let own_read = resolved
        .iter()
        .find(|grant| grant.class == "filesystem")
        .unwrap()
        .clone();
    assert!(!own_read.owner.contains('?'));
    assert!(own_read.owner.contains("cart-1"));
    let ceiling = concrete_pattern_grants(&parent.upper_positive).unwrap();
    assert!(
        derive_grant_sets(&parent, std::slice::from_ref(&own_read), &[], &ceiling, &[]).is_ok()
    );
    let requested_with_slot = permissions_types::PatternGrant {
        owner: "?agent".to_string(),
        ..own_read.clone()
    };
    assert!(
        matches!(
            derive_grant_sets(&parent, &[requested_with_slot], &[], &ceiling, &[]),
            Err(permissions_types::PermissionError::LowerBoundTooBroad(_))
        ),
        "a caller-scoped parent cannot delegate the slot to every holder of its agent type"
    );

    let sibling = permissions_types::PatternGrant {
        owner: own_read.owner.replace("cart-1", "cart-2"),
        ..own_read.clone()
    };
    assert!(derive_grant_sets(&parent, &[sibling], &[], &ceiling, &[]).is_err());

    assert!(matches!(
        wallet_parent_for_agent(&[], &template, &wallet_derivation_context()),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
}

#[test]
fn scope_derivation_rejects_a_persistent_parent_outside_the_wallet() {
    let grant = valid_filesystem_grant();
    let wallet = [wallet_parent(1, &grant)];
    let resolved = wallet_parent(2, &grant);

    assert!(matches!(
        persistent_scope_parent_from_wallet(&wallet, &resolved, &wallet_derivation_context(),),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
}

#[test]
fn scope_derivation_requires_the_exact_pinned_parent_snapshot() {
    let grant = valid_filesystem_grant();
    let wallet = [wallet_parent(1, &grant)];
    let mut changed_grant = grant;
    changed_grant.resource_id = "/data/changed/**".to_string();
    let changed_snapshot = wallet_parent(1, &changed_grant);

    assert!(matches!(
        persistent_scope_parent_from_wallet(
            &wallet,
            &changed_snapshot,
            &wallet_derivation_context(),
        ),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
    assert!(
        persistent_scope_parent_from_wallet(&wallet, &wallet[0], &wallet_derivation_context(),)
            .is_ok()
    );
}

#[test]
fn wallet_derivation_returns_the_deterministically_selected_parent() {
    let parent_grant = valid_filesystem_grant();
    let mut child_grant = parent_grant.clone();
    child_grant.resource_id = "/data/public.txt".to_string();
    let wallet = [
        wallet_parent(1, &parent_grant),
        wallet_parent(2, &parent_grant),
    ];
    let grants = parse_derived_grant_sets(&[child_grant], &[], &[], &[]).unwrap();

    let parent =
        select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &grants).unwrap();

    assert_eq!(parent.card_id, CardId(Uuid::from_u128(2)));
}

#[test]
fn wallet_derivation_maps_multi_source_selection_to_typed_error() {
    let mut first = valid_filesystem_grant();
    first.resource_id = "/data/first.txt".to_string();
    let mut second = valid_filesystem_grant();
    second.resource_id = "/data/second.txt".to_string();
    let wallet = [wallet_parent(1, &first), wallet_parent(2, &second)];
    let grants = parse_derived_grant_sets(&[first, second], &[], &[], &[]).unwrap();

    assert!(matches!(
        select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &grants,),
        Err(permissions_types::PermissionError::WalletMultiSourceRequired(_))
    ));
}

#[test]
fn wallet_derivation_rejects_an_empty_active_wallet() {
    let grants = parse_derived_grant_sets(&[], &[], &[], &[]).unwrap();

    assert!(matches!(
        select_wallet_derivation_parent(&[], &wallet_derivation_context(), &grants,),
        Err(permissions_types::PermissionError::NotPermitted(_))
    ));
}

#[test]
fn wallet_derivation_selects_parents_by_raw_recipient_not_holder_projection() {
    let mut grant_for_another_holder = valid_filesystem_grant();
    grant_for_another_holder.recipient =
        "alice@example.com/shop/prod/cart-svc/OtherAgent".to_string();
    let wallet = [wallet_parent(1, &grant_for_another_holder)];
    let grants = parse_derived_grant_sets(
        std::slice::from_ref(&grant_for_another_holder),
        &[],
        &[],
        &[],
    )
    .unwrap();

    let parent = select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &grants)
        .expect("raw recipient delegation must be permitted");

    assert_eq!(parent.card_id, CardId(Uuid::from_u128(1)));
}

#[test]
fn wallet_derivation_treats_negative_lists_as_complete_child_bounds() {
    let positive = valid_filesystem_grant();
    let parent = concrete_parent(&positive);
    let lower_negative = concrete_pattern_grant(&parent.lower_negative[0]).unwrap();
    let upper_negative = concrete_pattern_grant(&parent.upper_negative[0]).unwrap();
    let wallet = [StoredCard::Concrete(parent.clone())];
    let incomplete = parse_derived_grant_sets(
        std::slice::from_ref(&positive),
        &[],
        std::slice::from_ref(&positive),
        std::slice::from_ref(&upper_negative),
    )
    .unwrap();

    assert!(matches!(
        select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &incomplete,),
        Err(permissions_types::PermissionError::LowerBoundTooBroad(_))
    ));

    let complete = parse_derived_grant_sets(
        std::slice::from_ref(&positive),
        std::slice::from_ref(&lower_negative),
        std::slice::from_ref(&positive),
        std::slice::from_ref(&upper_negative),
    )
    .unwrap();
    assert_eq!(
        select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &complete,)
            .unwrap()
            .card_id,
        parent.card_id
    );
}

#[test]
fn derive_grants_retain_positives_and_append_negatives() {
    let retained = valid_filesystem_grant();
    let parent = concrete_parent(&retained);
    let mut lower_negative_to_add = retained.clone();
    lower_negative_to_add.resource_id = "/data/tmp/**".to_string();
    let mut upper_negative_to_add = retained.clone();
    upper_negative_to_add.resource_id = "/data/admin/**".to_string();

    let grants = derive_grant_sets(
        &parent,
        std::slice::from_ref(&retained),
        std::slice::from_ref(&lower_negative_to_add),
        std::slice::from_ref(&retained),
        std::slice::from_ref(&upper_negative_to_add),
    )
    .unwrap()
    .as_concrete()
    .unwrap();

    assert_eq!(grants.lower_positive, parent.lower_positive);
    assert_eq!(grants.upper_positive, parent.upper_positive);
    assert_eq!(
        grants.lower_negative,
        [
            parent.lower_negative.clone(),
            vec![parse_pattern_grant(&lower_negative_to_add).unwrap()]
        ]
        .concat()
    );
    assert_eq!(
        grants.upper_negative,
        [
            parent.upper_negative.clone(),
            vec![parse_pattern_grant(&upper_negative_to_add).unwrap()]
        ]
        .concat()
    );
}

#[test]
fn derive_grants_reject_a_positive_broader_than_the_parent() {
    let parent_grant = valid_filesystem_grant();
    let parent = concrete_parent(&parent_grant);
    let mut broader = parent_grant.clone();
    broader.resource_id = "/**".to_string();

    assert!(matches!(
        derive_grant_sets(
            &parent,
            &[broader],
            &[],
            std::slice::from_ref(&parent_grant),
            &[],
        ),
        Err(permissions_types::PermissionError::LowerBoundTooBroad(_))
    ));
}

#[test]
fn derive_grants_reject_an_upper_positive_broader_than_the_parent() {
    let parent_grant = valid_filesystem_grant();
    let parent = concrete_parent(&parent_grant);
    let mut broader = parent_grant.clone();
    broader.resource_id = "/**".to_string();

    assert!(matches!(
        derive_grant_sets(
            &parent,
            std::slice::from_ref(&parent_grant),
            &[],
            &[broader],
            &[],
        ),
        Err(permissions_types::PermissionError::UpperBoundTooBroad(_))
    ));
}

#[test]
fn parse_pattern_grant_rejects_invalid_structured_fields_after_separator_injection() {
    let grant = permissions_types::PatternGrant {
        class: "filesystem".to_string(),
        owner: "acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : /tmp".to_string(),
        recipient: "not a valid recipient".to_string(),
        verb: "not-a-real-verb".to_string(),
        resource_id: "not a valid resource".to_string(),
    };

    assert!(parse_pattern_grant(&grant).is_err());
}

#[test]
fn parse_pattern_grant_accepts_valid_account_emails_in_paths() {
    let grant = permissions_types::PatternGrant {
        class: "filesystem".to_string(),
        owner: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        verb: "read".to_string(),
        resource_id: "/data/**".to_string(),
    };

    assert!(parse_pattern_grant(&grant).is_ok());
}

#[test]
fn parse_pattern_grant_accepts_valid_agent_owner_type_wildcard() {
    let grant = permissions_types::PatternGrant {
        class: "filesystem".to_string(),
        owner: "alice@example.com/shop/prod/cart-svc/CartAgent(*)".to_string(),
        recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        verb: "read".to_string(),
        resource_id: "/data/**".to_string(),
    };

    assert!(parse_pattern_grant(&grant).is_ok());
}

#[test]
fn concrete_pattern_grant_preserves_valid_structured_fields_with_delimiters() {
    let grant = permissions_types::PatternGrant {
        class: "filesystem".to_string(),
        owner: "alice@example.com/shop/prod/cart-svc/CartAgent(\"id) @ spoof : read : tmp\")"
            .to_string(),
        recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        verb: "read".to_string(),
        resource_id: "/data/**".to_string(),
    };

    let pattern = parse_pattern_grant(&grant).expect("structured grant should be valid");
    let rendered = concrete_pattern_grant(&pattern).expect("valid pattern should inspect");

    assert_eq!(rendered.class, grant.class);
    assert_eq!(rendered.owner, grant.owner);
    assert_eq!(rendered.recipient, grant.recipient);
    assert_eq!(rendered.verb, grant.verb);
    assert_eq!(rendered.resource_id, grant.resource_id);
}

#[test]
fn concrete_pattern_grant_preserves_multi_segment_tool_command() {
    let grant = permissions_types::PatternGrant {
        class: "tool".to_string(),
        owner: "alice@example.com/shop/prod/cli-tools/grep".to_string(),
        recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        verb: "invoke".to_string(),
        resource_id:
            "search.files --pattern=* --path=src/** --ignore-case --line-number -- README.md"
                .to_string(),
    };

    let pattern = parse_pattern_grant(&grant).expect("structured grant should be valid");
    let inspected = concrete_pattern_grant(&pattern).expect("valid pattern should inspect");
    let reparsed = parse_pattern_grant(&inspected).expect("an inspected grant should remain valid");

    assert_eq!(reparsed, pattern);
}

#[test]
fn parse_pattern_grant_rejects_verb_suffix_that_completes_resource_separator() {
    let grant = permissions_types::PatternGrant {
        class: "config".to_string(),
        owner: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
        verb: "read :".to_string(),
        resource_id: "ignored".to_string(),
    };

    assert!(matches!(
        parse_pattern_grant(&grant),
        Err(permissions_types::PermissionError::InvalidVerb(verb)) if verb == "read :"
    ));
}

fn lifespan(value: &str) -> permissions_types::Duration {
    permissions_types::Duration {
        iso_8601: value.to_string(),
    }
}

#[test]
fn derive_expiry_parses_fixed_iso_8601_duration() {
    let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();

    assert_eq!(
        derive_expires_at(now, Some(&lifespan("P1DT2H3M4.5S")), None).unwrap(),
        Some(
            now + TimeDelta::days(1)
                + TimeDelta::hours(2)
                + TimeDelta::minutes(3)
                + TimeDelta::seconds(4)
                + TimeDelta::milliseconds(500)
        )
    );
    assert_eq!(
        derive_expires_at(now, Some(&lifespan("PT0S")), None).unwrap(),
        Some(now)
    );
}

#[test]
fn derive_expiry_rejects_non_fixed_or_malformed_duration() {
    for value in ["1s", "-PT1S", "P1Y", "P1M", "P1W", "PT", ""] {
        assert!(
            matches!(
                derive_expires_at(
                    DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                    Some(&lifespan(value)),
                    None,
                ),
                Err(permissions_types::PermissionError::InvalidPattern(_))
            ),
            "{value} must be rejected"
        );
    }
}

#[test]
fn derive_expiry_enforces_parent_expiry() {
    let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let parent_expires_at = now + TimeDelta::hours(1);

    assert_eq!(
        derive_expires_at(now, Some(&lifespan("PT1H")), Some(parent_expires_at),).unwrap(),
        Some(parent_expires_at)
    );
    assert!(matches!(
        derive_expires_at(
            now,
            Some(&lifespan("PT1H0.000000001S")),
            Some(parent_expires_at),
        ),
        Err(permissions_types::PermissionError::CardExpired(_))
    ));
    assert!(matches!(
        derive_expires_at(now, None, Some(parent_expires_at)),
        Err(permissions_types::PermissionError::CardExpired(_))
    ));
    assert!(matches!(
        derive_expires_at(now, Some(&lifespan("PT0S")), Some(now)),
        Err(permissions_types::PermissionError::CardExpired(_))
    ));
}

#[test]
fn derive_expiry_handles_indefinite_parent_and_timestamp_overflow() {
    let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();

    assert_eq!(derive_expires_at(now, None, None).unwrap(), None);
    assert!(matches!(
        derive_expires_at(
            DateTime::<Utc>::MAX_UTC,
            Some(&lifespan("PT0.000000001S")),
            None,
        ),
        Err(permissions_types::PermissionError::InvalidPattern(_))
    ));
}

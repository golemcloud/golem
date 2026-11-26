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

#[test]
fn component_external_tool_owner_recipient_roundtrips_and_agent_substitution_is_scoped() {
    use crate::model::account::AccountEmail;
    use crate::model::agent::AgentTypeName;
    use crate::model::application::ApplicationName;
    use crate::model::card::recipient::{
        PolymorphicRecipientPattern, RecipientMonomorphizationContext, RecipientOwnerContext,
        RecipientPattern,
    };
    use crate::model::component::ComponentName;
    use crate::model::environment::EnvironmentName;

    let external =
        RecipientPattern::parse("owner@example.com/app/prod/component/~external-tool-owner")
            .unwrap();
    assert_eq!(
        external.render(),
        "owner@example.com/app/prod/component/~external-tool-owner"
    );

    let base = RecipientMonomorphizationContext {
        account: AccountEmail::new("owner@example.com"),
        application: ApplicationName("app".to_string()),
        environment: EnvironmentName("prod".to_string()),
        component: ComponentName("component".to_string()),
        owner: RecipientOwnerContext::ComponentExternalToolOwner,
    };
    assert_eq!(
        PolymorphicRecipientPattern::parse("?agent")
            .unwrap()
            .monomorphize(&base),
        external
    );

    let real = RecipientMonomorphizationContext {
        owner: RecipientOwnerContext::AgentType(AgentTypeName("real".to_string())),
        ..base
    };
    assert!(matches!(
        PolymorphicRecipientPattern::parse("?agent").unwrap().monomorphize(&real),
        RecipientPattern::Agent { agent_type, .. } if agent_type.0 == "real"
    ));
}
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::component::{ComponentId, ComponentRevision};
use crate::base_model::environment::EnvironmentId;
use crate::base_model::{AgentId, InvocationId, OplogIndex};
use crate::model::card::owner::{
    AgentOwnerLeafPattern, AgentOwnerPattern, EnvironmentOwnerPattern,
};
use crate::model::card::recipient::RecipientPattern;
use RecipientPattern as AccountRecipientPattern;
use RecipientPattern as AgentRecipientPattern;
use RecipientPattern as EnvironmentRecipientPattern;
use jiff::Timestamp as JiffTimestamp;
use test_r::test;
use uuid::uuid;

#[derive(desert_rust::BinaryCodec)]
enum LegacyCardManagedBy {
    AccountRoot(CardManagedByAccountRoot),
    EnvironmentDefault(CardManagedByEnvironmentDefault),
    PermissionShare(CardManagedByPermissionShare),
    AgentInitial(CardManagedByAgentInitial),
}

#[test]
fn runtime_derived_provenance_preserves_existing_managed_by_encoding() {
    let account_root = CardManagedByAccountRoot {
        account_id: AccountId(uuid!("00112233-4455-6677-8899-aabbccddeeff")),
    };
    let environment_default = CardManagedByEnvironmentDefault {
        environment_id: EnvironmentId(uuid!("11223344-5566-7788-99aa-bbccddeeff00")),
    };
    let permission_share = CardManagedByPermissionShare {
        permission_share_id: uuid!("22334455-6677-8899-aabb-ccddeeff0011"),
    };
    let agent_initial = CardManagedByAgentInitial {
        component_id: ComponentId(uuid!("33445566-7788-99aa-bbcc-ddeeff001122")),
        component_revision: ComponentRevision::new(7).unwrap(),
        agent_type: AgentTypeName("cart".to_string()),
    };
    let cases = [
        (
            LegacyCardManagedBy::AccountRoot(account_root.clone()),
            CardManagedBy::AccountRoot(account_root),
        ),
        (
            LegacyCardManagedBy::EnvironmentDefault(environment_default.clone()),
            CardManagedBy::EnvironmentDefault(environment_default),
        ),
        (
            LegacyCardManagedBy::PermissionShare(permission_share.clone()),
            CardManagedBy::PermissionShare(permission_share),
        ),
        (
            LegacyCardManagedBy::AgentInitial(agent_initial.clone()),
            CardManagedBy::AgentInitial(agent_initial),
        ),
    ];

    for (legacy, expected) in cases {
        let bytes = crate::serialization::serialize(&legacy).unwrap();
        let decoded: CardManagedBy = crate::serialization::deserialize(&bytes).unwrap();
        assert_eq!(decoded, expected);
        assert_eq!(
            bytes,
            crate::serialization::serialize(&expected).unwrap(),
            "adding RuntimeDerived must not renumber existing union variants"
        );
    }
}

#[test]
fn runtime_derived_card_provenance_binary_roundtrip() {
    let provenance = CardManagedBy::RuntimeDerived(CardManagedByRuntimeDerived {
        environment_id: EnvironmentId(uuid!("00112233-4455-6677-8899-aabbccddeeff")),
        agent_id: AgentId::from_parts(7, 1),
        invocation_key: InvocationId(uuid!("01234567-89ab-cdef-fedc-ba9876543210")),
        oplog_index: OplogIndex::from_u64(42),
    });

    let bytes = crate::serialization::serialize(&provenance).unwrap();
    let decoded: CardManagedBy = crate::serialization::deserialize(&bytes).unwrap();

    assert_eq!(decoded, provenance);
}

#[test]
fn card_holder_wallet_identity_has_stable_canonical_encoding() {
    let wallet_uuid = uuid!("00112233-4455-6677-8899-aabbccddeeff");

    let account = CardHolder::Account(AccountCardHolder {
        account_id: wallet_uuid,
    });
    let application = CardHolder::Application(ApplicationCardHolder {
        application_id: wallet_uuid,
    });
    let agent = CardHolder::Agent(AgentCardHolder {
        environment_id: EnvironmentId(uuid!("ffeeddcc-bbaa-4988-8776-655443322110")),
        agent_id: AgentId::from_parts(7, 2),
    });

    assert_eq!(
        account.canonical_wallet_id_bytes(),
        [
            b"golem:permissions:wallet-id:v2\0".as_slice(),
            &[0],
            wallet_uuid.as_bytes(),
        ]
        .concat()
    );
    assert_eq!(
        application.canonical_wallet_id_bytes(),
        [
            b"golem:permissions:wallet-id:v2\0".as_slice(),
            &[1],
            wallet_uuid.as_bytes(),
        ]
        .concat()
    );
    assert_eq!(
        agent.canonical_wallet_id_bytes(),
        [
            b"golem:permissions:wallet-id:v2\0".as_slice(),
            &[2],
            uuid!("ffeeddcc-bbaa-4988-8776-655443322110").as_bytes(),
            uuid!("00000007-0000-8000-8000-000000000002").as_bytes(),
        ]
        .concat()
    );
}

#[test]
fn card_holder_wallet_id_hash_has_golden_vectors() {
    let wallet_uuid = uuid!("00112233-4455-6677-8899-aabbccddeeff");

    let account = CardHolder::Account(AccountCardHolder {
        account_id: wallet_uuid,
    });
    let application = CardHolder::Application(ApplicationCardHolder {
        application_id: wallet_uuid,
    });
    let agent = CardHolder::Agent(AgentCardHolder {
        environment_id: EnvironmentId(uuid!("ffeeddcc-bbaa-4988-8776-655443322110")),
        agent_id: AgentId::from_parts(7, 2),
    });

    assert_eq!(
        account.wallet_id_hash(),
        *blake3::Hash::from_hex("18c16764e50881c125d9e4cff1ba00f6ef3eb869e58b3e907c888d8200a803bb")
            .unwrap()
            .as_bytes()
    );
    assert_eq!(
        application.wallet_id_hash(),
        *blake3::Hash::from_hex("f354dc05ef022b933b689de37def42a4b03af5eb9b5bef7f902bd7146a5c20b2")
            .unwrap()
            .as_bytes()
    );
    assert_eq!(
        agent.wallet_id_hash(),
        *blake3::Hash::from_hex("2e9e7a24ed79b13705d40d5d49d81f1c0274f3cbcbf1bc6783a98dc4c0aadb89")
            .unwrap()
            .as_bytes()
    );
}

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
        created_at: JiffTimestamp::now(),
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
        "acme/shop/prod/cart/00000007-0000-8000-8000-000000000001",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::GlobStar,
            ],
        }),
    );
    let read_secret_target = fs_target(
        "acme/shop/prod/cart/00000007-0000-8000-8000-000000000001",
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::Literal("secret.txt".to_string()),
            ],
        }),
    );
    let read_public = fs(
        "acme/shop/prod/cart/00000007-0000-8000-8000-000000000001",
        "acme/shop/prod/cart/agent",
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::Literal("public.txt".to_string()),
            ],
        }),
    );
    let read_public_target = fs_target(
        "acme/shop/prod/cart/00000007-0000-8000-8000-000000000001",
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
    let owner = "acme/shop/prod/cart/00000007-0000-8000-8000-000000000001";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let read_tmp = fs(
        owner,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("tmp".to_string()),
                FilesystemPathSegmentPattern::Literal("file.txt".to_string()),
            ],
        }),
    );
    let read_tmp_target = fs_target(
        owner,
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
    let ceiling_surface = DelegationSurface::from_cards(std::slice::from_ref(&ceiling));

    assert!(surface.authorize(&read_tmp_target).unwrap());
    assert!(!surface.authorize(&reveal_secret_target).unwrap());
    assert!(
        ceiling_surface
            .validate_attenuation(
                &[],
                &[],
                std::slice::from_ref(&read_tmp),
                std::slice::from_ref(&reveal_secret),
            )
            .is_ok()
    );
    assert!(
        ceiling_surface
            .validate_attenuation(
                &[],
                &[],
                std::slice::from_ref(&reveal_secret),
                std::slice::from_ref(&reveal_secret),
            )
            .is_ok()
    );
}

#[test]
fn lower_negative_is_scoped_to_its_card() {
    let holder = "acme/shop/prod/cart/agent";
    let owner = "acme/shop/prod/cart/00000007-0000-8000-8000-000000000001";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let read_all = fs(
        owner,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::GlobStar,
            ],
        }),
    );
    let read_secret = fs(
        owner,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::Literal("secret.txt".to_string()),
            ],
        }),
    );
    let read_secret_target = fs_target(
        owner,
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
    let owner = "acme/shop/prod/cart/00000007-0000-8000-8000-000000000001";
    let read_all = fs(
        owner,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("data".to_string()),
                FilesystemPathSegmentPattern::GlobStar,
            ],
        }),
    );
    let read_secret = fs(
        owner,
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
    let parent_surface = DelegationSurface::from_cards(&[parent_allow, parent_deny]);

    assert!(matches!(
        parent_surface.validate_attenuation(&[], &[], std::slice::from_ref(&read_secret), &[],),
        Err(CardAlgebraError::UpperDenialNotKept { denial }) if *denial == read_secret
    ));
    assert!(
        parent_surface
            .validate_attenuation(
                &[],
                &[],
                std::slice::from_ref(&read_secret),
                std::slice::from_ref(&read_secret),
            )
            .is_ok()
    );
}

#[test]
fn derivation_witness_excludes_unrelated_parent() {
    let holder = "acme/shop/prod/cart/agent";
    let owner = "acme/shop/prod/cart/00000007-0000-8000-8000-000000000001";
    let read_tmp = fs(
        owner,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("tmp".to_string()),
                FilesystemPathSegmentPattern::GlobStar,
            ],
        }),
    );
    let reveal_secret = secret_reveal("acme/shop/prod", holder, SecretResourcePattern::Any);

    let needed_parent = card(vec![read_tmp.clone()], Vec::new());
    let needed_parent_id = needed_parent.card_id;
    let unrelated_parent = card(vec![reveal_secret], Vec::new());
    let surface = DelegationSurface::from_cards(&[needed_parent, unrelated_parent]);

    let witness = surface
        .validate_attenuation(std::slice::from_ref(&read_tmp), &[], &[], &[])
        .unwrap();

    assert_eq!(witness, vec![needed_parent_id]);
}

#[test]
fn derivation_witness_includes_multiple_needed_parents() {
    let holder = "acme/shop/prod/cart/agent";
    let owner = "acme/shop/prod/cart/00000007-0000-8000-8000-000000000001";
    let read_tmp = fs(
        owner,
        holder,
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![
                FilesystemPathSegmentPattern::Literal("tmp".to_string()),
                FilesystemPathSegmentPattern::GlobStar,
            ],
        }),
    );
    let reveal_secret = secret_reveal("acme/shop/prod", holder, SecretResourcePattern::Any);

    let filesystem_parent = card(vec![read_tmp.clone()], Vec::new());
    let filesystem_parent_id = filesystem_parent.card_id;
    let secret_parent = card(vec![reveal_secret.clone()], Vec::new());
    let secret_parent_id = secret_parent.card_id;
    let surface = DelegationSurface::from_cards(&[filesystem_parent, secret_parent]);

    let witness = surface
        .validate_attenuation(&[read_tmp, reveal_secret], &[], &[], &[])
        .unwrap();

    assert_eq!(witness, vec![filesystem_parent_id, secret_parent_id]);
}

#[test]
fn a_type_wildcard_denial_applies_to_an_agent_without_its_type_name() {
    let holder = "acme/shop/prod/cart/Cart";
    let recipient = RecipientPattern::parse(holder).unwrap();
    let data = || {
        FilesystemResourcePattern::Path(FilesystemPathPattern {
            segments: vec![FilesystemPathSegmentPattern::Literal("data".to_string())],
        })
    };
    let target = |agent: &str, agent_type: Option<&str>| {
        let mut target = fs_target(&format!("acme/shop/prod/cart/{agent}"), data());
        if let PermissionTarget::Filesystem(target) = &mut target
            && let AgentOwnerPattern::Agent {
                agent: AgentOwnerLeafPattern::Agent(leaf),
                ..
            } = &mut target.owner
        {
            leaf.agent_type = agent_type.map(|name| AgentTypeName(name.to_string()));
        }
        target
    };
    let typed_agent = "00000007-0000-8000-8000-000000000001";
    let reserved_owner = "00000000-0000-8000-8000-000000000001";

    let deny_carts = card_with_bounds(
        vec![fs("acme/shop/prod/cart/*", holder, data())],
        vec![fs("acme/shop/prod/cart/Cart(*)", holder, data())],
        Vec::new(),
        Vec::new(),
    );
    let surface = EffectiveSurface::from_cards(&[deny_carts], &recipient).unwrap();
    assert!(
        !surface
            .authorize(&target(typed_agent, Some("Cart")))
            .unwrap()
    );
    assert!(!surface.authorize(&target(typed_agent, None)).unwrap());
    assert!(
        surface
            .authorize(&target(typed_agent, Some("Order")))
            .unwrap()
    );
    assert!(surface.authorize(&target(reserved_owner, None)).unwrap());

    let grant_carts = card(
        vec![fs("acme/shop/prod/cart/Cart(*)", holder, data())],
        Vec::new(),
    );
    let surface = EffectiveSurface::from_cards(&[grant_carts], &recipient).unwrap();
    assert!(
        surface
            .authorize(&target(typed_agent, Some("Cart")))
            .unwrap()
    );
    assert!(!surface.authorize(&target(typed_agent, None)).unwrap());
}

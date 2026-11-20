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
use crate::base_model::caller::test_support::{
    CLAIMS_JSON, ISSUER, account_principal, every_principal, sample_caller, tool_principal,
    user_principal,
};
use crate::base_model::caller::{Claims, UserPrincipal};
use crate::base_model::component::ComponentId;
use pretty_assertions::assert_eq;
use prost::Message;
use test_r::test;

#[test]
fn public_caller_keeps_both_roles_and_tokens_and_hides_claims() {
    let caller = sample_caller();
    let hidden = PublicCaller::new(&caller, ClaimVisibility::Hidden);

    let PublicPrincipal::User(user) = &hidden.original else {
        panic!("expected a user at the root, got {:?}", hidden.original);
    };
    assert_eq!(user.pair, Pair::new("https://idp.example/", "user-1"));
    assert_eq!(
        user.authenticated_at,
        "2026-01-02T03:04:05.678Z".parse::<Timestamp>().unwrap()
    );
    assert_eq!(user.claims, None);

    let PublicPrincipal::Agent(agent) = &hidden.immediate else {
        panic!(
            "expected an agent as the immediate caller, got {:?}",
            hidden.immediate
        );
    };
    assert_eq!(Some(agent.pair.clone()), caller.immediate.pair());
    assert_eq!(
        agent.agent_id,
        AgentId {
            component_id: ComponentId(Uuid::from_u128(0xc0)),
            agent_id: "Relay(\"r\")".to_string(),
        }
    );
    assert_eq!(agent.agent_type, "Relay");
    assert_eq!(agent.environment_id, EnvironmentId(Uuid::from_u128(0xe0)));

    assert_eq!(
        hidden.tokens,
        vec![
            PublicTokenProvenance {
                id: Uuid::from_u128(0x70c1),
                maker: Some(Pair::new(ISSUER, "account:1")),
            },
            PublicTokenProvenance {
                id: Uuid::from_u128(0x70c2),
                maker: None,
            },
        ]
    );

    let json = serde_json::to_value(&hidden).unwrap();
    assert_eq!(json["original"]["kind"], "user");
    assert_eq!(json["immediate"]["kind"], "agent");
    assert!(json["original"].get("claims").is_none());

    let shown = PublicCaller::new(&caller, ClaimVisibility::Shown);
    assert_eq!(
        shown.original.claims().map(PublicClaims::as_json),
        Some(CLAIMS_JSON)
    );
    assert_eq!(shown.immediate, hidden.immediate);
    assert_eq!(shown.tokens, hidden.tokens);
    let json = serde_json::to_value(&shown).unwrap();
    assert_eq!(json["original"]["claims"]["email"], "a@b.example");

    let PublicPrincipal::Account(account) =
        PublicPrincipal::new(&account_principal(), ClaimVisibility::Shown)
    else {
        panic!("expected an account");
    };
    assert_eq!(
        account.pair,
        Pair::new(ISSUER, format!("account:{}", account.account_id))
    );
    assert_eq!(account.authentication, AccountAuthentication::SignIn);

    let PublicPrincipal::Tool(tool) =
        PublicPrincipal::new(&tool_principal(), ClaimVisibility::Shown)
    else {
        panic!("expected a tool");
    };
    assert_eq!(Some(tool.pair.clone()), tool_principal().pair());
    assert_eq!(tool.tool_name, "grep");
    assert_eq!(tool.environment_id, EnvironmentId(Uuid::from_u128(0xe0)));

    for principal in every_principal() {
        let public = PublicPrincipal::new(&principal, ClaimVisibility::Hidden);
        assert_eq!(public.kind(), principal.kind());
        assert_eq!(public.pair().cloned(), principal.pair());
    }
}

/// A user whose claims are mostly one long string, so that their size dominates an encoding.
fn user_with_large_claims() -> Principal {
    let padding = "x".repeat(4096);
    Principal::User(UserPrincipal {
        pair: Pair::new("http://test-idp.com", "bob"),
        claims: Claims::from_json(format!(r#"{{"sub":"bob","padding":"{padding}"}}"#)).unwrap(),
        authenticated_at: Timestamp::from(1_000),
    })
}

#[test]
fn public_caller_shows_an_entry_caller_in_both_roles() {
    let caller = Caller::entry(user_with_large_claims());

    for claims in [ClaimVisibility::Hidden, ClaimVisibility::Shown] {
        let public = PublicCaller::new(&caller, claims);
        assert_eq!(public.original, public.immediate);

        let json = serde_json::to_value(&public).unwrap();
        assert_eq!(json["original"]["pair"]["subject"], "bob");
        assert_eq!(json["original"], json["immediate"]);

        let proto = golem_api_grpc::proto::golem::component::PublicCaller::from(public.clone());
        assert!(proto.original.is_some());
        assert!(proto.immediate.is_none());
        assert_eq!(PublicCaller::try_from(proto).unwrap(), public);
    }

    // Shown claims cross the wire once: the encoding grows by about one copy of the claims.
    let hidden = golem_api_grpc::proto::golem::component::PublicCaller::from(PublicCaller::new(
        &caller,
        ClaimVisibility::Hidden,
    ))
    .encoded_len();
    let shown = golem_api_grpc::proto::golem::component::PublicCaller::from(PublicCaller::new(
        &caller,
        ClaimVisibility::Shown,
    ))
    .encoded_len();
    assert!(shown > hidden + 4096, "hidden {hidden}, shown {shown}");
    assert!(shown < hidden + 2 * 4096, "hidden {hidden}, shown {shown}");

    // Different roles both cross the wire.
    let relayed = Caller {
        original: user_principal(),
        immediate: account_principal(),
        tokens: Vec::new(),
    };
    let public = PublicCaller::new(&relayed, ClaimVisibility::Shown);
    let proto = golem_api_grpc::proto::golem::component::PublicCaller::from(public.clone());
    assert!(proto.immediate.is_some());
    assert_eq!(PublicCaller::try_from(proto).unwrap(), public);
}

#[test]
fn public_caller_protobuf_refuses_a_missing_original_or_claims_that_are_not_an_object() {
    let public = PublicCaller::new(&sample_caller(), ClaimVisibility::Shown);
    let mut proto = golem_api_grpc::proto::golem::component::PublicCaller::from(public);

    let mut without_original = proto.clone();
    without_original.original = None;
    assert!(PublicCaller::try_from(without_original).is_err());

    let Some(golem_api_grpc::proto::golem::component::public_principal::Value::User(user)) =
        proto.original.as_mut().unwrap().value.as_mut()
    else {
        panic!("expected a user at the root");
    };
    user.claims = Some("[1,2]".to_string());
    assert!(PublicCaller::try_from(proto).is_err());
}

#[test]
fn public_principal_display_never_shows_claims() {
    let texts: Vec<String> = every_principal()
        .iter()
        .map(|principal| PublicPrincipal::new(principal, ClaimVisibility::Shown).to_string())
        .collect();
    assert_eq!(
        texts,
        vec![
            r#"user ("https://idp.example/","user-1")"#.to_string(),
            format!(
                r#"account ("{ISSUER}","{}")"#,
                account_principal().pair().unwrap().subject
            ),
            format!(
                r#"agent ("{ISSUER}","{}")"#,
                every_principal()[2].pair().unwrap().subject
            ),
            format!(
                r#"tool ("{ISSUER}","{}")"#,
                tool_principal().pair().unwrap().subject
            ),
            "anonymous".to_string(),
        ]
    );
    for text in &texts {
        assert!(!text.contains("a@b.example"), "{text}");
    }
}

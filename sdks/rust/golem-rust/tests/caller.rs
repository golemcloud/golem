// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

test_r::enable!();

use golem_rust::agentic::{Caller, InvocationContext, Principal, PrincipalKind, PrincipalRef};
use golem_rust::golem_agentic::golem::agent::common as wit;
use golem_rust::schema::wit::wire;
use test_r::test;

const TEXT_FORM_VECTORS: &str =
    include_str!("../../../../test-data/caller-identity/text-form-v1.json");

#[test]
fn principal_ref_text_form_matches_the_shared_vectors() {
    let vectors: serde_json::Value = serde_json::from_str(TEXT_FORM_VECTORS).unwrap();
    let round_trip = vectors["roundTrip"].as_array().unwrap();
    assert!(!round_trip.is_empty());
    for case in round_trip {
        let principal_ref = PrincipalRef::new(
            case["issuer"].as_str().unwrap(),
            case["subject"].as_str().unwrap(),
        );
        let text = case["text"].as_str().unwrap();
        assert_eq!(principal_ref.to_string(), text, "{}", case["name"]);
        assert_eq!(
            PrincipalRef::parse(text).unwrap(),
            principal_ref,
            "{}",
            case["name"]
        );
        assert_eq!(text.parse::<PrincipalRef>().unwrap(), principal_ref);
    }
    let reject = vectors["reject"].as_array().unwrap();
    assert!(!reject.is_empty());
    for case in reject {
        let text = case["text"].as_str().unwrap();
        assert!(PrincipalRef::parse(text).is_err(), "{}", case["name"]);
    }
}

#[test]
fn principal_ref_equality_is_byte_equality_of_both_strings() {
    let base = PrincipalRef::new("https://login.example/", "Subject-1");
    assert!(base.equals(&PrincipalRef::new("https://login.example/", "Subject-1")));
    assert!(!base.equals(&PrincipalRef::new("https://login.example", "Subject-1")));
    assert!(!base.equals(&PrincipalRef::new("https://login.example/", "subject-1")));
    assert!(!base.equals(&PrincipalRef::new("https://login.example/", "Subject-1 ")));
}

fn uuid(high_bits: u64) -> wire::Uuid {
    wire::Uuid {
        high_bits,
        low_bits: 7,
    }
}

fn pair(issuer: &str, subject: &str) -> wit::PrincipalRef {
    wit::PrincipalRef {
        issuer: issuer.to_string(),
        subject: subject.to_string(),
    }
}

fn user(subject: &str, claims: &str) -> wit::Principal {
    wit::Principal::User(wit::UserPrincipal {
        pair: pair("https://login.example", subject),
        claims: claims.to_string(),
        authenticated_at: wire::Datetime {
            seconds: 1_700_000_000,
            nanoseconds: 5,
        },
    })
}

fn account() -> wit::Principal {
    wit::Principal::Account(wit::AccountPrincipal {
        pair: pair("https://golem.example", "account:1"),
        account_id: wire::AccountId { uuid: uuid(1) },
        authentication: wit::AccountAuthentication::ApiToken,
    })
}

fn agent() -> wit::Principal {
    wit::Principal::Agent(wit::AgentPrincipal {
        pair: pair("https://golem.example", "agent:2/3"),
        agent_id: wire::AgentId {
            component_id: wire::ComponentId { uuid: uuid(2) },
            agent_id: "Relay(\"r\")".to_string(),
        },
        agent_type: "Relay".to_string(),
        environment_id: wire::EnvironmentId { uuid: uuid(4) },
    })
}

fn tool() -> wit::Principal {
    wit::Principal::Tool(wit::ToolPrincipal {
        pair: pair("https://golem.example", "tool:4/report"),
        environment_id: wire::EnvironmentId { uuid: uuid(4) },
        tool_name: "report".to_string(),
    })
}

#[test]
fn every_principal_kind_reads_its_details_and_prints_without_claims() {
    let claims = r#"{"role":"admin","note":"secret"}"#;
    let user = Principal::from(user("u-1", claims));
    assert_eq!(user.kind(), PrincipalKind::User);
    assert_eq!(user.to_string(), r#"user ("https://login.example","u-1")"#);
    assert!(!user.to_string().contains("secret"));
    let Principal::User(details) = &user else {
        panic!("a user");
    };
    assert_eq!(details.issuer(), "https://login.example");
    assert_eq!(details.subject(), "u-1");
    assert_eq!(details.claims_json(), claims);
    assert_eq!(details.claims().unwrap()["role"], "admin");
    assert_eq!(
        details
            .authenticated_at()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap(),
        std::time::Duration::new(1_700_000_000, 5)
    );

    let account = Principal::from(account());
    assert_eq!(account.kind(), PrincipalKind::Account);
    assert_eq!(
        account.to_string(),
        r#"account ("https://golem.example","account:1")"#
    );

    let agent = Principal::from(agent());
    assert_eq!(agent.kind(), PrincipalKind::Agent);
    let Principal::Agent(details) = &agent else {
        panic!("an agent");
    };
    assert_eq!(details.agent_type(), "Relay");
    assert_eq!(details.agent_id().agent_id, "Relay(\"r\")");

    let tool = Principal::from(tool());
    assert_eq!(tool.kind(), PrincipalKind::Tool);
    let Principal::Tool(details) = &tool else {
        panic!("a tool");
    };
    assert_eq!(details.tool_name(), "report");

    let anonymous = Principal::from(wit::Principal::Anonymous);
    assert_eq!(anonymous.kind(), PrincipalKind::Anonymous);
    assert_eq!(anonymous.to_string(), "anonymous");
    assert!(anonymous.principal_ref().is_none());
}

#[test]
fn principal_equality_is_identity_and_anonymous_equals_nothing() {
    let first = Principal::from(user("u-1", "{}"));
    let same_identity_new_claims = Principal::from(user("u-1", r#"{"iat":2}"#));
    let other = Principal::from(user("u-2", "{}"));
    assert!(first.equals(&same_identity_new_claims));
    assert!(!first.equals(&other));
    assert!(first.equals_ref(&PrincipalRef::new("https://login.example", "u-1")));

    let anonymous = Principal::from(wit::Principal::Anonymous);
    assert!(!anonymous.equals(&anonymous));
    assert!(!anonymous.equals(&first));
    assert!(!first.equals(&anonymous));
}

#[test]
fn a_caller_reads_both_roles_and_the_token_provenance() {
    let maker = pair("https://golem.example", "account:1");
    let caller = Caller::from(wit::Caller {
        original: user("u-1", "{}"),
        immediate: agent(),
        tokens: vec![
            wit::TokenProvenance {
                id: uuid(9),
                maker: Some(maker),
            },
            wit::TokenProvenance {
                id: uuid(10),
                maker: None,
            },
        ],
    });
    assert_eq!(caller.original().kind(), PrincipalKind::User);
    assert_eq!(caller.immediate().kind(), PrincipalKind::Agent);
    assert_eq!(caller.tokens().len(), 2);
    assert_eq!(
        caller.tokens()[0].maker(),
        Some(&PrincipalRef::new("https://golem.example", "account:1"))
    );
    assert!(caller.tokens()[1].maker().is_none());

    let context = InvocationContext::from_caller(caller.clone());
    assert_eq!(context.caller(), &caller);
    assert_eq!(
        InvocationContext::anonymous().caller().original().kind(),
        PrincipalKind::Anonymous
    );
}

#[test]
fn a_principal_ref_is_the_named_record_principal_ref_as_data() {
    use golem_rust::schema::{SchemaType, try_into_schema_graph};
    let graph = try_into_schema_graph::<PrincipalRef>().unwrap();
    let definition = &graph.defs[0];
    assert_eq!(definition.id.as_str(), "principal-ref");
    let SchemaType::Record { fields, .. } = &definition.body else {
        panic!("a record");
    };
    assert_eq!(
        fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        ["issuer", "subject"]
    );

    let original = PrincipalRef::new("https://login.example", "u-1");
    let encoded = golem_rust::schema::wit::direct::encode(&original).unwrap();
    assert_eq!(
        golem_rust::schema::wit::direct::decode::<PrincipalRef>(encoded).unwrap(),
        original
    );
}

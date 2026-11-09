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

use super::{
    AccountAuthentication, AccountPrincipal, AgentPrincipal, Caller, Claims, GolemSubject, Kind,
    Pair, Principal, PrincipalRef, TokenProvenance, ToolPrincipal, UserPrincipal,
};
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool::ToolName;
use crate::base_model::{AgentFingerprint, Timestamp};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write;
use std::str::FromStr;
use test_r::test;
use uuid::Uuid;

const ISSUER: &str = "https://id.golem.acme.example";
const AGENT_ID: &str = "0f3a5c21-8d4e-8b6f-9a1c-2e7d4b6a9c10";
const FINGERPRINT: &str = "9b2d4e6f-1a3c-4d5e-8f70-6a8b9c0d1e2f";
const FINGERPRINT_2: &str = "c1d2e3f4-a5b6-4c7d-8e9f-0a1b2c3d4e5f";
const ENVIRONMENT: &str = "5e7f9a1b-2c3d-4e5f-8a6b-7c8d9e0f1a2b";
const ACCOUNT: &str = "3c4d5e6f-7a8b-4c9d-8e0f-1a2b3c4d5e6f";

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).unwrap()
}

fn claims(json: &str) -> Claims {
    Claims::from_json(json.to_string()).unwrap()
}

fn user_with(issuer: &str, subject: &str, claim_set: &str, authenticated_at: u64) -> Principal {
    Principal::User(UserPrincipal {
        pair: Pair::new(issuer, subject),
        claims: claims(claim_set),
        authenticated_at: Timestamp::from(authenticated_at),
    })
}

fn user(issuer: &str, subject: &str) -> Principal {
    user_with(issuer, subject, "{}", 1_794_230_000_000)
}

fn agent(fingerprint: &str) -> AgentPrincipal {
    AgentPrincipal {
        issuer: ISSUER.to_string(),
        agent_id: uuid(AGENT_ID),
        fingerprint: AgentFingerprint(uuid(fingerprint)),
        agent_type: AgentTypeName("Analyst".to_string()),
        environment_id: EnvironmentId(uuid(ENVIRONMENT)),
    }
}

fn account() -> AccountPrincipal {
    AccountPrincipal {
        issuer: ISSUER.to_string(),
        account_id: AccountId(uuid(ACCOUNT)),
        authentication: AccountAuthentication::ApiToken,
    }
}

fn tool(name: &str) -> ToolPrincipal {
    ToolPrincipal {
        issuer: ISSUER.to_string(),
        environment_id: EnvironmentId(uuid(ENVIRONMENT)),
        tool_name: ToolName::try_from(name).unwrap(),
    }
}

fn subject_vectors() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-data/caller-identity/subjects-v1.json"
    )))
    .unwrap()
}

#[test]
fn pairs_that_differ_in_case_trailing_slash_or_normalization_are_different_principals() {
    let issuer = "https://login.acme.example";
    let subject = "\u{e9}l\u{e8}ve";
    let principal = user(issuer, subject);
    assert!(principal.equals(&user(issuer, subject)));

    let variants = [
        user("https://Login.acme.example", subject),
        user("https://login.acme.example/", subject),
        user(issuer, "\u{c9}L\u{c8}VE"),
        user(issuer, "e\u{301}le\u{300}ve"),
    ];
    for variant in &variants {
        assert!(!principal.equals(variant), "{variant:?}");
        assert!(!variant.equals(&principal), "{variant:?}");
        assert_ne!(principal.principal_ref(), variant.principal_ref());
        assert_ne!(
            principal.principal_ref().unwrap().to_string(),
            variant.principal_ref().unwrap().to_string()
        );
    }
}

#[test]
fn anonymous_equals_nothing_not_even_anonymous() {
    let anonymous = Principal::Anonymous;
    let someone = user("https://login.acme.example", "u-123");

    assert_eq!(anonymous.kind(), Kind::Anonymous);
    assert_eq!(anonymous.pair(), None);
    assert_eq!(anonymous.principal_ref(), None);
    assert!(!anonymous.equals(&anonymous));
    assert!(!anonymous.equals(&Principal::Anonymous));
    assert!(!anonymous.equals(&someone));
    assert!(!someone.equals(&anonymous));
    assert!(someone.equals(&someone));
}

#[test]
fn kind_claims_and_authentication_time_never_take_part_in_equality() {
    let agent = agent(FINGERPRINT);
    let pair = agent.pair();
    let agent = Principal::Agent(agent);
    let user_with_the_agent_pair = user_with(
        &pair.issuer,
        &pair.subject,
        r#"{"groups":["admins"]}"#,
        1_794_239_999_000,
    );
    assert!(agent.equals(&user_with_the_agent_pair));
    assert!(agent.equals_ref(&PrincipalRef::new(pair.clone())));
    assert!(!agent.equals_ref(&PrincipalRef::new(Pair::new(ISSUER, "account:x"))));

    let signed_in_once = user_with("https://login.acme.example", "u-1", "{}", 1);
    let signed_in_again = user_with(
        "https://login.acme.example",
        "u-1",
        r#"{"email":"new@acme.example"}"#,
        2,
    );
    assert!(signed_in_once.equals(&signed_in_again));
}

fn subject_from_vector(case: &Value) -> GolemSubject {
    match case["kind"].as_str().unwrap() {
        "agent" => GolemSubject::Agent {
            agent_id: uuid(case["agentId"].as_str().unwrap()),
            fingerprint: AgentFingerprint(uuid(case["fingerprint"].as_str().unwrap())),
        },
        "account" => GolemSubject::Account {
            account_id: AccountId(uuid(case["accountId"].as_str().unwrap())),
        },
        "tool" => GolemSubject::Tool {
            environment_id: EnvironmentId(uuid(case["environmentId"].as_str().unwrap())),
            tool_name: ToolName::try_from(case["toolName"].as_str().unwrap()).unwrap(),
        },
        other => panic!("unknown kind {other}"),
    }
}

#[test]
fn golem_subjects_print_and_parse_as_the_golden_vectors_say() {
    let vectors = subject_vectors();
    assert_eq!(vectors["version"], 1);
    let cases = vectors["roundTrip"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let subject = subject_from_vector(case);
        let text = case["subject"].as_str().unwrap();
        assert_eq!(subject.to_string(), text);
        assert_eq!(GolemSubject::from_str(text), Ok(subject.clone()), "{text}");
        assert_eq!(subject.kind().as_str(), case["kind"].as_str().unwrap());
    }
}

#[test]
fn golem_subject_parser_refuses_every_golden_non_canonical_subject() {
    let vectors = subject_vectors();
    let cases = vectors["reject"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let text = case["subject"].as_str().unwrap();
        assert!(
            GolemSubject::from_str(text).is_err(),
            "`{}` was accepted: {text}",
            case["name"].as_str().unwrap()
        );
    }
}

#[test]
fn subjects_of_different_kinds_never_collide() {
    let id = uuid(ACCOUNT);
    let subjects = [
        GolemSubject::Agent {
            agent_id: id,
            fingerprint: AgentFingerprint(id),
        },
        GolemSubject::Account {
            account_id: AccountId(id),
        },
        GolemSubject::Tool {
            environment_id: EnvironmentId(id),
            tool_name: ToolName::try_from("report").unwrap(),
        },
    ];
    let texts: BTreeSet<String> = subjects.iter().map(|s| s.to_string()).collect();
    assert_eq!(texts.len(), subjects.len(), "{texts:?}");
    for subject in &subjects {
        let parsed = GolemSubject::from_str(&subject.to_string()).unwrap();
        assert_eq!(parsed.kind(), subject.kind());
    }

    let principals = [
        Principal::Agent(agent(FINGERPRINT)),
        Principal::Account(account()),
        Principal::Tool(tool("report")),
    ];
    for (i, left) in principals.iter().enumerate() {
        for (j, right) in principals.iter().enumerate() {
            assert_eq!(left.equals(right), i == j, "{left:?} and {right:?}");
        }
    }
}

#[test]
fn an_agent_created_again_under_the_same_id_is_a_new_pair() {
    let first = agent(FINGERPRINT);
    let second = agent(FINGERPRINT_2);

    assert_eq!(
        first.pair(),
        Pair::new(ISSUER, format!("agent:{AGENT_ID}/{FINGERPRINT}"))
    );
    assert_ne!(first.subject(), second.subject());
    assert_ne!(first.pair(), second.pair());
    assert!(!Principal::Agent(first).equals(&Principal::Agent(second)));
}

#[test]
fn a_tool_declared_again_under_the_same_name_keeps_its_pair() {
    let declared = tool("report");
    let declared_again = tool("report");

    assert_eq!(
        declared.pair(),
        Pair::new(ISSUER, format!("tool:{ENVIRONMENT}/report"))
    );
    assert!(Principal::Tool(declared).equals(&Principal::Tool(declared_again)));
    for suffix in ["/3", "@3", ":3", "#3"] {
        let text = format!("tool:{ENVIRONMENT}/report{suffix}");
        assert!(GolemSubject::from_str(&text).is_err(), "{text}");
    }
}

#[test]
fn each_golem_principal_has_its_kind_and_its_pair_under_its_issuer() {
    let account = Principal::Account(account());
    assert_eq!(account.kind(), Kind::Account);
    assert_eq!(
        account.pair(),
        Some(Pair::new(ISSUER, format!("account:{ACCOUNT}")))
    );

    let agent = Principal::Agent(agent(FINGERPRINT));
    assert_eq!(agent.kind(), Kind::Agent);
    assert_eq!(
        agent.principal_ref(),
        Some(PrincipalRef::new(Pair::new(
            ISSUER,
            format!("agent:{AGENT_ID}/{FINGERPRINT}")
        )))
    );

    let tool = Principal::Tool(tool("report"));
    assert_eq!(tool.kind(), Kind::Tool);
    assert_eq!(
        tool.pair(),
        Some(Pair::new(ISSUER, format!("tool:{ENVIRONMENT}/report")))
    );

    let person = user("https://login.acme.example", "u-123");
    assert_eq!(person.kind(), Kind::User);
    assert_eq!(
        person.pair(),
        Some(Pair::new("https://login.acme.example", "u-123"))
    );
}

/// A claim set of at least `min_len` bytes, with nested values and with spellings that a parsed
/// and re-encoded JSON value would change: key order, number forms, escapes and spaces.
fn large_claim_set(min_len: usize) -> String {
    let mut json = String::from(
        r#"{"sub":"u-123","z-first":1,"a-second":1.0,"big":123456789012345678901234567890,"exponent":1E3,"escaped":"é\/","spaced" : [ 1 , 2 ],"groups":["#,
    );
    let mut index = 0;
    while json.len() < min_len {
        if index > 0 {
            json.push(',');
        }
        write!(
            json,
            r#"{{"name":"group-{index}","nested":{{"level":[{index},{{"deep":[true,null,-0.5e-3,"\u0001"]}}]}}}}"#
        )
        .unwrap();
        index += 1;
    }
    json.push_str("]}");
    json
}

#[test]
fn a_512_kib_claim_set_with_nested_values_round_trips_byte_for_byte() {
    let claim_set = large_claim_set(512 * 1024);
    assert!(claim_set.len() >= 512 * 1024);
    let person = Principal::User(UserPrincipal {
        pair: Pair::new("https://login.acme.example", "u-123"),
        claims: Claims::from_json(claim_set.clone()).unwrap(),
        authenticated_at: Timestamp::from(1_794_230_000_000),
    });
    let caller = Caller {
        original: person.clone(),
        immediate: person,
        tokens: Vec::new(),
    };

    let serialized = serde_json::to_vec(&caller).unwrap();
    let read_back: Caller = serde_json::from_slice(&serialized).unwrap();
    for principal in [&read_back.original, &read_back.immediate] {
        match principal {
            Principal::User(user) => assert!(user.claims.as_json() == claim_set),
            other => panic!("expected a user, got {other:?}"),
        }
    }
    assert!(serde_json::to_vec(&read_back).unwrap() == serialized);
}

#[test]
fn a_claim_set_is_one_json_object() {
    for not_an_object in [
        "",
        "[]",
        "1",
        "\"text\"",
        "null",
        "{",
        "{}{}",
        "{\"a\":1} x",
    ] {
        assert!(
            Claims::from_json(not_an_object.to_string()).is_err(),
            "accepted {not_an_object:?}"
        );
    }
    assert_eq!(claims("{}").as_json(), "{}");
    assert_eq!(claims(r#"{"b":1,"a":2}"#).as_json(), r#"{"b":1,"a":2}"#);

    let user_with_list_claims = r#"{"user":{"pair":{"issuer":"i","subject":"s"},"claims":[1],"authenticatedAt":"2026-11-09T10:00:00Z"}}"#;
    assert!(serde_json::from_str::<Principal>(user_with_list_claims).is_err());
}

fn collect_field_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(fields) => {
            for (name, field) in fields {
                names.insert(name.clone());
                // A claim set is the provider's data, not a field of the model.
                if name != "claims" {
                    collect_field_names(field, names);
                }
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect_field_names(item, names)),
        _ => {}
    }
}

#[test]
fn a_serialized_caller_has_no_token_value_cookie_key_or_proof_field() {
    let maker = PrincipalRef::new(Pair::new("https://login.acme.example", "u-456"));
    let callers = [
        Caller {
            original: user_with(
                "https://login.acme.example",
                "u-123",
                r#"{"token":"claims are data"}"#,
                1,
            ),
            immediate: Principal::Agent(agent(FINGERPRINT)),
            tokens: vec![
                TokenProvenance {
                    id: uuid("1d2e3f4a-5b6c-4d7e-8f9a-0b1c2d3e4f5a"),
                    maker: Some(maker),
                },
                TokenProvenance {
                    id: uuid("2e3f4a5b-6c7d-4e8f-9a0b-1c2d3e4f5a6b"),
                    maker: None,
                },
            ],
        },
        Caller {
            original: Principal::Account(account()),
            immediate: Principal::Tool(tool("report")),
            tokens: Vec::new(),
        },
        Caller {
            original: Principal::Anonymous,
            immediate: Principal::Anonymous,
            tokens: Vec::new(),
        },
    ];

    let mut names = BTreeSet::new();
    for caller in &callers {
        collect_field_names(&serde_json::to_value(caller).unwrap(), &mut names);
    }

    // Every field that a caller serializes. A new field fails this test, so that a credential
    // never joins the caller unnoticed.
    let expected: BTreeSet<String> = [
        "original",
        "immediate",
        "tokens",
        "id",
        "maker",
        "user",
        "account",
        "agent",
        "tool",
        "pair",
        "issuer",
        "subject",
        "claims",
        "authenticatedAt",
        "accountId",
        "authentication",
        "agentId",
        "fingerprint",
        "agentType",
        "environmentId",
        "toolName",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(names, expected);

    for name in &names {
        let name = name.to_lowercase();
        for credential in [
            "secret", "cookie", "key", "proof", "password", "bearer", "jwt",
        ] {
            assert!(!name.contains(credential), "field `{name}`");
        }
        assert!(
            name == "tokens" || !name.contains("token"),
            "field `{name}`"
        );
    }
}

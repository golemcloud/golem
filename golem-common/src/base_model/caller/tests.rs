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

//! Checks the vectors of the conversion from 1.6 principals. The conversion itself is not shared
//! code: each SDK implements it, so these tests read the 1.6 principals as plain JSON.

use super::{GolemSubject, Pair, PrincipalRef};
use crate::base_model::AgentFingerprint;
use crate::base_model::account::AccountId;
use serde_json::Value;
use std::collections::BTreeSet;
use test_r::test;
use uuid::Uuid;

const OIDC: &str = "oidc";
const GOLEM_USER_WITH_LOGIN_MAPPING: &str = "golem-user with an external login mapping";
const GOLEM_USER_WITH_GITHUB_MAPPING: &str = "golem-user with a GitHub mapping";
const GOLEM_USER_WITHOUT_MAPPING: &str = "golem-user without a mapping";
const AGENT: &str = "agent with its fingerprint";
const AGENT_NOT_CARRIED_OVER: &str = "agent that did not carry over";
const ANONYMOUS: &str = "anonymous";

fn vectors() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-data/caller-identity/conversion-from-1.6-v1.json"
    )))
    .unwrap()
}

fn cases(vectors: &Value) -> &Vec<Value> {
    vectors["cases"].as_array().unwrap()
}

fn text<'a>(value: &'a Value, what: &str) -> &'a str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("{what} is not a string: {value}"))
}

/// A UUID in the one form the vectors use: lower case, with hyphens. Other spellings that
/// `Uuid::parse_str` accepts (upper case, braces, `urn:uuid:`, no hyphens) are refused.
fn uuid(value: &Value) -> Uuid {
    let written = text(value, "a UUID");
    let parsed = Uuid::parse_str(written).unwrap_or_else(|error| panic!("{written}: {error}"));
    assert_eq!(
        parsed.hyphenated().to_string(),
        written,
        "a UUID must be lower case with hyphens"
    );
    parsed
}

/// A GitHub user ID in canonical decimal text: no sign, no leading zero.
fn github_id(value: &Value) -> u64 {
    let written = text(value, "externalId");
    let parsed: u64 = written
        .parse()
        .unwrap_or_else(|error| panic!("externalId {written}: {error}"));
    assert_eq!(
        parsed.to_string(),
        written,
        "a GitHub user ID must be canonical decimal text"
    );
    parsed
}

/// The 1.6 case that a vector covers, read from its inputs.
fn case_of(case: &Value) -> &'static str {
    match text(&case["principal"]["tag"], "the principal tag") {
        "oidc" => OIDC,
        "golem-user" if case["loginMapping"].is_null() => GOLEM_USER_WITHOUT_MAPPING,
        "golem-user" => {
            let mapping = &case["loginMapping"];
            let provider = mapping
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a login mapping without a provider: {mapping}"));
            let fields: &[&str] = match provider {
                "github" => &["externalId", "login"],
                "oidc" => &["issuer", "subject"],
                other => panic!("unknown login mapping provider {other:?}"),
            };
            for field in fields {
                assert!(
                    mapping[field].is_string(),
                    "the {provider} login mapping has no string {field}: {mapping}"
                );
            }
            if provider == "github" {
                GOLEM_USER_WITH_GITHUB_MAPPING
            } else {
                GOLEM_USER_WITH_LOGIN_MAPPING
            }
        }
        "agent" if case["importedAgent"].is_null() => AGENT_NOT_CARRIED_OVER,
        "agent" => AGENT,
        "anonymous" => ANONYMOUS,
        other => panic!("unknown 1.6 principal tag {other:?}"),
    }
}

#[test]
fn the_vectors_cover_every_1_6_principal_case() {
    let vectors = vectors();
    let covered: BTreeSet<&str> = cases(&vectors).iter().map(case_of).collect();
    for required in [
        OIDC,
        GOLEM_USER_WITH_LOGIN_MAPPING,
        GOLEM_USER_WITH_GITHUB_MAPPING,
        GOLEM_USER_WITHOUT_MAPPING,
        AGENT,
        AGENT_NOT_CARRIED_OVER,
        ANONYMOUS,
    ] {
        assert!(covered.contains(required), "no vector for {required}");
    }
}

/// Each of these `oidc` cases is the only vector that catches one wrong implementation.
#[test]
fn the_vectors_cover_the_oidc_traps() {
    let vectors = vectors();
    let oidc: Vec<&Value> = cases(&vectors)
        .iter()
        .filter(|case| case_of(case) == OIDC)
        .collect();
    // A conversion that takes the email as the subject when there is one.
    assert!(
        oidc.iter().any(|case| {
            let val = &case["principal"]["val"];
            val["email"].is_string() && val["email"] != val["sub"]
        }),
        "no oidc vector whose email is set and differs from its sub"
    );
    // A conversion that trims the issuer or lower-cases the sub.
    assert!(
        oidc.iter().any(|case| {
            let val = &case["principal"]["val"];
            text(&val["issuer"], "issuer").ends_with('/')
                && text(&val["sub"], "sub").chars().any(char::is_uppercase)
        }),
        "no oidc vector whose issuer ends in a slash and whose sub has an upper-case letter"
    );
    // A conversion that joins the strings instead of writing them in the text form.
    assert!(
        cases(&vectors).iter().any(|case| {
            !case["expected"].is_null()
                && text(&case["expected"]["pair"], "the expected pair").contains('\\')
        }),
        "no vector whose expected pair needs an escape in the text form"
    );
}

fn string_or_null(value: &Value, what: &str) {
    assert!(
        value.is_string() || value.is_null(),
        "{what} is neither a string nor null: {value}"
    );
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {value}"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// Every input principal is in the form that the 1.6 SDKs write.
#[test]
fn every_input_principal_is_in_the_1_6_sdk_form() {
    let vectors = vectors();
    for case in cases(&vectors) {
        let name = text(&case["name"], "the name");
        let principal = &case["principal"];
        let val = &principal["val"];
        match text(&principal["tag"], "the principal tag") {
            "anonymous" => assert_eq!(keys(principal), BTreeSet::from(["tag"]), "{name}"),
            "golem-user" => {
                assert_eq!(keys(val), BTreeSet::from(["accountId"]), "{name}");
                uuid(&val["accountId"]);
            }
            "agent" => {
                assert_eq!(
                    keys(val),
                    BTreeSet::from(["componentId", "agentId"]),
                    "{name}"
                );
                uuid(&val["componentId"]);
                text(&val["agentId"], "agentId");
            }
            "oidc" => {
                assert_eq!(
                    keys(val),
                    BTreeSet::from([
                        "sub",
                        "issuer",
                        "email",
                        "name",
                        "emailVerified",
                        "givenName",
                        "familyName",
                        "picture",
                        "preferredUsername",
                        "claims",
                    ]),
                    "{name}"
                );
                text(&val["sub"], "sub");
                text(&val["issuer"], "issuer");
                for key in [
                    "email",
                    "name",
                    "givenName",
                    "familyName",
                    "picture",
                    "preferredUsername",
                ] {
                    string_or_null(&val[key], key);
                }
                assert!(
                    val["emailVerified"].is_boolean() || val["emailVerified"].is_null(),
                    "{name}: emailVerified is neither a bool nor null"
                );
                let claims = text(&val["claims"], "claims");
                let parsed: Value = serde_json::from_str(claims)
                    .unwrap_or_else(|error| panic!("{name}: claims do not parse: {error}"));
                assert!(parsed.is_object(), "{name}: claims is not a JSON object");
            }
            other => panic!("{name}: unknown 1.6 principal tag {other:?}"),
        }
    }
}

#[test]
fn every_expected_pair_is_written_in_the_one_text_form() {
    let vectors = vectors();
    for case in cases(&vectors) {
        let name = text(&case["name"], "the name");
        if case["expected"].is_null() {
            continue;
        }
        let written = text(&case["expected"]["pair"], "the expected pair");
        let parsed: PrincipalRef = written
            .parse()
            .unwrap_or_else(|error| panic!("{name}: {written} does not parse: {error}"));
        assert_eq!(parsed.to_string(), written, "{name}");
    }
}

#[test]
fn every_expected_reference_follows_the_conversion_rules() {
    let vectors = vectors();
    let golem_issuer = text(&vectors["golemIssuer"], "golemIssuer");
    for case in cases(&vectors) {
        let name = text(&case["name"], "the name");
        let principal = &case["principal"]["val"];
        let mapping = &case["loginMapping"];
        let rule: Option<(&str, Pair)> = match case_of(case) {
            OIDC => Some((
                "user",
                Pair::new(
                    text(&principal["issuer"], "issuer"),
                    text(&principal["sub"], "sub"),
                ),
            )),
            GOLEM_USER_WITH_LOGIN_MAPPING => Some((
                "user",
                Pair::new(
                    text(&mapping["issuer"], "issuer"),
                    text(&mapping["subject"], "subject"),
                ),
            )),
            GOLEM_USER_WITH_GITHUB_MAPPING => {
                Some(("user", Pair::github_user(github_id(&mapping["externalId"]))))
            }
            GOLEM_USER_WITHOUT_MAPPING => Some((
                "account",
                Pair::new(
                    golem_issuer,
                    GolemSubject::Account {
                        account_id: AccountId(uuid(&principal["accountId"])),
                    }
                    .to_string(),
                ),
            )),
            AGENT => Some((
                "agent",
                Pair::new(
                    golem_issuer,
                    GolemSubject::Agent {
                        agent_id: uuid(&case["importedAgent"]["agentId"]),
                        fingerprint: AgentFingerprint(uuid(&case["importedAgent"]["fingerprint"])),
                    }
                    .to_string(),
                ),
            )),
            _ => None,
        };
        let expected = &case["expected"];
        let vector: Option<(&str, Pair)> = (!expected.is_null()).then(|| {
            let written = text(&expected["pair"], "the expected pair");
            let pair: PrincipalRef = written
                .parse()
                .unwrap_or_else(|error| panic!("{name}: {written} does not parse: {error}"));
            (
                text(&expected["kind"], "the expected kind"),
                pair.into_pair(),
            )
        });
        assert_eq!(vector, rule, "{name}");
    }
}

/// The live entry points build principals from the same inputs as the 1.6 conversion: the user
/// of a sign-in is the token's issuer and subject exactly as written, and an account without a
/// login mapping is the account pair under the Golem issuer.
#[test]
fn live_entry_pairs_match_the_1_6_conversion_vectors() {
    use super::{AccountAuthentication, AccountPrincipal, Claims, Principal, UserPrincipal};
    use crate::base_model::Timestamp;

    let vectors = vectors();
    let golem_issuer = text(&vectors["golemIssuer"], "golemIssuer").to_string();
    let mut checked = BTreeSet::new();
    for case in cases(&vectors) {
        let name = text(&case["name"], "a case name");
        let (live, expected_kind): (Principal, &str) = match case_of(case) {
            OIDC => {
                let oidc = &case["principal"]["val"];
                (
                    Principal::User(UserPrincipal {
                        pair: Pair::new(text(&oidc["issuer"], "issuer"), text(&oidc["sub"], "sub")),
                        claims: Claims::from_json(text(&oidc["claims"], "claims").to_string())
                            .unwrap(),
                        authenticated_at: Timestamp::from(1_893_452_400_000),
                    }),
                    "user",
                )
            }
            GOLEM_USER_WITHOUT_MAPPING => (
                Principal::Account(AccountPrincipal {
                    issuer: golem_issuer.clone(),
                    account_id: AccountId(uuid(&case["principal"]["val"]["accountId"])),
                    authentication: AccountAuthentication::ApiToken,
                }),
                "account",
            ),
            _ => continue,
        };
        let expected = &case["expected"];
        assert_eq!(live.kind().as_str(), expected_kind, "{name}");
        assert_eq!(
            live.pair().expect("a pair").to_string(),
            text(&expected["pair"], "the expected pair"),
            "{name}"
        );
        checked.insert(name.to_string());
    }
    assert!(checked.len() >= 3, "checked only {checked:?}");
}

#[test]
fn caller_with_512_kib_claims_round_trips_byte_for_byte_through_desert() {
    use super::test_support::{original_claims_json, user_principal};
    use super::{Caller, Claims, Principal, UserPrincipal};

    let padding = "x".repeat(512 * 1024);
    let json =
        format!("{{\"sub\":\"user-1\",\"padding\":\"{padding}\",\"spaced\":  [ 1 ,2.50,  1e3 ]}}");
    let Principal::User(user) = user_principal() else {
        unreachable!()
    };
    let claims = Claims::from_json(json.clone()).unwrap();
    let caller = Caller::entry(Principal::User(UserPrincipal { claims, ..user }));

    let bytes = desert_rust::serialize_to_byte_vec(&caller).unwrap();
    let decoded: Caller = desert_rust::deserialize(&bytes).unwrap();

    assert_eq!(decoded, caller);
    assert_eq!(original_claims_json(&decoded), json);
}

#[test]
fn a_caller_with_equal_roles_stores_its_claims_once_in_the_oplog_encoding() {
    use super::test_support::{agent_principal, user_principal};
    use super::{Caller, Claims, Principal, UserPrincipal};

    let padding = "x".repeat(64 * 1024);
    let json = format!("{{\"sub\":\"user-1\",\"padding\":\"{padding}\"}}");
    let Principal::User(user) = user_principal() else {
        unreachable!()
    };
    let user = Principal::User(UserPrincipal {
        claims: Claims::from_json(json).unwrap(),
        ..user
    });

    let same = Caller::entry(user.clone());
    let same_bytes = desert_rust::serialize_to_byte_vec(&same).unwrap();
    assert!(
        same_bytes.len() < 64 * 1024 + 1024,
        "equal roles wrote {} bytes: the claims are stored more than once",
        same_bytes.len()
    );
    let decoded: Caller = desert_rust::deserialize(&same_bytes).unwrap();
    assert_eq!(decoded, same);
    assert!(decoded.original.same_record(&decoded.immediate));

    // Different roles are both kept.
    let different = Caller {
        original: user.clone(),
        immediate: agent_principal(),
        tokens: vec![],
    };
    let different_bytes = desert_rust::serialize_to_byte_vec(&different).unwrap();
    let decoded: Caller = desert_rust::deserialize(&different_bytes).unwrap();
    assert_eq!(decoded, different);

    // The same user as the immediate caller and another user as the original stay two records.
    let swapped = Caller {
        original: agent_principal(),
        immediate: user,
        tokens: vec![],
    };
    let swapped_bytes = desert_rust::serialize_to_byte_vec(&swapped).unwrap();
    let decoded: Caller = desert_rust::deserialize(&swapped_bytes).unwrap();
    assert_eq!(decoded, swapped);
}

#[test]
fn recorded_callers_are_equal_only_when_every_field_is_the_same_record() {
    use super::test_support::{every_principal, sample_caller, user_principal};
    use super::{Caller, Claims, Principal, UserPrincipal};

    assert_eq!(sample_caller(), sample_caller());
    for principal in every_principal() {
        assert_eq!(Caller::entry(principal.clone()), Caller::entry(principal));
    }

    // The same user with claims that differ only in whitespace is another record.
    let Principal::User(user) = user_principal() else {
        unreachable!()
    };
    let respaced = Principal::User(UserPrincipal {
        claims: Claims::from_json(user.claims.as_json().replace(',', ", ")).unwrap(),
        ..user.clone()
    });
    assert_ne!(
        Caller::entry(Principal::User(user)),
        Caller::entry(respaced)
    );

    let mut other_tokens = sample_caller();
    other_tokens.tokens.pop();
    assert_ne!(sample_caller(), other_tokens);

    let mut swapped = sample_caller();
    std::mem::swap(&mut swapped.original, &mut swapped.immediate);
    assert_ne!(sample_caller(), swapped);

    // Identity is not record equality: anonymous equals nothing, yet two anonymous callers are
    // the same record.
    assert!(!Principal::Anonymous.equals(&Principal::Anonymous));
    assert_eq!(Caller::anonymous(), Caller::anonymous());
}

#[test]
fn caller_propagation_keeps_the_original_and_the_tokens() {
    use super::Caller;
    use super::test_support::{
        agent_principal, original_claims_json, sample_caller, tool_principal, user_principal,
    };

    let user = Caller {
        original: user_principal(),
        immediate: user_principal(),
        tokens: sample_caller().tokens,
    };

    let by_agent = user.propagate(agent_principal());
    assert!(by_agent.original.same_record(&user.original));
    assert_eq!(original_claims_json(&by_agent), original_claims_json(&user));
    assert!(by_agent.immediate.same_record(&agent_principal()));
    assert_eq!(by_agent.tokens, user.tokens);

    let by_tool = by_agent.propagate(tool_principal());
    assert!(by_tool.original.same_record(&user.original));
    assert_eq!(original_claims_json(&by_tool), original_claims_json(&user));
    assert!(by_tool.immediate.same_record(&tool_principal()));
    assert_eq!(by_tool.tokens, user.tokens);

    // The caller that propagates is left as it was.
    assert!(by_agent.immediate.same_record(&agent_principal()));

    // Both roles survive the stored form.
    let bytes = desert_rust::serialize_to_byte_vec(&by_tool).unwrap();
    let decoded: Caller = desert_rust::deserialize(&bytes).unwrap();
    assert_eq!(decoded, by_tool);
}

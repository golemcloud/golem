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

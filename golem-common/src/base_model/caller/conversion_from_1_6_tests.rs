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

fn uuid(value: &Value) -> Uuid {
    Uuid::parse_str(text(value, "a UUID")).unwrap()
}

/// The 1.6 case that a vector covers, read from its inputs.
fn case_of(case: &Value) -> &'static str {
    match text(&case["principal"]["tag"], "the principal tag") {
        "oidc" => OIDC,
        "golem-user" => match case["loginMapping"]["provider"].as_str() {
            Some("github") => GOLEM_USER_WITH_GITHUB_MAPPING,
            Some(_) => GOLEM_USER_WITH_LOGIN_MAPPING,
            None => GOLEM_USER_WITHOUT_MAPPING,
        },
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
            GOLEM_USER_WITH_GITHUB_MAPPING => Some((
                "user",
                Pair::github_user(text(&mapping["externalId"], "externalId").parse().unwrap()),
            )),
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

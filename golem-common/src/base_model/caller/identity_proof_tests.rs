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
    AWS_SESSION_TAGS_CLAIM, AWS_STS_AUDIENCE, AgentPrincipal, AwsSessionTagError, AwsSessionTags,
    GolemProofClaim, GolemSubject, IDENTITY_PROOF_LIFETIME_SECONDS, IdentityProofClaims,
    IdentityProofHeader, Kind, ProofReceiver, ProofSubject, ToolPrincipal,
};
use crate::base_model::AgentFingerprint;
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool::ToolName;
use serde_json::Value;
use std::collections::BTreeSet;
use std::str::FromStr;
use test_r::test;
use uuid::Uuid;

fn proof_vectors() -> Vec<Value> {
    let vectors: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-data/caller-identity/identity-proofs-v1.json"
    )))
    .unwrap();
    assert_eq!(vectors["version"], 1);
    let proofs = vectors["proofs"].as_array().unwrap().clone();
    assert!(!proofs.is_empty());
    proofs
}

fn text<'a>(input: &'a Value, field: &str) -> &'a str {
    input[field]
        .as_str()
        .unwrap_or_else(|| panic!("no `{field}`"))
}

fn uuid(input: &Value, field: &str) -> Uuid {
    Uuid::parse_str(text(input, field)).unwrap()
}

enum Workload {
    Agent(AgentPrincipal),
    Tool(ToolPrincipal),
}

impl Workload {
    fn from_input(input: &Value) -> Self {
        let issuer = text(input, "issuer").to_string();
        let environment_id = EnvironmentId(uuid(input, "environmentId"));
        match text(input, "kind") {
            "agent" => Workload::Agent(AgentPrincipal {
                issuer,
                agent_id: uuid(input, "agentId"),
                fingerprint: AgentFingerprint(uuid(input, "fingerprint")),
                agent_type: AgentTypeName(text(input, "agentType").to_string()),
                environment_id,
            }),
            "tool" => Workload::Tool(ToolPrincipal {
                issuer,
                environment_id,
                tool_name: ToolName::try_from(text(input, "toolName")).unwrap(),
            }),
            other => panic!("unknown kind {other}"),
        }
    }

    fn proof_subject(&self) -> ProofSubject<'_> {
        match self {
            Workload::Agent(agent) => ProofSubject::Agent(agent),
            Workload::Tool(tool) => ProofSubject::Tool(tool),
        }
    }

    fn subject(&self) -> GolemSubject {
        match self {
            Workload::Agent(agent) => agent.subject(),
            Workload::Tool(tool) => tool.subject(),
        }
    }
}

fn receiver(input: &Value) -> ProofReceiver {
    let audience = text(input, "audience").to_string();
    if input["awsRoleAssumption"].as_bool().unwrap() {
        ProofReceiver::AwsRoleAssumption { audience }
    } else {
        ProofReceiver::Audience(audience)
    }
}

fn build(input: &Value) -> IdentityProofClaims {
    IdentityProofClaims::new(
        Workload::from_input(input).proof_subject(),
        AccountId(uuid(input, "accountId")),
        &receiver(input),
        input["issuedAt"].as_u64().unwrap(),
        text(input, "jti"),
    )
    .unwrap()
}

#[test]
fn identity_proof_header_and_claims_match_the_golden_vectors() {
    for proof in proof_vectors() {
        let name = text(&proof, "name");
        let input = &proof["input"];

        let header = IdentityProofHeader::new(text(input, "kid"));
        assert_eq!(
            serde_json::to_value(&header).unwrap(),
            proof["header"],
            "{name}"
        );
        assert_eq!(header.alg, "ES256");
        assert_eq!(header.typ, "JWT");
        assert_eq!(
            serde_json::from_value::<IdentityProofHeader>(proof["header"].clone()).unwrap(),
            header,
            "{name}"
        );

        let claims = build(input);
        assert_eq!(
            serde_json::to_value(&claims).unwrap(),
            proof["claims"],
            "{name}"
        );
        assert_eq!(
            serde_json::from_value::<IdentityProofClaims>(proof["claims"].clone()).unwrap(),
            claims,
            "{name}"
        );
    }
}

#[test]
fn identity_proof_names_its_own_subject_for_five_minutes_with_the_account_id() {
    let mut ids = BTreeSet::new();
    for proof in proof_vectors() {
        let name = text(&proof, "name");
        let input = &proof["input"];
        let claims: IdentityProofClaims = serde_json::from_value(proof["claims"].clone()).unwrap();
        let workload = Workload::from_input(input);

        assert_eq!(claims.iss, text(input, "issuer"), "{name}");
        assert_eq!(claims.sub, workload.subject().to_string(), "{name}");
        let subject = GolemSubject::from_str(&claims.sub).unwrap();
        assert_eq!(subject.kind(), claims.golem.kind(), "{name}");
        assert_eq!(claims.golem.kind().as_str(), text(input, "kind"), "{name}");
        assert_eq!(
            claims.exp,
            claims.iat + IDENTITY_PROOF_LIFETIME_SECONDS,
            "{name}"
        );
        assert_eq!(IDENTITY_PROOF_LIFETIME_SECONDS, 5 * 60);
        assert!(ids.insert(claims.jti.clone()), "{name}: jti repeats");

        let account = text(&proof["claims"]["golem"], "account");
        assert_eq!(account, text(input, "accountId"), "{name}");
        assert_eq!(AccountId::from_str(account).unwrap().to_string(), account);
        assert!(!account.starts_with('/'), "{name}: an account path");
    }
}

#[test]
fn a_proof_for_aws_sts_repeats_the_golem_claim_in_session_tags_and_no_other_proof_has_them() {
    let mut aws_proofs = 0;
    for proof in proof_vectors() {
        let name = text(&proof, "name");
        let input = &proof["input"];
        let claims: IdentityProofClaims = serde_json::from_value(proof["claims"].clone()).unwrap();
        let tags_json = &proof["claims"][AWS_SESSION_TAGS_CLAIM];

        if !input["awsRoleAssumption"].as_bool().unwrap() {
            assert!(
                tags_json.is_null(),
                "{name}: AWS claim in a proof for another receiver"
            );
            assert_eq!(claims.aws_session_tags, None, "{name}");
            continue;
        }
        aws_proofs += 1;

        assert_eq!(claims.aud, AWS_STS_AUDIENCE, "{name}");
        assert_eq!(
            claims.aws_session_tags,
            Some(AwsSessionTags::from_golem_claim(&claims.golem).unwrap()),
            "{name}"
        );

        let tags = tags_json.as_object().unwrap();
        assert_eq!(
            tags.keys().collect::<Vec<_>>(),
            vec!["principal_tags"],
            "{name}: only principal tags, no transitive tag keys"
        );
        let golem = &proof["claims"]["golem"];
        let (workload_tag, workload_value) = match claims.golem {
            GolemProofClaim::Agent { .. } => ("golem:agent-type", text(golem, "agentType")),
            GolemProofClaim::Tool { .. } => ("golem:tool", text(golem, "tool")),
        };
        let expected = serde_json::json!({
            "golem:environment": [text(golem, "environment")],
            "golem:account": [text(golem, "account")],
            "golem:kind": [text(golem, "kind")],
            workload_tag: [workload_value],
        });
        assert_eq!(tags["principal_tags"], expected, "{name}");
    }
    assert_eq!(aws_proofs, 1);
}

#[test]
fn a_proof_for_another_receiver_has_no_aws_session_tags_even_for_the_sts_audience() {
    let input = &proof_vectors()[0]["input"];
    let workload = Workload::from_input(input);
    let claims = IdentityProofClaims::new(
        workload.proof_subject(),
        AccountId(uuid(input, "accountId")),
        &ProofReceiver::Audience(AWS_STS_AUDIENCE.to_string()),
        1,
        "jti",
    )
    .unwrap();
    assert_eq!(claims.aws_session_tags, None);
    assert_eq!(claims.golem.kind(), Kind::Agent);
    assert_eq!(
        ProofReceiver::aws_sts(),
        ProofReceiver::AwsRoleAssumption {
            audience: "sts.amazonaws.com".to_string()
        }
    );
}

fn make_agent(agent_type: &str) -> AgentPrincipal {
    AgentPrincipal {
        issuer: "https://golem.example".to_string(),
        agent_id: Uuid::parse_str("abcdef12-3456-4789-abcd-ef0123456789").unwrap(),
        fingerprint: AgentFingerprint(
            Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap(),
        ),
        agent_type: AgentTypeName(agent_type.to_string()),
        environment_id: EnvironmentId(
            Uuid::parse_str("22222222-3333-4444-8555-666666666666").unwrap(),
        ),
    }
}

fn make_tool(name: &str) -> ToolPrincipal {
    ToolPrincipal {
        issuer: "https://golem.example".to_string(),
        environment_id: EnvironmentId(
            Uuid::parse_str("22222222-3333-4444-8555-666666666666").unwrap(),
        ),
        tool_name: ToolName::try_from(name).unwrap(),
    }
}

fn claims_for(
    subject: ProofSubject<'_>,
    receiver: &ProofReceiver,
) -> Result<IdentityProofClaims, AwsSessionTagError> {
    IdentityProofClaims::new(
        subject,
        AccountId(Uuid::parse_str("33333333-4444-4555-8666-777777777777").unwrap()),
        receiver,
        1,
        "jti",
    )
}

#[test]
fn an_agent_type_with_a_character_aws_refuses_fails_an_aws_proof_and_builds_for_another_receiver() {
    for refused in [
        '<', '>', '&', '"', '\'', '\\', '#', '%', '(', ')', '*', ',', ';', '\n', '\t',
    ] {
        let agent = make_agent(&format!("Report{refused}Q3"));
        let error = claims_for(ProofSubject::Agent(&agent), &ProofReceiver::aws_sts()).unwrap_err();
        assert_eq!(
            error,
            AwsSessionTagError::CharacterRefused {
                tag: "golem:agent-type",
                character: refused
            },
            "{refused:?}"
        );
        assert!(error.to_string().contains("golem:agent-type"));

        let claims = claims_for(
            ProofSubject::Agent(&agent),
            &ProofReceiver::Audience("https://other.example".to_string()),
        )
        .unwrap();
        assert_eq!(claims.aws_session_tags, None);
    }
}

#[test]
fn the_characters_that_aws_allows_in_a_tag_value_are_accepted() {
    for allowed in [
        "Report Q3",
        "a_b.c:d/e=f+g-h@i",
        "Rapport-d\u{e9}t\u{e9}",
        "\u{5831}\u{544a}",
        "",
    ] {
        let agent = make_agent(allowed);
        let claims = claims_for(ProofSubject::Agent(&agent), &ProofReceiver::aws_sts()).unwrap();
        let tags = claims.aws_session_tags.unwrap();
        assert_eq!(
            tags.principal_tags.agent_type.unwrap(),
            [allowed.to_string()]
        );
    }
}

#[test]
fn separators_aws_allows_are_accepted_and_combining_marks_it_refuses_are_refused() {
    // AWS STS documents tag values as [\p{L}\p{Z}\p{N}_.:/=+\-@]*: general categories L, Z and N,
    // not the Unicode "Alphabetic" property that `char::is_alphanumeric` tests.
    let no_break_space = char::from_u32(0x00a0).unwrap();
    let line_separator = char::from_u32(0x2028).unwrap();
    let combining_ypogegrammeni = char::from_u32(0x0345).unwrap();
    let hebrew_point_sheva = char::from_u32(0x05b0).unwrap();

    for separator in [no_break_space, line_separator] {
        let value = format!("Report{separator}Q3");
        let agent = make_agent(&value);
        let claims = claims_for(ProofSubject::Agent(&agent), &ProofReceiver::aws_sts())
            .unwrap_or_else(|error| panic!("{separator:?} was refused: {error}"));
        assert_eq!(
            claims.aws_session_tags.unwrap().principal_tags.agent_type,
            Some([value])
        );
    }

    for mark in [combining_ypogegrammeni, hebrew_point_sheva] {
        let agent = make_agent(&format!("Report{mark}Q3"));
        assert_eq!(
            claims_for(ProofSubject::Agent(&agent), &ProofReceiver::aws_sts()).unwrap_err(),
            AwsSessionTagError::CharacterRefused {
                tag: "golem:agent-type",
                character: mark
            },
            "{mark:?}"
        );
    }
}

#[test]
fn a_tag_value_that_begins_with_aws_colon_fails_an_aws_proof() {
    let agent = make_agent("aws:X");
    assert_eq!(
        claims_for(ProofSubject::Agent(&agent), &ProofReceiver::aws_sts()).unwrap_err(),
        AwsSessionTagError::ReservedPrefix {
            tag: "golem:agent-type"
        }
    );
    let inside = make_agent("x-aws:X");
    claims_for(ProofSubject::Agent(&inside), &ProofReceiver::aws_sts()).unwrap();
}

#[test]
fn a_300_character_tool_name_fails_an_aws_proof_and_a_256_character_one_passes() {
    let long = "a".repeat(300);
    let tool_300 = make_tool(&long);
    assert_eq!(
        claims_for(ProofSubject::Tool(&tool_300), &ProofReceiver::aws_sts()).unwrap_err(),
        AwsSessionTagError::ValueTooLong {
            tag: "golem:tool",
            length: 300
        }
    );
    let other = claims_for(
        ProofSubject::Tool(&tool_300),
        &ProofReceiver::Audience("https://other.example".to_string()),
    )
    .unwrap();
    assert_eq!(other.aws_session_tags, None);

    let tool_256 = make_tool(&"a".repeat(256));
    let claims = claims_for(ProofSubject::Tool(&tool_256), &ProofReceiver::aws_sts()).unwrap();
    assert_eq!(
        claims
            .aws_session_tags
            .unwrap()
            .principal_tags
            .tool
            .unwrap()[0]
            .len(),
        256
    );
}

#[test]
fn the_length_limit_counts_characters_not_bytes() {
    let agent_256 = make_agent(&"\u{e9}".repeat(256));
    claims_for(ProofSubject::Agent(&agent_256), &ProofReceiver::aws_sts()).unwrap();
    let agent_257 = make_agent(&"\u{e9}".repeat(257));
    assert_eq!(
        claims_for(ProofSubject::Agent(&agent_257), &ProofReceiver::aws_sts()).unwrap_err(),
        AwsSessionTagError::ValueTooLong {
            tag: "golem:agent-type",
            length: 257
        }
    );
}

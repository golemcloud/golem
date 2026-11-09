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
    AWS_SESSION_TAGS_CLAIM, AWS_STS_AUDIENCE, AgentPrincipal, AwsSessionTags, GolemProofClaim,
    GolemSubject, IDENTITY_PROOF_LIFETIME_SECONDS, IdentityProofClaims, IdentityProofHeader, Kind,
    ProofReceiver, ProofSubject, ToolPrincipal,
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
            Some(AwsSessionTags::from_golem_claim(&claims.golem)),
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
    );
    assert_eq!(claims.aws_session_tags, None);
    assert_eq!(claims.golem.kind(), Kind::Agent);
    assert_eq!(
        ProofReceiver::aws_sts(),
        ProofReceiver::AwsRoleAssumption {
            audience: "sts.amazonaws.com".to_string()
        }
    );
}

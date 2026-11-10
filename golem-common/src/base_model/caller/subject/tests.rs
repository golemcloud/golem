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

use super::GolemSubject;
use crate::base_model::AgentFingerprint;
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::caller::{
    AccountAuthentication, AccountPrincipal, AgentPrincipal, Pair, Principal, ToolPrincipal,
};
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool::ToolName;
use serde_json::Value;
use std::collections::BTreeSet;
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
    // The agent's ID is the tool's environment ID, and the tool name is the agent's fingerprint
    // text, a valid tool name. Without the kind prefix both would print `<id>/<name>`.
    let shared_id = uuid(ENVIRONMENT);
    let name_id = uuid("abcdef12-3456-4789-abcd-ef0123456789");
    let agent_subject = GolemSubject::Agent {
        agent_id: shared_id,
        fingerprint: AgentFingerprint(name_id),
    };
    let tool_subject = GolemSubject::Tool {
        environment_id: EnvironmentId(shared_id),
        tool_name: ToolName::try_from(name_id.to_string().as_str()).unwrap(),
    };
    assert_ne!(agent_subject.to_string(), tool_subject.to_string());
    assert_eq!(
        GolemSubject::from_str(&tool_subject.to_string()),
        Ok(tool_subject.clone())
    );
    assert_eq!(
        GolemSubject::from_str(&agent_subject.to_string()),
        Ok(agent_subject.clone())
    );

    let mut same_id_agent = agent(FINGERPRINT);
    same_id_agent.agent_id = shared_id;
    same_id_agent.fingerprint = AgentFingerprint(name_id);
    let same_text_tool = tool(&name_id.to_string());
    let (agent_principal, tool_principal) = (
        Principal::Agent(same_id_agent),
        Principal::Tool(same_text_tool),
    );
    assert_ne!(agent_principal.pair(), tool_principal.pair());
    assert_ne!(
        agent_principal.principal_ref(),
        tool_principal.principal_ref()
    );
    assert!(!agent_principal.equals(&tool_principal));
    assert!(!tool_principal.equals(&agent_principal));
    assert!(!agent_principal.equals_ref(&tool_principal.principal_ref().unwrap()));

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

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

//! Callers for the unit tests of this crate.

use golem_common::model::agent::AgentTypeName;
use golem_common::model::caller::{
    AgentPrincipal, Caller, Claims, Pair, Principal, PrincipalRef, TokenProvenance, UserPrincipal,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentFingerprint, AgentId, Timestamp};
use uuid::Uuid;

pub(crate) const TEST_ISSUER: &str = "https://golem.test";

/// The agent as a principal, with a fixed issuer, fingerprint and type.
pub(crate) fn agent_principal(agent_id: AgentId) -> Principal {
    Principal::Agent(AgentPrincipal::of_agent(
        TEST_ISSUER.to_string(),
        agent_id,
        AgentFingerprint(Uuid::nil()),
        AgentTypeName("TestAgent".to_string()),
        EnvironmentId(Uuid::nil()),
    ))
}

/// The agent in both roles.
pub(crate) fn agent_caller(agent_id: AgentId) -> Caller {
    Caller::entry(agent_principal(agent_id))
}

/// A user with nested claims.
pub(crate) fn user_principal() -> Principal {
    Principal::User(UserPrincipal {
        pair: Pair::new("https://idp.example/", "user-1"),
        claims: Claims::from_json(
            r#"{"sub":"user-1","nested":{"list":[1,2.50,{"deep":null}]},"n":1e3}"#.to_string(),
        )
        .unwrap(),
        authenticated_at: "2026-01-02T03:04:05.678Z".parse::<Timestamp>().unwrap(),
    })
}

/// A user at the root, an agent as the immediate caller, and two tokens.
pub(crate) fn sample_caller() -> Caller {
    Caller {
        original: user_principal(),
        immediate: agent_principal(AgentId::from_parts(7, 0xc0)),
        tokens: vec![
            TokenProvenance {
                id: Uuid::from_u128(0x70c1),
                maker: Some(PrincipalRef::new(Pair::new(TEST_ISSUER, "account:1"))),
            },
            TokenProvenance {
                id: Uuid::from_u128(0x70c2),
                maker: None,
            },
        ],
    }
}

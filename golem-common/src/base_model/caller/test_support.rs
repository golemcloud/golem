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

//! Callers for the tests of the types that carry one.

use super::{
    AccountAuthentication, AccountPrincipal, AgentPrincipal, Caller, Claims, Pair, Principal,
    PrincipalRef, TokenProvenance, ToolPrincipal, UserPrincipal,
};
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::component::ComponentId;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool::ToolName;
use crate::base_model::{AgentFingerprint, AgentId, Timestamp};
use uuid::Uuid;

pub const ISSUER: &str = "https://golem.example";

pub const CLAIMS_JSON: &str = r#"{"sub":"user-1","email":"a@b.example","nested":{"list":[1,2.50,{"deep":null}],"flag":true},"n":1e3}"#;

pub fn user_principal() -> Principal {
    Principal::User(UserPrincipal {
        pair: Pair::new("https://idp.example/", "user-1"),
        claims: Claims::from_json(CLAIMS_JSON.to_string()).unwrap(),
        authenticated_at: "2026-01-02T03:04:05.678Z".parse::<Timestamp>().unwrap(),
    })
}

pub fn account_principal() -> Principal {
    Principal::Account(AccountPrincipal {
        issuer: ISSUER.to_string(),
        account_id: AccountId(Uuid::from_u128(0xacc0)),
        authentication: AccountAuthentication::SignIn,
    })
}

pub fn agent_principal() -> Principal {
    Principal::Agent(AgentPrincipal::of_agent(
        ISSUER.to_string(),
        AgentId {
            component_id: ComponentId(Uuid::from_u128(0xc0)),
            agent_id: "Relay(\"r\")".to_string(),
        },
        AgentFingerprint(Uuid::from_u128(0xf1)),
        AgentTypeName("Relay".to_string()),
        EnvironmentId(Uuid::from_u128(0xe0)),
    ))
}

pub fn tool_principal() -> Principal {
    Principal::Tool(ToolPrincipal {
        issuer: ISSUER.to_string(),
        environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
        tool_name: ToolName::try_from("grep").unwrap(),
    })
}

pub fn every_principal() -> Vec<Principal> {
    vec![
        user_principal(),
        account_principal(),
        agent_principal(),
        tool_principal(),
        Principal::Anonymous,
    ]
}

/// A caller with a user at the root, an agent as the immediate caller, and two tokens, one with a
/// maker and one without.
pub fn sample_caller() -> Caller {
    Caller {
        original: user_principal(),
        immediate: agent_principal(),
        tokens: vec![
            TokenProvenance {
                id: Uuid::from_u128(0x70c1),
                maker: Some(PrincipalRef::new(Pair::new(ISSUER, "account:1"))),
            },
            TokenProvenance {
                id: Uuid::from_u128(0x70c2),
                maker: None,
            },
        ],
    }
}

/// The exact claims text of the user at the root of the caller.
pub fn original_claims_json(caller: &Caller) -> &str {
    match &caller.original {
        Principal::User(user) => user.claims.as_json(),
        other => panic!("expected a user at the root, got {other:?}"),
    }
}

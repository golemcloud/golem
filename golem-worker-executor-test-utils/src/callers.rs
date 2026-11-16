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

//! Callers for the executor tests. Every principal is under the one Golem issuer that the test
//! executor is configured with.

use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::caller::{
    AccountAuthentication, AccountPrincipal, AgentPrincipal, Caller, Claims, Pair, Principal,
    UserPrincipal,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentFingerprint, AgentId, Timestamp};

/// The Golem issuer of the test executor.
pub const GOLEM_ISSUER: &str = "https://issuer.golem-test.example";

pub fn anonymous() -> Caller {
    Caller::anonymous()
}

pub fn account_principal(account_id: AccountId) -> Principal {
    Principal::Account(AccountPrincipal {
        issuer: GOLEM_ISSUER.to_string(),
        account_id,
        authentication: AccountAuthentication::ApiToken,
    })
}

/// An account, in both roles.
pub fn account(account_id: AccountId) -> Caller {
    Caller::entry(account_principal(account_id))
}

/// A user of a sign-in provider, in both roles. The claims are the given JSON object.
pub fn user_with_claims(issuer: &str, subject: &str, claims: &str) -> Caller {
    Caller::entry(Principal::User(UserPrincipal {
        pair: Pair::new(issuer, subject),
        claims: Claims::from_json(claims.to_string()).expect("the claims are one JSON object"),
        authenticated_at: Timestamp::from(1_794_230_000_000u64),
    }))
}

/// A user of a sign-in provider with empty claims, in both roles.
pub fn user(issuer: &str, subject: &str) -> Caller {
    user_with_claims(issuer, subject, "{}")
}

/// An agent as a principal. Its type is the one in its name, and its fingerprint and environment
/// are nil.
pub fn agent_principal(agent_id: AgentId) -> Principal {
    let agent_type = agent_id
        .agent_id
        .split('(')
        .next()
        .unwrap_or_default()
        .to_string();
    Principal::Agent(AgentPrincipal::of_agent(
        GOLEM_ISSUER.to_string(),
        agent_id,
        AgentFingerprint(uuid::Uuid::nil()),
        AgentTypeName(agent_type),
        EnvironmentId(uuid::Uuid::nil()),
    ))
}

/// An agent, in both roles.
pub fn agent(agent_id: AgentId) -> Caller {
    Caller::entry(agent_principal(agent_id))
}

/// The display text that every SDK gives a user principal: its kind and its pair.
pub fn user_text(issuer: &str, subject: &str) -> String {
    format!("user (\"{issuer}\",\"{subject}\")")
}

/// The display text that every SDK gives an account principal under the test Golem issuer.
pub fn account_text(account_id: AccountId) -> String {
    format!(
        "account (\"{GOLEM_ISSUER}\",\"account:{}\")",
        account_id.0.hyphenated()
    )
}

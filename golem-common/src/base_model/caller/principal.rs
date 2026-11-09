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

use super::{GolemSubject, Pair, PrincipalRef};
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool::ToolName;
use crate::base_model::{AgentFingerprint, Timestamp};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use std::fmt::{Display, Formatter};
use uuid::Uuid;

/// The verified kind of a principal. The kind is metadata: it never takes part in equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    User,
    Account,
    Agent,
    Tool,
    Anonymous,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::User => "user",
            Kind::Account => "account",
            Kind::Agent => "agent",
            Kind::Tool => "tool",
            Kind::Anonymous => "anonymous",
        }
    }
}

impl Display for Kind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One identity, with the details that Golem verified for it.
///
/// The type has no `==`: two principals are the same identity exactly when [`Principal::equals`]
/// says so, and that compares only their pairs. A principal holds no token value, cookie, key or
/// identity proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Principal {
    User(UserPrincipal),
    Account(AccountPrincipal),
    Agent(AgentPrincipal),
    Tool(ToolPrincipal),
    Anonymous,
}

impl Principal {
    pub fn kind(&self) -> Kind {
        match self {
            Principal::User(_) => Kind::User,
            Principal::Account(_) => Kind::Account,
            Principal::Agent(_) => Kind::Agent,
            Principal::Tool(_) => Kind::Tool,
            Principal::Anonymous => Kind::Anonymous,
        }
    }

    /// The principal's pair, or `None` for an anonymous principal.
    pub fn pair(&self) -> Option<Pair> {
        match self {
            Principal::User(user) => Some(user.pair.clone()),
            Principal::Account(account) => Some(account.pair()),
            Principal::Agent(agent) => Some(agent.pair()),
            Principal::Tool(tool) => Some(tool.pair()),
            Principal::Anonymous => None,
        }
    }

    pub fn principal_ref(&self) -> Option<PrincipalRef> {
        self.pair().map(PrincipalRef::new)
    }

    /// True exactly when both principals have a pair and the pairs are equal. Kind, claims and
    /// display text never take part, and an anonymous principal equals nothing, not even
    /// another anonymous principal.
    pub fn equals(&self, other: &Principal) -> bool {
        match (self.pair(), other.pair()) {
            (Some(pair), Some(other_pair)) => pair == other_pair,
            _ => false,
        }
    }

    /// True exactly when the principal has a pair and it is the referenced pair.
    pub fn equals_ref(&self, other: &PrincipalRef) -> bool {
        self.pair().as_ref() == Some(other.pair())
    }
}

/// A user, authenticated by a sign-in provider or at the CLI. The pair is the provider's
/// `(issuer, subject)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPrincipal {
    pub pair: Pair,
    pub claims: Claims,
    pub authenticated_at: Timestamp,
}

/// An account, authenticated by an account API token, by the bootstrap admin token or by a
/// sign-in. The pair is `(Golem issuer, account:<account-id>)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPrincipal {
    pub issuer: String,
    pub account_id: AccountId,
    pub authentication: AccountAuthentication,
}

impl AccountPrincipal {
    pub fn subject(&self) -> GolemSubject {
        GolemSubject::Account {
            account_id: self.account_id,
        }
    }

    pub fn pair(&self) -> Pair {
        Pair::new(self.issuer.clone(), self.subject().to_string())
    }
}

/// How an account authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountAuthentication {
    ApiToken,
    BootstrapToken,
    SignIn,
}

/// An agent. The pair is `(issuer, agent:<agent-id>/<fingerprint>)`, where the issuer is this
/// installation's Golem issuer, or another installation's issuer for an agent that an identity
/// proof of that installation proved.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPrincipal {
    pub issuer: String,
    /// The agent's 2.0 ID, never its key.
    pub agent_id: Uuid,
    pub fingerprint: AgentFingerprint,
    pub agent_type: AgentTypeName,
    pub environment_id: EnvironmentId,
}

impl AgentPrincipal {
    pub fn subject(&self) -> GolemSubject {
        GolemSubject::Agent {
            agent_id: self.agent_id,
            fingerprint: self.fingerprint,
        }
    }

    pub fn pair(&self) -> Pair {
        Pair::new(self.issuer.clone(), self.subject().to_string())
    }
}

/// A tool. The pair is `(issuer, tool:<environment-id>/<tool-name>)`, with the issuer as for an
/// agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPrincipal {
    pub issuer: String,
    pub environment_id: EnvironmentId,
    pub tool_name: ToolName,
}

impl ToolPrincipal {
    pub fn subject(&self) -> GolemSubject {
        GolemSubject::Tool {
            environment_id: self.environment_id,
            tool_name: self.tool_name.clone(),
        }
    }

    pub fn pair(&self) -> Pair {
        Pair::new(self.issuer.clone(), self.subject().to_string())
    }
}

/// The verified claims of a user's sign-in, kept whole: the JSON object exactly as Golem
/// received it, never cut, reordered or encoded again, so code that reads claims replays
/// identically.
#[derive(Debug, Clone)]
pub struct Claims(Box<RawValue>);

impl Claims {
    /// Reads a claim set. The text must be one JSON object.
    pub fn from_json(json: String) -> Result<Self, ClaimsError> {
        let raw = RawValue::from_string(json)
            .map_err(|error| ClaimsError::InvalidJson(error.to_string()))?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: Box<RawValue>) -> Result<Self, ClaimsError> {
        if raw.get().starts_with('{') {
            Ok(Self(raw))
        } else {
            Err(ClaimsError::NotAnObject)
        }
    }

    /// The claim set as JSON text, exactly as Golem received it.
    pub fn as_json(&self) -> &str {
        self.0.get()
    }
}

impl Serialize for Claims {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Claims {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        Claims::from_raw(raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaimsError {
    #[error("the claim set is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("the claim set must be a JSON object")]
    NotAnObject,
}

/// A token that the request presented at the entry point, with its maker. It names the token by
/// its ID and never holds the token's value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenProvenance {
    pub id: Uuid,
    pub maker: Option<PrincipalRef>,
}

/// The caller of one invocation: the principal that started the chain, the principal that made
/// this call, and one entry for each token presented at the entry point. A caller holds no
/// credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Caller {
    pub original: Principal,
    pub immediate: Principal,
    pub tokens: Vec<TokenProvenance>,
}

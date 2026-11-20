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

//! The public form of a caller: what a reader of an oplog sees of the principals behind a recorded
//! invocation or request.
//!
//! The public form keeps both roles and the tokens of a [`Caller`], with the details that Golem
//! verified for each principal. It holds no credential, and a user's claims only when the reader
//! asked for them and may read the oplog ([`ClaimVisibility::Shown`]).

mod claims;

pub use claims::PublicClaims;

use super::{AccountAuthentication, Caller, Kind, Pair, Principal, TokenProvenance};
use crate::base_model::account::AccountId;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::oplog::OplogInclude;
use crate::base_model::{AgentId, Timestamp};
use golem_schema::Empty;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

/// Whether the public form of a caller holds the users' verified claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClaimVisibility {
    /// No claims. Every reader gets this unless it asks for the claims.
    #[default]
    Hidden,
    /// The claims, for a reader that asked for them and passed the oplog read check.
    Shown,
}

impl ClaimVisibility {
    /// `Shown` exactly when the reader asked for the claims.
    pub fn from_include(include: Option<OplogInclude>) -> Self {
        match include {
            Some(OplogInclude::Claims) => Self::Shown,
            None => Self::Hidden,
        }
    }

    pub fn is_shown(&self) -> bool {
        matches!(self, Self::Shown)
    }
}

/// The public form of a caller. Both roles are always present, also when they are the same
/// principal. It holds no credential, and it holds claims only when the reader asks for them
/// (`include=claims`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicCaller {
    /// The principal that started the chain.
    pub original: PublicPrincipal,
    /// The principal that made this call.
    pub immediate: PublicPrincipal,
    /// The tokens that the request presented at the entry point, with their makers.
    pub tokens: Vec<PublicTokenProvenance>,
}

impl PublicCaller {
    /// The public form of a decoded caller, which always holds both roles.
    pub fn new(caller: &Caller, claims: ClaimVisibility) -> Self {
        Self {
            original: PublicPrincipal::new(&caller.original, claims),
            immediate: PublicPrincipal::new(&caller.immediate, claims),
            tokens: caller
                .tokens
                .iter()
                .map(PublicTokenProvenance::from)
                .collect(),
        }
    }
}

/// The public form of one principal, with the details that Golem verified for it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(
    feature = "full",
    oai(discriminator_name = "kind", one_of = true, rename_all = "camelCase")
)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PublicPrincipal {
    User(PublicUserPrincipal),
    Account(PublicAccountPrincipal),
    Agent(PublicAgentPrincipal),
    Tool(PublicToolPrincipal),
    Anonymous(Empty),
}

// Read without a buffer, so the claims of a user keep their exact JSON text. See `raw_json`.
crate::deserialize_internally_tagged!(PublicPrincipal, "kind", {
    "user" => User,
    "account" => Account,
    "agent" => Agent,
    "tool" => Tool,
    "anonymous" => Anonymous,
});

impl PublicPrincipal {
    pub fn new(principal: &Principal, claims: ClaimVisibility) -> Self {
        match principal {
            Principal::User(user) => Self::User(PublicUserPrincipal {
                pair: user.pair.clone(),
                authenticated_at: user.authenticated_at,
                claims: claims.is_shown().then(|| PublicClaims::from(&user.claims)),
            }),
            Principal::Account(account) => Self::Account(PublicAccountPrincipal {
                pair: account.pair(),
                account_id: account.account_id,
                authentication: account.authentication,
            }),
            Principal::Agent(agent) => Self::Agent(PublicAgentPrincipal {
                pair: agent.pair(),
                agent_id: agent.address,
                agent_type: agent.agent_type.0.clone(),
                environment_id: agent.environment_id,
            }),
            Principal::Tool(tool) => Self::Tool(PublicToolPrincipal {
                pair: tool.pair(),
                environment_id: tool.environment_id,
                tool_name: tool.tool_name.to_string(),
            }),
            Principal::Anonymous => Self::Anonymous(Empty {}),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Self::User(_) => Kind::User,
            Self::Account(_) => Kind::Account,
            Self::Agent(_) => Kind::Agent,
            Self::Tool(_) => Kind::Tool,
            Self::Anonymous(_) => Kind::Anonymous,
        }
    }

    /// The principal's pair, or `None` for an anonymous principal.
    pub fn pair(&self) -> Option<&Pair> {
        match self {
            Self::User(user) => Some(&user.pair),
            Self::Account(account) => Some(&account.pair),
            Self::Agent(agent) => Some(&agent.pair),
            Self::Tool(tool) => Some(&tool.pair),
            Self::Anonymous(_) => None,
        }
    }

    /// The user's claims, when the principal is a user and the claims are shown.
    pub fn claims(&self) -> Option<&PublicClaims> {
        match self {
            Self::User(user) => user.claims.as_ref(),
            _ => None,
        }
    }
}

/// `<kind> <pair text>`, for example `user ("https://idp.example","bob")`, or `anonymous`. The text
/// never holds claims.
impl Display for PublicPrincipal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.pair() {
            Some(pair) => write!(f, "{} {pair}", self.kind()),
            None => write!(f, "{}", self.kind()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicUserPrincipal {
    pub pair: Pair,
    pub authenticated_at: Timestamp,
    /// The verified claims, only when the reader asks for them (`include=claims`). The key is
    /// absent otherwise.
    #[cfg_attr(feature = "full", oai(skip_serializing_if_is_none))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<PublicClaims>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicAccountPrincipal {
    pub pair: Pair,
    pub account_id: AccountId,
    pub authentication: AccountAuthentication,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicAgentPrincipal {
    pub pair: Pair,
    /// The agent's ID today: its component and its name.
    pub agent_id: AgentId,
    pub agent_type: String,
    pub environment_id: EnvironmentId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicToolPrincipal {
    pub pair: Pair,
    pub environment_id: EnvironmentId,
    pub tool_name: String,
}

/// A token that the request presented, with the pair of its maker. It never holds the token's
/// value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicTokenProvenance {
    pub id: Uuid,
    pub maker: Option<Pair>,
}

impl From<&TokenProvenance> for PublicTokenProvenance {
    fn from(token: &TokenProvenance) -> Self {
        Self {
            id: token.id,
            maker: token.maker.as_ref().map(|maker| maker.pair().clone()),
        }
    }
}

#[cfg(test)]
mod tests;

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

use super::AgentTarget;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::card::CardId;
use crate::base_model::security_scheme::SecuritySchemeId;
use crate::base_model::tool::ToolName;
use crate::newtype_uuid;
use serde::{Deserialize, Serialize};

/// The answer to an [`AuthorizationRequest`](super::AuthorizationRequest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase", tag = "outcome", content = "detail")]
pub enum Decision {
    /// Every part of the operation is allowed. For a compound operation, the grantors are the
    /// union over its parts.
    Allow(Grantors),
    /// The denial of the first part that fails.
    Deny(Denial),
}

/// How a denial must look to the caller. Each surface renders it with its own renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "denial"
)]
pub enum Denial {
    /// No authenticated principal, and a candidate card would admit the request once the caller
    /// signs in: a `401` with the challenge of the scheme, or Golem's sign-in flow
    /// when there is no scheme.
    SignInRequired { scheme: Option<SecuritySchemeId> },
    /// The response must be byte-identical to "unknown ID" for this kind.
    LooksUnknown { kind: DesignatedKind },
    /// A `403`. It names the missing grant only for a target that the principal may view.
    Forbidden { missing_grant: Option<GrantText> },
}

/// The kinds of target whose denial looks like an unknown ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub enum DesignatedKind {
    Agent,
    Invocation,
    Task,
    Stream,
    Promise,
    Webhook,
    Artifact,
}

/// A grant in its text form, as a denial names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
pub struct GrantText(pub String);

/// The cards that covered an allowed operation. Issuance inherits their bounds
/// and conditions, the audit records them, and an entry point registers a long-lived
/// connection under every card and token ID in them, so that revoking any of them closes it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct Grantors {
    pub cards: Vec<GrantorCard>,
}

/// One covering card, with its full ancestor list and the token that presented it, if any.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct GrantorCard {
    pub card_id: CardId,
    /// Every ancestor of the card, with no length limit.
    pub ancestors: Vec<CardId>,
    /// The card token that presented the card. `None` for a card from the principal's own wallet.
    pub presented_by: Option<CardTokenId>,
}

newtype_uuid!(CardTokenId);

/// The owners that a request's class and verb grants cover, for a listing. It may be wider than
/// the final answer, never narrower: `filter` decides each row.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    /// A grant covers every target in the request's environment.
    pub whole_environment: bool,
    pub agent_types: Vec<AgentTypeName>,
    pub tools: Vec<ToolName>,
    pub agents: Vec<AgentTarget>,
}

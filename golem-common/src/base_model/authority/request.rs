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

use super::{InvocationTarget, IpAddress, Operation};
use crate::base_model::caller::{Caller, Principal};
use crate::base_model::card::CardId;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};

/// One access question: may this principal, with the authority that this request brings, do this
/// operation?
///
/// Build it with [`AuthorizationRequest::new`]: the principal whose authority decides is then the
/// caller's immediate principal. The decision never reads the original caller's wallet or tokens;
/// the original caller is in the request for the audit only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AuthorizationRequest {
    /// The principal whose authority decides. It is the caller's immediate principal, except where
    /// Golem acts on another principal's authority, as volume I/O does on the placement owner's.
    pub principal: Principal,
    pub caller: Caller,
    /// The invocation that the host attributes the operation to: present for every
    /// host function and every typed internal call, absent at entry points.
    pub invocation: Option<InvocationTarget>,
    pub presented: PresentedAuthority,
    pub operation: Operation,
}

impl AuthorizationRequest {
    pub fn new(caller: Caller, operation: Operation) -> Self {
        Self {
            principal: caller.immediate.clone(),
            caller,
            invocation: None,
            presented: PresentedAuthority::none(),
            operation,
        }
    }

    pub fn with_invocation(mut self, invocation: InvocationTarget) -> Self {
        self.invocation = Some(invocation);
        self
    }

    pub fn with_presented(mut self, presented: PresentedAuthority) -> Self {
        self.presented = presented;
        self
    }

    /// Decides on another principal's authority, as volume I/O does on the placement owner's.
    /// The caller stays the same.
    pub fn on_authority_of(mut self, principal: Principal) -> Self {
        self.principal = principal;
        self
    }
}

/// The same request: every field is equal, and principals are the same record.
impl PartialEq for AuthorizationRequest {
    fn eq(&self, other: &Self) -> bool {
        self.principal.same_record(&other.principal)
            && self.caller == other.caller
            && self.invocation == other.invocation
            && self.presented == other.presented
            && self.operation == other.operation
    }
}

impl Eq for AuthorizationRequest {}

/// The authority that a request brings besides the principal's wallet.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PresentedAuthority {
    /// Card tokens from the `Golem-Token` header and the path prefix.
    #[serde(default)]
    pub tokens: Vec<PresentedToken>,
    /// The courtesy card of the route, or of the targeted package.
    #[serde(default)]
    pub courtesy_card: Option<CardId>,
    /// The request's source address from Golem's trusted ingress, for `sourceNetworks`
    /// conditions. Absent inside agents and tools.
    #[serde(default)]
    pub source: Option<IpAddress>,
    /// The live sign-in session on the presentation origin, for session-bound tokens.
    #[serde(default)]
    pub session: Option<SessionRef>,
}

impl PresentedAuthority {
    /// No authority besides the principal's wallet.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn with_token(mut self, token: PresentedToken) -> Self {
        self.tokens.push(token);
        self
    }

    pub fn with_courtesy_card(mut self, card: CardId) -> Self {
        self.courtesy_card = Some(card);
        self
    }

    pub fn with_source(mut self, source: IpAddress) -> Self {
        self.source = Some(source);
        self
    }

    pub fn with_session(mut self, session: SessionRef) -> Self {
        self.session = Some(session);
        self
    }
}

/// A card token value as the request presented it. It is a bearer credential: `Debug` never
/// shows it, and it must not be logged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
pub struct PresentedToken(String);

impl PresentedToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn value(&self) -> &str {
        &self.0
    }
}

impl Debug for PresentedToken {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("PresentedToken(<redacted>)")
    }
}

/// A sign-in session on a presentation origin, by the text form of its ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
pub struct SessionRef(pub String);

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
use crate::base_model::{AgentFingerprint, AgentId, Timestamp};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use std::collections::HashSet;
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
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct UserPrincipal {
    pub pair: Pair,
    pub claims: Claims,
    pub authenticated_at: Timestamp,
}

/// An account, authenticated by an account API token, by the bootstrap admin token or by a
/// sign-in. The pair is `(Golem issuer, account:<account-id>)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentPrincipal {
    pub issuer: String,
    /// The agent's 2.0 ID, never its key. Until agents have UUID IDs it is
    /// [`agent_subject_uuid`] of `address`.
    pub agent_id: Uuid,
    /// The agent's ID today (its component and name). It goes away when `agent_id` is the agent's
    /// own ID.
    pub address: AgentId,
    pub fingerprint: AgentFingerprint,
    pub agent_type: AgentTypeName,
    pub environment_id: EnvironmentId,
}

impl AgentPrincipal {
    /// The only constructor that the executor uses. It keeps `agent_id` and `address` in step.
    pub fn of_agent(
        issuer: String,
        address: AgentId,
        fingerprint: AgentFingerprint,
        agent_type: AgentTypeName,
        environment_id: EnvironmentId,
    ) -> Self {
        Self {
            issuer,
            agent_id: agent_subject_uuid(&address),
            address,
            fingerprint,
            agent_type,
            environment_id,
        }
    }

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

/// The namespace of [`agent_subject_uuid`].
const AGENT_SUBJECT_NAMESPACE: Uuid = Uuid::from_u128(0xccee40b434a645959874802e49d72242);

/// The UUID that names an agent in its Golem subject until agents have UUID IDs: the UUIDv5 of
/// the agent's ID text (`<component-id>/<agent-name>`) in a fixed namespace.
pub fn agent_subject_uuid(agent_id: &AgentId) -> Uuid {
    Uuid::new_v5(&AGENT_SUBJECT_NAMESPACE, agent_id.to_string().as_bytes())
}

/// A tool. The pair is `(issuer, tool:<environment-id>/<tool-name>)`, with the issuer as for an
/// agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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
///
/// No object in a claim set may repeat a member name. JSON readers disagree on which of two
/// equal names wins, so a repeated `sub` could name one user to Golem and another to the code
/// that reads the claims. RFC 7493 (I-JSON) forbids repeated names, and escaped lone surrogates
/// in names and in values.
///
/// Serialize a `Caller` to text with `serde_json::to_string` or `to_writer`. A round trip through
/// `serde_json::Value` encodes the claims again: it sorts the keys, rounds numbers, fails on
/// numbers that no `f64` holds, and so loses the exact bytes.
#[derive(Debug, Clone)]
pub struct Claims(Box<RawValue>);

impl Claims {
    /// Reads a claim set. The text must be one JSON object, and no object in it may repeat a
    /// member name. Whitespace around the object is dropped.
    pub fn from_json(json: String) -> Result<Self, ClaimsError> {
        let raw = RawValue::from_string(json)
            .map_err(|error| ClaimsError::InvalidJson(error.to_string()))?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: Box<RawValue>) -> Result<Self, ClaimsError> {
        if !raw.get().starts_with('{') {
            return Err(ClaimsError::NotAnObject);
        }
        check_member_names(raw.get())?;
        Ok(Self(raw))
    }

    /// The claim set as JSON text, as Golem received it, without the whitespace around it.
    pub fn as_json(&self) -> &str {
        self.0.get()
    }
}

/// One open container of the claim set while `check_member_names` scans it.
enum Frame {
    /// An object, with the names seen so far and whether the next string is a member name.
    Object {
        names: HashSet<String>,
        name_next: bool,
    },
    Array,
}

/// Checks that no object in `json` repeats a member name, at any depth, and that no string holds
/// an escaped lone surrogate. Names are compared after their escapes are decoded, so `"a/b"` and
/// `"a\/b"` are one name. `json` must already be valid JSON (`RawValue` checked it), so a string
/// that does not decode is an escaped lone surrogate. Numbers and literals are skipped, never
/// read as numbers. The scan keeps its own stack of frames, so the Rust stack depth does not
/// depend on the nesting of the input, and time and memory are linear in its length.
fn check_member_names(json: &str) -> Result<(), ClaimsError> {
    let bytes = json.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => stack.push(Frame::Object {
                names: HashSet::new(),
                name_next: true,
            }),
            b'[' => stack.push(Frame::Array),
            b'}' | b']' => {
                stack.pop();
            }
            b',' => {
                if let Some(Frame::Object { name_next, .. }) = stack.last_mut() {
                    *name_next = true;
                }
            }
            b'"' => {
                let start = i;
                i += 1;
                while bytes[i] != b'"' {
                    // A backslash escapes the next byte, so skip both.
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                let token = &json[start..=i];
                let text = serde_json::from_str::<String>(token)
                    .map_err(|_| ClaimsError::LoneSurrogate)?;
                if let Some(Frame::Object { names, name_next }) = stack.last_mut()
                    && *name_next
                {
                    *name_next = false;
                    if names.contains(&text) {
                        return Err(ClaimsError::DuplicateName(text));
                    }
                    names.insert(text);
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
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
    #[error("the claim set has the member name {0:?} more than once in one object")]
    DuplicateName(String),
    #[error("the claim set has an escaped lone surrogate, which RFC 7493 (I-JSON) forbids")]
    LoneSurrogate,
}

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for Claims {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::BinarySerializer::serialize(&self.as_json(), context)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for Claims {
    fn deserialize(
        context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        let json = <String as desert_rust::BinaryDeserializer>::deserialize(context)?;
        Claims::from_json(json).map_err(|error| {
            desert_rust::Error::DeserializationFailure(format!("Invalid claims: {error}"))
        })
    }
}

/// A token that the request presented at the entry point, with its maker. It names the token by
/// its ID and never holds the token's value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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

/// The stored form of a [`Caller`]: the immediate principal is absent when it is the same record
/// as the original one, so that the claims of an entry-point caller are stored once.
#[cfg(feature = "full")]
#[derive(desert_rust::BinaryCodec)]
#[desert(evolution())]
struct CallerRecord {
    original: Principal,
    immediate: Option<Principal>,
    tokens: Vec<TokenProvenance>,
}

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for Caller {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::BinarySerializer::serialize(
            &CallerRecord {
                original: self.original.clone(),
                immediate: (!self.original.same_record(&self.immediate))
                    .then(|| self.immediate.clone()),
                tokens: self.tokens.clone(),
            },
            context,
        )
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for Caller {
    fn deserialize(
        context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        let record: CallerRecord = desert_rust::BinaryDeserializer::deserialize(context)?;
        Ok(Self {
            immediate: record.immediate.unwrap_or_else(|| record.original.clone()),
            original: record.original,
            tokens: record.tokens,
        })
    }
}

impl Caller {
    /// The caller at an entry point: one principal in both roles, and no tokens.
    pub fn entry(principal: Principal) -> Self {
        Self {
            original: principal.clone(),
            immediate: principal,
            tokens: Vec::new(),
        }
    }

    pub fn anonymous() -> Self {
        Self::entry(Principal::Anonymous)
    }

    /// The caller of a call that an agent or tool makes inside the invocation that has this
    /// caller: the same original caller and tokens, and `immediate` as the immediate caller.
    pub fn propagate(&self, immediate: Principal) -> Caller {
        Caller {
            original: self.original.clone(),
            immediate,
            tokens: self.tokens.clone(),
        }
    }
}

/// The same recorded caller: every field is equal, and claims are equal byte for byte.
///
/// This is not identity. Identity is [`Principal::equals`], which compares only pairs. A recorded
/// request and its retry are the same request exactly when their callers are the same record.
impl PartialEq for Caller {
    fn eq(&self, other: &Self) -> bool {
        self.original.same_record(&other.original)
            && self.immediate.same_record(&other.immediate)
            && self.tokens == other.tokens
    }
}

impl Eq for Caller {}

impl Principal {
    /// True exactly when every field of both principals is equal, claims byte for byte.
    pub fn same_record(&self, other: &Principal) -> bool {
        match (self, other) {
            (Principal::User(a), Principal::User(b)) => {
                a.pair == b.pair
                    && a.claims.as_json() == b.claims.as_json()
                    && a.authenticated_at == b.authenticated_at
            }
            (Principal::Account(a), Principal::Account(b)) => {
                a.issuer == b.issuer
                    && a.account_id == b.account_id
                    && a.authentication == b.authentication
            }
            (Principal::Agent(a), Principal::Agent(b)) => {
                a.issuer == b.issuer
                    && a.agent_id == b.agent_id
                    && a.address == b.address
                    && a.fingerprint == b.fingerprint
                    && a.agent_type == b.agent_type
                    && a.environment_id == b.environment_id
            }
            (Principal::Tool(a), Principal::Tool(b)) => {
                a.issuer == b.issuer
                    && a.environment_id == b.environment_id
                    && a.tool_name == b.tool_name
            }
            (Principal::Anonymous, Principal::Anonymous) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;

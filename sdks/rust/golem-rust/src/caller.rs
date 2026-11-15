// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Who caused the running invocation.
//!
//! An agent method, constructor, tool or tool middleware that wants to know its caller declares
//! an [`InvocationContext`] parameter and reads [`InvocationContext::caller`]. The context is
//! injected by the SDK: it is not part of the method's input schema.
//!
//! A [`Caller`] has two roles, `original` (the principal that started the chain) and `immediate`
//! (the principal that made this call), plus the provenance of the tokens it carries.
//!
//! [`Principal`] and [`Caller`] are live values and have no schema. To store who called, keep
//! [`Caller::original`]'s [`PrincipalRef`], which is plain data.

#[cfg(not(feature = "export_golem_agentic"))]
use crate::bindings::golem::agent::common as wit;
#[cfg(feature = "export_golem_agentic")]
use crate::golem_agentic::golem::agent::common as wit;
use crate::{FromSchema, FromWire, IntoSchema, IntoWire, WireSchema};
use std::fmt::{self, Display, Formatter, Write};
use std::str::FromStr;
use std::time::{Duration, UNIX_EPOCH};

/// The text of the build error that every attempt to use a [`Principal`], a [`Caller`] or a
/// [`TokenProvenance`] as data produces.
macro_rules! rule_8_message {
    () => {
        "Principal and Caller cannot appear in a method, constructor or tool signature, or inside a value type. Read the caller with ctx.caller() (golem_rust::agentic::InvocationContext), and store ctx.caller().original().principal_ref() as a PrincipalRef."
    };
}

/// The identity of one principal: an issuer and a subject.
///
/// A reference is data: holding one grants nothing. Two references name the same principal
/// exactly when both strings are equal, byte for byte. The text form, from [`Display`] and
/// [`PrincipalRef::parse`], is `("<issuer>","<subject>")`, and the parser refuses every other
/// spelling.
///
/// As method data it is the named record `principal-ref { issuer, subject }`.
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Hash,
    PartialOrd,
    Ord,
    IntoSchema,
    FromSchema,
    IntoWire,
    FromWire,
    WireSchema,
)]
#[schema(named = "principal-ref")]
pub struct PrincipalRef {
    issuer: String,
    subject: String,
}

impl PrincipalRef {
    pub fn new(issuer: impl Into<String>, subject: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            subject: subject.into(),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Reads the canonical text form `("<issuer>","<subject>")`.
    pub fn parse(text: &str) -> Result<Self, PrincipalRefParseError> {
        let mut parser = Parser { text, position: 0 };
        parser.expect('(', "`(`")?;
        let issuer = parser.string()?;
        parser.expect(',', "`,`")?;
        let subject = parser.string()?;
        parser.expect(')', "`)`")?;
        if parser.position < text.len() {
            return Err(PrincipalRefParseError::TrailingText {
                position: parser.position,
            });
        }
        Ok(Self { issuer, subject })
    }

    /// Byte equality of both strings.
    pub fn equals(&self, other: &PrincipalRef) -> bool {
        self == other
    }
}

impl Display for PrincipalRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_char('(')?;
        write_string(f, &self.issuer)?;
        f.write_char(',')?;
        write_string(f, &self.subject)?;
        f.write_char(')')
    }
}

impl FromStr for PrincipalRef {
    type Err = PrincipalRefParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl From<wit::PrincipalRef> for PrincipalRef {
    fn from(value: wit::PrincipalRef) -> Self {
        Self {
            issuer: value.issuer,
            subject: value.subject,
        }
    }
}

impl From<PrincipalRef> for wit::PrincipalRef {
    fn from(value: PrincipalRef) -> Self {
        Self {
            issuer: value.issuer,
            subject: value.subject,
        }
    }
}

/// Writes `value` as a quoted string, escaped the way RFC 8785 serializes JSON strings.
fn write_string(f: &mut Formatter<'_>, value: &str) -> fmt::Result {
    f.write_char('"')?;
    for character in value.chars() {
        match character {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\u{08}' => f.write_str("\\b")?,
            '\t' => f.write_str("\\t")?,
            '\n' => f.write_str("\\n")?,
            '\u{0c}' => f.write_str("\\f")?,
            '\r' => f.write_str("\\r")?,
            control if control < '\u{20}' => write!(f, "\\u{:04x}", control as u32)?,
            other => f.write_char(other)?,
        }
    }
    f.write_char('"')
}

/// Why a text is not the canonical text form of a [`PrincipalRef`]. Each position is a byte
/// offset into the text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrincipalRefParseError {
    Expected {
        expected: &'static str,
        position: usize,
    },
    UnterminatedString {
        position: usize,
    },
    UnescapedControlCharacter {
        position: usize,
    },
    NonCanonicalEscape {
        position: usize,
    },
    TrailingText {
        position: usize,
    },
}

impl Display for PrincipalRefParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expected { expected, position } => {
                write!(f, "expected {expected} at byte {position}")
            }
            Self::UnterminatedString { position } => {
                write!(f, "the string that starts at byte {position} is not closed")
            }
            Self::UnescapedControlCharacter { position } => {
                write!(f, "unescaped control character at byte {position}")
            }
            Self::NonCanonicalEscape { position } => {
                write!(f, "non-canonical escape at byte {position}")
            }
            Self::TrailingText { position } => write!(f, "trailing text at byte {position}"),
        }
    }
}

impl std::error::Error for PrincipalRefParseError {}

struct Parser<'a> {
    text: &'a str,
    position: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.position..].chars().next()
    }

    fn expect(&mut self, expected: char, name: &'static str) -> Result<(), PrincipalRefParseError> {
        if self.peek() == Some(expected) {
            self.position += expected.len_utf8();
            Ok(())
        } else {
            Err(PrincipalRefParseError::Expected {
                expected: name,
                position: self.position,
            })
        }
    }

    fn string(&mut self) -> Result<String, PrincipalRefParseError> {
        let start = self.position;
        self.expect('"', "`\"`")?;
        let mut value = String::new();
        loop {
            let at = self.position;
            let character = self
                .peek()
                .ok_or(PrincipalRefParseError::UnterminatedString { position: start })?;
            self.position += character.len_utf8();
            match character {
                '"' => return Ok(value),
                '\\' => value.push(self.escape(at)?),
                control if control < '\u{20}' => {
                    return Err(PrincipalRefParseError::UnescapedControlCharacter { position: at });
                }
                other => value.push(other),
            }
        }
    }

    /// Reads the escape after a backslash at `at`. Only the escapes that the printer writes are
    /// accepted, so every character has exactly one spelling.
    fn escape(&mut self, at: usize) -> Result<char, PrincipalRefParseError> {
        let non_canonical = PrincipalRefParseError::NonCanonicalEscape { position: at };
        let character = self.peek().ok_or(non_canonical.clone())?;
        self.position += character.len_utf8();
        match character {
            '"' => Ok('"'),
            '\\' => Ok('\\'),
            'b' => Ok('\u{08}'),
            't' => Ok('\t'),
            'n' => Ok('\n'),
            'f' => Ok('\u{0c}'),
            'r' => Ok('\r'),
            'u' => {
                let digits = self
                    .text
                    .get(self.position..self.position + 4)
                    .ok_or(non_canonical.clone())?;
                if !digits
                    .bytes()
                    .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(non_canonical);
                }
                let code = u32::from_str_radix(digits, 16).map_err(|_| non_canonical.clone())?;
                let has_short_escape = matches!(code, 0x08 | 0x09 | 0x0a | 0x0c | 0x0d);
                if code >= 0x20 || has_short_escape {
                    return Err(non_canonical);
                }
                self.position += 4;
                char::from_u32(code).ok_or(non_canonical)
            }
            _ => Err(non_canonical),
        }
    }
}

/// The kind of a [`Principal`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PrincipalKind {
    User,
    Account,
    Agent,
    Tool,
    Anonymous,
}

impl PrincipalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Account => "account",
            Self::Agent => "agent",
            Self::Tool => "tool",
            Self::Anonymous => "anonymous",
        }
    }
}

impl Display for PrincipalKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A person who signed in through an identity provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserPrincipal {
    principal_ref: PrincipalRef,
    claims: String,
    authenticated_at: (i64, u32),
}

impl UserPrincipal {
    pub fn principal_ref(&self) -> &PrincipalRef {
        &self.principal_ref
    }

    pub fn issuer(&self) -> &str {
        self.principal_ref.issuer()
    }

    pub fn subject(&self) -> &str {
        self.principal_ref.subject()
    }

    /// The claims of the sign-in as the exact JSON object text that the host recorded.
    pub fn claims_json(&self) -> &str {
        &self.claims
    }

    /// The claims, parsed on demand.
    #[cfg(any(feature = "json", feature = "export_golem_agentic"))]
    pub fn claims(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(&self.claims)
    }

    /// When the user signed in.
    pub fn authenticated_at(&self) -> std::time::SystemTime {
        let (seconds, nanoseconds) = self.authenticated_at;
        let nanos = Duration::from_nanos(u64::from(nanoseconds));
        if seconds >= 0 {
            UNIX_EPOCH + Duration::from_secs(seconds as u64) + nanos
        } else {
            UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs()) + nanos
        }
    }
}

/// How an account principal proved itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AccountAuthentication {
    ApiToken,
    BootstrapToken,
    SignIn,
}

/// A Golem account, as seen through the CLI, the REST API or a token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountPrincipal {
    principal_ref: PrincipalRef,
    account_id: crate::golem_schema::AccountId,
    authentication: AccountAuthentication,
}

impl AccountPrincipal {
    pub fn principal_ref(&self) -> &PrincipalRef {
        &self.principal_ref
    }

    pub fn account_id(&self) -> &crate::golem_schema::AccountId {
        &self.account_id
    }

    pub fn authentication(&self) -> AccountAuthentication {
        self.authentication
    }
}

/// A running agent that made a call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentPrincipal {
    principal_ref: PrincipalRef,
    agent_id: crate::AgentId,
    agent_type: String,
    environment_id: crate::EnvironmentId,
}

impl AgentPrincipal {
    pub fn principal_ref(&self) -> &PrincipalRef {
        &self.principal_ref
    }

    pub fn agent_id(&self) -> &crate::AgentId {
        &self.agent_id
    }

    pub fn agent_type(&self) -> &str {
        &self.agent_type
    }

    pub fn environment_id(&self) -> &crate::EnvironmentId {
        &self.environment_id
    }
}

/// A tool that made a call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolPrincipal {
    principal_ref: PrincipalRef,
    environment_id: crate::EnvironmentId,
    tool_name: String,
}

impl ToolPrincipal {
    pub fn principal_ref(&self) -> &PrincipalRef {
        &self.principal_ref
    }

    pub fn environment_id(&self) -> &crate::EnvironmentId {
        &self.environment_id
    }

    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
}

/// One side of a call: a user, an account, an agent, a tool or nobody.
///
/// A principal is a live value, not data. It has no schema: store [`Principal::principal_ref`]
/// instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Principal {
    User(UserPrincipal),
    Account(AccountPrincipal),
    Agent(AgentPrincipal),
    Tool(ToolPrincipal),
    Anonymous,
}

impl Principal {
    pub fn kind(&self) -> PrincipalKind {
        match self {
            Self::User(_) => PrincipalKind::User,
            Self::Account(_) => PrincipalKind::Account,
            Self::Agent(_) => PrincipalKind::Agent,
            Self::Tool(_) => PrincipalKind::Tool,
            Self::Anonymous => PrincipalKind::Anonymous,
        }
    }

    /// The identity of this principal, or `None` for an anonymous one.
    pub fn principal_ref(&self) -> Option<&PrincipalRef> {
        match self {
            Self::User(user) => Some(&user.principal_ref),
            Self::Account(account) => Some(&account.principal_ref),
            Self::Agent(agent) => Some(&agent.principal_ref),
            Self::Tool(tool) => Some(&tool.principal_ref),
            Self::Anonymous => None,
        }
    }

    /// Whether this principal is the one `other` names. An anonymous principal equals nothing.
    pub fn equals(&self, other: &Principal) -> bool {
        match (self.principal_ref(), other.principal_ref()) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        }
    }

    /// Whether this principal is the one that `other` names. An anonymous principal equals
    /// nothing.
    pub fn equals_ref(&self, other: &PrincipalRef) -> bool {
        self.principal_ref() == Some(other)
    }
}

/// `"<kind> <ref text>"`, or `"anonymous"`. It never includes claims.
impl Display for Principal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.principal_ref() {
            Some(principal_ref) => write!(f, "{} {}", self.kind(), principal_ref),
            None => f.write_str("anonymous"),
        }
    }
}

impl From<wit::Principal> for Principal {
    fn from(value: wit::Principal) -> Self {
        match value {
            wit::Principal::User(user) => Self::User(UserPrincipal {
                principal_ref: user.pair.into(),
                claims: user.claims,
                authenticated_at: (
                    user.authenticated_at.seconds,
                    user.authenticated_at.nanoseconds,
                ),
            }),
            wit::Principal::Account(account) => Self::Account(AccountPrincipal {
                principal_ref: account.pair.into(),
                account_id: account.account_id.into(),
                authentication: match account.authentication {
                    wit::AccountAuthentication::ApiToken => AccountAuthentication::ApiToken,
                    wit::AccountAuthentication::BootstrapToken => {
                        AccountAuthentication::BootstrapToken
                    }
                    wit::AccountAuthentication::SignIn => AccountAuthentication::SignIn,
                },
            }),
            wit::Principal::Agent(agent) => Self::Agent(AgentPrincipal {
                principal_ref: agent.pair.into(),
                agent_id: agent.agent_id.into(),
                agent_type: agent.agent_type,
                environment_id: agent.environment_id.into(),
            }),
            wit::Principal::Tool(tool) => Self::Tool(ToolPrincipal {
                principal_ref: tool.pair.into(),
                environment_id: tool.environment_id.into(),
                tool_name: tool.tool_name,
            }),
            wit::Principal::Anonymous => Self::Anonymous,
        }
    }
}

/// Who made a token and which token it is. The id is the token's own, never its secret.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenProvenance {
    id: crate::Uuid,
    maker: Option<PrincipalRef>,
}

impl TokenProvenance {
    pub fn id(&self) -> crate::Uuid {
        self.id
    }

    pub fn maker(&self) -> Option<&PrincipalRef> {
        self.maker.as_ref()
    }
}

impl From<wit::TokenProvenance> for TokenProvenance {
    fn from(value: wit::TokenProvenance) -> Self {
        Self {
            id: value.id.into(),
            maker: value.maker.map(Into::into),
        }
    }
}

/// Who caused the running invocation: the `original` principal that started the chain, the
/// `immediate` principal that made this call, and the provenance of the tokens on the way.
///
/// A caller is a live value, not data. It has no schema: store
/// `caller.original().principal_ref()` instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Caller {
    original: Principal,
    immediate: Principal,
    tokens: Vec<TokenProvenance>,
}

impl Caller {
    /// A caller with no identity: an anonymous principal in both roles and no tokens.
    pub fn anonymous() -> Self {
        Self {
            original: Principal::Anonymous,
            immediate: Principal::Anonymous,
            tokens: Vec::new(),
        }
    }

    pub fn original(&self) -> &Principal {
        &self.original
    }

    pub fn immediate(&self) -> &Principal {
        &self.immediate
    }

    pub fn tokens(&self) -> &[TokenProvenance] {
        &self.tokens
    }
}

impl From<wit::Caller> for Caller {
    fn from(value: wit::Caller) -> Self {
        Self {
            original: value.original.into(),
            immediate: value.immediate.into(),
            tokens: value.tokens.into_iter().map(Into::into).collect(),
        }
    }
}

/// What the host tells an invocation about itself. Declare a parameter of this type in an agent
/// constructor or method, a tool or a tool middleware to receive it. It is not part of the
/// input schema and callers do not supply it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationContext {
    caller: Caller,
}

impl InvocationContext {
    /// The context of the running invocation, read from the host.
    #[cfg(feature = "export_golem_agentic")]
    pub(crate) fn current() -> Self {
        Self {
            caller: crate::golem_agentic::golem::agent::invocation_context::caller().into(),
        }
    }

    /// The context of an invocation that nobody is behind: an anonymous caller.
    pub fn anonymous() -> Self {
        Self::from_caller(Caller::anonymous())
    }

    /// The context of an invocation made by `caller`.
    pub fn from_caller(caller: Caller) -> Self {
        Self { caller }
    }

    pub fn caller(&self) -> &Caller {
        &self.caller
    }
}

/// Marks the types that are live values and never data: a [`Principal`], a [`Caller`] and a
/// [`TokenProvenance`], and the containers of one.
#[doc(hidden)]
pub trait LiveValue {}

impl LiveValue for Principal {}
impl LiveValue for Caller {}
impl LiveValue for TokenProvenance {}
impl<T: LiveValue> LiveValue for Vec<T> {}
impl<T: LiveValue> LiveValue for Option<T> {}
impl<T: LiveValue + ?Sized> LiveValue for &T {}
impl<T: LiveValue + ?Sized> LiveValue for Box<T> {}

/// Referenced by the code that a signature expands to, when its type is a live value: it ends
/// the build with the message below.
#[doc(hidden)]
pub struct Rule8<T: ?Sized>(std::marker::PhantomData<T>);

impl<T: ?Sized> Rule8<T> {
    #[doc(hidden)]
    pub const REJECTED: () = panic!(rule_8_message!());
}

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

//! The format of an identity proof: a short-lived JWT, in JWS compact form, that states an agent's
//! or a tool's own pair for one audience. These types fix the header and the claim set that the
//! builder signs and that a receiver checks; signing and checking are not here.

use super::{AgentPrincipal, Kind, ToolPrincipal};
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool::ToolName;
use serde::{Deserialize, Serialize};

/// The signature algorithm of every identity proof. AWS STS and SPIFFE JWT-SVID do not accept
/// EdDSA, and ES256 keeps proofs short.
pub const IDENTITY_PROOF_ALGORITHM: &str = "ES256";

/// The `typ` header of every identity proof, for generic JWT receivers.
pub const IDENTITY_PROOF_TYPE: &str = "JWT";

/// `exp` is this many seconds after `iat`. Golem makes a new proof for each request, so a short
/// lifetime costs nothing and limits a leaked proof.
pub const IDENTITY_PROOF_LIFETIME_SECONDS: u64 = 300;

/// The audience of a proof for AWS role assumption, unless the receiver sets another one.
pub const AWS_STS_AUDIENCE: &str = "sts.amazonaws.com";

/// The claim that carries AWS session tags. An AWS trust policy cannot read a nested claim, so a
/// proof for AWS role assumption repeats the values of the `golem` claim as session tags.
pub const AWS_SESSION_TAGS_CLAIM: &str = "https://aws.amazon.com/tags";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityProofHeader {
    pub alg: String,
    /// The ID of the issuer key that signs the proof, in the issuer's key set.
    pub kid: String,
    pub typ: String,
}

impl IdentityProofHeader {
    pub fn new(kid: impl Into<String>) -> Self {
        Self {
            alg: IDENTITY_PROOF_ALGORITHM.to_string(),
            kid: kid.into(),
            typ: IDENTITY_PROOF_TYPE.to_string(),
        }
    }
}

/// The principal that a proof names. A proof only ever names the executing principal's own pair.
#[derive(Debug, Clone, Copy)]
pub enum ProofSubject<'a> {
    Agent(&'a AgentPrincipal),
    Tool(&'a ToolPrincipal),
}

/// Who receives the proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofReceiver {
    /// A receiver that reads the `golem` claim, for one audience.
    Audience(String),
    /// AWS STS role assumption with a web identity. The proof also carries AWS session tags.
    AwsRoleAssumption { audience: String },
}

impl ProofReceiver {
    /// AWS STS role assumption with the default audience.
    pub fn aws_sts() -> Self {
        ProofReceiver::AwsRoleAssumption {
            audience: AWS_STS_AUDIENCE.to_string(),
        }
    }
}

/// The claim set of an identity proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityProofClaims {
    /// The Golem issuer of the principal.
    pub iss: String,
    /// The principal's Golem subject.
    pub sub: String,
    /// The one audience of the proof.
    pub aud: String,
    pub iat: u64,
    pub exp: u64,
    /// Unique for each proof.
    pub jti: String,
    pub golem: GolemProofClaim,
    #[serde(
        rename = "https://aws.amazon.com/tags",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub aws_session_tags: Option<AwsSessionTags>,
}

impl IdentityProofClaims {
    /// The claim set of a proof for `subject`, issued at `issued_at` (seconds since the Unix
    /// epoch). `account_id` is the stable ID of the account that owns the principal's
    /// environment, never the account's path.
    ///
    /// This fails only for an AWS role assumption whose session tag values AWS STS would refuse
    /// (see [`AwsSessionTags::from_golem_claim`]); a proof for any other receiver never fails.
    pub fn new(
        subject: ProofSubject<'_>,
        account_id: AccountId,
        receiver: &ProofReceiver,
        issued_at: u64,
        jti: impl Into<String>,
    ) -> Result<Self, AwsSessionTagError> {
        let (pair, golem) = match subject {
            ProofSubject::Agent(agent) => (
                agent.pair(),
                GolemProofClaim::Agent {
                    environment: agent.environment_id,
                    account: account_id,
                    agent_type: agent.agent_type.clone(),
                },
            ),
            ProofSubject::Tool(tool) => (
                tool.pair(),
                GolemProofClaim::Tool {
                    environment: tool.environment_id,
                    account: account_id,
                    tool: tool.tool_name.clone(),
                },
            ),
        };
        let (audience, aws_session_tags) = match receiver {
            ProofReceiver::Audience(audience) => (audience.clone(), None),
            ProofReceiver::AwsRoleAssumption { audience } => (
                audience.clone(),
                Some(AwsSessionTags::from_golem_claim(&golem)?),
            ),
        };
        Ok(Self {
            iss: pair.issuer,
            sub: pair.subject,
            aud: audience,
            iat: issued_at,
            exp: issued_at + IDENTITY_PROOF_LIFETIME_SECONDS,
            jti: jti.into(),
            golem,
            aws_session_tags,
        })
    }
}

/// The nested `golem` claim: what the principal is, and where it lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GolemProofClaim {
    Agent {
        environment: EnvironmentId,
        account: AccountId,
        #[serde(rename = "agentType")]
        agent_type: AgentTypeName,
    },
    Tool {
        environment: EnvironmentId,
        account: AccountId,
        tool: ToolName,
    },
}

impl GolemProofClaim {
    pub fn kind(&self) -> Kind {
        match self {
            GolemProofClaim::Agent { .. } => Kind::Agent,
            GolemProofClaim::Tool { .. } => Kind::Tool,
        }
    }
}

/// The AWS session tags of a proof for AWS role assumption. There are no transitive tag keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwsSessionTags {
    pub principal_tags: AwsPrincipalTags,
}

/// The longest tag value that AWS STS accepts, in characters. The IAM User Guide, "Passing session
/// tags in AWS STS": "The plaintext of your session tag values exceeds 256 characters" makes the
/// operation fail.
pub const AWS_SESSION_TAG_VALUE_MAX_CHARACTERS: usize = 256;

/// The prefix that AWS reserves in tag keys and values (IAM User Guide, "Rules for tagging in IAM
/// and AWS STS": "You cannot create a tag key or value that begins with the text aws:").
const AWS_RESERVED_TAG_PREFIX: &str = "aws:";

/// A session tag value that AWS STS would refuse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AwsSessionTagError {
    #[error(
        "the AWS session tag {tag} has a value of {length} characters, and AWS allows at most 256"
    )]
    ValueTooLong { tag: &'static str, length: usize },
    #[error(
        "the AWS session tag {tag} has a value with the character {character:?}, which AWS refuses (letters, numbers, spaces and _ . : / = + - @ are allowed)"
    )]
    CharacterRefused { tag: &'static str, character: char },
    #[error("the AWS session tag {tag} has a value that begins with aws:, which AWS reserves")]
    ReservedPrefix { tag: &'static str },
}

impl AwsSessionTags {
    /// The session tags that repeat the values of a `golem` claim.
    ///
    /// Every tag value is checked against the rules that the AWS documentation states: at most
    /// 256 characters ("Passing session tags in AWS STS"); letters, numbers, spaces and
    /// `_ . : / = + - @` only, and no `aws:` prefix ("Rules for tagging in IAM and AWS STS").
    /// "Letters" and "numbers" are read as the Unicode letters and numbers (`char::is_alphanumeric`)
    /// and "spaces" as U+0020 only. The pages do not say more, so the real-AWS test of the
    /// 1.5 plan decides the final form of these rules.
    pub fn from_golem_claim(golem: &GolemProofClaim) -> Result<Self, AwsSessionTagError> {
        let (environment, account, agent_type, tool) = match golem {
            GolemProofClaim::Agent {
                environment,
                account,
                agent_type,
            } => (environment, account, Some(agent_type.to_string()), None),
            GolemProofClaim::Tool {
                environment,
                account,
                tool,
            } => (environment, account, None, Some(tool.to_string())),
        };
        let environment = check_tag_value("golem:environment", environment.to_string())?;
        let account = check_tag_value("golem:account", account.to_string())?;
        let kind = check_tag_value("golem:kind", golem.kind().to_string())?;
        let agent_type = agent_type
            .map(|value| check_tag_value("golem:agent-type", value))
            .transpose()?;
        let tool = tool
            .map(|value| check_tag_value("golem:tool", value))
            .transpose()?;
        Ok(Self {
            principal_tags: AwsPrincipalTags {
                environment: [environment],
                account: [account],
                kind: [kind],
                agent_type: agent_type.map(|value| [value]),
                tool: tool.map(|value| [value]),
            },
        })
    }
}

fn check_tag_value(tag: &'static str, value: String) -> Result<String, AwsSessionTagError> {
    let length = value.chars().count();
    if length > AWS_SESSION_TAG_VALUE_MAX_CHARACTERS {
        return Err(AwsSessionTagError::ValueTooLong { tag, length });
    }
    if let Some(character) = value.chars().find(|c| {
        !(c.is_alphanumeric() || matches!(c, ' ' | '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@'))
    }) {
        return Err(AwsSessionTagError::CharacterRefused { tag, character });
    }
    if value.starts_with(AWS_RESERVED_TAG_PREFIX) {
        return Err(AwsSessionTagError::ReservedPrefix { tag });
    }
    Ok(value)
}

/// Each tag holds one value, in the one-element list form of the example on the AWS page "Passing
/// session tags in AWS STS". The same page says in its text that "Each principal tag is a single
/// string value" and "You must pass a single value for each session tag", so the page does not
/// agree with itself on the form. The real-AWS test of the 1.5 plan decides between the list and
/// the plain string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwsPrincipalTags {
    #[serde(rename = "golem:environment")]
    pub environment: [String; 1],
    #[serde(rename = "golem:account")]
    pub account: [String; 1],
    #[serde(rename = "golem:kind")]
    pub kind: [String; 1],
    #[serde(
        rename = "golem:agent-type",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_type: Option<[String; 1]>,
    #[serde(
        rename = "golem:tool",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub tool: Option<[String; 1]>,
}

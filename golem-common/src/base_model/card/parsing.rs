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

use crate::base_model::card::*;
use nom::IResult;
use nom::bytes::complete::{tag, take_until};
use nom::character::complete::{char, multispace0};
use nom::combinator::{all_consuming, rest};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardParseError {
    MissingAtSeparator,
    MissingClassOpenParen,
    MissingClassCloseParen,
    MissingRecipient,
    MissingVerb,
    MissingResource,
    InvalidRecipientPath(String),
    InvalidOwnerPath { class: String, owner: String },
    Malformed(String),
    UnknownClass(String),
    UnknownVerb { class: String, verb: String },
    InvalidResource { class: String, resource: String },
    SlotVariableInConcreteGrant(String),
}

impl std::fmt::Display for CardParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingAtSeparator => write!(f, "missing @ separator"),
            Self::MissingClassOpenParen => write!(f, "missing class owner open parenthesis"),
            Self::MissingClassCloseParen => write!(f, "missing class owner close parenthesis"),
            Self::MissingRecipient => write!(f, "missing recipient"),
            Self::MissingVerb => write!(f, "missing verb"),
            Self::MissingResource => write!(f, "missing resource"),
            Self::InvalidRecipientPath(path) => write!(f, "invalid recipient path {path}"),
            Self::InvalidOwnerPath { class, owner } => {
                write!(f, "invalid owner path {owner} for permission class {class}")
            }
            Self::Malformed(message) => write!(f, "malformed card grant: {message}"),
            Self::UnknownClass(class) => write!(f, "unknown permission class {class}"),
            Self::UnknownVerb { class, verb } => {
                write!(f, "unknown verb {verb} for permission class {class}")
            }
            Self::InvalidResource { class, resource } => {
                write!(
                    f,
                    "invalid resource {resource} for permission class {class}"
                )
            }
            Self::SlotVariableInConcreteGrant(value) => {
                write!(
                    f,
                    "slot variable is only valid in polymorphic grant {value}"
                )
            }
        }
    }
}

impl std::error::Error for CardParseError {}

impl FromStr for PatternGrant {
    type Err = CardParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_pattern_grant(value)
    }
}

impl FromStr for PolymorphicPatternGrant {
    type Err = CardParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_polymorphic_pattern_grant(value)
    }
}

pub fn parse_pattern_grant(value: &str) -> Result<PatternGrant, CardParseError> {
    if !value.contains('@') {
        return Err(CardParseError::MissingAtSeparator);
    }

    let (_, parts) = all_consuming(pattern_grant_parts)(value)
        .map_err(|err| CardParseError::Malformed(err.to_string()))?;

    if parts.class.is_empty() {
        return Err(CardParseError::MissingClassOpenParen);
    }
    if parts.owner.is_empty() && !value.contains("()") {
        return Err(CardParseError::MissingClassCloseParen);
    }
    if parts.recipient.is_empty() {
        return Err(CardParseError::MissingRecipient);
    }
    if parts.verb.is_empty() {
        return Err(CardParseError::MissingVerb);
    }
    reject_slot_variables(&parts)?;

    Ok(PatternGrant {
        permission: parse_permission(
            &parts.class,
            &parts.owner,
            &parts.recipient,
            &parts.verb,
            &parts.resource,
        )?,
    })
}

pub fn parse_polymorphic_pattern_grant(
    value: &str,
) -> Result<PolymorphicPatternGrant, CardParseError> {
    if !value.contains('@') {
        return Err(CardParseError::MissingAtSeparator);
    }

    let (_, parts) = all_consuming(pattern_grant_parts)(value)
        .map_err(|err| CardParseError::Malformed(err.to_string()))?;

    if parts.class.is_empty() {
        return Err(CardParseError::MissingClassOpenParen);
    }
    if parts.owner.is_empty() && !value.contains("()") {
        return Err(CardParseError::MissingClassCloseParen);
    }
    if parts.recipient.is_empty() {
        return Err(CardParseError::MissingRecipient);
    }
    if parts.verb.is_empty() {
        return Err(CardParseError::MissingVerb);
    }

    Ok(PolymorphicPatternGrant {
        permission: parse_polymorphic_permission(
            &parts.class,
            &parts.owner,
            &parts.recipient,
            &parts.verb,
            &parts.resource,
        )?,
    })
}

#[derive(Debug, Clone)]
struct PatternGrantParts {
    class: String,
    owner: String,
    recipient: String,
    verb: String,
    resource: String,
}

fn pattern_grant_parts(input: &str) -> IResult<&str, PatternGrantParts> {
    let (input, class) = take_until("(")(input)?;
    let (input, _) = char('(')(input)?;
    let (input, owner) = take_until("@")(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char('@')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, recipient) = take_until(":")(input)?;
    let (input, _) = tag(":")(input)?;
    let (input, verb) = take_until(":")(input)?;
    let (input, _) = tag(":")(input)?;
    let (input, resource) = rest(input)?;

    let owner = owner.trim();
    let owner = owner.strip_suffix(')').unwrap_or(owner).trim();

    Ok((
        input,
        PatternGrantParts {
            class: class.trim().to_string(),
            owner: owner.to_string(),
            recipient: recipient.trim().to_string(),
            verb: verb.trim().to_string(),
            resource: resource.trim().to_string(),
        },
    ))
}

fn reject_slot_variables(parts: &PatternGrantParts) -> Result<(), CardParseError> {
    for value in [&parts.owner, &parts.recipient, &parts.resource] {
        if contains_slot_reference(value) {
            return Err(CardParseError::SlotVariableInConcreteGrant(
                value.to_string(),
            ));
        }
    }
    Ok(())
}

fn parse_permission(
    class: &str,
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PermissionPattern, CardParseError> {
    match class {
        FilesystemClass::NAME => {
            parse_class_permission::<FilesystemClass>(owner, recipient, verb, resource)
        }
        NetworkClass::NAME => {
            parse_class_permission::<NetworkClass>(owner, recipient, verb, resource)
        }
        EnvClass::NAME => parse_class_permission::<EnvClass>(owner, recipient, verb, resource),
        OplogClass::NAME => parse_class_permission::<OplogClass>(owner, recipient, verb, resource),
        ConfigClass::NAME => {
            parse_class_permission::<ConfigClass>(owner, recipient, verb, resource)
        }
        SecretClass::NAME => {
            parse_class_permission::<SecretClass>(owner, recipient, verb, resource)
        }
        AgentClass::NAME => parse_class_permission::<AgentClass>(owner, recipient, verb, resource),
        ToolClass::NAME => parse_class_permission::<ToolClass>(owner, recipient, verb, resource),
        KvClass::NAME => parse_class_permission::<KvClass>(owner, recipient, verb, resource),
        BlobClass::NAME => parse_class_permission::<BlobClass>(owner, recipient, verb, resource),
        RdbmsClass::NAME => parse_class_permission::<RdbmsClass>(owner, recipient, verb, resource),
        CardClass::NAME => parse_class_permission::<CardClass>(owner, recipient, verb, resource),
        SystemClass::NAME => {
            parse_class_permission::<SystemClass>(owner, recipient, verb, resource)
        }
        PlanClass::NAME => parse_class_permission::<PlanClass>(owner, recipient, verb, resource),
        AccountClass::NAME => {
            parse_class_permission::<AccountClass>(owner, recipient, verb, resource)
        }
        AccountUsageClass::NAME => {
            parse_class_permission::<AccountUsageClass>(owner, recipient, verb, resource)
        }
        AccountTokenClass::NAME => {
            parse_class_permission::<AccountTokenClass>(owner, recipient, verb, resource)
        }
        AccountPluginClass::NAME => {
            parse_class_permission::<AccountPluginClass>(owner, recipient, verb, resource)
        }
        ApplicationClass::NAME => {
            parse_class_permission::<ApplicationClass>(owner, recipient, verb, resource)
        }
        EnvironmentClass::NAME => {
            parse_class_permission::<EnvironmentClass>(owner, recipient, verb, resource)
        }
        EnvironmentShareClass::NAME => {
            parse_class_permission::<EnvironmentShareClass>(owner, recipient, verb, resource)
        }
        EnvironmentPluginGrantClass::NAME => {
            parse_class_permission::<EnvironmentPluginGrantClass>(owner, recipient, verb, resource)
        }
        EnvironmentDomainRegistrationClass::NAME => parse_class_permission::<
            EnvironmentDomainRegistrationClass,
        >(owner, recipient, verb, resource),
        EnvironmentSecuritySchemeClass::NAME => parse_class_permission::<
            EnvironmentSecuritySchemeClass,
        >(owner, recipient, verb, resource),
        EnvironmentHttpApiDeploymentClass::NAME => parse_class_permission::<
            EnvironmentHttpApiDeploymentClass,
        >(owner, recipient, verb, resource),
        EnvironmentMcpDeploymentClass::NAME => parse_class_permission::<
            EnvironmentMcpDeploymentClass,
        >(owner, recipient, verb, resource),
        EnvironmentAgentSecretClass::NAME => {
            parse_class_permission::<EnvironmentAgentSecretClass>(owner, recipient, verb, resource)
        }
        EnvironmentResourceDefinitionClass::NAME => parse_class_permission::<
            EnvironmentResourceDefinitionClass,
        >(owner, recipient, verb, resource),
        EnvironmentRetryPolicyClass::NAME => {
            parse_class_permission::<EnvironmentRetryPolicyClass>(owner, recipient, verb, resource)
        }
        ComponentClass::NAME => {
            parse_class_permission::<ComponentClass>(owner, recipient, verb, resource)
        }
        AccountOauth2IdentityClass::NAME => {
            parse_class_permission::<AccountOauth2IdentityClass>(owner, recipient, verb, resource)
        }
        EnvironmentInitialFilesClass::NAME => {
            parse_class_permission::<EnvironmentInitialFilesClass>(owner, recipient, verb, resource)
        }
        EnvironmentKvBucketClass::NAME => {
            parse_class_permission::<EnvironmentKvBucketClass>(owner, recipient, verb, resource)
        }
        EnvironmentBlobBucketClass::NAME => {
            parse_class_permission::<EnvironmentBlobBucketClass>(owner, recipient, verb, resource)
        }
        _ => Err(CardParseError::UnknownClass(class.to_string())),
    }
}

fn parse_polymorphic_permission(
    class: &str,
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PolymorphicPermissionPattern, CardParseError> {
    match class {
        FilesystemClass::NAME => {
            parse_polymorphic_class_permission::<FilesystemClass>(owner, recipient, verb, resource)
        }
        NetworkClass::NAME => {
            parse_polymorphic_class_permission::<NetworkClass>(owner, recipient, verb, resource)
        }
        EnvClass::NAME => {
            parse_polymorphic_class_permission::<EnvClass>(owner, recipient, verb, resource)
        }
        OplogClass::NAME => {
            parse_polymorphic_class_permission::<OplogClass>(owner, recipient, verb, resource)
        }
        ConfigClass::NAME => {
            parse_polymorphic_class_permission::<ConfigClass>(owner, recipient, verb, resource)
        }
        SecretClass::NAME => {
            parse_polymorphic_class_permission::<SecretClass>(owner, recipient, verb, resource)
        }
        AgentClass::NAME => {
            parse_polymorphic_class_permission::<AgentClass>(owner, recipient, verb, resource)
        }
        ToolClass::NAME => {
            parse_polymorphic_class_permission::<ToolClass>(owner, recipient, verb, resource)
        }
        KvClass::NAME => {
            parse_polymorphic_class_permission::<KvClass>(owner, recipient, verb, resource)
        }
        BlobClass::NAME => {
            parse_polymorphic_class_permission::<BlobClass>(owner, recipient, verb, resource)
        }
        RdbmsClass::NAME => {
            parse_polymorphic_class_permission::<RdbmsClass>(owner, recipient, verb, resource)
        }
        CardClass::NAME => {
            parse_polymorphic_class_permission::<CardClass>(owner, recipient, verb, resource)
        }
        SystemClass::NAME => {
            parse_polymorphic_class_permission::<SystemClass>(owner, recipient, verb, resource)
        }
        PlanClass::NAME => {
            parse_polymorphic_class_permission::<PlanClass>(owner, recipient, verb, resource)
        }
        AccountClass::NAME => {
            parse_polymorphic_class_permission::<AccountClass>(owner, recipient, verb, resource)
        }
        AccountUsageClass::NAME => parse_polymorphic_class_permission::<AccountUsageClass>(
            owner, recipient, verb, resource,
        ),
        AccountTokenClass::NAME => parse_polymorphic_class_permission::<AccountTokenClass>(
            owner, recipient, verb, resource,
        ),
        AccountPluginClass::NAME => parse_polymorphic_class_permission::<AccountPluginClass>(
            owner, recipient, verb, resource,
        ),
        ApplicationClass::NAME => {
            parse_polymorphic_class_permission::<ApplicationClass>(owner, recipient, verb, resource)
        }
        EnvironmentClass::NAME => {
            parse_polymorphic_class_permission::<EnvironmentClass>(owner, recipient, verb, resource)
        }
        EnvironmentShareClass::NAME => parse_polymorphic_class_permission::<EnvironmentShareClass>(
            owner, recipient, verb, resource,
        ),
        EnvironmentPluginGrantClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentPluginGrantClass,
        >(owner, recipient, verb, resource),
        EnvironmentDomainRegistrationClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentDomainRegistrationClass,
        >(owner, recipient, verb, resource),
        EnvironmentSecuritySchemeClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentSecuritySchemeClass,
        >(owner, recipient, verb, resource),
        EnvironmentHttpApiDeploymentClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentHttpApiDeploymentClass,
        >(owner, recipient, verb, resource),
        EnvironmentMcpDeploymentClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentMcpDeploymentClass,
        >(owner, recipient, verb, resource),
        EnvironmentAgentSecretClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentAgentSecretClass,
        >(owner, recipient, verb, resource),
        EnvironmentResourceDefinitionClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentResourceDefinitionClass,
        >(owner, recipient, verb, resource),
        EnvironmentRetryPolicyClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentRetryPolicyClass,
        >(owner, recipient, verb, resource),
        ComponentClass::NAME => {
            parse_polymorphic_class_permission::<ComponentClass>(owner, recipient, verb, resource)
        }
        AccountOauth2IdentityClass::NAME => parse_polymorphic_class_permission::<
            AccountOauth2IdentityClass,
        >(owner, recipient, verb, resource),
        EnvironmentInitialFilesClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentInitialFilesClass,
        >(owner, recipient, verb, resource),
        EnvironmentKvBucketClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentKvBucketClass,
        >(owner, recipient, verb, resource),
        EnvironmentBlobBucketClass::NAME => parse_polymorphic_class_permission::<
            EnvironmentBlobBucketClass,
        >(owner, recipient, verb, resource),
        _ => Err(CardParseError::UnknownClass(class.to_string())),
    }
}

fn parse_class_permission<C: PermissionClass>(
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PermissionPattern, CardParseError> {
    let owner = C::Owner::parse(owner).map_err(|_| CardParseError::InvalidOwnerPath {
        class: C::NAME.to_string(),
        owner: owner.to_string(),
    })?;
    let recipient = C::Recipient::parse(recipient).map_err(CardParseError::InvalidRecipientPath)?;
    let resource = C::parse_resource(resource)?;
    let pattern = if verb == "*" {
        ClassPermissionPattern::<C>::Any {
            owner,
            recipient,
            resource,
        }
    } else {
        ClassPermissionPattern::<C>::Verb {
            verb: C::parse_verb(verb).ok_or_else(|| CardParseError::UnknownVerb {
                class: C::NAME.to_string(),
                verb: verb.to_string(),
            })?,
            owner,
            recipient,
            resource,
        }
    };
    Ok(C::into_permission(pattern))
}

fn parse_polymorphic_class_permission<C: PermissionClass>(
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PolymorphicPermissionPattern, CardParseError> {
    let owner =
        C::Owner::parse_polymorphic(owner).map_err(|_| CardParseError::InvalidOwnerPath {
            class: C::NAME.to_string(),
            owner: owner.to_string(),
        })?;
    let recipient =
        C::Recipient::parse_polymorphic(recipient).map_err(CardParseError::InvalidRecipientPath)?;
    if contains_slot_reference(resource) {
        return Err(CardParseError::InvalidResource {
            class: C::NAME.to_string(),
            resource: resource.to_string(),
        });
    }
    let resource = C::parse_resource(resource)?;
    let pattern = if verb == "*" {
        PolymorphicClassPermissionPattern::<C>::Any {
            owner,
            recipient,
            resource,
        }
    } else {
        PolymorphicClassPermissionPattern::<C>::Verb {
            verb: C::parse_verb(verb).ok_or_else(|| CardParseError::UnknownVerb {
                class: C::NAME.to_string(),
                verb: verb.to_string(),
            })?,
            owner,
            recipient,
            resource,
        }
    };
    Ok(C::into_polymorphic_permission(pattern))
}

pub(crate) fn contains_slot_reference(value: &str) -> bool {
    value
        .match_indices('?')
        .any(|(idx, _)| slot_prefix(&value[idx..]).is_some())
}

fn slot_prefix(value: &str) -> Option<&str> {
    let mut chars = value.char_indices();
    let (_, first) = chars.next()?;
    if first != '?' {
        return None;
    }

    let mut end = 1;
    for (idx, c) in chars {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            end = idx + c.len_utf8();
        } else {
            break;
        }
    }

    if end == 1 { None } else { Some(&value[..end]) }
}

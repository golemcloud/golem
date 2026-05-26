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
use crate::model::card::owner::OwnerPattern;
use crate::model::card::recipient::{PolymorphicRecipientPattern, RecipientPattern};
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

impl FromStr for PolymorphicManifestPatternGrant {
    type Err = CardParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_polymorphic_manifest_pattern_grant(value)
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

pub fn parse_polymorphic_manifest_pattern_grant(
    value: &str,
) -> Result<PolymorphicManifestPatternGrant, CardParseError> {
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

    Ok(PolymorphicManifestPatternGrant {
        permission: parse_polymorphic_manifest_permission(
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

macro_rules! dispatch_permission_class {
    ($case:ident, $class:expr, $owner:expr, $recipient:expr, $verb:expr, $resource:expr) => {
        match $class {
            FilesystemClass::NAME => $case!(
                FilesystemClass,
                Filesystem,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            NetworkClass::NAME => {
                $case!(NetworkClass, Network, $owner, $recipient, $verb, $resource)
            }
            EnvClass::NAME => $case!(EnvClass, Env, $owner, $recipient, $verb, $resource),
            OplogClass::NAME => $case!(OplogClass, Oplog, $owner, $recipient, $verb, $resource),
            ConfigClass::NAME => $case!(ConfigClass, Config, $owner, $recipient, $verb, $resource),
            SecretClass::NAME => $case!(SecretClass, Secret, $owner, $recipient, $verb, $resource),
            AgentClass::NAME => $case!(AgentClass, Agent, $owner, $recipient, $verb, $resource),
            ToolClass::NAME => $case!(ToolClass, Tool, $owner, $recipient, $verb, $resource),
            KvClass::NAME => $case!(KvClass, Kv, $owner, $recipient, $verb, $resource),
            BlobClass::NAME => $case!(BlobClass, Blob, $owner, $recipient, $verb, $resource),
            RdbmsClass::NAME => $case!(RdbmsClass, Rdbms, $owner, $recipient, $verb, $resource),
            CardClass::NAME => $case!(CardClass, Card, $owner, $recipient, $verb, $resource),
            SystemClass::NAME => $case!(SystemClass, System, $owner, $recipient, $verb, $resource),
            PlanClass::NAME => $case!(PlanClass, Plan, $owner, $recipient, $verb, $resource),
            AccountClass::NAME => {
                $case!(AccountClass, Account, $owner, $recipient, $verb, $resource)
            }
            AccountUsageClass::NAME => $case!(
                AccountUsageClass,
                AccountUsage,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            AccountTokenClass::NAME => $case!(
                AccountTokenClass,
                AccountToken,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            AccountPluginClass::NAME => $case!(
                AccountPluginClass,
                AccountPlugin,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            ApplicationClass::NAME => $case!(
                ApplicationClass,
                Application,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentClass::NAME => $case!(
                EnvironmentClass,
                Environment,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentShareClass::NAME => $case!(
                EnvironmentShareClass,
                EnvironmentShare,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentPluginGrantClass::NAME => $case!(
                EnvironmentPluginGrantClass,
                EnvironmentPluginGrant,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentDomainRegistrationClass::NAME => $case!(
                EnvironmentDomainRegistrationClass,
                EnvironmentDomainRegistration,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentSecuritySchemeClass::NAME => $case!(
                EnvironmentSecuritySchemeClass,
                EnvironmentSecurityScheme,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentHttpApiDeploymentClass::NAME => $case!(
                EnvironmentHttpApiDeploymentClass,
                EnvironmentHttpApiDeployment,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentMcpDeploymentClass::NAME => $case!(
                EnvironmentMcpDeploymentClass,
                EnvironmentMcpDeployment,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentAgentSecretClass::NAME => $case!(
                EnvironmentAgentSecretClass,
                EnvironmentAgentSecret,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentResourceDefinitionClass::NAME => $case!(
                EnvironmentResourceDefinitionClass,
                EnvironmentResourceDefinition,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentRetryPolicyClass::NAME => $case!(
                EnvironmentRetryPolicyClass,
                EnvironmentRetryPolicy,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            ComponentClass::NAME => $case!(
                ComponentClass,
                Component,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            AccountOauth2IdentityClass::NAME => $case!(
                AccountOauth2IdentityClass,
                AccountOauth2Identity,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentInitialFilesClass::NAME => $case!(
                EnvironmentInitialFilesClass,
                EnvironmentInitialFiles,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentKvBucketClass::NAME => $case!(
                EnvironmentKvBucketClass,
                EnvironmentKvBucket,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            EnvironmentBlobBucketClass::NAME => $case!(
                EnvironmentBlobBucketClass,
                EnvironmentBlobBucket,
                $owner,
                $recipient,
                $verb,
                $resource
            ),
            _ => Err(CardParseError::UnknownClass($class.to_string())),
        }
    };
}

macro_rules! parse_permission_case {
    ($class:ty, $variant:ident, $owner:expr, $recipient:expr, $verb:expr, $resource:expr) => {
        parse_class_permission::<$class>($owner, $recipient, $verb, $resource)
    };
}

macro_rules! parse_polymorphic_permission_case {
    ($class:ty, $variant:ident, $owner:expr, $recipient:expr, $verb:expr, $resource:expr) => {
        parse_polymorphic_class_permission::<$class>($owner, $recipient, $verb, $resource)
    };
}

macro_rules! parse_polymorphic_manifest_permission_case {
    ($class:ty, $variant:ident, $owner:expr, $recipient:expr, $verb:expr, $resource:expr) => {
        parse_polymorphic_manifest_class_permission::<$class>($owner, $recipient, $verb, $resource)
            .map(PolymorphicManifestPermissionPattern::$variant)
    };
}

fn parse_permission(
    class: &str,
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PermissionPattern, CardParseError> {
    dispatch_permission_class!(
        parse_permission_case,
        class,
        owner,
        recipient,
        verb,
        resource
    )
}

fn parse_polymorphic_permission(
    class: &str,
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PolymorphicPermissionPattern, CardParseError> {
    dispatch_permission_class!(
        parse_polymorphic_permission_case,
        class,
        owner,
        recipient,
        verb,
        resource
    )
}

fn parse_polymorphic_manifest_permission(
    class: &str,
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PolymorphicManifestPermissionPattern, CardParseError> {
    dispatch_permission_class!(
        parse_polymorphic_manifest_permission_case,
        class,
        owner,
        recipient,
        verb,
        resource
    )
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
    let recipient =
        RecipientPattern::parse(recipient).map_err(CardParseError::InvalidRecipientPath)?;
    let resource = C::Resource::parse_resource(resource)?;
    let pattern = if verb == "*" {
        ClassPermissionPattern::<C>::Any {
            owner,
            recipient,
            resource,
        }
    } else {
        ClassPermissionPattern::<C>::Verb {
            verb: C::Verb::parse_verb(verb).ok_or_else(|| CardParseError::UnknownVerb {
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
        RecipientPattern::parse(recipient).map_err(CardParseError::InvalidRecipientPath)?;
    if contains_slot_reference(resource) {
        return Err(CardParseError::InvalidResource {
            class: C::NAME.to_string(),
            resource: resource.to_string(),
        });
    }
    let resource = C::Resource::parse_resource(resource)?;
    let pattern = if verb == "*" {
        PolymorphicClassPermissionPattern::<C>::Any {
            owner,
            recipient,
            resource,
        }
    } else {
        PolymorphicClassPermissionPattern::<C>::Verb {
            verb: C::Verb::parse_verb(verb).ok_or_else(|| CardParseError::UnknownVerb {
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

fn parse_polymorphic_manifest_class_permission<C: PermissionClass>(
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PolymorphicManifestClassPermissionPattern<C>, CardParseError> {
    let owner =
        C::Owner::parse_polymorphic(owner).map_err(|_| CardParseError::InvalidOwnerPath {
            class: C::NAME.to_string(),
            owner: owner.to_string(),
        })?;
    let recipient = PolymorphicRecipientPattern::parse(recipient)
        .map_err(CardParseError::InvalidRecipientPath)?;
    if contains_slot_reference(resource) {
        return Err(CardParseError::InvalidResource {
            class: C::NAME.to_string(),
            resource: resource.to_string(),
        });
    }
    let resource = C::Resource::parse_resource(resource)?;
    if verb == "*" {
        Ok(PolymorphicManifestClassPermissionPattern::<C>::Any {
            owner,
            recipient,
            resource,
        })
    } else {
        Ok(PolymorphicManifestClassPermissionPattern::<C>::Verb {
            verb: C::Verb::parse_verb(verb).ok_or_else(|| CardParseError::UnknownVerb {
                class: C::NAME.to_string(),
                verb: verb.to_string(),
            })?,
            owner,
            recipient,
            resource,
        })
    }
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

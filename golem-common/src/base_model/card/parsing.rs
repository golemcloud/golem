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
            FilesystemClass::parse_permission(owner, recipient, verb, resource)
        }
        NetworkClass::NAME => NetworkClass::parse_permission(owner, recipient, verb, resource),
        EnvClass::NAME => EnvClass::parse_permission(owner, recipient, verb, resource),
        OplogClass::NAME => OplogClass::parse_permission(owner, recipient, verb, resource),
        ConfigClass::NAME => ConfigClass::parse_permission(owner, recipient, verb, resource),
        SecretClass::NAME => SecretClass::parse_permission(owner, recipient, verb, resource),
        AgentClass::NAME => AgentClass::parse_permission(owner, recipient, verb, resource),
        ToolClass::NAME => ToolClass::parse_permission(owner, recipient, verb, resource),
        KvClass::NAME => KvClass::parse_permission(owner, recipient, verb, resource),
        BlobClass::NAME => BlobClass::parse_permission(owner, recipient, verb, resource),
        RdbmsClass::NAME => RdbmsClass::parse_permission(owner, recipient, verb, resource),
        CardClass::NAME => CardClass::parse_permission(owner, recipient, verb, resource),
        SystemClass::NAME => SystemClass::parse_permission(owner, recipient, verb, resource),
        PlanClass::NAME => PlanClass::parse_permission(owner, recipient, verb, resource),
        AccountClass::NAME => AccountClass::parse_permission(owner, recipient, verb, resource),
        AccountUsageClass::NAME => {
            AccountUsageClass::parse_permission(owner, recipient, verb, resource)
        }
        AccountTokenClass::NAME => {
            AccountTokenClass::parse_permission(owner, recipient, verb, resource)
        }
        AccountPluginClass::NAME => {
            AccountPluginClass::parse_permission(owner, recipient, verb, resource)
        }
        ApplicationClass::NAME => {
            ApplicationClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentClass::NAME => {
            EnvironmentClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentShareClass::NAME => {
            EnvironmentShareClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentPluginGrantClass::NAME => {
            EnvironmentPluginGrantClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentDomainRegistrationClass::NAME => {
            EnvironmentDomainRegistrationClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentSecuritySchemeClass::NAME => {
            EnvironmentSecuritySchemeClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentHttpApiDeploymentClass::NAME => {
            EnvironmentHttpApiDeploymentClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentMcpDeploymentClass::NAME => {
            EnvironmentMcpDeploymentClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentAgentSecretClass::NAME => {
            EnvironmentAgentSecretClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentResourceDefinitionClass::NAME => {
            EnvironmentResourceDefinitionClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentRetryPolicyClass::NAME => {
            EnvironmentRetryPolicyClass::parse_permission(owner, recipient, verb, resource)
        }
        ComponentClass::NAME => ComponentClass::parse_permission(owner, recipient, verb, resource),
        AccountOauth2IdentityClass::NAME => {
            AccountOauth2IdentityClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentInitialFilesClass::NAME => {
            EnvironmentInitialFilesClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentKvBucketClass::NAME => {
            EnvironmentKvBucketClass::parse_permission(owner, recipient, verb, resource)
        }
        EnvironmentBlobBucketClass::NAME => {
            EnvironmentBlobBucketClass::parse_permission(owner, recipient, verb, resource)
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
            FilesystemClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        NetworkClass::NAME => {
            NetworkClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        EnvClass::NAME => EnvClass::parse_polymorphic_permission(owner, recipient, verb, resource),
        OplogClass::NAME => {
            OplogClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        ConfigClass::NAME => {
            ConfigClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        SecretClass::NAME => {
            SecretClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        AgentClass::NAME => {
            AgentClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        ToolClass::NAME => {
            ToolClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        KvClass::NAME => KvClass::parse_polymorphic_permission(owner, recipient, verb, resource),
        BlobClass::NAME => {
            BlobClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        RdbmsClass::NAME => {
            RdbmsClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        CardClass::NAME => {
            CardClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        SystemClass::NAME => {
            SystemClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        PlanClass::NAME => {
            PlanClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        AccountClass::NAME => {
            AccountClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        AccountUsageClass::NAME => {
            AccountUsageClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        AccountTokenClass::NAME => {
            AccountTokenClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        AccountPluginClass::NAME => {
            AccountPluginClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        ApplicationClass::NAME => {
            ApplicationClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        EnvironmentClass::NAME => {
            EnvironmentClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        EnvironmentShareClass::NAME => {
            EnvironmentShareClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        EnvironmentPluginGrantClass::NAME => {
            EnvironmentPluginGrantClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        EnvironmentDomainRegistrationClass::NAME => {
            EnvironmentDomainRegistrationClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        EnvironmentSecuritySchemeClass::NAME => {
            EnvironmentSecuritySchemeClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        EnvironmentHttpApiDeploymentClass::NAME => {
            EnvironmentHttpApiDeploymentClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        EnvironmentMcpDeploymentClass::NAME => {
            EnvironmentMcpDeploymentClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        EnvironmentAgentSecretClass::NAME => {
            EnvironmentAgentSecretClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        EnvironmentResourceDefinitionClass::NAME => {
            EnvironmentResourceDefinitionClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        EnvironmentRetryPolicyClass::NAME => {
            EnvironmentRetryPolicyClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        ComponentClass::NAME => {
            ComponentClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        AccountOauth2IdentityClass::NAME => {
            AccountOauth2IdentityClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        EnvironmentInitialFilesClass::NAME => {
            EnvironmentInitialFilesClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        EnvironmentKvBucketClass::NAME => {
            EnvironmentKvBucketClass::parse_polymorphic_permission(owner, recipient, verb, resource)
        }
        EnvironmentBlobBucketClass::NAME => {
            EnvironmentBlobBucketClass::parse_polymorphic_permission(
                owner, recipient, verb, resource,
            )
        }
        _ => Err(CardParseError::UnknownClass(class.to_string())),
    }
}
macro_rules! define_owner_parser {
    ($parser:ident, $owner:ident) => {
        pub(crate) fn $parser(class: &str, owner: &str) -> Result<$owner, CardParseError> {
            $owner::parse(owner).map_err(|_| CardParseError::InvalidOwnerPath {
                class: class.to_string(),
                owner: owner.to_string(),
            })
        }
    };
}

define_owner_parser!(parse_empty_owner, EmptyOwnerPattern);
define_owner_parser!(parse_account_owner, AccountOwnerPattern);
define_owner_parser!(parse_application_owner, ApplicationOwnerPattern);
define_owner_parser!(parse_environment_owner, EnvironmentOwnerPattern);
define_owner_parser!(parse_component_owner, ComponentOwnerPattern);
define_owner_parser!(parse_agent_owner, AgentOwnerPattern);
define_owner_parser!(parse_tool_owner, ToolOwnerPattern);

macro_rules! define_recipient_parser {
    ($parser:ident, $recipient:ident) => {
        pub(crate) fn $parser(value: &str) -> Result<$recipient, CardParseError> {
            $recipient::parse(value).map_err(CardParseError::InvalidRecipientPath)
        }
    };
}

define_recipient_parser!(parse_account_recipient, AccountRecipientPattern);
define_recipient_parser!(parse_environment_recipient, EnvironmentRecipientPattern);
define_recipient_parser!(parse_agent_recipient, AgentRecipientPattern);

macro_rules! define_polymorphic_recipient_parser {
    ($parser:ident, $concrete_parser:ident, $recipient:ident) => {
        pub(crate) fn $parser(value: &str) -> Result<$recipient, CardParseError> {
            parse_polymorphic_typed_recipient(
                value,
                $concrete_parser,
                $recipient::Concrete,
                $recipient::Slot,
                $recipient::Template,
            )
        }
    };
}

define_polymorphic_recipient_parser!(
    parse_polymorphic_account_recipient,
    parse_account_recipient,
    PolymorphicAccountRecipientPattern
);
define_polymorphic_recipient_parser!(
    parse_polymorphic_environment_recipient,
    parse_environment_recipient,
    PolymorphicEnvironmentRecipientPattern
);
define_polymorphic_recipient_parser!(
    parse_polymorphic_agent_recipient,
    parse_agent_recipient,
    PolymorphicAgentRecipientPattern
);

fn parse_polymorphic_typed_recipient<T, U, Parse, Concrete, Slot, Template>(
    value: &str,
    parse_concrete: Parse,
    concrete: Concrete,
    slot: Slot,
    template: Template,
) -> Result<T, CardParseError>
where
    Parse: Fn(&str) -> Result<U, CardParseError>,
    Concrete: Fn(U) -> T,
    Slot: Fn(RecipientPathSlot) -> T,
    Template: Fn(RecipientPathTemplate) -> T,
{
    if let Ok(recipient_slot) = RecipientPathSlot::parse(value) {
        match recipient_slot {
            RecipientPathSlot::Slot => return Ok(slot(recipient_slot)),
            RecipientPathSlot::Env => {
                let template = RecipientPathTemplate::parse(value)
                    .map_err(CardParseError::InvalidRecipientPath)?;
                let validation_path = template.validation_path();
                parse_concrete(&validation_path)
                    .map_err(|_| CardParseError::InvalidRecipientPath(value.to_string()))?;
                return Ok(slot(recipient_slot));
            }
        }
    }

    if contains_slot_reference(value) {
        let recipient_template =
            RecipientPathTemplate::parse(value).map_err(CardParseError::InvalidRecipientPath)?;
        let validation_path = recipient_template.validation_path();
        parse_concrete(&validation_path)
            .map_err(|_| CardParseError::InvalidRecipientPath(value.to_string()))?;
        return Ok(template(recipient_template));
    }

    parse_concrete(value).map(concrete)
}

macro_rules! define_polymorphic_owner_parser {
    ($parser:ident, $concrete_parser:ident, $owner:ident) => {
        pub(crate) fn $parser(class: &str, owner: &str) -> Result<$owner, CardParseError> {
            parse_polymorphic_owner(
                class,
                owner,
                $concrete_parser,
                $owner::Concrete,
                $owner::Slot,
                $owner::Template,
            )
        }
    };
}

define_polymorphic_owner_parser!(
    parse_polymorphic_empty_owner,
    parse_empty_owner,
    PolymorphicEmptyOwnerPattern
);
define_polymorphic_owner_parser!(
    parse_polymorphic_account_owner,
    parse_account_owner,
    PolymorphicAccountOwnerPattern
);
define_polymorphic_owner_parser!(
    parse_polymorphic_application_owner,
    parse_application_owner,
    PolymorphicApplicationOwnerPattern
);
define_polymorphic_owner_parser!(
    parse_polymorphic_environment_owner,
    parse_environment_owner,
    PolymorphicEnvironmentOwnerPattern
);
define_polymorphic_owner_parser!(
    parse_polymorphic_component_owner,
    parse_component_owner,
    PolymorphicComponentOwnerPattern
);
define_polymorphic_owner_parser!(
    parse_polymorphic_agent_owner,
    parse_agent_owner,
    PolymorphicAgentOwnerPattern
);
define_polymorphic_owner_parser!(
    parse_polymorphic_tool_owner,
    parse_tool_owner,
    PolymorphicToolOwnerPattern
);

fn parse_polymorphic_owner<T, U, Parse, Concrete, Slot, Template>(
    class: &str,
    owner: &str,
    parse_concrete: Parse,
    concrete: Concrete,
    slot: Slot,
    template: Template,
) -> Result<T, CardParseError>
where
    Parse: Fn(&str, &str) -> Result<U, CardParseError>,
    Concrete: Fn(U) -> T,
    Slot: Fn(SlotVariable) -> T,
    Template: Fn(String) -> T,
{
    if let Ok(variable) = SlotVariable::parse(owner) {
        return Ok(slot(variable));
    }

    if contains_slot_reference(owner) {
        return Ok(template(owner.to_string()));
    }

    parse_concrete(class, owner).map(concrete)
}

pub(crate) fn parse_polymorphic_resource<T, U, Parse, Concrete, Slot, Template>(
    class: &str,
    resource: &str,
    parse_concrete: Parse,
    concrete: Concrete,
    slot: Slot,
    template: Template,
) -> Result<T, CardParseError>
where
    Parse: Fn(&str, &str) -> Result<U, CardParseError>,
    Concrete: Fn(U) -> T,
    Slot: Fn(SlotVariable) -> T,
    Template: Fn(String) -> T,
{
    if let Ok(variable) = SlotVariable::parse(resource) {
        return Ok(slot(variable));
    }

    if contains_slot_reference(resource) {
        return Ok(template(resource.to_string()));
    }

    match parse_concrete(class, resource) {
        Ok(resource) => Ok(concrete(resource)),
        Err(err) => Err(err),
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

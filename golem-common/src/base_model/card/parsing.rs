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

macro_rules! parse_permission {
    ($class:expr, $owner:expr, $recipient:expr, $verb:expr, $resource:expr, $class_name:literal, $variant:ident, $pattern:ident, $owner_parser:ident, $recipient_parser:ident, $resource_parser:ident, [$($verb_name:literal => $verb_variant:ident),+ $(,)?]) => {
        if $class == $class_name {
            let owner = $owner_parser($class, $owner)?;
            let recipient = $recipient_parser($recipient)?;
            let resource = $resource_parser($class, $resource)?;
            return Ok(PermissionPattern::$variant(match $verb {
                "*" => $pattern::Any { owner, recipient, resource },
                $($verb_name => $pattern::$verb_variant { owner, recipient, resource },)+
                other => return Err(CardParseError::UnknownVerb {
                    class: $class.to_string(),
                    verb: other.to_string(),
                }),
            }));
        }
    };
}

fn parse_permission(
    class: &str,
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PermissionPattern, CardParseError> {
    parse_permission!(class, owner, recipient, verb, resource, "filesystem", Filesystem, FilesystemPermissionPattern, parse_agent_owner, parse_agent_recipient, parse_glob_resource, ["read" => Read, "write" => Write, "list" => List, "stat" => Stat, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "network", Network, NetworkPermissionPattern, parse_empty_owner, parse_agent_recipient, parse_network_resource, ["connect" => Connect]);
    parse_permission!(class, owner, recipient, verb, resource, "env", Env, EnvPermissionPattern, parse_agent_owner, parse_agent_recipient, parse_identifier_resource, ["read" => Read]);
    parse_permission!(class, owner, recipient, verb, resource, "oplog", Oplog, OplogPermissionPattern, parse_agent_owner, parse_agent_recipient, parse_oplog_resource, ["read" => Read]);
    parse_permission!(class, owner, recipient, verb, resource, "config", Config, ConfigPermissionPattern, parse_agent_owner, parse_agent_recipient, parse_glob_resource, ["read" => Read]);
    parse_permission!(class, owner, recipient, verb, resource, "secret", Secret, SecretPermissionPattern, parse_environment_owner, parse_agent_recipient, parse_glob_resource, ["hold" => Hold, "mint" => Mint, "reveal" => Reveal]);
    parse_permission!(class, owner, recipient, verb, resource, "agent", Agent, AgentPermissionPattern, parse_agent_owner, parse_agent_recipient, parse_agent_resource, ["invoke" => Invoke, "view" => View, "create" => Create, "delete" => Delete, "interrupt" => Interrupt, "resume" => Resume, "update-revision" => UpdateRevision, "fork" => Fork, "revert" => Revert, "cancel-invocation" => CancelInvocation, "activate-plugin" => ActivatePlugin, "deactivate-plugin" => DeactivatePlugin]);
    parse_permission!(class, owner, recipient, verb, resource, "tool", Tool, ToolPermissionPattern, parse_tool_owner, parse_agent_recipient, parse_tool_resource, ["invoke" => Invoke]);
    parse_permission!(class, owner, recipient, verb, resource, "kv", Kv, KvPermissionPattern, parse_environment_owner, parse_agent_recipient, parse_glob_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "blob", Blob, BlobPermissionPattern, parse_environment_owner, parse_agent_recipient, parse_glob_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "rdbms", Rdbms, RdbmsPermissionPattern, parse_environment_owner, parse_agent_recipient, parse_glob_resource, ["query" => Query, "execute" => Execute]);
    parse_permission!(class, owner, recipient, verb, resource, "card", Card, CardPermissionPattern, parse_account_owner, parse_agent_recipient, parse_card_resource, ["derive" => Derive, "revoke" => Revoke, "inspect" => Inspect, "install" => Install]);
    parse_permission!(class, owner, recipient, verb, resource, "system", System, SystemPermissionPattern, parse_empty_owner, parse_account_recipient, parse_empty_resource, ["create-account" => CreateAccount, "view-default-plan" => ViewDefaultPlan, "view-account-summaries-report" => ViewAccountSummariesReport, "view-account-counts-report" => ViewAccountCountsReport]);
    parse_permission!(class, owner, recipient, verb, resource, "plan", Plan, PlanPermissionPattern, parse_empty_owner, parse_account_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update]);
    parse_permission!(class, owner, recipient, verb, resource, "account", Account, AccountPermissionPattern, parse_account_owner, parse_account_recipient, parse_empty_resource, ["view" => View, "update" => Update, "delete" => Delete, "set-roles" => SetRoles, "set-plan" => SetPlan, "restore" => Restore]);
    parse_permission!(class, owner, recipient, verb, resource, "account.usage", AccountUsage, AccountUsagePermissionPattern, parse_account_owner, parse_account_recipient, parse_empty_resource, ["view" => View]);
    parse_permission!(class, owner, recipient, verb, resource, "account.token", AccountToken, AccountTokenPermissionPattern, parse_account_owner, parse_account_recipient, parse_identifier_resource, ["create" => Create, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "account.plugin", AccountPlugin, AccountPluginPermissionPattern, parse_account_owner, parse_account_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "application", Application, ApplicationPermissionPattern, parse_application_owner, parse_account_recipient, parse_empty_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "mint-credential" => MintCredential, "rotate-credential" => RotateCredential, "revoke-credential" => RevokeCredential, "view-credentials" => ViewCredentials]);
    parse_permission!(class, owner, recipient, verb, resource, "environment", Environment, EnvironmentPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_empty_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "deploy" => Deploy, "rollback" => Rollback, "view-deployment-plan" => ViewDeploymentPlan, "write-deployment-record" => WriteDeploymentRecord]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.share", EnvironmentShare, EnvironmentSharePermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.plugin-grant", EnvironmentPluginGrant, EnvironmentPluginGrantPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.domain-registration", EnvironmentDomainRegistration, EnvironmentDomainRegistrationPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.security-scheme", EnvironmentSecurityScheme, EnvironmentSecuritySchemePermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.http-api-deployment", EnvironmentHttpApiDeployment, EnvironmentHttpApiDeploymentPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.mcp-deployment", EnvironmentMcpDeployment, EnvironmentMcpDeploymentPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.agent-secret", EnvironmentAgentSecret, EnvironmentAgentSecretPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_glob_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.resource-definition", EnvironmentResourceDefinition, EnvironmentResourceDefinitionPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.retry-policy", EnvironmentRetryPolicy, EnvironmentRetryPolicyPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "component", Component, ComponentPermissionPattern, parse_component_owner, parse_environment_recipient, parse_empty_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "account.oauth2-identity", AccountOauth2Identity, AccountOauth2IdentityPermissionPattern, parse_account_owner, parse_account_recipient, parse_identifier_resource, ["view" => View, "link" => Link, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.initial-files", EnvironmentInitialFiles, EnvironmentInitialFilesPermissionPattern, parse_component_owner, parse_environment_recipient, parse_glob_resource, ["view" => View, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.kv-bucket", EnvironmentKvBucket, EnvironmentKvBucketPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.blob-bucket", EnvironmentBlobBucket, EnvironmentBlobBucketPermissionPattern, parse_environment_owner, parse_environment_recipient, parse_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);

    Err(CardParseError::UnknownClass(class.to_string()))
}

macro_rules! parse_polymorphic_permission {
    ($class:expr, $owner:expr, $recipient:expr, $verb:expr, $resource:expr, $class_name:literal, $variant:ident, $pattern:ident, $owner_parser:ident, $recipient_parser:ident, $resource_parser:ident, [$($verb_name:literal => $verb_variant:ident),+ $(,)?]) => {
        if $class == $class_name {
            let owner = $owner_parser($class, $owner)?;
            let recipient = $recipient_parser($recipient)?;
            let resource = $resource_parser($class, $resource)?;
            return Ok(PolymorphicPermissionPattern::$variant(match $verb {
                "*" => $pattern::Any { owner, recipient, resource },
                $($verb_name => $pattern::$verb_variant { owner, recipient, resource },)+
                other => return Err(CardParseError::UnknownVerb {
                    class: $class.to_string(),
                    verb: other.to_string(),
                }),
            }));
        }
    };
}

fn parse_polymorphic_permission(
    class: &str,
    owner: &str,
    recipient: &str,
    verb: &str,
    resource: &str,
) -> Result<PolymorphicPermissionPattern, CardParseError> {
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "filesystem", Filesystem, PolymorphicFilesystemPermissionPattern, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_glob_resource, ["read" => Read, "write" => Write, "list" => List, "stat" => Stat, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "network", Network, PolymorphicNetworkPermissionPattern, parse_polymorphic_empty_owner, parse_polymorphic_agent_recipient, parse_polymorphic_network_resource, ["connect" => Connect]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "env", Env, PolymorphicEnvPermissionPattern, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_identifier_resource, ["read" => Read]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "oplog", Oplog, PolymorphicOplogPermissionPattern, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_oplog_resource, ["read" => Read]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "config", Config, PolymorphicConfigPermissionPattern, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_glob_resource, ["read" => Read]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "secret", Secret, PolymorphicSecretPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_agent_recipient, parse_polymorphic_glob_resource, ["hold" => Hold, "mint" => Mint, "reveal" => Reveal]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "agent", Agent, PolymorphicAgentPermissionPattern, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_agent_resource, ["invoke" => Invoke, "view" => View, "create" => Create, "delete" => Delete, "interrupt" => Interrupt, "resume" => Resume, "update-revision" => UpdateRevision, "fork" => Fork, "revert" => Revert, "cancel-invocation" => CancelInvocation, "activate-plugin" => ActivatePlugin, "deactivate-plugin" => DeactivatePlugin]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "tool", Tool, PolymorphicToolPermissionPattern, parse_polymorphic_tool_owner, parse_polymorphic_agent_recipient, parse_polymorphic_tool_resource, ["invoke" => Invoke]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "kv", Kv, PolymorphicKvPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_agent_recipient, parse_polymorphic_glob_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "blob", Blob, PolymorphicBlobPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_agent_recipient, parse_polymorphic_glob_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "rdbms", Rdbms, PolymorphicRdbmsPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_agent_recipient, parse_polymorphic_glob_resource, ["query" => Query, "execute" => Execute]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "card", Card, PolymorphicCardPermissionPattern, parse_polymorphic_account_owner, parse_polymorphic_agent_recipient, parse_polymorphic_card_resource, ["derive" => Derive, "revoke" => Revoke, "inspect" => Inspect, "install" => Install]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "system", System, PolymorphicSystemPermissionPattern, parse_polymorphic_empty_owner, parse_polymorphic_account_recipient, parse_polymorphic_empty_resource, ["create-account" => CreateAccount, "view-default-plan" => ViewDefaultPlan, "view-account-summaries-report" => ViewAccountSummariesReport, "view-account-counts-report" => ViewAccountCountsReport]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "plan", Plan, PolymorphicPlanPermissionPattern, parse_polymorphic_empty_owner, parse_polymorphic_account_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "update" => Update]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account", Account, PolymorphicAccountPermissionPattern, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_empty_resource, ["view" => View, "update" => Update, "delete" => Delete, "set-roles" => SetRoles, "set-plan" => SetPlan, "restore" => Restore]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account.usage", AccountUsage, PolymorphicAccountUsagePermissionPattern, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_empty_resource, ["view" => View]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account.token", AccountToken, PolymorphicAccountTokenPermissionPattern, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_identifier_resource, ["create" => Create, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account.plugin", AccountPlugin, PolymorphicAccountPluginPermissionPattern, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "application", Application, PolymorphicApplicationPermissionPattern, parse_polymorphic_application_owner, parse_polymorphic_account_recipient, parse_polymorphic_empty_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "mint-credential" => MintCredential, "rotate-credential" => RotateCredential, "revoke-credential" => RevokeCredential, "view-credentials" => ViewCredentials]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment", Environment, PolymorphicEnvironmentPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_empty_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "deploy" => Deploy, "rollback" => Rollback, "view-deployment-plan" => ViewDeploymentPlan, "write-deployment-record" => WriteDeploymentRecord]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.share", EnvironmentShare, PolymorphicEnvironmentSharePermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.plugin-grant", EnvironmentPluginGrant, PolymorphicEnvironmentPluginGrantPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.domain-registration", EnvironmentDomainRegistration, PolymorphicEnvironmentDomainRegistrationPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.security-scheme", EnvironmentSecurityScheme, PolymorphicEnvironmentSecuritySchemePermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.http-api-deployment", EnvironmentHttpApiDeployment, PolymorphicEnvironmentHttpApiDeploymentPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.mcp-deployment", EnvironmentMcpDeployment, PolymorphicEnvironmentMcpDeploymentPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.agent-secret", EnvironmentAgentSecret, PolymorphicEnvironmentAgentSecretPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_glob_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.resource-definition", EnvironmentResourceDefinition, PolymorphicEnvironmentResourceDefinitionPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.retry-policy", EnvironmentRetryPolicy, PolymorphicEnvironmentRetryPolicyPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "component", Component, PolymorphicComponentPermissionPattern, parse_polymorphic_component_owner, parse_polymorphic_environment_recipient, parse_polymorphic_empty_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account.oauth2-identity", AccountOauth2Identity, PolymorphicAccountOauth2IdentityPermissionPattern, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_identifier_resource, ["view" => View, "link" => Link, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.initial-files", EnvironmentInitialFiles, PolymorphicEnvironmentInitialFilesPermissionPattern, parse_polymorphic_component_owner, parse_polymorphic_environment_recipient, parse_polymorphic_glob_resource, ["view" => View, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.kv-bucket", EnvironmentKvBucket, PolymorphicEnvironmentKvBucketPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.blob-bucket", EnvironmentBlobBucket, PolymorphicEnvironmentBlobBucketPermissionPattern, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);

    Err(CardParseError::UnknownClass(class.to_string()))
}

macro_rules! define_owner_parser {
    ($parser:ident, $owner:ident) => {
        fn $parser(class: &str, owner: &str) -> Result<$owner, CardParseError> {
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
        fn $parser(value: &str) -> Result<$recipient, CardParseError> {
            $recipient::parse(value).map_err(CardParseError::InvalidRecipientPath)
        }
    };
}

define_recipient_parser!(parse_account_recipient, AccountRecipientPattern);
define_recipient_parser!(parse_environment_recipient, EnvironmentRecipientPattern);
define_recipient_parser!(parse_agent_recipient, AgentRecipientPattern);

macro_rules! define_polymorphic_recipient_parser {
    ($parser:ident, $concrete_parser:ident, $recipient:ident) => {
        fn $parser(value: &str) -> Result<$recipient, CardParseError> {
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
    Slot: Fn(SlotVariable) -> T,
    Template: Fn(String) -> T,
{
    if let Ok(variable) = SlotVariable::parse(value) {
        return Ok(slot(variable));
    }

    if contains_slot_reference(value) {
        if value != "*" && (value.is_empty() || value.split('/').any(str::is_empty)) {
            return Err(CardParseError::InvalidRecipientPath(value.to_string()));
        }
        return Ok(template(value.to_string()));
    }

    parse_concrete(value).map(concrete)
}

macro_rules! define_polymorphic_owner_parser {
    ($parser:ident, $concrete_parser:ident, $owner:ident) => {
        fn $parser(class: &str, owner: &str) -> Result<$owner, CardParseError> {
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

fn parse_empty_resource(
    class: &str,
    resource: &str,
) -> Result<EmptyResourcePattern, CardParseError> {
    if resource.is_empty() {
        Ok(EmptyResourcePattern)
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_identifier_resource(
    _class: &str,
    resource: &str,
) -> Result<IdentifierResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(IdentifierResourcePattern::Any)
    } else {
        Ok(IdentifierResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_glob_resource(
    _class: &str,
    resource: &str,
) -> Result<GlobResourcePattern, CardParseError> {
    if resource == "*" || resource == "**" {
        Ok(GlobResourcePattern::Any)
    } else if resource.contains('*') {
        Ok(GlobResourcePattern::Glob(resource.to_string()))
    } else {
        Ok(GlobResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_network_resource(
    _class: &str,
    resource: &str,
) -> Result<NetworkResourcePattern, CardParseError> {
    if resource == "*" {
        return Ok(NetworkResourcePattern::Any);
    }

    let (host, ports) = if let Some((host, port)) = resource.rsplit_once(':') {
        if port.chars().all(|c| c.is_ascii_digit() || c == '-') {
            (host.to_string(), parse_port_pattern(port)?)
        } else {
            (resource.to_string(), PortPattern::Any)
        }
    } else {
        (resource.to_string(), PortPattern::Any)
    };

    Ok(NetworkResourcePattern::HostPort { host, ports })
}

fn parse_port_pattern(port: &str) -> Result<PortPattern, CardParseError> {
    if let Some((start, end)) = port.split_once('-') {
        let start = start.parse().map_err(|_| CardParseError::InvalidResource {
            class: "network".to_string(),
            resource: port.to_string(),
        })?;
        let end = end.parse().map_err(|_| CardParseError::InvalidResource {
            class: "network".to_string(),
            resource: port.to_string(),
        })?;
        Ok(PortPattern::Range { start, end })
    } else {
        Ok(PortPattern::Single(port.parse().map_err(|_| {
            CardParseError::InvalidResource {
                class: "network".to_string(),
                resource: port.to_string(),
            }
        })?))
    }
}

fn parse_oplog_resource(
    class: &str,
    resource: &str,
) -> Result<OplogResourcePattern, CardParseError> {
    if resource == "*" {
        return Ok(OplogResourcePattern::Any);
    }
    let mut start = None;
    let mut end = None;
    for part in resource.split(':') {
        if let Some(value) = part.strip_prefix("start=") {
            start = Some(value.parse().map_err(|_| CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })?);
        } else if let Some(value) = part.strip_prefix("end=") {
            end = Some(value.parse().map_err(|_| CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })?);
        } else {
            return Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            });
        }
    }
    Ok(OplogResourcePattern::Range { start, end })
}

fn parse_agent_resource(
    _class: &str,
    resource: &str,
) -> Result<AgentResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(AgentResourcePattern::Any)
    } else if resource.is_empty() {
        Ok(AgentResourcePattern::Empty)
    } else {
        Ok(AgentResourcePattern::Method(resource.to_string()))
    }
}

fn parse_tool_resource(
    _class: &str,
    resource: &str,
) -> Result<ToolResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(ToolResourcePattern::Any)
    } else {
        Ok(ToolResourcePattern::Command(resource.to_string()))
    }
}

fn parse_card_resource(
    _class: &str,
    resource: &str,
) -> Result<CardResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(CardResourcePattern::Any)
    } else if resource.is_empty() {
        Ok(CardResourcePattern::Empty)
    } else {
        Ok(CardResourcePattern::InstallTarget(
            RecipientPathPattern::parse(resource).map_err(CardParseError::InvalidRecipientPath)?,
        ))
    }
}

fn parse_polymorphic_empty_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEmptyResourcePattern, CardParseError> {
    if let Ok(resource) = parse_empty_resource(class, resource) {
        Ok(PolymorphicEmptyResourcePattern::Concrete(resource))
    } else if let Ok(slot) = SlotVariable::parse(resource) {
        Ok(PolymorphicEmptyResourcePattern::Slot(slot))
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_polymorphic_identifier_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicIdentifierResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_identifier_resource,
        PolymorphicIdentifierResourcePattern::Concrete,
        PolymorphicIdentifierResourcePattern::Slot,
        PolymorphicIdentifierResourcePattern::Template,
    )
}

fn parse_polymorphic_glob_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicGlobResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_glob_resource,
        PolymorphicGlobResourcePattern::Concrete,
        PolymorphicGlobResourcePattern::Slot,
        PolymorphicGlobResourcePattern::Template,
    )
}

fn parse_polymorphic_network_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicNetworkResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_network_resource,
        PolymorphicNetworkResourcePattern::Concrete,
        PolymorphicNetworkResourcePattern::Slot,
        PolymorphicNetworkResourcePattern::Template,
    )
}

fn parse_polymorphic_oplog_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicOplogResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_oplog_resource,
        PolymorphicOplogResourcePattern::Concrete,
        PolymorphicOplogResourcePattern::Slot,
        PolymorphicOplogResourcePattern::Template,
    )
}

fn parse_polymorphic_agent_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicAgentResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_agent_resource,
        PolymorphicAgentResourcePattern::Concrete,
        PolymorphicAgentResourcePattern::Slot,
        PolymorphicAgentResourcePattern::Template,
    )
}

fn parse_polymorphic_tool_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicToolResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_tool_resource,
        PolymorphicToolResourcePattern::Concrete,
        PolymorphicToolResourcePattern::Slot,
        PolymorphicToolResourcePattern::Template,
    )
}

fn parse_polymorphic_card_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicCardResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_card_resource,
        PolymorphicCardResourcePattern::Concrete,
        PolymorphicCardResourcePattern::Slot,
        PolymorphicCardResourcePattern::Template,
    )
}

fn parse_polymorphic_resource<T, U, Parse, Concrete, Slot, Template>(
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

fn contains_slot_reference(value: &str) -> bool {
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

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
    Malformed(String),
    UnknownClass(String),
    UnknownVerb { class: String, verb: String },
    InvalidResource { class: String, resource: String },
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

    Ok(PatternGrant {
        owner: OwnerPathPattern(parts.owner),
        recipient: RecipientPathPattern::parse(&parts.recipient)
            .map_err(CardParseError::InvalidRecipientPath)?,
        permission: parse_permission(&parts.class, &parts.verb, &parts.resource)?,
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

macro_rules! parse_permission {
    ($class:expr, $verb:expr, $resource:expr, $class_name:literal, $variant:ident, $pattern:ident, $resource_parser:ident, [$($verb_name:literal => $verb_variant:ident),+ $(,)?]) => {
        if $class == $class_name {
            let resource = $resource_parser($class, $resource)?;
            return Ok(PermissionPattern::$variant(match $verb {
                "*" => $pattern::Any(resource),
                $($verb_name => $pattern::$verb_variant(resource),)+
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
    verb: &str,
    resource: &str,
) -> Result<PermissionPattern, CardParseError> {
    parse_permission!(class, verb, resource, "filesystem", Filesystem, FilesystemPermissionPattern, parse_glob_resource, ["read" => Read, "write" => Write, "list" => List, "stat" => Stat, "delete" => Delete]);
    parse_permission!(class, verb, resource, "network", Network, NetworkPermissionPattern, parse_network_resource, ["connect" => Connect]);
    parse_permission!(class, verb, resource, "env", Env, EnvPermissionPattern, parse_identifier_resource, ["read" => Read]);
    parse_permission!(class, verb, resource, "oplog", Oplog, OplogPermissionPattern, parse_oplog_resource, ["read" => Read]);
    parse_permission!(class, verb, resource, "config", Config, ConfigPermissionPattern, parse_glob_resource, ["read" => Read]);
    parse_permission!(class, verb, resource, "secret", Secret, SecretPermissionPattern, parse_glob_resource, ["hold" => Hold, "mint" => Mint, "reveal" => Reveal]);
    parse_permission!(class, verb, resource, "agent", Agent, AgentPermissionPattern, parse_agent_resource, ["invoke" => Invoke, "view" => View, "create" => Create, "delete" => Delete, "interrupt" => Interrupt, "resume" => Resume, "update-revision" => UpdateRevision, "fork" => Fork, "revert" => Revert, "cancel-invocation" => CancelInvocation, "activate-plugin" => ActivatePlugin, "deactivate-plugin" => DeactivatePlugin]);
    parse_permission!(class, verb, resource, "tool", Tool, ToolPermissionPattern, parse_tool_resource, ["invoke" => Invoke]);
    parse_permission!(class, verb, resource, "kv", Kv, KvPermissionPattern, parse_glob_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_permission!(class, verb, resource, "blob", Blob, BlobPermissionPattern, parse_glob_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_permission!(class, verb, resource, "rdbms", Rdbms, RdbmsPermissionPattern, parse_glob_resource, ["query" => Query, "execute" => Execute]);
    parse_permission!(class, verb, resource, "card", Card, CardPermissionPattern, parse_card_resource, ["derive" => Derive, "revoke" => Revoke, "inspect" => Inspect, "install" => Install]);
    parse_permission!(class, verb, resource, "system", System, SystemPermissionPattern, parse_empty_resource, ["create-account" => CreateAccount, "view-default-plan" => ViewDefaultPlan, "view-account-summaries-report" => ViewAccountSummariesReport, "view-account-counts-report" => ViewAccountCountsReport]);
    parse_permission!(class, verb, resource, "plan", Plan, PlanPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update]);
    parse_permission!(class, verb, resource, "account", Account, AccountPermissionPattern, parse_empty_resource, ["view" => View, "update" => Update, "delete" => Delete, "set-roles" => SetRoles, "set-plan" => SetPlan, "restore" => Restore]);
    parse_permission!(class, verb, resource, "account.usage", AccountUsage, AccountUsagePermissionPattern, parse_empty_resource, ["view" => View]);
    parse_permission!(class, verb, resource, "account.token", AccountToken, AccountTokenPermissionPattern, parse_identifier_resource, ["create" => Create, "delete" => Delete]);
    parse_permission!(class, verb, resource, "account.plugin", AccountPlugin, AccountPluginPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "application", Application, ApplicationPermissionPattern, parse_empty_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "mint-credential" => MintCredential, "rotate-credential" => RotateCredential, "revoke-credential" => RevokeCredential, "view-credentials" => ViewCredentials]);
    parse_permission!(class, verb, resource, "environment", Environment, EnvironmentPermissionPattern, parse_empty_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "deploy" => Deploy, "rollback" => Rollback, "view-deployment-plan" => ViewDeploymentPlan, "write-deployment-record" => WriteDeploymentRecord]);
    parse_permission!(class, verb, resource, "environment.share", EnvironmentShare, EnvironmentSharePermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.plugin-grant", EnvironmentPluginGrant, EnvironmentPluginGrantPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.domain-registration", EnvironmentDomainRegistration, EnvironmentDomainRegistrationPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.security-scheme", EnvironmentSecurityScheme, EnvironmentSecuritySchemePermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.http-api-deployment", EnvironmentHttpApiDeployment, EnvironmentHttpApiDeploymentPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.mcp-deployment", EnvironmentMcpDeployment, EnvironmentMcpDeploymentPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.agent-secret", EnvironmentAgentSecret, EnvironmentAgentSecretPermissionPattern, parse_glob_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.resource-definition", EnvironmentResourceDefinition, EnvironmentResourceDefinitionPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.retry-policy", EnvironmentRetryPolicy, EnvironmentRetryPolicyPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "component", Component, ComponentPermissionPattern, parse_empty_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "account.oauth2-identity", AccountOauth2Identity, AccountOauth2IdentityPermissionPattern, parse_identifier_resource, ["view" => View, "link" => Link, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.initial-files", EnvironmentInitialFiles, EnvironmentInitialFilesPermissionPattern, parse_glob_resource, ["view" => View, "update" => Update, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.kv-bucket", EnvironmentKvBucket, EnvironmentKvBucketPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_permission!(class, verb, resource, "environment.blob-bucket", EnvironmentBlobBucket, EnvironmentBlobBucketPermissionPattern, parse_identifier_resource, ["view" => View, "create" => Create, "delete" => Delete]);

    Err(CardParseError::UnknownClass(class.to_string()))
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

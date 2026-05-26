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
    ($class:expr, $owner:expr, $recipient:expr, $verb:expr, $resource:expr, $class_name:literal, $variant:ident, $pattern:ident, $verb_ty:ident, $owner_parser:ident, $recipient_parser:ident, $resource_parser:ident, [$($verb_name:literal => $verb_variant:ident),+ $(,)?]) => {
        if $class == $class_name {
            let owner = $owner_parser($class, $owner)?;
            let recipient = $recipient_parser($recipient)?;
            let resource = $resource_parser($class, $resource)?;
            return Ok(PermissionPattern::$variant(match $verb {
                "*" => $pattern::Any { owner, recipient, resource },
                $($verb_name => $pattern::Verb { verb: $verb_ty::$verb_variant, owner, recipient, resource },)+
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
    parse_permission!(class, owner, recipient, verb, resource, "filesystem", Filesystem, FilesystemPermissionPattern, FilesystemVerb, parse_agent_owner, parse_agent_recipient, parse_filesystem_resource, ["read" => Read, "write" => Write, "list" => List, "stat" => Stat, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "network", Network, NetworkPermissionPattern, NetworkVerb, parse_empty_owner, parse_agent_recipient, parse_network_resource, ["connect" => Connect]);
    parse_permission!(class, owner, recipient, verb, resource, "env", Env, EnvPermissionPattern, EnvVerb, parse_agent_owner, parse_agent_recipient, parse_env_resource, ["read" => Read]);
    parse_permission!(class, owner, recipient, verb, resource, "oplog", Oplog, OplogPermissionPattern, OplogVerb, parse_agent_owner, parse_agent_recipient, parse_oplog_resource, ["read" => Read]);
    parse_permission!(class, owner, recipient, verb, resource, "config", Config, ConfigPermissionPattern, ConfigVerb, parse_agent_owner, parse_agent_recipient, parse_config_resource, ["read" => Read]);
    parse_permission!(class, owner, recipient, verb, resource, "secret", Secret, SecretPermissionPattern, SecretVerb, parse_environment_owner, parse_agent_recipient, parse_secret_resource, ["hold" => Hold, "mint" => Mint, "reveal" => Reveal]);
    parse_permission!(class, owner, recipient, verb, resource, "agent", Agent, AgentPermissionPattern, AgentVerb, parse_agent_owner, parse_agent_recipient, parse_agent_resource, ["invoke" => Invoke, "view" => View, "create" => Create, "delete" => Delete, "interrupt" => Interrupt, "resume" => Resume, "update-revision" => UpdateRevision, "fork" => Fork, "revert" => Revert, "cancel-invocation" => CancelInvocation, "activate-plugin" => ActivatePlugin, "deactivate-plugin" => DeactivatePlugin]);
    parse_permission!(class, owner, recipient, verb, resource, "tool", Tool, ToolPermissionPattern, ToolVerb, parse_tool_owner, parse_agent_recipient, parse_tool_resource, ["invoke" => Invoke]);
    parse_permission!(class, owner, recipient, verb, resource, "kv", Kv, KvPermissionPattern, KvVerb, parse_environment_owner, parse_agent_recipient, parse_kv_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "blob", Blob, BlobPermissionPattern, BlobVerb, parse_environment_owner, parse_agent_recipient, parse_blob_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "rdbms", Rdbms, RdbmsPermissionPattern, RdbmsVerb, parse_environment_owner, parse_agent_recipient, parse_rdbms_resource, ["query" => Query, "execute" => Execute]);
    parse_permission!(class, owner, recipient, verb, resource, "card", Card, CardPermissionPattern, CardVerb, parse_account_owner, parse_agent_recipient, parse_card_resource, ["derive" => Derive, "revoke" => Revoke, "inspect" => Inspect, "install" => Install]);
    parse_permission!(class, owner, recipient, verb, resource, "system", System, SystemPermissionPattern, SystemVerb, parse_empty_owner, parse_account_recipient, parse_system_resource, ["create-account" => CreateAccount, "view-default-plan" => ViewDefaultPlan, "view-account-summaries-report" => ViewAccountSummariesReport, "view-account-counts-report" => ViewAccountCountsReport]);
    parse_permission!(class, owner, recipient, verb, resource, "plan", Plan, PlanPermissionPattern, PlanVerb, parse_empty_owner, parse_account_recipient, parse_plan_resource, ["view" => View, "create" => Create, "update" => Update]);
    parse_permission!(class, owner, recipient, verb, resource, "account", Account, AccountPermissionPattern, AccountVerb, parse_account_owner, parse_account_recipient, parse_account_resource, ["view" => View, "update" => Update, "delete" => Delete, "set-roles" => SetRoles, "set-plan" => SetPlan, "restore" => Restore]);
    parse_permission!(class, owner, recipient, verb, resource, "account.usage", AccountUsage, AccountUsagePermissionPattern, AccountUsageVerb, parse_account_owner, parse_account_recipient, parse_account_usage_resource, ["view" => View]);
    parse_permission!(class, owner, recipient, verb, resource, "account.token", AccountToken, AccountTokenPermissionPattern, AccountTokenVerb, parse_account_owner, parse_account_recipient, parse_account_token_resource, ["create" => Create, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "account.plugin", AccountPlugin, AccountPluginPermissionPattern, AccountPluginVerb, parse_account_owner, parse_account_recipient, parse_account_plugin_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "application", Application, ApplicationPermissionPattern, ApplicationVerb, parse_application_owner, parse_account_recipient, parse_application_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "mint-credential" => MintCredential, "rotate-credential" => RotateCredential, "revoke-credential" => RevokeCredential, "view-credentials" => ViewCredentials]);
    parse_permission!(class, owner, recipient, verb, resource, "environment", Environment, EnvironmentPermissionPattern, EnvironmentVerb, parse_environment_owner, parse_environment_recipient, parse_environment_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "deploy" => Deploy, "rollback" => Rollback, "view-deployment-plan" => ViewDeploymentPlan, "write-deployment-record" => WriteDeploymentRecord]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.share", EnvironmentShare, EnvironmentSharePermissionPattern, EnvironmentShareVerb, parse_environment_owner, parse_environment_recipient, parse_environment_share_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.plugin-grant", EnvironmentPluginGrant, EnvironmentPluginGrantPermissionPattern, EnvironmentPluginGrantVerb, parse_environment_owner, parse_environment_recipient, parse_environment_plugin_grant_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.domain-registration", EnvironmentDomainRegistration, EnvironmentDomainRegistrationPermissionPattern, EnvironmentDomainRegistrationVerb, parse_environment_owner, parse_environment_recipient, parse_environment_domain_registration_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.security-scheme", EnvironmentSecurityScheme, EnvironmentSecuritySchemePermissionPattern, EnvironmentSecuritySchemeVerb, parse_environment_owner, parse_environment_recipient, parse_environment_security_scheme_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.http-api-deployment", EnvironmentHttpApiDeployment, EnvironmentHttpApiDeploymentPermissionPattern, EnvironmentHttpApiDeploymentVerb, parse_environment_owner, parse_environment_recipient, parse_environment_http_api_deployment_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.mcp-deployment", EnvironmentMcpDeployment, EnvironmentMcpDeploymentPermissionPattern, EnvironmentMcpDeploymentVerb, parse_environment_owner, parse_environment_recipient, parse_environment_mcp_deployment_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.agent-secret", EnvironmentAgentSecret, EnvironmentAgentSecretPermissionPattern, EnvironmentAgentSecretVerb, parse_environment_owner, parse_environment_recipient, parse_environment_agent_secret_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.resource-definition", EnvironmentResourceDefinition, EnvironmentResourceDefinitionPermissionPattern, EnvironmentResourceDefinitionVerb, parse_environment_owner, parse_environment_recipient, parse_environment_resource_definition_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.retry-policy", EnvironmentRetryPolicy, EnvironmentRetryPolicyPermissionPattern, EnvironmentRetryPolicyVerb, parse_environment_owner, parse_environment_recipient, parse_environment_retry_policy_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "component", Component, ComponentPermissionPattern, ComponentVerb, parse_component_owner, parse_environment_recipient, parse_component_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "account.oauth2-identity", AccountOauth2Identity, AccountOauth2IdentityPermissionPattern, AccountOauth2IdentityVerb, parse_account_owner, parse_account_recipient, parse_account_oauth2_identity_resource, ["view" => View, "link" => Link, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.initial-files", EnvironmentInitialFiles, EnvironmentInitialFilesPermissionPattern, EnvironmentInitialFilesVerb, parse_component_owner, parse_environment_recipient, parse_environment_initial_files_resource, ["view" => View, "update" => Update, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.kv-bucket", EnvironmentKvBucket, EnvironmentKvBucketPermissionPattern, EnvironmentKvBucketVerb, parse_environment_owner, parse_environment_recipient, parse_environment_kv_bucket_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_permission!(class, owner, recipient, verb, resource, "environment.blob-bucket", EnvironmentBlobBucket, EnvironmentBlobBucketPermissionPattern, EnvironmentBlobBucketVerb, parse_environment_owner, parse_environment_recipient, parse_environment_blob_bucket_resource, ["view" => View, "create" => Create, "delete" => Delete]);

    Err(CardParseError::UnknownClass(class.to_string()))
}

macro_rules! parse_polymorphic_permission {
    ($class:expr, $owner:expr, $recipient:expr, $verb:expr, $resource:expr, $class_name:literal, $variant:ident, $pattern:ident, $verb_ty:ident, $owner_parser:ident, $recipient_parser:ident, $resource_parser:ident, [$($verb_name:literal => $verb_variant:ident),+ $(,)?]) => {
        if $class == $class_name {
            let owner = $owner_parser($class, $owner)?;
            let recipient = $recipient_parser($recipient)?;
            let resource = $resource_parser($class, $resource)?;
            return Ok(PolymorphicPermissionPattern::$variant(match $verb {
                "*" => $pattern::Any { owner, recipient, resource },
                $($verb_name => $pattern::Verb { verb: $verb_ty::$verb_variant, owner, recipient, resource },)+
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
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "filesystem", Filesystem, PolymorphicFilesystemPermissionPattern, FilesystemVerb, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_filesystem_resource, ["read" => Read, "write" => Write, "list" => List, "stat" => Stat, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "network", Network, PolymorphicNetworkPermissionPattern, NetworkVerb, parse_polymorphic_empty_owner, parse_polymorphic_agent_recipient, parse_polymorphic_network_resource, ["connect" => Connect]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "env", Env, PolymorphicEnvPermissionPattern, EnvVerb, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_env_resource, ["read" => Read]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "oplog", Oplog, PolymorphicOplogPermissionPattern, OplogVerb, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_oplog_resource, ["read" => Read]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "config", Config, PolymorphicConfigPermissionPattern, ConfigVerb, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_config_resource, ["read" => Read]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "secret", Secret, PolymorphicSecretPermissionPattern, SecretVerb, parse_polymorphic_environment_owner, parse_polymorphic_agent_recipient, parse_polymorphic_secret_resource, ["hold" => Hold, "mint" => Mint, "reveal" => Reveal]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "agent", Agent, PolymorphicAgentPermissionPattern, AgentVerb, parse_polymorphic_agent_owner, parse_polymorphic_agent_recipient, parse_polymorphic_agent_resource, ["invoke" => Invoke, "view" => View, "create" => Create, "delete" => Delete, "interrupt" => Interrupt, "resume" => Resume, "update-revision" => UpdateRevision, "fork" => Fork, "revert" => Revert, "cancel-invocation" => CancelInvocation, "activate-plugin" => ActivatePlugin, "deactivate-plugin" => DeactivatePlugin]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "tool", Tool, PolymorphicToolPermissionPattern, ToolVerb, parse_polymorphic_tool_owner, parse_polymorphic_agent_recipient, parse_polymorphic_tool_resource, ["invoke" => Invoke]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "kv", Kv, PolymorphicKvPermissionPattern, KvVerb, parse_polymorphic_environment_owner, parse_polymorphic_agent_recipient, parse_polymorphic_kv_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "blob", Blob, PolymorphicBlobPermissionPattern, BlobVerb, parse_polymorphic_environment_owner, parse_polymorphic_agent_recipient, parse_polymorphic_blob_resource, ["read" => Read, "write" => Write, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "rdbms", Rdbms, PolymorphicRdbmsPermissionPattern, RdbmsVerb, parse_polymorphic_environment_owner, parse_polymorphic_agent_recipient, parse_polymorphic_rdbms_resource, ["query" => Query, "execute" => Execute]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "card", Card, PolymorphicCardPermissionPattern, CardVerb, parse_polymorphic_account_owner, parse_polymorphic_agent_recipient, parse_polymorphic_card_resource, ["derive" => Derive, "revoke" => Revoke, "inspect" => Inspect, "install" => Install]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "system", System, PolymorphicSystemPermissionPattern, SystemVerb, parse_polymorphic_empty_owner, parse_polymorphic_account_recipient, parse_polymorphic_system_resource, ["create-account" => CreateAccount, "view-default-plan" => ViewDefaultPlan, "view-account-summaries-report" => ViewAccountSummariesReport, "view-account-counts-report" => ViewAccountCountsReport]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "plan", Plan, PolymorphicPlanPermissionPattern, PlanVerb, parse_polymorphic_empty_owner, parse_polymorphic_account_recipient, parse_polymorphic_plan_resource, ["view" => View, "create" => Create, "update" => Update]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account", Account, PolymorphicAccountPermissionPattern, AccountVerb, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_account_resource, ["view" => View, "update" => Update, "delete" => Delete, "set-roles" => SetRoles, "set-plan" => SetPlan, "restore" => Restore]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account.usage", AccountUsage, PolymorphicAccountUsagePermissionPattern, AccountUsageVerb, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_account_usage_resource, ["view" => View]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account.token", AccountToken, PolymorphicAccountTokenPermissionPattern, AccountTokenVerb, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_account_token_resource, ["create" => Create, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account.plugin", AccountPlugin, PolymorphicAccountPluginPermissionPattern, AccountPluginVerb, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_account_plugin_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "application", Application, PolymorphicApplicationPermissionPattern, ApplicationVerb, parse_polymorphic_application_owner, parse_polymorphic_account_recipient, parse_polymorphic_application_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "mint-credential" => MintCredential, "rotate-credential" => RotateCredential, "revoke-credential" => RevokeCredential, "view-credentials" => ViewCredentials]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment", Environment, PolymorphicEnvironmentPermissionPattern, EnvironmentVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete, "restore" => Restore, "deploy" => Deploy, "rollback" => Rollback, "view-deployment-plan" => ViewDeploymentPlan, "write-deployment-record" => WriteDeploymentRecord]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.share", EnvironmentShare, PolymorphicEnvironmentSharePermissionPattern, EnvironmentShareVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_share_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.plugin-grant", EnvironmentPluginGrant, PolymorphicEnvironmentPluginGrantPermissionPattern, EnvironmentPluginGrantVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_plugin_grant_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.domain-registration", EnvironmentDomainRegistration, PolymorphicEnvironmentDomainRegistrationPermissionPattern, EnvironmentDomainRegistrationVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_domain_registration_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.security-scheme", EnvironmentSecurityScheme, PolymorphicEnvironmentSecuritySchemePermissionPattern, EnvironmentSecuritySchemeVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_security_scheme_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.http-api-deployment", EnvironmentHttpApiDeployment, PolymorphicEnvironmentHttpApiDeploymentPermissionPattern, EnvironmentHttpApiDeploymentVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_http_api_deployment_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.mcp-deployment", EnvironmentMcpDeployment, PolymorphicEnvironmentMcpDeploymentPermissionPattern, EnvironmentMcpDeploymentVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_mcp_deployment_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.agent-secret", EnvironmentAgentSecret, PolymorphicEnvironmentAgentSecretPermissionPattern, EnvironmentAgentSecretVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_agent_secret_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.resource-definition", EnvironmentResourceDefinition, PolymorphicEnvironmentResourceDefinitionPermissionPattern, EnvironmentResourceDefinitionVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_resource_definition_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.retry-policy", EnvironmentRetryPolicy, PolymorphicEnvironmentRetryPolicyPermissionPattern, EnvironmentRetryPolicyVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_retry_policy_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "component", Component, PolymorphicComponentPermissionPattern, ComponentVerb, parse_polymorphic_component_owner, parse_polymorphic_environment_recipient, parse_polymorphic_component_resource, ["view" => View, "create" => Create, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "account.oauth2-identity", AccountOauth2Identity, PolymorphicAccountOauth2IdentityPermissionPattern, AccountOauth2IdentityVerb, parse_polymorphic_account_owner, parse_polymorphic_account_recipient, parse_polymorphic_account_oauth2_identity_resource, ["view" => View, "link" => Link, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.initial-files", EnvironmentInitialFiles, PolymorphicEnvironmentInitialFilesPermissionPattern, EnvironmentInitialFilesVerb, parse_polymorphic_component_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_initial_files_resource, ["view" => View, "update" => Update, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.kv-bucket", EnvironmentKvBucket, PolymorphicEnvironmentKvBucketPermissionPattern, EnvironmentKvBucketVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_kv_bucket_resource, ["view" => View, "create" => Create, "delete" => Delete]);
    parse_polymorphic_permission!(class, owner, recipient, verb, resource, "environment.blob-bucket", EnvironmentBlobBucket, PolymorphicEnvironmentBlobBucketPermissionPattern, EnvironmentBlobBucketVerb, parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient, parse_polymorphic_environment_blob_bucket_resource, ["view" => View, "create" => Create, "delete" => Delete]);

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

fn parse_filesystem_resource(
    _class: &str,
    resource: &str,
) -> Result<FilesystemResourcePattern, CardParseError> {
    if resource == "*" || resource == "**" {
        Ok(FilesystemResourcePattern::Any)
    } else if resource.contains('*') {
        Ok(FilesystemResourcePattern::Glob(resource.to_string()))
    } else {
        Ok(FilesystemResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_filesystem_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicFilesystemResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_filesystem_resource,
        PolymorphicFilesystemResourcePattern::Concrete,
        PolymorphicFilesystemResourcePattern::Slot,
        PolymorphicFilesystemResourcePattern::Template,
    )
}

fn parse_network_resource(
    class: &str,
    resource: &str,
) -> Result<NetworkResourcePattern, CardParseError> {
    if resource == "*" {
        return Ok(NetworkResourcePattern::Any);
    }

    let (host, ports) = if let Some((host, port)) = resource.rsplit_once(':') {
        if port.chars().all(|c| c.is_ascii_digit() || c == '-') {
            (host.to_string(), parse_port_pattern(class, port)?)
        } else {
            (resource.to_string(), PortPattern::Any)
        }
    } else {
        (resource.to_string(), PortPattern::Any)
    };

    Ok(NetworkResourcePattern::HostPort { host, ports })
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

fn parse_env_resource(_class: &str, resource: &str) -> Result<EnvResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvResourcePattern::Any)
    } else {
        Ok(EnvResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_env_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_env_resource,
        PolymorphicEnvResourcePattern::Concrete,
        PolymorphicEnvResourcePattern::Slot,
        PolymorphicEnvResourcePattern::Template,
    )
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

fn parse_config_resource(
    _class: &str,
    resource: &str,
) -> Result<ConfigResourcePattern, CardParseError> {
    if resource == "*" || resource == "**" {
        Ok(ConfigResourcePattern::Any)
    } else if resource.contains('*') {
        Ok(ConfigResourcePattern::Glob(resource.to_string()))
    } else {
        Ok(ConfigResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_config_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicConfigResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_config_resource,
        PolymorphicConfigResourcePattern::Concrete,
        PolymorphicConfigResourcePattern::Slot,
        PolymorphicConfigResourcePattern::Template,
    )
}

fn parse_secret_resource(
    _class: &str,
    resource: &str,
) -> Result<SecretResourcePattern, CardParseError> {
    if resource == "*" || resource == "**" {
        Ok(SecretResourcePattern::Any)
    } else if resource.contains('*') {
        Ok(SecretResourcePattern::Glob(resource.to_string()))
    } else {
        Ok(SecretResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_secret_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicSecretResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_secret_resource,
        PolymorphicSecretResourcePattern::Concrete,
        PolymorphicSecretResourcePattern::Slot,
        PolymorphicSecretResourcePattern::Template,
    )
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

fn parse_kv_resource(_class: &str, resource: &str) -> Result<KvResourcePattern, CardParseError> {
    if resource == "*" || resource == "**" {
        Ok(KvResourcePattern::Any)
    } else if resource.contains('*') {
        Ok(KvResourcePattern::Glob(resource.to_string()))
    } else {
        Ok(KvResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_kv_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicKvResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_kv_resource,
        PolymorphicKvResourcePattern::Concrete,
        PolymorphicKvResourcePattern::Slot,
        PolymorphicKvResourcePattern::Template,
    )
}

fn parse_blob_resource(
    _class: &str,
    resource: &str,
) -> Result<BlobResourcePattern, CardParseError> {
    if resource == "*" || resource == "**" {
        Ok(BlobResourcePattern::Any)
    } else if resource.contains('*') {
        Ok(BlobResourcePattern::Glob(resource.to_string()))
    } else {
        Ok(BlobResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_blob_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicBlobResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_blob_resource,
        PolymorphicBlobResourcePattern::Concrete,
        PolymorphicBlobResourcePattern::Slot,
        PolymorphicBlobResourcePattern::Template,
    )
}

fn parse_rdbms_resource(
    _class: &str,
    resource: &str,
) -> Result<RdbmsResourcePattern, CardParseError> {
    if resource == "*" || resource == "**" {
        Ok(RdbmsResourcePattern::Any)
    } else if resource.contains('*') {
        Ok(RdbmsResourcePattern::Glob(resource.to_string()))
    } else {
        Ok(RdbmsResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_rdbms_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicRdbmsResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_rdbms_resource,
        PolymorphicRdbmsResourcePattern::Concrete,
        PolymorphicRdbmsResourcePattern::Slot,
        PolymorphicRdbmsResourcePattern::Template,
    )
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

fn parse_system_resource(
    class: &str,
    resource: &str,
) -> Result<SystemResourcePattern, CardParseError> {
    if resource.is_empty() {
        Ok(SystemResourcePattern)
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_polymorphic_system_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicSystemResourcePattern, CardParseError> {
    if let Ok(resource) = parse_system_resource(class, resource) {
        Ok(PolymorphicSystemResourcePattern::Concrete(resource))
    } else if let Ok(slot) = SlotVariable::parse(resource) {
        Ok(PolymorphicSystemResourcePattern::Slot(slot))
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_plan_resource(
    _class: &str,
    resource: &str,
) -> Result<PlanResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(PlanResourcePattern::Any)
    } else {
        Ok(PlanResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_plan_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicPlanResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_plan_resource,
        PolymorphicPlanResourcePattern::Concrete,
        PolymorphicPlanResourcePattern::Slot,
        PolymorphicPlanResourcePattern::Template,
    )
}

fn parse_account_resource(
    class: &str,
    resource: &str,
) -> Result<AccountResourcePattern, CardParseError> {
    if resource.is_empty() {
        Ok(AccountResourcePattern)
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_polymorphic_account_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicAccountResourcePattern, CardParseError> {
    if let Ok(resource) = parse_account_resource(class, resource) {
        Ok(PolymorphicAccountResourcePattern::Concrete(resource))
    } else if let Ok(slot) = SlotVariable::parse(resource) {
        Ok(PolymorphicAccountResourcePattern::Slot(slot))
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_account_usage_resource(
    class: &str,
    resource: &str,
) -> Result<AccountUsageResourcePattern, CardParseError> {
    if resource.is_empty() {
        Ok(AccountUsageResourcePattern)
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_polymorphic_account_usage_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicAccountUsageResourcePattern, CardParseError> {
    if let Ok(resource) = parse_account_usage_resource(class, resource) {
        Ok(PolymorphicAccountUsageResourcePattern::Concrete(resource))
    } else if let Ok(slot) = SlotVariable::parse(resource) {
        Ok(PolymorphicAccountUsageResourcePattern::Slot(slot))
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_account_token_resource(
    _class: &str,
    resource: &str,
) -> Result<AccountTokenResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(AccountTokenResourcePattern::Any)
    } else {
        Ok(AccountTokenResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_account_token_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicAccountTokenResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_account_token_resource,
        PolymorphicAccountTokenResourcePattern::Concrete,
        PolymorphicAccountTokenResourcePattern::Slot,
        PolymorphicAccountTokenResourcePattern::Template,
    )
}

fn parse_account_plugin_resource(
    _class: &str,
    resource: &str,
) -> Result<AccountPluginResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(AccountPluginResourcePattern::Any)
    } else {
        Ok(AccountPluginResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_account_plugin_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicAccountPluginResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_account_plugin_resource,
        PolymorphicAccountPluginResourcePattern::Concrete,
        PolymorphicAccountPluginResourcePattern::Slot,
        PolymorphicAccountPluginResourcePattern::Template,
    )
}

fn parse_application_resource(
    class: &str,
    resource: &str,
) -> Result<ApplicationResourcePattern, CardParseError> {
    if resource.is_empty() {
        Ok(ApplicationResourcePattern)
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_polymorphic_application_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicApplicationResourcePattern, CardParseError> {
    if let Ok(resource) = parse_application_resource(class, resource) {
        Ok(PolymorphicApplicationResourcePattern::Concrete(resource))
    } else if let Ok(slot) = SlotVariable::parse(resource) {
        Ok(PolymorphicApplicationResourcePattern::Slot(slot))
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_environment_resource(
    class: &str,
    resource: &str,
) -> Result<EnvironmentResourcePattern, CardParseError> {
    if resource.is_empty() {
        Ok(EnvironmentResourcePattern)
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_polymorphic_environment_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentResourcePattern, CardParseError> {
    if let Ok(resource) = parse_environment_resource(class, resource) {
        Ok(PolymorphicEnvironmentResourcePattern::Concrete(resource))
    } else if let Ok(slot) = SlotVariable::parse(resource) {
        Ok(PolymorphicEnvironmentResourcePattern::Slot(slot))
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_environment_share_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentShareResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentShareResourcePattern::Any)
    } else {
        Ok(EnvironmentShareResourcePattern::Exact(resource.to_string()))
    }
}

fn parse_polymorphic_environment_share_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentShareResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_share_resource,
        PolymorphicEnvironmentShareResourcePattern::Concrete,
        PolymorphicEnvironmentShareResourcePattern::Slot,
        PolymorphicEnvironmentShareResourcePattern::Template,
    )
}

fn parse_environment_plugin_grant_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentPluginGrantResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentPluginGrantResourcePattern::Any)
    } else {
        Ok(EnvironmentPluginGrantResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_plugin_grant_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentPluginGrantResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_plugin_grant_resource,
        PolymorphicEnvironmentPluginGrantResourcePattern::Concrete,
        PolymorphicEnvironmentPluginGrantResourcePattern::Slot,
        PolymorphicEnvironmentPluginGrantResourcePattern::Template,
    )
}

fn parse_environment_domain_registration_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentDomainRegistrationResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentDomainRegistrationResourcePattern::Any)
    } else {
        Ok(EnvironmentDomainRegistrationResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_domain_registration_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentDomainRegistrationResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_domain_registration_resource,
        PolymorphicEnvironmentDomainRegistrationResourcePattern::Concrete,
        PolymorphicEnvironmentDomainRegistrationResourcePattern::Slot,
        PolymorphicEnvironmentDomainRegistrationResourcePattern::Template,
    )
}

fn parse_environment_security_scheme_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentSecuritySchemeResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentSecuritySchemeResourcePattern::Any)
    } else {
        Ok(EnvironmentSecuritySchemeResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_security_scheme_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentSecuritySchemeResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_security_scheme_resource,
        PolymorphicEnvironmentSecuritySchemeResourcePattern::Concrete,
        PolymorphicEnvironmentSecuritySchemeResourcePattern::Slot,
        PolymorphicEnvironmentSecuritySchemeResourcePattern::Template,
    )
}

fn parse_environment_http_api_deployment_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentHttpApiDeploymentResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentHttpApiDeploymentResourcePattern::Any)
    } else {
        Ok(EnvironmentHttpApiDeploymentResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_http_api_deployment_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentHttpApiDeploymentResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_http_api_deployment_resource,
        PolymorphicEnvironmentHttpApiDeploymentResourcePattern::Concrete,
        PolymorphicEnvironmentHttpApiDeploymentResourcePattern::Slot,
        PolymorphicEnvironmentHttpApiDeploymentResourcePattern::Template,
    )
}

fn parse_environment_mcp_deployment_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentMcpDeploymentResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentMcpDeploymentResourcePattern::Any)
    } else {
        Ok(EnvironmentMcpDeploymentResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_mcp_deployment_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentMcpDeploymentResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_mcp_deployment_resource,
        PolymorphicEnvironmentMcpDeploymentResourcePattern::Concrete,
        PolymorphicEnvironmentMcpDeploymentResourcePattern::Slot,
        PolymorphicEnvironmentMcpDeploymentResourcePattern::Template,
    )
}

fn parse_environment_agent_secret_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentAgentSecretResourcePattern, CardParseError> {
    if resource == "*" || resource == "**" {
        Ok(EnvironmentAgentSecretResourcePattern::Any)
    } else if resource.contains('*') {
        Ok(EnvironmentAgentSecretResourcePattern::Glob(
            resource.to_string(),
        ))
    } else {
        Ok(EnvironmentAgentSecretResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_agent_secret_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentAgentSecretResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_agent_secret_resource,
        PolymorphicEnvironmentAgentSecretResourcePattern::Concrete,
        PolymorphicEnvironmentAgentSecretResourcePattern::Slot,
        PolymorphicEnvironmentAgentSecretResourcePattern::Template,
    )
}

fn parse_environment_resource_definition_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentResourceDefinitionResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentResourceDefinitionResourcePattern::Any)
    } else {
        Ok(EnvironmentResourceDefinitionResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_resource_definition_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentResourceDefinitionResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_resource_definition_resource,
        PolymorphicEnvironmentResourceDefinitionResourcePattern::Concrete,
        PolymorphicEnvironmentResourceDefinitionResourcePattern::Slot,
        PolymorphicEnvironmentResourceDefinitionResourcePattern::Template,
    )
}

fn parse_environment_retry_policy_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentRetryPolicyResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentRetryPolicyResourcePattern::Any)
    } else {
        Ok(EnvironmentRetryPolicyResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_retry_policy_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentRetryPolicyResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_retry_policy_resource,
        PolymorphicEnvironmentRetryPolicyResourcePattern::Concrete,
        PolymorphicEnvironmentRetryPolicyResourcePattern::Slot,
        PolymorphicEnvironmentRetryPolicyResourcePattern::Template,
    )
}

fn parse_component_resource(
    class: &str,
    resource: &str,
) -> Result<ComponentResourcePattern, CardParseError> {
    if resource.is_empty() {
        Ok(ComponentResourcePattern)
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_polymorphic_component_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicComponentResourcePattern, CardParseError> {
    if let Ok(resource) = parse_component_resource(class, resource) {
        Ok(PolymorphicComponentResourcePattern::Concrete(resource))
    } else if let Ok(slot) = SlotVariable::parse(resource) {
        Ok(PolymorphicComponentResourcePattern::Slot(slot))
    } else {
        Err(CardParseError::InvalidResource {
            class: class.to_string(),
            resource: resource.to_string(),
        })
    }
}

fn parse_account_oauth2_identity_resource(
    _class: &str,
    resource: &str,
) -> Result<AccountOauth2IdentityResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(AccountOauth2IdentityResourcePattern::Any)
    } else {
        Ok(AccountOauth2IdentityResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_account_oauth2_identity_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicAccountOauth2IdentityResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_account_oauth2_identity_resource,
        PolymorphicAccountOauth2IdentityResourcePattern::Concrete,
        PolymorphicAccountOauth2IdentityResourcePattern::Slot,
        PolymorphicAccountOauth2IdentityResourcePattern::Template,
    )
}

fn parse_environment_initial_files_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentInitialFilesResourcePattern, CardParseError> {
    if resource == "*" || resource == "**" {
        Ok(EnvironmentInitialFilesResourcePattern::Any)
    } else if resource.contains('*') {
        Ok(EnvironmentInitialFilesResourcePattern::Glob(
            resource.to_string(),
        ))
    } else {
        Ok(EnvironmentInitialFilesResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_initial_files_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentInitialFilesResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_initial_files_resource,
        PolymorphicEnvironmentInitialFilesResourcePattern::Concrete,
        PolymorphicEnvironmentInitialFilesResourcePattern::Slot,
        PolymorphicEnvironmentInitialFilesResourcePattern::Template,
    )
}

fn parse_environment_kv_bucket_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentKvBucketResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentKvBucketResourcePattern::Any)
    } else {
        Ok(EnvironmentKvBucketResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_kv_bucket_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentKvBucketResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_kv_bucket_resource,
        PolymorphicEnvironmentKvBucketResourcePattern::Concrete,
        PolymorphicEnvironmentKvBucketResourcePattern::Slot,
        PolymorphicEnvironmentKvBucketResourcePattern::Template,
    )
}

fn parse_environment_blob_bucket_resource(
    _class: &str,
    resource: &str,
) -> Result<EnvironmentBlobBucketResourcePattern, CardParseError> {
    if resource == "*" {
        Ok(EnvironmentBlobBucketResourcePattern::Any)
    } else {
        Ok(EnvironmentBlobBucketResourcePattern::Exact(
            resource.to_string(),
        ))
    }
}

fn parse_polymorphic_environment_blob_bucket_resource(
    class: &str,
    resource: &str,
) -> Result<PolymorphicEnvironmentBlobBucketResourcePattern, CardParseError> {
    parse_polymorphic_resource(
        class,
        resource,
        parse_environment_blob_bucket_resource,
        PolymorphicEnvironmentBlobBucketResourcePattern::Concrete,
        PolymorphicEnvironmentBlobBucketResourcePattern::Slot,
        PolymorphicEnvironmentBlobBucketResourcePattern::Template,
    )
}

fn parse_port_pattern(class: &str, port: &str) -> Result<PortPattern, CardParseError> {
    if let Some((start, end)) = port.split_once('-') {
        let start = start.parse().map_err(|_| CardParseError::InvalidResource {
            class: class.to_string(),
            resource: port.to_string(),
        })?;
        let end = end.parse().map_err(|_| CardParseError::InvalidResource {
            class: class.to_string(),
            resource: port.to_string(),
        })?;
        Ok(PortPattern::Range { start, end })
    } else {
        Ok(PortPattern::Single(port.parse().map_err(|_| {
            CardParseError::InvalidResource {
                class: class.to_string(),
                resource: port.to_string(),
            }
        })?))
    }
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

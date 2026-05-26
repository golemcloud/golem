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

mod account;
mod account_oauth2_identity;
mod account_plugin;
mod account_token;
mod account_usage;
mod agent;
mod application;
mod blob;
mod card;
mod component;
mod config;
mod env;
mod environment;
mod environment_agent_secret;
mod environment_blob_bucket;
mod environment_domain_registration;
mod environment_http_api_deployment;
mod environment_initial_files;
mod environment_kv_bucket;
mod environment_mcp_deployment;
mod environment_plugin_grant;
mod environment_resource_definition;
mod environment_retry_policy;
mod environment_security_scheme;
mod environment_share;
mod filesystem;
mod kv;
mod network;
mod oplog;
mod plan;
mod rdbms;
mod secret;
mod system;
mod tool;

use super::owner::OwnerPattern;
use super::recipient::RecipientPattern;
use crate::base_model::card::CardParseError;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub use account::*;
pub use account_oauth2_identity::*;
pub use account_plugin::*;
pub use account_token::*;
pub use account_usage::*;
pub use agent::*;
pub use application::*;
pub use blob::*;
pub use card::*;
pub use component::*;
pub use config::*;
pub use env::*;
pub use environment::*;
pub use environment_agent_secret::*;
pub use environment_blob_bucket::*;
pub use environment_domain_registration::*;
pub use environment_http_api_deployment::*;
pub use environment_initial_files::*;
pub use environment_kv_bucket::*;
pub use environment_mcp_deployment::*;
pub use environment_plugin_grant::*;
pub use environment_resource_definition::*;
pub use environment_retry_policy::*;
pub use environment_security_scheme::*;
pub use environment_share::*;
pub use filesystem::*;
pub use kv::*;
pub use network::*;
pub use oplog::*;
pub use plan::*;
pub use rdbms::*;
pub use secret::*;
pub use system::*;
pub use tool::*;

#[cfg(feature = "full")]
pub trait VerbPattern:
    Debug
    + Copy
    + Clone
    + PartialEq
    + Eq
    + Serialize
    + for<'de> Deserialize<'de>
    + desert_rust::BinarySerializer
    + desert_rust::BinaryDeserializer
{
    fn parse_verb(verb: &str) -> Option<Self>
    where
        Self: Sized;
}

#[cfg(not(feature = "full"))]
pub trait VerbPattern:
    Debug + Copy + Clone + PartialEq + Eq + Serialize + for<'de> Deserialize<'de>
{
    fn parse_verb(verb: &str) -> Option<Self>
    where
        Self: Sized;
}

#[cfg(feature = "full")]
pub trait ResourcePattern:
    Debug
    + Clone
    + PartialEq
    + Eq
    + Serialize
    + for<'de> Deserialize<'de>
    + desert_rust::BinarySerializer
    + desert_rust::BinaryDeserializer
{
    fn parse_resource(resource: &str) -> Result<Self, CardParseError>
    where
        Self: Sized;

    fn subsumes(&self, other: &Self) -> bool;
}

#[cfg(not(feature = "full"))]
pub trait ResourcePattern:
    Debug + Clone + PartialEq + Eq + Serialize + for<'de> Deserialize<'de>
{
    fn parse_resource(resource: &str) -> Result<Self, CardParseError>
    where
        Self: Sized;

    fn subsumes(&self, other: &Self) -> bool;
}

pub trait PermissionClass {
    type Verb: VerbPattern;
    type Owner: OwnerPattern;
    type Recipient: RecipientPattern;
    type Resource: ResourcePattern;

    const NAME: &'static str;

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern
    where
        Self: Sized;
    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern
    where
        Self: Sized;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(
    serialize = "C::Verb: Serialize, C::Owner: Serialize, C::Recipient: Serialize, C::Resource: Serialize",
    deserialize = "C::Verb: Deserialize<'de>, C::Owner: Deserialize<'de>, C::Recipient: Deserialize<'de>, C::Resource: Deserialize<'de>"
))]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ClassPermissionPattern<C: PermissionClass> {
    Any {
        owner: C::Owner,
        recipient: C::Recipient,
        resource: C::Resource,
    },
    Verb {
        verb: C::Verb,
        owner: C::Owner,
        recipient: C::Recipient,
        resource: C::Resource,
    },
}

impl<C: PermissionClass> ClassPermissionPattern<C> {
    pub fn subsumes(&self, other: &Self) -> bool {
        let (self_verb, self_owner, self_recipient, self_resource) = self.parts();
        let (other_verb, other_owner, other_recipient, other_resource) = other.parts();
        self_owner.subsumes(other_owner)
            && self_recipient.subsumes(other_recipient)
            && (self_verb.is_none() || self_verb == other_verb)
            && self_resource.subsumes(other_resource)
    }

    pub fn matches_holder(&self, holder: &str) -> bool {
        let (_, _, recipient, _) = self.parts();
        recipient.matches_holder(holder)
    }
}

impl<C: PermissionClass> ClassPermissionPattern<C> {
    fn parts(&self) -> (Option<C::Verb>, &C::Owner, &C::Recipient, &C::Resource) {
        match self {
            Self::Any {
                owner,
                recipient,
                resource,
            } => (None, owner, recipient, resource),
            Self::Verb {
                verb,
                owner,
                recipient,
                resource,
            } => (Some(*verb), owner, recipient, resource),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(
    serialize = "C::Verb: Serialize, <C::Owner as OwnerPattern>::Polymorphic: Serialize, <C::Recipient as RecipientPattern>::Polymorphic: Serialize, C::Resource: Serialize",
    deserialize = "C::Verb: Deserialize<'de>, <C::Owner as OwnerPattern>::Polymorphic: Deserialize<'de>, <C::Recipient as RecipientPattern>::Polymorphic: Deserialize<'de>, C::Resource: Deserialize<'de>"
))]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicClassPermissionPattern<C: PermissionClass> {
    Any {
        owner: <C::Owner as OwnerPattern>::Polymorphic,
        recipient: <C::Recipient as RecipientPattern>::Polymorphic,
        resource: C::Resource,
    },
    Verb {
        verb: C::Verb,
        owner: <C::Owner as OwnerPattern>::Polymorphic,
        recipient: <C::Recipient as RecipientPattern>::Polymorphic,
        resource: C::Resource,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PermissionPattern {
    Filesystem(FilesystemPermissionPattern),
    Network(NetworkPermissionPattern),
    Env(EnvPermissionPattern),
    Oplog(OplogPermissionPattern),
    Config(ConfigPermissionPattern),
    Secret(SecretPermissionPattern),
    Agent(AgentPermissionPattern),
    Tool(ToolPermissionPattern),
    Kv(KvPermissionPattern),
    Blob(BlobPermissionPattern),
    Rdbms(RdbmsPermissionPattern),
    Card(CardPermissionPattern),
    System(SystemPermissionPattern),
    Plan(PlanPermissionPattern),
    Account(AccountPermissionPattern),
    AccountUsage(AccountUsagePermissionPattern),
    AccountToken(AccountTokenPermissionPattern),
    AccountPlugin(AccountPluginPermissionPattern),
    Application(ApplicationPermissionPattern),
    Environment(EnvironmentPermissionPattern),
    EnvironmentShare(EnvironmentSharePermissionPattern),
    EnvironmentPluginGrant(EnvironmentPluginGrantPermissionPattern),
    EnvironmentDomainRegistration(EnvironmentDomainRegistrationPermissionPattern),
    EnvironmentSecurityScheme(EnvironmentSecuritySchemePermissionPattern),
    EnvironmentHttpApiDeployment(EnvironmentHttpApiDeploymentPermissionPattern),
    EnvironmentMcpDeployment(EnvironmentMcpDeploymentPermissionPattern),
    EnvironmentAgentSecret(EnvironmentAgentSecretPermissionPattern),
    EnvironmentResourceDefinition(EnvironmentResourceDefinitionPermissionPattern),
    EnvironmentRetryPolicy(EnvironmentRetryPolicyPermissionPattern),
    Component(ComponentPermissionPattern),
    AccountOauth2Identity(AccountOauth2IdentityPermissionPattern),
    EnvironmentInitialFiles(EnvironmentInitialFilesPermissionPattern),
    EnvironmentKvBucket(EnvironmentKvBucketPermissionPattern),
    EnvironmentBlobBucket(EnvironmentBlobBucketPermissionPattern),
}

impl PermissionPattern {
    pub fn class_name(&self) -> &'static str {
        match self {
            Self::Filesystem(_) => FilesystemClass::NAME,
            Self::Network(_) => NetworkClass::NAME,
            Self::Env(_) => EnvClass::NAME,
            Self::Oplog(_) => OplogClass::NAME,
            Self::Config(_) => ConfigClass::NAME,
            Self::Secret(_) => SecretClass::NAME,
            Self::Agent(_) => AgentClass::NAME,
            Self::Tool(_) => ToolClass::NAME,
            Self::Kv(_) => KvClass::NAME,
            Self::Blob(_) => BlobClass::NAME,
            Self::Rdbms(_) => RdbmsClass::NAME,
            Self::Card(_) => CardClass::NAME,
            Self::System(_) => SystemClass::NAME,
            Self::Plan(_) => PlanClass::NAME,
            Self::Account(_) => AccountClass::NAME,
            Self::AccountUsage(_) => AccountUsageClass::NAME,
            Self::AccountToken(_) => AccountTokenClass::NAME,
            Self::AccountPlugin(_) => AccountPluginClass::NAME,
            Self::Application(_) => ApplicationClass::NAME,
            Self::Environment(_) => EnvironmentClass::NAME,
            Self::EnvironmentShare(_) => EnvironmentShareClass::NAME,
            Self::EnvironmentPluginGrant(_) => EnvironmentPluginGrantClass::NAME,
            Self::EnvironmentDomainRegistration(_) => EnvironmentDomainRegistrationClass::NAME,
            Self::EnvironmentSecurityScheme(_) => EnvironmentSecuritySchemeClass::NAME,
            Self::EnvironmentHttpApiDeployment(_) => EnvironmentHttpApiDeploymentClass::NAME,
            Self::EnvironmentMcpDeployment(_) => EnvironmentMcpDeploymentClass::NAME,
            Self::EnvironmentAgentSecret(_) => EnvironmentAgentSecretClass::NAME,
            Self::EnvironmentResourceDefinition(_) => EnvironmentResourceDefinitionClass::NAME,
            Self::EnvironmentRetryPolicy(_) => EnvironmentRetryPolicyClass::NAME,
            Self::Component(_) => ComponentClass::NAME,
            Self::AccountOauth2Identity(_) => AccountOauth2IdentityClass::NAME,
            Self::EnvironmentInitialFiles(_) => EnvironmentInitialFilesClass::NAME,
            Self::EnvironmentKvBucket(_) => EnvironmentKvBucketClass::NAME,
            Self::EnvironmentBlobBucket(_) => EnvironmentBlobBucketClass::NAME,
        }
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Filesystem(a), Self::Filesystem(b)) => a.subsumes(b),
            (Self::Network(a), Self::Network(b)) => a.subsumes(b),
            (Self::Env(a), Self::Env(b)) => a.subsumes(b),
            (Self::Oplog(a), Self::Oplog(b)) => a.subsumes(b),
            (Self::Config(a), Self::Config(b)) => a.subsumes(b),
            (Self::Secret(a), Self::Secret(b)) => a.subsumes(b),
            (Self::Agent(a), Self::Agent(b)) => a.subsumes(b),
            (Self::Tool(a), Self::Tool(b)) => a.subsumes(b),
            (Self::Kv(a), Self::Kv(b)) => a.subsumes(b),
            (Self::Blob(a), Self::Blob(b)) => a.subsumes(b),
            (Self::Rdbms(a), Self::Rdbms(b)) => a.subsumes(b),
            (Self::Card(a), Self::Card(b)) => a.subsumes(b),
            (Self::System(a), Self::System(b)) => a.subsumes(b),
            (Self::Plan(a), Self::Plan(b)) => a.subsumes(b),
            (Self::Account(a), Self::Account(b)) => a.subsumes(b),
            (Self::AccountUsage(a), Self::AccountUsage(b)) => a.subsumes(b),
            (Self::AccountToken(a), Self::AccountToken(b)) => a.subsumes(b),
            (Self::AccountPlugin(a), Self::AccountPlugin(b)) => a.subsumes(b),
            (Self::Application(a), Self::Application(b)) => a.subsumes(b),
            (Self::Environment(a), Self::Environment(b)) => a.subsumes(b),
            (Self::EnvironmentShare(a), Self::EnvironmentShare(b)) => a.subsumes(b),
            (Self::EnvironmentPluginGrant(a), Self::EnvironmentPluginGrant(b)) => a.subsumes(b),
            (Self::EnvironmentDomainRegistration(a), Self::EnvironmentDomainRegistration(b)) => {
                a.subsumes(b)
            }
            (Self::EnvironmentSecurityScheme(a), Self::EnvironmentSecurityScheme(b)) => {
                a.subsumes(b)
            }
            (Self::EnvironmentHttpApiDeployment(a), Self::EnvironmentHttpApiDeployment(b)) => {
                a.subsumes(b)
            }
            (Self::EnvironmentMcpDeployment(a), Self::EnvironmentMcpDeployment(b)) => a.subsumes(b),
            (Self::EnvironmentAgentSecret(a), Self::EnvironmentAgentSecret(b)) => a.subsumes(b),
            (Self::EnvironmentResourceDefinition(a), Self::EnvironmentResourceDefinition(b)) => {
                a.subsumes(b)
            }
            (Self::EnvironmentRetryPolicy(a), Self::EnvironmentRetryPolicy(b)) => a.subsumes(b),
            (Self::Component(a), Self::Component(b)) => a.subsumes(b),
            (Self::AccountOauth2Identity(a), Self::AccountOauth2Identity(b)) => a.subsumes(b),
            (Self::EnvironmentInitialFiles(a), Self::EnvironmentInitialFiles(b)) => a.subsumes(b),
            (Self::EnvironmentKvBucket(a), Self::EnvironmentKvBucket(b)) => a.subsumes(b),
            (Self::EnvironmentBlobBucket(a), Self::EnvironmentBlobBucket(b)) => a.subsumes(b),
            _ => false,
        }
    }

    pub fn matches_recipient(&self, holder: &str) -> bool {
        match self {
            Self::Filesystem(pattern) => pattern.matches_holder(holder),
            Self::Network(pattern) => pattern.matches_holder(holder),
            Self::Env(pattern) => pattern.matches_holder(holder),
            Self::Oplog(pattern) => pattern.matches_holder(holder),
            Self::Config(pattern) => pattern.matches_holder(holder),
            Self::Secret(pattern) => pattern.matches_holder(holder),
            Self::Agent(pattern) => pattern.matches_holder(holder),
            Self::Tool(pattern) => pattern.matches_holder(holder),
            Self::Kv(pattern) => pattern.matches_holder(holder),
            Self::Blob(pattern) => pattern.matches_holder(holder),
            Self::Rdbms(pattern) => pattern.matches_holder(holder),
            Self::Card(pattern) => pattern.matches_holder(holder),
            Self::System(pattern) => pattern.matches_holder(holder),
            Self::Plan(pattern) => pattern.matches_holder(holder),
            Self::Account(pattern) => pattern.matches_holder(holder),
            Self::AccountUsage(pattern) => pattern.matches_holder(holder),
            Self::AccountToken(pattern) => pattern.matches_holder(holder),
            Self::AccountPlugin(pattern) => pattern.matches_holder(holder),
            Self::Application(pattern) => pattern.matches_holder(holder),
            Self::Environment(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentShare(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentPluginGrant(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentDomainRegistration(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentSecurityScheme(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentHttpApiDeployment(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentMcpDeployment(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentAgentSecret(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentResourceDefinition(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentRetryPolicy(pattern) => pattern.matches_holder(holder),
            Self::Component(pattern) => pattern.matches_holder(holder),
            Self::AccountOauth2Identity(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentInitialFiles(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentKvBucket(pattern) => pattern.matches_holder(holder),
            Self::EnvironmentBlobBucket(pattern) => pattern.matches_holder(holder),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicPermissionPattern {
    Filesystem(PolymorphicFilesystemPermissionPattern),
    Network(PolymorphicNetworkPermissionPattern),
    Env(PolymorphicEnvPermissionPattern),
    Oplog(PolymorphicOplogPermissionPattern),
    Config(PolymorphicConfigPermissionPattern),
    Secret(PolymorphicSecretPermissionPattern),
    Agent(PolymorphicAgentPermissionPattern),
    Tool(PolymorphicToolPermissionPattern),
    Kv(PolymorphicKvPermissionPattern),
    Blob(PolymorphicBlobPermissionPattern),
    Rdbms(PolymorphicRdbmsPermissionPattern),
    Card(PolymorphicCardPermissionPattern),
    System(PolymorphicSystemPermissionPattern),
    Plan(PolymorphicPlanPermissionPattern),
    Account(PolymorphicAccountPermissionPattern),
    AccountUsage(PolymorphicAccountUsagePermissionPattern),
    AccountToken(PolymorphicAccountTokenPermissionPattern),
    AccountPlugin(PolymorphicAccountPluginPermissionPattern),
    Application(PolymorphicApplicationPermissionPattern),
    Environment(PolymorphicEnvironmentPermissionPattern),
    EnvironmentShare(PolymorphicEnvironmentSharePermissionPattern),
    EnvironmentPluginGrant(PolymorphicEnvironmentPluginGrantPermissionPattern),
    EnvironmentDomainRegistration(PolymorphicEnvironmentDomainRegistrationPermissionPattern),
    EnvironmentSecurityScheme(PolymorphicEnvironmentSecuritySchemePermissionPattern),
    EnvironmentHttpApiDeployment(PolymorphicEnvironmentHttpApiDeploymentPermissionPattern),
    EnvironmentMcpDeployment(PolymorphicEnvironmentMcpDeploymentPermissionPattern),
    EnvironmentAgentSecret(PolymorphicEnvironmentAgentSecretPermissionPattern),
    EnvironmentResourceDefinition(PolymorphicEnvironmentResourceDefinitionPermissionPattern),
    EnvironmentRetryPolicy(PolymorphicEnvironmentRetryPolicyPermissionPattern),
    Component(PolymorphicComponentPermissionPattern),
    AccountOauth2Identity(PolymorphicAccountOauth2IdentityPermissionPattern),
    EnvironmentInitialFiles(PolymorphicEnvironmentInitialFilesPermissionPattern),
    EnvironmentKvBucket(PolymorphicEnvironmentKvBucketPermissionPattern),
    EnvironmentBlobBucket(PolymorphicEnvironmentBlobBucketPermissionPattern),
}

impl PolymorphicPermissionPattern {
    pub fn class_name(&self) -> &'static str {
        match self {
            Self::Filesystem(_) => FilesystemClass::NAME,
            Self::Network(_) => NetworkClass::NAME,
            Self::Env(_) => EnvClass::NAME,
            Self::Oplog(_) => OplogClass::NAME,
            Self::Config(_) => ConfigClass::NAME,
            Self::Secret(_) => SecretClass::NAME,
            Self::Agent(_) => AgentClass::NAME,
            Self::Tool(_) => ToolClass::NAME,
            Self::Kv(_) => KvClass::NAME,
            Self::Blob(_) => BlobClass::NAME,
            Self::Rdbms(_) => RdbmsClass::NAME,
            Self::Card(_) => CardClass::NAME,
            Self::System(_) => SystemClass::NAME,
            Self::Plan(_) => PlanClass::NAME,
            Self::Account(_) => AccountClass::NAME,
            Self::AccountUsage(_) => AccountUsageClass::NAME,
            Self::AccountToken(_) => AccountTokenClass::NAME,
            Self::AccountPlugin(_) => AccountPluginClass::NAME,
            Self::Application(_) => ApplicationClass::NAME,
            Self::Environment(_) => EnvironmentClass::NAME,
            Self::EnvironmentShare(_) => EnvironmentShareClass::NAME,
            Self::EnvironmentPluginGrant(_) => EnvironmentPluginGrantClass::NAME,
            Self::EnvironmentDomainRegistration(_) => EnvironmentDomainRegistrationClass::NAME,
            Self::EnvironmentSecurityScheme(_) => EnvironmentSecuritySchemeClass::NAME,
            Self::EnvironmentHttpApiDeployment(_) => EnvironmentHttpApiDeploymentClass::NAME,
            Self::EnvironmentMcpDeployment(_) => EnvironmentMcpDeploymentClass::NAME,
            Self::EnvironmentAgentSecret(_) => EnvironmentAgentSecretClass::NAME,
            Self::EnvironmentResourceDefinition(_) => EnvironmentResourceDefinitionClass::NAME,
            Self::EnvironmentRetryPolicy(_) => EnvironmentRetryPolicyClass::NAME,
            Self::Component(_) => ComponentClass::NAME,
            Self::AccountOauth2Identity(_) => AccountOauth2IdentityClass::NAME,
            Self::EnvironmentInitialFiles(_) => EnvironmentInitialFilesClass::NAME,
            Self::EnvironmentKvBucket(_) => EnvironmentKvBucketClass::NAME,
            Self::EnvironmentBlobBucket(_) => EnvironmentBlobBucketClass::NAME,
        }
    }
}

fn glob_subsumes(left: &str, right: &str) -> bool {
    left == "**" || left == "*" || left == right
}

fn range_subsumes(
    left_start: Option<u64>,
    left_end: Option<u64>,
    right_start: Option<u64>,
    right_end: Option<u64>,
) -> bool {
    left_start.unwrap_or(0) <= right_start.unwrap_or(0)
        && right_end.unwrap_or(u64::MAX) <= left_end.unwrap_or(u64::MAX)
}

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
use super::recipient::{PolymorphicRecipientPattern, RecipientPattern};
use crate::base_model::card::{CardParseError, PermissionPattern, PolymorphicPermissionPattern};
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

macro_rules! card_permission_classes {
    ($macro:ident) => {
        $macro! {
            Filesystem: FilesystemClass,
            Network: NetworkClass,
            Env: EnvClass,
            Oplog: OplogClass,
            Config: ConfigClass,
            Secret: SecretClass,
            Agent: AgentClass,
            Tool: ToolClass,
            Kv: KvClass,
            Blob: BlobClass,
            Rdbms: RdbmsClass,
            Card: CardClass,
            System: SystemClass,
            Plan: PlanClass,
            Account: AccountClass,
            AccountUsage: AccountUsageClass,
            AccountToken: AccountTokenClass,
            AccountPlugin: AccountPluginClass,
            Application: ApplicationClass,
            Environment: EnvironmentClass,
            EnvironmentShare: EnvironmentShareClass,
            EnvironmentPluginGrant: EnvironmentPluginGrantClass,
            EnvironmentDomainRegistration: EnvironmentDomainRegistrationClass,
            EnvironmentSecurityScheme: EnvironmentSecuritySchemeClass,
            EnvironmentHttpApiDeployment: EnvironmentHttpApiDeploymentClass,
            EnvironmentMcpDeployment: EnvironmentMcpDeploymentClass,
            EnvironmentAgentSecret: EnvironmentAgentSecretClass,
            EnvironmentResourceDefinition: EnvironmentResourceDefinitionClass,
            EnvironmentRetryPolicy: EnvironmentRetryPolicyClass,
            Component: ComponentClass,
            AccountOauth2Identity: AccountOauth2IdentityClass,
            EnvironmentInitialFiles: EnvironmentInitialFilesClass,
            EnvironmentKvBucket: EnvironmentKvBucketClass,
            EnvironmentBlobBucket: EnvironmentBlobBucketClass,
        }
    };
}

pub(crate) use card_permission_classes;

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
    serialize = "C::Verb: Serialize, C::Owner: Serialize, C::Resource: Serialize",
    deserialize = "C::Verb: Deserialize<'de>, C::Owner: Deserialize<'de>, C::Resource: Deserialize<'de>"
))]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ClassPermissionPattern<C: PermissionClass> {
    Any {
        owner: C::Owner,
        recipient: RecipientPattern,
        resource: C::Resource,
    },
    Verb {
        verb: C::Verb,
        owner: C::Owner,
        recipient: RecipientPattern,
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
    fn parts(&self) -> (Option<C::Verb>, &C::Owner, &RecipientPattern, &C::Resource) {
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
    serialize = "C::Verb: Serialize, <C::Owner as OwnerPattern>::Polymorphic: Serialize, C::Resource: Serialize",
    deserialize = "C::Verb: Deserialize<'de>, <C::Owner as OwnerPattern>::Polymorphic: Deserialize<'de>, C::Resource: Deserialize<'de>"
))]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicClassPermissionPattern<C: PermissionClass> {
    Any {
        owner: <C::Owner as OwnerPattern>::Polymorphic,
        recipient: RecipientPattern,
        resource: C::Resource,
    },
    Verb {
        verb: C::Verb,
        owner: <C::Owner as OwnerPattern>::Polymorphic,
        recipient: RecipientPattern,
        resource: C::Resource,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(
    serialize = "C::Verb: Serialize, <C::Owner as OwnerPattern>::Polymorphic: Serialize, C::Resource: Serialize",
    deserialize = "C::Verb: Deserialize<'de>, <C::Owner as OwnerPattern>::Polymorphic: Deserialize<'de>, C::Resource: Deserialize<'de>"
))]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicManifestClassPermissionPattern<C: PermissionClass> {
    Any {
        owner: <C::Owner as OwnerPattern>::Polymorphic,
        recipient: PolymorphicRecipientPattern,
        resource: C::Resource,
    },
    Verb {
        verb: C::Verb,
        owner: <C::Owner as OwnerPattern>::Polymorphic,
        recipient: PolymorphicRecipientPattern,
        resource: C::Resource,
    },
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

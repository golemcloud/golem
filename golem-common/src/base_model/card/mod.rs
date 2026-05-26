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

mod algebra;
mod class;
mod owner;
mod parsing;
mod recipient;
mod types;

#[cfg(test)]
mod parsing_tests;

#[cfg(test)]
mod subsumption_tests;

#[cfg(test)]
mod tests;

pub use algebra::*;
pub use class::*;
pub use parsing::*;
pub use types::*;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PermissionPattern {
    Filesystem(ClassPermissionPattern<FilesystemClass>),
    Network(ClassPermissionPattern<NetworkClass>),
    Env(ClassPermissionPattern<EnvClass>),
    Oplog(ClassPermissionPattern<OplogClass>),
    Config(ClassPermissionPattern<ConfigClass>),
    Secret(ClassPermissionPattern<SecretClass>),
    Agent(ClassPermissionPattern<AgentClass>),
    Tool(ClassPermissionPattern<ToolClass>),
    Kv(ClassPermissionPattern<KvClass>),
    Blob(ClassPermissionPattern<BlobClass>),
    Rdbms(ClassPermissionPattern<RdbmsClass>),
    Card(ClassPermissionPattern<CardClass>),
    System(ClassPermissionPattern<SystemClass>),
    Plan(ClassPermissionPattern<PlanClass>),
    Account(ClassPermissionPattern<AccountClass>),
    AccountUsage(ClassPermissionPattern<AccountUsageClass>),
    AccountToken(ClassPermissionPattern<AccountTokenClass>),
    AccountPlugin(ClassPermissionPattern<AccountPluginClass>),
    Application(ClassPermissionPattern<ApplicationClass>),
    Environment(ClassPermissionPattern<EnvironmentClass>),
    EnvironmentShare(ClassPermissionPattern<EnvironmentShareClass>),
    EnvironmentPluginGrant(ClassPermissionPattern<EnvironmentPluginGrantClass>),
    EnvironmentDomainRegistration(ClassPermissionPattern<EnvironmentDomainRegistrationClass>),
    EnvironmentSecurityScheme(ClassPermissionPattern<EnvironmentSecuritySchemeClass>),
    EnvironmentHttpApiDeployment(ClassPermissionPattern<EnvironmentHttpApiDeploymentClass>),
    EnvironmentMcpDeployment(ClassPermissionPattern<EnvironmentMcpDeploymentClass>),
    EnvironmentAgentSecret(ClassPermissionPattern<EnvironmentAgentSecretClass>),
    EnvironmentResourceDefinition(ClassPermissionPattern<EnvironmentResourceDefinitionClass>),
    EnvironmentRetryPolicy(ClassPermissionPattern<EnvironmentRetryPolicyClass>),
    Component(ClassPermissionPattern<ComponentClass>),
    AccountOauth2Identity(ClassPermissionPattern<AccountOauth2IdentityClass>),
    EnvironmentInitialFiles(ClassPermissionPattern<EnvironmentInitialFilesClass>),
    EnvironmentKvBucket(ClassPermissionPattern<EnvironmentKvBucketClass>),
    EnvironmentBlobBucket(ClassPermissionPattern<EnvironmentBlobBucketClass>),
}

impl PermissionPattern {
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
    Filesystem(PolymorphicClassPermissionPattern<FilesystemClass>),
    Network(PolymorphicClassPermissionPattern<NetworkClass>),
    Env(PolymorphicClassPermissionPattern<EnvClass>),
    Oplog(PolymorphicClassPermissionPattern<OplogClass>),
    Config(PolymorphicClassPermissionPattern<ConfigClass>),
    Secret(PolymorphicClassPermissionPattern<SecretClass>),
    Agent(PolymorphicClassPermissionPattern<AgentClass>),
    Tool(PolymorphicClassPermissionPattern<ToolClass>),
    Kv(PolymorphicClassPermissionPattern<KvClass>),
    Blob(PolymorphicClassPermissionPattern<BlobClass>),
    Rdbms(PolymorphicClassPermissionPattern<RdbmsClass>),
    Card(PolymorphicClassPermissionPattern<CardClass>),
    System(PolymorphicClassPermissionPattern<SystemClass>),
    Plan(PolymorphicClassPermissionPattern<PlanClass>),
    Account(PolymorphicClassPermissionPattern<AccountClass>),
    AccountUsage(PolymorphicClassPermissionPattern<AccountUsageClass>),
    AccountToken(PolymorphicClassPermissionPattern<AccountTokenClass>),
    AccountPlugin(PolymorphicClassPermissionPattern<AccountPluginClass>),
    Application(PolymorphicClassPermissionPattern<ApplicationClass>),
    Environment(PolymorphicClassPermissionPattern<EnvironmentClass>),
    EnvironmentShare(PolymorphicClassPermissionPattern<EnvironmentShareClass>),
    EnvironmentPluginGrant(PolymorphicClassPermissionPattern<EnvironmentPluginGrantClass>),
    EnvironmentDomainRegistration(PolymorphicClassPermissionPattern<EnvironmentDomainRegistrationClass>),
    EnvironmentSecurityScheme(PolymorphicClassPermissionPattern<EnvironmentSecuritySchemeClass>),
    EnvironmentHttpApiDeployment(PolymorphicClassPermissionPattern<EnvironmentHttpApiDeploymentClass>),
    EnvironmentMcpDeployment(PolymorphicClassPermissionPattern<EnvironmentMcpDeploymentClass>),
    EnvironmentAgentSecret(PolymorphicClassPermissionPattern<EnvironmentAgentSecretClass>),
    EnvironmentResourceDefinition(PolymorphicClassPermissionPattern<EnvironmentResourceDefinitionClass>),
    EnvironmentRetryPolicy(PolymorphicClassPermissionPattern<EnvironmentRetryPolicyClass>),
    Component(PolymorphicClassPermissionPattern<ComponentClass>),
    AccountOauth2Identity(PolymorphicClassPermissionPattern<AccountOauth2IdentityClass>),
    EnvironmentInitialFiles(PolymorphicClassPermissionPattern<EnvironmentInitialFilesClass>),
    EnvironmentKvBucket(PolymorphicClassPermissionPattern<EnvironmentKvBucketClass>),
    EnvironmentBlobBucket(PolymorphicClassPermissionPattern<EnvironmentBlobBucketClass>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicManifestPermissionPattern {
    Filesystem(PolymorphicManifestClassPermissionPattern<FilesystemClass>),
    Network(PolymorphicManifestClassPermissionPattern<NetworkClass>),
    Env(PolymorphicManifestClassPermissionPattern<EnvClass>),
    Oplog(PolymorphicManifestClassPermissionPattern<OplogClass>),
    Config(PolymorphicManifestClassPermissionPattern<ConfigClass>),
    Secret(PolymorphicManifestClassPermissionPattern<SecretClass>),
    Agent(PolymorphicManifestClassPermissionPattern<AgentClass>),
    Tool(PolymorphicManifestClassPermissionPattern<ToolClass>),
    Kv(PolymorphicManifestClassPermissionPattern<KvClass>),
    Blob(PolymorphicManifestClassPermissionPattern<BlobClass>),
    Rdbms(PolymorphicManifestClassPermissionPattern<RdbmsClass>),
    Card(PolymorphicManifestClassPermissionPattern<CardClass>),
    System(PolymorphicManifestClassPermissionPattern<SystemClass>),
    Plan(PolymorphicManifestClassPermissionPattern<PlanClass>),
    Account(PolymorphicManifestClassPermissionPattern<AccountClass>),
    AccountUsage(PolymorphicManifestClassPermissionPattern<AccountUsageClass>),
    AccountToken(PolymorphicManifestClassPermissionPattern<AccountTokenClass>),
    AccountPlugin(PolymorphicManifestClassPermissionPattern<AccountPluginClass>),
    Application(PolymorphicManifestClassPermissionPattern<ApplicationClass>),
    Environment(PolymorphicManifestClassPermissionPattern<EnvironmentClass>),
    EnvironmentShare(PolymorphicManifestClassPermissionPattern<EnvironmentShareClass>),
    EnvironmentPluginGrant(PolymorphicManifestClassPermissionPattern<EnvironmentPluginGrantClass>),
    EnvironmentDomainRegistration(
        PolymorphicManifestClassPermissionPattern<EnvironmentDomainRegistrationClass>,
    ),
    EnvironmentSecurityScheme(
        PolymorphicManifestClassPermissionPattern<EnvironmentSecuritySchemeClass>,
    ),
    EnvironmentHttpApiDeployment(
        PolymorphicManifestClassPermissionPattern<EnvironmentHttpApiDeploymentClass>,
    ),
    EnvironmentMcpDeployment(
        PolymorphicManifestClassPermissionPattern<EnvironmentMcpDeploymentClass>,
    ),
    EnvironmentAgentSecret(PolymorphicManifestClassPermissionPattern<EnvironmentAgentSecretClass>),
    EnvironmentResourceDefinition(
        PolymorphicManifestClassPermissionPattern<EnvironmentResourceDefinitionClass>,
    ),
    EnvironmentRetryPolicy(PolymorphicManifestClassPermissionPattern<EnvironmentRetryPolicyClass>),
    Component(PolymorphicManifestClassPermissionPattern<ComponentClass>),
    AccountOauth2Identity(PolymorphicManifestClassPermissionPattern<AccountOauth2IdentityClass>),
    EnvironmentInitialFiles(
        PolymorphicManifestClassPermissionPattern<EnvironmentInitialFilesClass>,
    ),
    EnvironmentKvBucket(PolymorphicManifestClassPermissionPattern<EnvironmentKvBucketClass>),
    EnvironmentBlobBucket(PolymorphicManifestClassPermissionPattern<EnvironmentBlobBucketClass>),
}

macro_rules! class_name_match {
    ($self:expr) => {
        match $self {
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
    };
}

impl PermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }
}

impl PolymorphicPermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }
}

impl PolymorphicManifestPermissionPattern {
    pub fn class_name(&self) -> &'static str {
        class_name_match!(self)
    }
}

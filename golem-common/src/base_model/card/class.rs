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

use crate::base_model::card::{RecipientPathPattern, SlotVariable};
use serde::{Deserialize, Serialize};

pub(crate) trait PermissionSubsumes {
    fn subsumes(&self, other: &Self) -> bool;
}

trait ResourceSubsumes {
    fn subsumes(&self, other: &Self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EmptyResourcePattern;

impl ResourceSubsumes for EmptyResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum IdentifierResourcePattern {
    Any,
    Exact(String),
}

impl ResourceSubsumes for IdentifierResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Exact(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum GlobResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl ResourceSubsumes for GlobResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Glob(a), Self::Glob(b)) => glob_subsumes(a, b),
            (Self::Glob(a), Self::Exact(b)) => glob_matches(a, b),
            (Self::Glob(_), Self::Any) => false,
            (Self::Exact(_), Self::Any | Self::Glob(_)) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum NetworkResourcePattern {
    Any,
    HostPort { host: String, ports: PortPattern },
}

impl ResourceSubsumes for NetworkResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (
                Self::HostPort {
                    host: ah,
                    ports: ap,
                },
                Self::HostPort {
                    host: bh,
                    ports: bp,
                },
            ) => glob_subsumes(ah, bh) && ap.subsumes(bp),
            (Self::HostPort { .. }, Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum OplogResourcePattern {
    Any,
    Range {
        start: Option<u64>,
        end: Option<u64>,
    },
}

impl ResourceSubsumes for OplogResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (
                Self::Range {
                    start: as_,
                    end: ae,
                },
                Self::Range { start: bs, end: be },
            ) => range_subsumes(*as_, *ae, *bs, *be),
            (Self::Range { .. }, Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentResourcePattern {
    Any,
    Empty,
    Method(String),
}

impl ResourceSubsumes for AgentResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Empty, Self::Empty) => true,
            (Self::Method(a), Self::Method(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolResourcePattern {
    Any,
    Command(String),
}

impl ResourceSubsumes for ToolResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Command(a), Self::Command(b)) => a == b,
            (Self::Command(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardResourcePattern {
    Any,
    Empty,
    InstallTarget(RecipientPathPattern),
}

impl ResourceSubsumes for CardResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Empty, Self::Empty) => true,
            (Self::InstallTarget(a), Self::InstallTarget(b)) => a.subsumes(b),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PortPattern {
    Any,
    Single(u16),
    Range { start: u16, end: u16 },
}

macro_rules! define_polymorphic_resource_pattern {
    ($name:ident, $concrete:ty) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum $name {
            Concrete($concrete),
            Slot(SlotVariable),
            Template(String),
        }
    };
}

define_polymorphic_resource_pattern!(PolymorphicEmptyResourcePattern, EmptyResourcePattern);
define_polymorphic_resource_pattern!(
    PolymorphicIdentifierResourcePattern,
    IdentifierResourcePattern
);
define_polymorphic_resource_pattern!(PolymorphicGlobResourcePattern, GlobResourcePattern);
define_polymorphic_resource_pattern!(PolymorphicNetworkResourcePattern, NetworkResourcePattern);
define_polymorphic_resource_pattern!(PolymorphicOplogResourcePattern, OplogResourcePattern);
define_polymorphic_resource_pattern!(PolymorphicAgentResourcePattern, AgentResourcePattern);
define_polymorphic_resource_pattern!(PolymorphicToolResourcePattern, ToolResourcePattern);
define_polymorphic_resource_pattern!(PolymorphicCardResourcePattern, CardResourcePattern);

impl PortPattern {
    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Single(a), Self::Single(b)) => a == b,
            (
                Self::Range {
                    start: as_,
                    end: ae,
                },
                Self::Single(b),
            ) => as_ <= b && b <= ae,
            (
                Self::Range {
                    start: as_,
                    end: ae,
                },
                Self::Range { start: bs, end: be },
            ) => as_ <= bs && be <= ae,
            (Self::Single(_), Self::Any | Self::Range { .. }) => false,
            (Self::Range { .. }, Self::Any) => false,
        }
    }
}

macro_rules! define_class_permission_pattern {
    ($name:ident, $resource:ty, [$($verb:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum $name {
            Any($resource),
            $($verb($resource)),+
        }

        impl PermissionSubsumes for $name {
            fn subsumes(&self, other: &Self) -> bool {
                let (self_verb, self_resource) = self.parts();
                let (other_verb, other_resource) = other.parts();
                (self_verb.is_none() || self_verb == other_verb)
                    && self_resource.subsumes(other_resource)
            }
        }

        impl $name {
            fn parts(&self) -> (Option<&'static str>, &$resource) {
                match self {
                    Self::Any(resource) => (None, resource),
                    $(Self::$verb(resource) => (Some(stringify!($verb)), resource)),+
                }
            }
        }
    };
}

macro_rules! define_polymorphic_class_permission_pattern {
    ($name:ident, $resource:ty, [$($verb:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum $name {
            Any($resource),
            $($verb($resource)),+
        }
    };
}

define_class_permission_pattern!(
    FilesystemPermissionPattern,
    GlobResourcePattern,
    [Read, Write, List, Stat, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicFilesystemPermissionPattern,
    PolymorphicGlobResourcePattern,
    [Read, Write, List, Stat, Delete]
);
define_class_permission_pattern!(NetworkPermissionPattern, NetworkResourcePattern, [Connect]);
define_polymorphic_class_permission_pattern!(
    PolymorphicNetworkPermissionPattern,
    PolymorphicNetworkResourcePattern,
    [Connect]
);
define_class_permission_pattern!(EnvPermissionPattern, IdentifierResourcePattern, [Read]);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [Read]
);
define_class_permission_pattern!(OplogPermissionPattern, OplogResourcePattern, [Read]);
define_polymorphic_class_permission_pattern!(
    PolymorphicOplogPermissionPattern,
    PolymorphicOplogResourcePattern,
    [Read]
);
define_class_permission_pattern!(ConfigPermissionPattern, GlobResourcePattern, [Read]);
define_polymorphic_class_permission_pattern!(
    PolymorphicConfigPermissionPattern,
    PolymorphicGlobResourcePattern,
    [Read]
);
define_class_permission_pattern!(
    SecretPermissionPattern,
    GlobResourcePattern,
    [Hold, Mint, Reveal]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicSecretPermissionPattern,
    PolymorphicGlobResourcePattern,
    [Hold, Mint, Reveal]
);
define_class_permission_pattern!(
    AgentPermissionPattern,
    AgentResourcePattern,
    [
        Invoke,
        View,
        Create,
        Delete,
        Interrupt,
        Resume,
        UpdateRevision,
        Fork,
        Revert,
        CancelInvocation,
        ActivatePlugin,
        DeactivatePlugin
    ]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAgentPermissionPattern,
    PolymorphicAgentResourcePattern,
    [
        Invoke,
        View,
        Create,
        Delete,
        Interrupt,
        Resume,
        UpdateRevision,
        Fork,
        Revert,
        CancelInvocation,
        ActivatePlugin,
        DeactivatePlugin
    ]
);
define_class_permission_pattern!(ToolPermissionPattern, ToolResourcePattern, [Invoke]);
define_polymorphic_class_permission_pattern!(
    PolymorphicToolPermissionPattern,
    PolymorphicToolResourcePattern,
    [Invoke]
);
define_class_permission_pattern!(
    KvPermissionPattern,
    GlobResourcePattern,
    [Read, Write, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicKvPermissionPattern,
    PolymorphicGlobResourcePattern,
    [Read, Write, Delete]
);
define_class_permission_pattern!(
    BlobPermissionPattern,
    GlobResourcePattern,
    [Read, Write, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicBlobPermissionPattern,
    PolymorphicGlobResourcePattern,
    [Read, Write, Delete]
);
define_class_permission_pattern!(
    RdbmsPermissionPattern,
    GlobResourcePattern,
    [Query, Execute]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicRdbmsPermissionPattern,
    PolymorphicGlobResourcePattern,
    [Query, Execute]
);
define_class_permission_pattern!(
    CardPermissionPattern,
    CardResourcePattern,
    [Derive, Revoke, Inspect, Install]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicCardPermissionPattern,
    PolymorphicCardResourcePattern,
    [Derive, Revoke, Inspect, Install]
);
define_class_permission_pattern!(
    SystemPermissionPattern,
    EmptyResourcePattern,
    [
        CreateAccount,
        ViewDefaultPlan,
        ViewAccountSummariesReport,
        ViewAccountCountsReport
    ]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicSystemPermissionPattern,
    PolymorphicEmptyResourcePattern,
    [
        CreateAccount,
        ViewDefaultPlan,
        ViewAccountSummariesReport,
        ViewAccountCountsReport
    ]
);
define_class_permission_pattern!(
    PlanPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Update]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicPlanPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update]
);
define_class_permission_pattern!(
    AccountPermissionPattern,
    EmptyResourcePattern,
    [View, Update, Delete, SetRoles, SetPlan, Restore]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountPermissionPattern,
    PolymorphicEmptyResourcePattern,
    [View, Update, Delete, SetRoles, SetPlan, Restore]
);
define_class_permission_pattern!(AccountUsagePermissionPattern, EmptyResourcePattern, [View]);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountUsagePermissionPattern,
    PolymorphicEmptyResourcePattern,
    [View]
);
define_class_permission_pattern!(
    AccountTokenPermissionPattern,
    IdentifierResourcePattern,
    [Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountTokenPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [Create, Delete]
);
define_class_permission_pattern!(
    AccountPluginPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountPluginPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    ApplicationPermissionPattern,
    EmptyResourcePattern,
    [
        View,
        Create,
        Update,
        Delete,
        Restore,
        MintCredential,
        RotateCredential,
        RevokeCredential,
        ViewCredentials
    ]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicApplicationPermissionPattern,
    PolymorphicEmptyResourcePattern,
    [
        View,
        Create,
        Update,
        Delete,
        Restore,
        MintCredential,
        RotateCredential,
        RevokeCredential,
        ViewCredentials
    ]
);
define_class_permission_pattern!(
    EnvironmentPermissionPattern,
    EmptyResourcePattern,
    [
        View,
        Create,
        Update,
        Delete,
        Restore,
        Deploy,
        Rollback,
        ViewDeploymentPlan,
        WriteDeploymentRecord
    ]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentPermissionPattern,
    PolymorphicEmptyResourcePattern,
    [
        View,
        Create,
        Update,
        Delete,
        Restore,
        Deploy,
        Rollback,
        ViewDeploymentPlan,
        WriteDeploymentRecord
    ]
);
define_class_permission_pattern!(
    EnvironmentSharePermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentSharePermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentPluginGrantPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentPluginGrantPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);
define_class_permission_pattern!(
    EnvironmentDomainRegistrationPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentDomainRegistrationPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);
define_class_permission_pattern!(
    EnvironmentSecuritySchemePermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentSecuritySchemePermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentHttpApiDeploymentPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentHttpApiDeploymentPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentMcpDeploymentPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentMcpDeploymentPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentAgentSecretPermissionPattern,
    GlobResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentAgentSecretPermissionPattern,
    PolymorphicGlobResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentResourceDefinitionPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentResourceDefinitionPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentRetryPolicyPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentRetryPolicyPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    ComponentPermissionPattern,
    EmptyResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicComponentPermissionPattern,
    PolymorphicEmptyResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    AccountOauth2IdentityPermissionPattern,
    IdentifierResourcePattern,
    [View, Link, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountOauth2IdentityPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Link, Delete]
);
define_class_permission_pattern!(
    EnvironmentInitialFilesPermissionPattern,
    GlobResourcePattern,
    [View, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentInitialFilesPermissionPattern,
    PolymorphicGlobResourcePattern,
    [View, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentKvBucketPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentKvBucketPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);
define_class_permission_pattern!(
    EnvironmentBlobBucketPermissionPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentBlobBucketPermissionPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);

macro_rules! define_permission_patterns {
    ($(
        $variant:ident($pattern:ident) => $class_name:literal
    ),+ $(,)?) => {

        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PermissionPattern {
            $($variant($pattern)),+
        }

        impl PermissionPattern {
            pub fn class_name(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $class_name),+
                }
            }

            pub fn subsumes(&self, other: &Self) -> bool {
                match (self, other) {
                    $((Self::$variant(a), Self::$variant(b)) => a.subsumes(b)),+,
                    _ => false,
                }
            }
        }
    };
}

define_permission_patterns! {
    Filesystem(FilesystemPermissionPattern) => "filesystem",
    Network(NetworkPermissionPattern) => "network",
    Env(EnvPermissionPattern) => "env",
    Oplog(OplogPermissionPattern) => "oplog",
    Config(ConfigPermissionPattern) => "config",
    Secret(SecretPermissionPattern) => "secret",
    Agent(AgentPermissionPattern) => "agent",
    Tool(ToolPermissionPattern) => "tool",
    Kv(KvPermissionPattern) => "kv",
    Blob(BlobPermissionPattern) => "blob",
    Rdbms(RdbmsPermissionPattern) => "rdbms",
    Card(CardPermissionPattern) => "card",
    System(SystemPermissionPattern) => "system",
    Plan(PlanPermissionPattern) => "plan",
    Account(AccountPermissionPattern) => "account",
    AccountUsage(AccountUsagePermissionPattern) => "account.usage",
    AccountToken(AccountTokenPermissionPattern) => "account.token",
    AccountPlugin(AccountPluginPermissionPattern) => "account.plugin",
    Application(ApplicationPermissionPattern) => "application",
    Environment(EnvironmentPermissionPattern) => "environment",
    EnvironmentShare(EnvironmentSharePermissionPattern) => "environment.share",
    EnvironmentPluginGrant(EnvironmentPluginGrantPermissionPattern) => "environment.plugin-grant",
    EnvironmentDomainRegistration(EnvironmentDomainRegistrationPermissionPattern) => "environment.domain-registration",
    EnvironmentSecurityScheme(EnvironmentSecuritySchemePermissionPattern) => "environment.security-scheme",
    EnvironmentHttpApiDeployment(EnvironmentHttpApiDeploymentPermissionPattern) => "environment.http-api-deployment",
    EnvironmentMcpDeployment(EnvironmentMcpDeploymentPermissionPattern) => "environment.mcp-deployment",
    EnvironmentAgentSecret(EnvironmentAgentSecretPermissionPattern) => "environment.agent-secret",
    EnvironmentResourceDefinition(EnvironmentResourceDefinitionPermissionPattern) => "environment.resource-definition",
    EnvironmentRetryPolicy(EnvironmentRetryPolicyPermissionPattern) => "environment.retry-policy",
    Component(ComponentPermissionPattern) => "component",
    AccountOauth2Identity(AccountOauth2IdentityPermissionPattern) => "account.oauth2-identity",
    EnvironmentInitialFiles(EnvironmentInitialFilesPermissionPattern) => "environment.initial-files",
    EnvironmentKvBucket(EnvironmentKvBucketPermissionPattern) => "environment.kv-bucket",
    EnvironmentBlobBucket(EnvironmentBlobBucketPermissionPattern) => "environment.blob-bucket",
}

macro_rules! define_polymorphic_permission_patterns {
    ($(
        $variant:ident($pattern:ident) => $class_name:literal
    ),+ $(,)?) => {

        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum PolymorphicPermissionPattern {
            $($variant($pattern)),+
        }

        impl PolymorphicPermissionPattern {
            pub fn class_name(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $class_name),+
                }
            }
        }
    };
}

define_polymorphic_permission_patterns! {
    Filesystem(PolymorphicFilesystemPermissionPattern) => "filesystem",
    Network(PolymorphicNetworkPermissionPattern) => "network",
    Env(PolymorphicEnvPermissionPattern) => "env",
    Oplog(PolymorphicOplogPermissionPattern) => "oplog",
    Config(PolymorphicConfigPermissionPattern) => "config",
    Secret(PolymorphicSecretPermissionPattern) => "secret",
    Agent(PolymorphicAgentPermissionPattern) => "agent",
    Tool(PolymorphicToolPermissionPattern) => "tool",
    Kv(PolymorphicKvPermissionPattern) => "kv",
    Blob(PolymorphicBlobPermissionPattern) => "blob",
    Rdbms(PolymorphicRdbmsPermissionPattern) => "rdbms",
    Card(PolymorphicCardPermissionPattern) => "card",
    System(PolymorphicSystemPermissionPattern) => "system",
    Plan(PolymorphicPlanPermissionPattern) => "plan",
    Account(PolymorphicAccountPermissionPattern) => "account",
    AccountUsage(PolymorphicAccountUsagePermissionPattern) => "account.usage",
    AccountToken(PolymorphicAccountTokenPermissionPattern) => "account.token",
    AccountPlugin(PolymorphicAccountPluginPermissionPattern) => "account.plugin",
    Application(PolymorphicApplicationPermissionPattern) => "application",
    Environment(PolymorphicEnvironmentPermissionPattern) => "environment",
    EnvironmentShare(PolymorphicEnvironmentSharePermissionPattern) => "environment.share",
    EnvironmentPluginGrant(PolymorphicEnvironmentPluginGrantPermissionPattern) => "environment.plugin-grant",
    EnvironmentDomainRegistration(PolymorphicEnvironmentDomainRegistrationPermissionPattern) => "environment.domain-registration",
    EnvironmentSecurityScheme(PolymorphicEnvironmentSecuritySchemePermissionPattern) => "environment.security-scheme",
    EnvironmentHttpApiDeployment(PolymorphicEnvironmentHttpApiDeploymentPermissionPattern) => "environment.http-api-deployment",
    EnvironmentMcpDeployment(PolymorphicEnvironmentMcpDeploymentPermissionPattern) => "environment.mcp-deployment",
    EnvironmentAgentSecret(PolymorphicEnvironmentAgentSecretPermissionPattern) => "environment.agent-secret",
    EnvironmentResourceDefinition(PolymorphicEnvironmentResourceDefinitionPermissionPattern) => "environment.resource-definition",
    EnvironmentRetryPolicy(PolymorphicEnvironmentRetryPolicyPermissionPattern) => "environment.retry-policy",
    Component(PolymorphicComponentPermissionPattern) => "component",
    AccountOauth2Identity(PolymorphicAccountOauth2IdentityPermissionPattern) => "account.oauth2-identity",
    EnvironmentInitialFiles(PolymorphicEnvironmentInitialFilesPermissionPattern) => "environment.initial-files",
    EnvironmentKvBucket(PolymorphicEnvironmentKvBucketPermissionPattern) => "environment.kv-bucket",
    EnvironmentBlobBucket(PolymorphicEnvironmentBlobBucketPermissionPattern) => "environment.blob-bucket",
}

fn glob_subsumes(left: &str, right: &str) -> bool {
    left == "**" || left == "*" || left == right
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    if pattern == "**" || pattern == "*" {
        true
    } else if let Some(prefix) = pattern.strip_suffix("**") {
        value.starts_with(prefix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        pattern == value
    }
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

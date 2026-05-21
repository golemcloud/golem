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

pub(crate) trait OwnerSubsumes {
    fn subsumes(&self, other: &Self) -> bool;
}

trait ResourceSubsumes {
    fn subsumes(&self, other: &Self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EmptyOwnerPattern;

impl EmptyOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            Ok(Self)
        } else {
            Err(value.to_string())
        }
    }
}

impl OwnerSubsumes for EmptyOwnerPattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

macro_rules! define_owner_pattern {
    ($name:ident, $depth:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        #[cfg_attr(feature = "full", desert(transparent))]
        pub struct $name(pub String);

        impl $name {
            pub fn new(path: impl Into<String>) -> Self {
                Self(path.into())
            }

            pub fn parse(value: &str) -> Result<Self, String> {
                parse_owner_path(value, $depth).map(|_| Self(value.to_string()))
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl OwnerSubsumes for $name {
            fn subsumes(&self, other: &Self) -> bool {
                let Ok(left) = parse_owner_path(&self.0, $depth) else {
                    return false;
                };
                let Ok(right) = parse_owner_path(&other.0, $depth) else {
                    return false;
                };
                owner_path_subsumes(&left, &right)
            }
        }
    };
}

define_owner_pattern!(AccountOwnerPattern, 1);
define_owner_pattern!(ApplicationOwnerPattern, 2);
define_owner_pattern!(EnvironmentOwnerPattern, 3);
define_owner_pattern!(ComponentOwnerPattern, 4);
define_owner_pattern!(AgentOwnerPattern, 5);
define_owner_pattern!(ToolOwnerPattern, 5);

macro_rules! define_polymorphic_owner_pattern {
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

define_polymorphic_owner_pattern!(PolymorphicEmptyOwnerPattern, EmptyOwnerPattern);
define_polymorphic_owner_pattern!(PolymorphicAccountOwnerPattern, AccountOwnerPattern);
define_polymorphic_owner_pattern!(PolymorphicApplicationOwnerPattern, ApplicationOwnerPattern);
define_polymorphic_owner_pattern!(PolymorphicEnvironmentOwnerPattern, EnvironmentOwnerPattern);
define_polymorphic_owner_pattern!(PolymorphicComponentOwnerPattern, ComponentOwnerPattern);
define_polymorphic_owner_pattern!(PolymorphicAgentOwnerPattern, AgentOwnerPattern);
define_polymorphic_owner_pattern!(PolymorphicToolOwnerPattern, ToolOwnerPattern);

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

impl IdentifierResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
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

impl GlobResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::Glob(value.into())
    }
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

impl NetworkResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn host(host: impl Into<String>) -> Self {
        Self::HostPort {
            host: host.into(),
            ports: PortPattern::Any,
        }
    }

    pub fn host_port(host: impl Into<String>, ports: PortPattern) -> Self {
        Self::HostPort {
            host: host.into(),
            ports,
        }
    }
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

impl OplogResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn range(start: Option<u64>, end: Option<u64>) -> Self {
        Self::Range { start, end }
    }
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

impl AgentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn empty() -> Self {
        Self::Empty
    }

    pub fn method(method: impl Into<String>) -> Self {
        Self::Method(method.into())
    }
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

impl ToolResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn command(command: impl Into<String>) -> Self {
        Self::Command(command.into())
    }
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

impl CardResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn empty() -> Self {
        Self::Empty
    }

    pub fn install_target(target: RecipientPathPattern) -> Self {
        Self::InstallTarget(target)
    }
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
    pub fn any() -> Self {
        Self::Any
    }

    pub fn single(port: u16) -> Self {
        Self::Single(port)
    }

    pub fn range(start: u16, end: u16) -> Self {
        Self::Range { start, end }
    }

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
    ($name:ident, $owner:ty, $resource:ty, [$($verb:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum $name {
            Any { owner: $owner, resource: $resource },
            $($verb { owner: $owner, resource: $resource }),+
        }

        impl PermissionSubsumes for $name {
            fn subsumes(&self, other: &Self) -> bool {
                let (self_verb, self_owner, self_resource) = self.parts();
                let (other_verb, other_owner, other_resource) = other.parts();
                self_owner.subsumes(other_owner)
                    && (self_verb.is_none() || self_verb == other_verb)
                    && self_resource.subsumes(other_resource)
            }
        }

        impl $name {
            fn parts(&self) -> (Option<&'static str>, &$owner, &$resource) {
                match self {
                    Self::Any { owner, resource } => (None, owner, resource),
                    $(Self::$verb { owner, resource } => (Some(stringify!($verb)), owner, resource)),+
                }
            }
        }
    };
}

macro_rules! define_polymorphic_class_permission_pattern {
    ($name:ident, $owner:ty, $resource:ty, [$($verb:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum $name {
            Any { owner: $owner, resource: $resource },
            $($verb { owner: $owner, resource: $resource }),+
        }
    };
}

define_class_permission_pattern!(
    FilesystemPermissionPattern,
    AgentOwnerPattern,
    GlobResourcePattern,
    [Read, Write, List, Stat, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicFilesystemPermissionPattern,
    PolymorphicAgentOwnerPattern,
    PolymorphicGlobResourcePattern,
    [Read, Write, List, Stat, Delete]
);
define_class_permission_pattern!(
    NetworkPermissionPattern,
    EmptyOwnerPattern,
    NetworkResourcePattern,
    [Connect]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicNetworkPermissionPattern,
    PolymorphicEmptyOwnerPattern,
    PolymorphicNetworkResourcePattern,
    [Connect]
);
define_class_permission_pattern!(
    EnvPermissionPattern,
    AgentOwnerPattern,
    IdentifierResourcePattern,
    [Read]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvPermissionPattern,
    PolymorphicAgentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [Read]
);
define_class_permission_pattern!(
    OplogPermissionPattern,
    AgentOwnerPattern,
    OplogResourcePattern,
    [Read]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicOplogPermissionPattern,
    PolymorphicAgentOwnerPattern,
    PolymorphicOplogResourcePattern,
    [Read]
);
define_class_permission_pattern!(
    ConfigPermissionPattern,
    AgentOwnerPattern,
    GlobResourcePattern,
    [Read]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicConfigPermissionPattern,
    PolymorphicAgentOwnerPattern,
    PolymorphicGlobResourcePattern,
    [Read]
);
define_class_permission_pattern!(
    SecretPermissionPattern,
    EnvironmentOwnerPattern,
    GlobResourcePattern,
    [Hold, Mint, Reveal]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicSecretPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicGlobResourcePattern,
    [Hold, Mint, Reveal]
);
define_class_permission_pattern!(
    AgentPermissionPattern,
    AgentOwnerPattern,
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
    PolymorphicAgentOwnerPattern,
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
define_class_permission_pattern!(
    ToolPermissionPattern,
    ToolOwnerPattern,
    ToolResourcePattern,
    [Invoke]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicToolPermissionPattern,
    PolymorphicToolOwnerPattern,
    PolymorphicToolResourcePattern,
    [Invoke]
);
define_class_permission_pattern!(
    KvPermissionPattern,
    EnvironmentOwnerPattern,
    GlobResourcePattern,
    [Read, Write, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicKvPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicGlobResourcePattern,
    [Read, Write, Delete]
);
define_class_permission_pattern!(
    BlobPermissionPattern,
    EnvironmentOwnerPattern,
    GlobResourcePattern,
    [Read, Write, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicBlobPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicGlobResourcePattern,
    [Read, Write, Delete]
);
define_class_permission_pattern!(
    RdbmsPermissionPattern,
    EnvironmentOwnerPattern,
    GlobResourcePattern,
    [Query, Execute]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicRdbmsPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicGlobResourcePattern,
    [Query, Execute]
);
define_class_permission_pattern!(
    CardPermissionPattern,
    AccountOwnerPattern,
    CardResourcePattern,
    [Derive, Revoke, Inspect, Install]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicCardPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicCardResourcePattern,
    [Derive, Revoke, Inspect, Install]
);
define_class_permission_pattern!(
    SystemPermissionPattern,
    EmptyOwnerPattern,
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
    PolymorphicEmptyOwnerPattern,
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
    EmptyOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Update]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicPlanPermissionPattern,
    PolymorphicEmptyOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update]
);
define_class_permission_pattern!(
    AccountPermissionPattern,
    AccountOwnerPattern,
    EmptyResourcePattern,
    [View, Update, Delete, SetRoles, SetPlan, Restore]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicEmptyResourcePattern,
    [View, Update, Delete, SetRoles, SetPlan, Restore]
);
define_class_permission_pattern!(
    AccountUsagePermissionPattern,
    AccountOwnerPattern,
    EmptyResourcePattern,
    [View]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountUsagePermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicEmptyResourcePattern,
    [View]
);
define_class_permission_pattern!(
    AccountTokenPermissionPattern,
    AccountOwnerPattern,
    IdentifierResourcePattern,
    [Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountTokenPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [Create, Delete]
);
define_class_permission_pattern!(
    AccountPluginPermissionPattern,
    AccountOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountPluginPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    ApplicationPermissionPattern,
    ApplicationOwnerPattern,
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
    PolymorphicApplicationOwnerPattern,
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
    EnvironmentOwnerPattern,
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
    PolymorphicEnvironmentOwnerPattern,
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
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentSharePermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentPluginGrantPermissionPattern,
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentPluginGrantPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);
define_class_permission_pattern!(
    EnvironmentDomainRegistrationPermissionPattern,
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentDomainRegistrationPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);
define_class_permission_pattern!(
    EnvironmentSecuritySchemePermissionPattern,
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentSecuritySchemePermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentHttpApiDeploymentPermissionPattern,
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentHttpApiDeploymentPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentMcpDeploymentPermissionPattern,
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentMcpDeploymentPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentAgentSecretPermissionPattern,
    EnvironmentOwnerPattern,
    GlobResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentAgentSecretPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicGlobResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentResourceDefinitionPermissionPattern,
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentResourceDefinitionPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentRetryPolicyPermissionPattern,
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentRetryPolicyPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    ComponentPermissionPattern,
    ComponentOwnerPattern,
    EmptyResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicComponentPermissionPattern,
    PolymorphicComponentOwnerPattern,
    PolymorphicEmptyResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    AccountOauth2IdentityPermissionPattern,
    AccountOwnerPattern,
    IdentifierResourcePattern,
    [View, Link, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountOauth2IdentityPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Link, Delete]
);
define_class_permission_pattern!(
    EnvironmentInitialFilesPermissionPattern,
    ComponentOwnerPattern,
    GlobResourcePattern,
    [View, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentInitialFilesPermissionPattern,
    PolymorphicComponentOwnerPattern,
    PolymorphicGlobResourcePattern,
    [View, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentKvBucketPermissionPattern,
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentKvBucketPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);
define_class_permission_pattern!(
    EnvironmentBlobBucketPermissionPattern,
    EnvironmentOwnerPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentBlobBucketPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
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

fn parse_owner_path(path: &str, depth: usize) -> Result<Vec<&str>, String> {
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() != depth || segments.iter().any(|segment| segment.is_empty()) {
        Err(path.to_string())
    } else {
        Ok(segments)
    }
}

fn owner_path_subsumes(left: &[&str], right: &[&str]) -> bool {
    left.iter()
        .zip(right.iter())
        .all(|(left, right)| owner_segment_subsumes(left, right))
}

fn owner_segment_subsumes(left: &str, right: &str) -> bool {
    left == "*" || left == right || agent_id_type_wildcard_subsumes(left, right)
}

fn agent_id_type_wildcard_subsumes(left: &str, right: &str) -> bool {
    let Some(agent_type) = left.strip_suffix("(*)") else {
        return false;
    };
    right
        .strip_prefix(agent_type)
        .is_some_and(|suffix| suffix.starts_with('(') && suffix.ends_with(')'))
}

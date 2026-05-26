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

use crate::base_model::card::{
    RecipientPathPattern, RecipientPathSlot, RecipientPathTemplate, SlotVariable,
};
use serde::{Deserialize, Serialize};

pub(crate) trait PermissionSubsumes {
    fn subsumes(&self, other: &Self) -> bool;
}

pub(crate) trait OwnerSubsumes {
    fn subsumes(&self, other: &Self) -> bool;
}

pub(crate) trait RecipientSubsumes {
    fn subsumes(&self, other: &Self) -> bool;
}

pub(crate) trait RecipientMatches {
    fn matches_holder(&self, holder: &RecipientPathPattern) -> bool;
}

trait ResourceSubsumes {
    fn subsumes(&self, other: &Self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountRecipientPattern {
    Any,
    Account { account: String },
}

impl AccountRecipientPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        Self::try_from(RecipientPathPattern::parse(value)?)
    }
}

impl TryFrom<RecipientPathPattern> for AccountRecipientPattern {
    type Error = String;

    fn try_from(value: RecipientPathPattern) -> Result<Self, Self::Error> {
        match value {
            RecipientPathPattern::Any => Ok(Self::Any),
            RecipientPathPattern::Account { account } => Ok(Self::Account { account }),
            other => Err(format!("{other:?}")),
        }
    }
}

impl RecipientSubsumes for AccountRecipientPattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Account { account: a }, Self::Account { account: b }) => a == b,
            (Self::Account { .. }, Self::Any) => false,
        }
    }
}

impl RecipientMatches for AccountRecipientPattern {
    fn matches_holder(&self, holder: &RecipientPathPattern) -> bool {
        match (self, holder) {
            (Self::Any, RecipientPathPattern::Account { .. }) => true,
            (Self::Account { account: a }, RecipientPathPattern::Account { account: b }) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentRecipientPattern {
    Any,
    AccountEnvironments {
        account: String,
    },
    ApplicationEnvironments {
        account: String,
        application: String,
    },
    Environment {
        account: String,
        application: String,
        environment: String,
    },
}

impl EnvironmentRecipientPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        Self::try_from(RecipientPathPattern::parse(value)?)
    }
}

impl TryFrom<RecipientPathPattern> for EnvironmentRecipientPattern {
    type Error = String;

    fn try_from(value: RecipientPathPattern) -> Result<Self, Self::Error> {
        match value {
            RecipientPathPattern::Any => Ok(Self::Any),
            RecipientPathPattern::AccountEnvironments { account } => {
                Ok(Self::AccountEnvironments { account })
            }
            RecipientPathPattern::ApplicationEnvironments {
                account,
                application,
            } => Ok(Self::ApplicationEnvironments {
                account,
                application,
            }),
            RecipientPathPattern::Environment {
                account,
                application,
                environment,
            } => Ok(Self::Environment {
                account,
                application,
                environment,
            }),
            other => Err(format!("{other:?}")),
        }
    }
}

impl RecipientSubsumes for EnvironmentRecipientPattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::AccountEnvironments { account: a }, other) => {
                other.account_part().is_some_and(|account| a == account)
            }
            (
                Self::ApplicationEnvironments {
                    account: aa,
                    application: ap,
                },
                other,
            ) => other
                .application_part()
                .is_some_and(|(ba, bp)| aa == ba && ap == bp),
            (
                Self::Environment {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                Self::Environment {
                    account: ba,
                    application: bp,
                    environment: be,
                },
            ) => aa == ba && ap == bp && ae == be,
            (Self::Environment { .. }, _) => false,
        }
    }
}

impl RecipientMatches for EnvironmentRecipientPattern {
    fn matches_holder(&self, holder: &RecipientPathPattern) -> bool {
        let Ok(holder) = Self::try_from(holder.clone()) else {
            return false;
        };
        self.subsumes(&holder)
    }
}

impl EnvironmentRecipientPattern {
    fn account_part(&self) -> Option<&str> {
        match self {
            Self::Any => None,
            Self::AccountEnvironments { account }
            | Self::ApplicationEnvironments { account, .. }
            | Self::Environment { account, .. } => Some(account),
        }
    }

    fn application_part(&self) -> Option<(&str, &str)> {
        match self {
            Self::ApplicationEnvironments {
                account,
                application,
            }
            | Self::Environment {
                account,
                application,
                ..
            } => Some((account, application)),
            Self::Any | Self::AccountEnvironments { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentRecipientPattern {
    Any,
    AccountAgents {
        account: String,
    },
    ApplicationAgents {
        account: String,
        application: String,
    },
    EnvironmentAgents {
        account: String,
        application: String,
        environment: String,
    },
    ComponentAgents {
        account: String,
        application: String,
        environment: String,
        component: String,
    },
    Agent {
        account: String,
        application: String,
        environment: String,
        component: String,
        agent: String,
    },
}

impl AgentRecipientPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        Self::try_from(RecipientPathPattern::parse(value)?)
    }
}

impl TryFrom<RecipientPathPattern> for AgentRecipientPattern {
    type Error = String;

    fn try_from(value: RecipientPathPattern) -> Result<Self, Self::Error> {
        match value {
            RecipientPathPattern::Any => Ok(Self::Any),
            RecipientPathPattern::AccountAgents { account } => Ok(Self::AccountAgents { account }),
            RecipientPathPattern::ApplicationAgents {
                account,
                application,
            } => Ok(Self::ApplicationAgents {
                account,
                application,
            }),
            RecipientPathPattern::EnvironmentAgents {
                account,
                application,
                environment,
            } => Ok(Self::EnvironmentAgents {
                account,
                application,
                environment,
            }),
            RecipientPathPattern::ComponentAgents {
                account,
                application,
                environment,
                component,
            } => Ok(Self::ComponentAgents {
                account,
                application,
                environment,
                component,
            }),
            RecipientPathPattern::Agent {
                account,
                application,
                environment,
                component,
                agent,
            } => Ok(Self::Agent {
                account,
                application,
                environment,
                component,
                agent,
            }),
            other => Err(format!("{other:?}")),
        }
    }
}

impl RecipientSubsumes for AgentRecipientPattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::AccountAgents { account: a }, other) => {
                other.account_part().is_some_and(|account| a == account)
            }
            (
                Self::ApplicationAgents {
                    account: aa,
                    application: ap,
                },
                other,
            ) => other
                .application_part()
                .is_some_and(|(ba, bp)| aa == ba && ap == bp),
            (
                Self::EnvironmentAgents {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                other,
            ) => other
                .environment_part()
                .is_some_and(|(ba, bp, be)| aa == ba && ap == bp && ae == be),
            (
                Self::ComponentAgents {
                    account: aa,
                    application: ap,
                    environment: ae,
                    component: ac,
                },
                other,
            ) => other
                .component_part()
                .is_some_and(|(ba, bp, be, bc)| aa == ba && ap == bp && ae == be && ac == bc),
            (
                Self::Agent {
                    account: aa,
                    application: ap,
                    environment: ae,
                    component: ac,
                    agent: ag,
                },
                Self::Agent {
                    account: ba,
                    application: bp,
                    environment: be,
                    component: bc,
                    agent: bg,
                },
            ) => aa == ba && ap == bp && ae == be && ac == bc && ag == bg,
            (Self::Agent { .. }, _) => false,
        }
    }
}

impl RecipientMatches for AgentRecipientPattern {
    fn matches_holder(&self, holder: &RecipientPathPattern) -> bool {
        let Ok(holder) = Self::try_from(holder.clone()) else {
            return false;
        };
        self.subsumes(&holder)
    }
}

impl AgentRecipientPattern {
    fn account_part(&self) -> Option<&str> {
        match self {
            Self::Any => None,
            Self::AccountAgents { account }
            | Self::ApplicationAgents { account, .. }
            | Self::EnvironmentAgents { account, .. }
            | Self::ComponentAgents { account, .. }
            | Self::Agent { account, .. } => Some(account),
        }
    }

    fn application_part(&self) -> Option<(&str, &str)> {
        match self {
            Self::ApplicationAgents {
                account,
                application,
            }
            | Self::EnvironmentAgents {
                account,
                application,
                ..
            }
            | Self::ComponentAgents {
                account,
                application,
                ..
            }
            | Self::Agent {
                account,
                application,
                ..
            } => Some((account, application)),
            Self::Any | Self::AccountAgents { .. } => None,
        }
    }

    fn environment_part(&self) -> Option<(&str, &str, &str)> {
        match self {
            Self::EnvironmentAgents {
                account,
                application,
                environment,
            }
            | Self::ComponentAgents {
                account,
                application,
                environment,
                ..
            }
            | Self::Agent {
                account,
                application,
                environment,
                ..
            } => Some((account, application, environment)),
            Self::Any | Self::AccountAgents { .. } | Self::ApplicationAgents { .. } => None,
        }
    }

    fn component_part(&self) -> Option<(&str, &str, &str, &str)> {
        match self {
            Self::ComponentAgents {
                account,
                application,
                environment,
                component,
            }
            | Self::Agent {
                account,
                application,
                environment,
                component,
                ..
            } => Some((account, application, environment, component)),
            Self::Any
            | Self::AccountAgents { .. }
            | Self::ApplicationAgents { .. }
            | Self::EnvironmentAgents { .. } => None,
        }
    }
}

macro_rules! define_polymorphic_recipient_pattern {
    ($name:ident, $concrete:ty) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum $name {
            Concrete($concrete),
            Slot(RecipientPathSlot),
            Template(RecipientPathTemplate),
        }
    };
}

define_polymorphic_recipient_pattern!(PolymorphicAccountRecipientPattern, AccountRecipientPattern);
define_polymorphic_recipient_pattern!(
    PolymorphicEnvironmentRecipientPattern,
    EnvironmentRecipientPattern
);
define_polymorphic_recipient_pattern!(PolymorphicAgentRecipientPattern, AgentRecipientPattern);

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
    ($name:ident, $owner:ty, $recipient:ty, $resource:ty, [$($verb:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum $name {
            Any { owner: $owner, recipient: $recipient, resource: $resource },
            $($verb { owner: $owner, recipient: $recipient, resource: $resource }),+
        }

        impl PermissionSubsumes for $name {
            fn subsumes(&self, other: &Self) -> bool {
                let (self_verb, self_owner, self_recipient, self_resource) = self.parts();
                let (other_verb, other_owner, other_recipient, other_resource) = other.parts();
                self_owner.subsumes(other_owner)
                    && self_recipient.subsumes(other_recipient)
                    && (self_verb.is_none() || self_verb == other_verb)
                    && self_resource.subsumes(other_resource)
            }
        }

        impl RecipientMatches for $name {
            fn matches_holder(&self, holder: &RecipientPathPattern) -> bool {
                let (_, _, recipient, _) = self.parts();
                recipient.matches_holder(holder)
            }
        }

        impl $name {
            fn parts(&self) -> (Option<&'static str>, &$owner, &$recipient, &$resource) {
                match self {
                    Self::Any { owner, recipient, resource } => (None, owner, recipient, resource),
                    $(Self::$verb { owner, recipient, resource } => (Some(stringify!($verb)), owner, recipient, resource)),+
                }
            }
        }
    };
}

macro_rules! define_polymorphic_class_permission_pattern {
    ($name:ident, $owner:ty, $recipient:ty, $resource:ty, [$($verb:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        pub enum $name {
            Any { owner: $owner, recipient: $recipient, resource: $resource },
            $($verb { owner: $owner, recipient: $recipient, resource: $resource }),+
        }
    };
}

define_class_permission_pattern!(
    FilesystemPermissionPattern,
    AgentOwnerPattern,
    AgentRecipientPattern,
    GlobResourcePattern,
    [Read, Write, List, Stat, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicFilesystemPermissionPattern,
    PolymorphicAgentOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicGlobResourcePattern,
    [Read, Write, List, Stat, Delete]
);
define_class_permission_pattern!(
    NetworkPermissionPattern,
    EmptyOwnerPattern,
    AgentRecipientPattern,
    NetworkResourcePattern,
    [Connect]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicNetworkPermissionPattern,
    PolymorphicEmptyOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicNetworkResourcePattern,
    [Connect]
);
define_class_permission_pattern!(
    EnvPermissionPattern,
    AgentOwnerPattern,
    AgentRecipientPattern,
    IdentifierResourcePattern,
    [Read]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvPermissionPattern,
    PolymorphicAgentOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [Read]
);
define_class_permission_pattern!(
    OplogPermissionPattern,
    AgentOwnerPattern,
    AgentRecipientPattern,
    OplogResourcePattern,
    [Read]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicOplogPermissionPattern,
    PolymorphicAgentOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicOplogResourcePattern,
    [Read]
);
define_class_permission_pattern!(
    ConfigPermissionPattern,
    AgentOwnerPattern,
    AgentRecipientPattern,
    GlobResourcePattern,
    [Read]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicConfigPermissionPattern,
    PolymorphicAgentOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicGlobResourcePattern,
    [Read]
);
define_class_permission_pattern!(
    SecretPermissionPattern,
    EnvironmentOwnerPattern,
    AgentRecipientPattern,
    GlobResourcePattern,
    [Hold, Mint, Reveal]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicSecretPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicGlobResourcePattern,
    [Hold, Mint, Reveal]
);
define_class_permission_pattern!(
    AgentPermissionPattern,
    AgentOwnerPattern,
    AgentRecipientPattern,
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
    PolymorphicAgentRecipientPattern,
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
    AgentRecipientPattern,
    ToolResourcePattern,
    [Invoke]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicToolPermissionPattern,
    PolymorphicToolOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicToolResourcePattern,
    [Invoke]
);
define_class_permission_pattern!(
    KvPermissionPattern,
    EnvironmentOwnerPattern,
    AgentRecipientPattern,
    GlobResourcePattern,
    [Read, Write, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicKvPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicGlobResourcePattern,
    [Read, Write, Delete]
);
define_class_permission_pattern!(
    BlobPermissionPattern,
    EnvironmentOwnerPattern,
    AgentRecipientPattern,
    GlobResourcePattern,
    [Read, Write, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicBlobPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicGlobResourcePattern,
    [Read, Write, Delete]
);
define_class_permission_pattern!(
    RdbmsPermissionPattern,
    EnvironmentOwnerPattern,
    AgentRecipientPattern,
    GlobResourcePattern,
    [Query, Execute]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicRdbmsPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicGlobResourcePattern,
    [Query, Execute]
);
define_class_permission_pattern!(
    CardPermissionPattern,
    AccountOwnerPattern,
    AgentRecipientPattern,
    CardResourcePattern,
    [Derive, Revoke, Inspect, Install]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicCardPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicAgentRecipientPattern,
    PolymorphicCardResourcePattern,
    [Derive, Revoke, Inspect, Install]
);
define_class_permission_pattern!(
    SystemPermissionPattern,
    EmptyOwnerPattern,
    AccountRecipientPattern,
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
    PolymorphicAccountRecipientPattern,
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
    AccountRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Update]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicPlanPermissionPattern,
    PolymorphicEmptyOwnerPattern,
    PolymorphicAccountRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update]
);
define_class_permission_pattern!(
    AccountPermissionPattern,
    AccountOwnerPattern,
    AccountRecipientPattern,
    EmptyResourcePattern,
    [View, Update, Delete, SetRoles, SetPlan, Restore]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicAccountRecipientPattern,
    PolymorphicEmptyResourcePattern,
    [View, Update, Delete, SetRoles, SetPlan, Restore]
);
define_class_permission_pattern!(
    AccountUsagePermissionPattern,
    AccountOwnerPattern,
    AccountRecipientPattern,
    EmptyResourcePattern,
    [View]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountUsagePermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicAccountRecipientPattern,
    PolymorphicEmptyResourcePattern,
    [View]
);
define_class_permission_pattern!(
    AccountTokenPermissionPattern,
    AccountOwnerPattern,
    AccountRecipientPattern,
    IdentifierResourcePattern,
    [Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountTokenPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicAccountRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [Create, Delete]
);
define_class_permission_pattern!(
    AccountPluginPermissionPattern,
    AccountOwnerPattern,
    AccountRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountPluginPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicAccountRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    ApplicationPermissionPattern,
    ApplicationOwnerPattern,
    AccountRecipientPattern,
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
    PolymorphicAccountRecipientPattern,
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
    EnvironmentRecipientPattern,
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
    PolymorphicEnvironmentRecipientPattern,
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
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentSharePermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentPluginGrantPermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentPluginGrantPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);
define_class_permission_pattern!(
    EnvironmentDomainRegistrationPermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentDomainRegistrationPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);
define_class_permission_pattern!(
    EnvironmentSecuritySchemePermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentSecuritySchemePermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentHttpApiDeploymentPermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentHttpApiDeploymentPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentMcpDeploymentPermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentMcpDeploymentPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentAgentSecretPermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    GlobResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentAgentSecretPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicGlobResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentResourceDefinitionPermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentResourceDefinitionPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentRetryPolicyPermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentRetryPolicyPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    ComponentPermissionPattern,
    ComponentOwnerPattern,
    EnvironmentRecipientPattern,
    EmptyResourcePattern,
    [View, Create, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicComponentPermissionPattern,
    PolymorphicComponentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicEmptyResourcePattern,
    [View, Create, Update, Delete]
);
define_class_permission_pattern!(
    AccountOauth2IdentityPermissionPattern,
    AccountOwnerPattern,
    AccountRecipientPattern,
    IdentifierResourcePattern,
    [View, Link, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicAccountOauth2IdentityPermissionPattern,
    PolymorphicAccountOwnerPattern,
    PolymorphicAccountRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Link, Delete]
);
define_class_permission_pattern!(
    EnvironmentInitialFilesPermissionPattern,
    ComponentOwnerPattern,
    EnvironmentRecipientPattern,
    GlobResourcePattern,
    [View, Update, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentInitialFilesPermissionPattern,
    PolymorphicComponentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicGlobResourcePattern,
    [View, Update, Delete]
);
define_class_permission_pattern!(
    EnvironmentKvBucketPermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentKvBucketPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
    PolymorphicIdentifierResourcePattern,
    [View, Create, Delete]
);
define_class_permission_pattern!(
    EnvironmentBlobBucketPermissionPattern,
    EnvironmentOwnerPattern,
    EnvironmentRecipientPattern,
    IdentifierResourcePattern,
    [View, Create, Delete]
);
define_polymorphic_class_permission_pattern!(
    PolymorphicEnvironmentBlobBucketPermissionPattern,
    PolymorphicEnvironmentOwnerPattern,
    PolymorphicEnvironmentRecipientPattern,
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

            pub fn matches_recipient(&self, holder: &RecipientPathPattern) -> bool {
                match self {
                    $(Self::$variant(pattern) => pattern.matches_holder(holder)),+
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

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
    CardParseError, RecipientPathPattern, RecipientPathSlot, RecipientPathTemplate, SlotVariable,
};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

#[cfg(feature = "full")]
pub trait CardBinaryCodec: desert_rust::BinarySerializer + desert_rust::BinaryDeserializer {}

#[cfg(feature = "full")]
impl<T: desert_rust::BinarySerializer + desert_rust::BinaryDeserializer> CardBinaryCodec for T {}

#[cfg(not(feature = "full"))]
pub trait CardBinaryCodec {}

#[cfg(not(feature = "full"))]
impl<T> CardBinaryCodec for T {}

pub trait Subsumes {
    fn subsumes(&self, other: &Self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct ResourceIdentifier(pub String);

impl ResourceIdentifier {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut chars = value.chars();
        if chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            Ok(Self(value.to_string()))
        } else {
            Err(value.to_string())
        }
    }
}

impl From<&str> for ResourceIdentifier {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct ResourceLiteral(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ResourcePathSegmentPattern {
    Literal(ResourceLiteral),
    Star,
    GlobStar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SlashPathPattern {
    pub segments: Vec<ResourcePathSegmentPattern>,
}

impl SlashPathPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        let Some(path) = value.strip_prefix('/') else {
            return Err(value.to_string());
        };

        let segments = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/')
                .map(parse_resource_path_segment)
                .collect::<Result<Vec<_>, _>>()?
        };

        Ok(Self { segments })
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        resource_segments_subsume(&self.segments, &other.segments)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct DotPathPattern {
    pub segments: Vec<ResourcePathSegmentPattern>,
}

impl DotPathPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            return Err(value.to_string());
        }
        Ok(Self {
            segments: value
                .split('.')
                .map(parse_resource_path_segment)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        resource_segments_subsume(&self.segments, &other.segments)
    }
}

fn parse_resource_path_segment(value: &str) -> Result<ResourcePathSegmentPattern, String> {
    if value.is_empty() {
        Err(value.to_string())
    } else if value == "*" {
        Ok(ResourcePathSegmentPattern::Star)
    } else if value == "**" {
        Ok(ResourcePathSegmentPattern::GlobStar)
    } else if value.contains('*') || value.contains('/') {
        Err(value.to_string())
    } else {
        Ok(ResourcePathSegmentPattern::Literal(ResourceLiteral(
            value.to_string(),
        )))
    }
}

fn resource_segments_subsume(
    left: &[ResourcePathSegmentPattern],
    right: &[ResourcePathSegmentPattern],
) -> bool {
    if left
        .first()
        .is_some_and(|segment| matches!(segment, ResourcePathSegmentPattern::GlobStar))
    {
        return true;
    }

    if left.len() != right.len() {
        return false;
    }

    left.iter()
        .zip(right)
        .all(|(left, right)| match (left, right) {
            (ResourcePathSegmentPattern::GlobStar, _) => true,
            (ResourcePathSegmentPattern::Star, ResourcePathSegmentPattern::Literal(_)) => true,
            (ResourcePathSegmentPattern::Star, ResourcePathSegmentPattern::Star) => true,
            (ResourcePathSegmentPattern::Literal(a), ResourcePathSegmentPattern::Literal(b)) => {
                a == b
            }
            _ => false,
        })
}

pub trait OwnerPattern:
    Subsumes + Debug + Clone + PartialEq + Eq + Serialize + for<'de> Deserialize<'de> + CardBinaryCodec
{
    type Polymorphic: Debug
        + Clone
        + PartialEq
        + Eq
        + Serialize
        + for<'de> Deserialize<'de>
        + CardBinaryCodec;
}

pub trait RecipientPattern:
    Subsumes + Debug + Clone + PartialEq + Eq + Serialize + for<'de> Deserialize<'de> + CardBinaryCodec
{
    type Polymorphic: Debug
        + Clone
        + PartialEq
        + Eq
        + Serialize
        + for<'de> Deserialize<'de>
        + CardBinaryCodec;

    fn matches_holder(&self, holder: &RecipientPathPattern) -> bool;
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

impl Subsumes for AccountRecipientPattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Account { account: a }, Self::Account { account: b }) => a == b,
            (Self::Account { .. }, Self::Any) => false,
        }
    }
}

impl RecipientPattern for AccountRecipientPattern {
    type Polymorphic = PolymorphicAccountRecipientPattern;

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

impl Subsumes for EnvironmentRecipientPattern {
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

impl RecipientPattern for EnvironmentRecipientPattern {
    type Polymorphic = PolymorphicEnvironmentRecipientPattern;

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

impl Subsumes for AgentRecipientPattern {
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

impl RecipientPattern for AgentRecipientPattern {
    type Polymorphic = PolymorphicAgentRecipientPattern;

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

impl Subsumes for EmptyOwnerPattern {
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

        impl Subsumes for $name {
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

impl OwnerPattern for EmptyOwnerPattern {
    type Polymorphic = PolymorphicEmptyOwnerPattern;
}

impl OwnerPattern for AccountOwnerPattern {
    type Polymorphic = PolymorphicAccountOwnerPattern;
}

impl OwnerPattern for ApplicationOwnerPattern {
    type Polymorphic = PolymorphicApplicationOwnerPattern;
}

impl OwnerPattern for EnvironmentOwnerPattern {
    type Polymorphic = PolymorphicEnvironmentOwnerPattern;
}

impl OwnerPattern for ComponentOwnerPattern {
    type Polymorphic = PolymorphicComponentOwnerPattern;
}

impl OwnerPattern for AgentOwnerPattern {
    type Polymorphic = PolymorphicAgentOwnerPattern;
}

impl OwnerPattern for ToolOwnerPattern {
    type Polymorphic = PolymorphicToolOwnerPattern;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PortPattern {
    Any,
    Single(u16),
    Range { start: u16, end: u16 },
}

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

pub trait PermissionClass {
    type Verb: Debug
        + Copy
        + Clone
        + PartialEq
        + Eq
        + Serialize
        + for<'de> Deserialize<'de>
        + CardBinaryCodec;
    type Owner: OwnerPattern;
    type Recipient: RecipientPattern;
    type Resource: Subsumes
        + Debug
        + Clone
        + PartialEq
        + Eq
        + Serialize
        + for<'de> Deserialize<'de>
        + CardBinaryCodec;

    const NAME: &'static str;

    fn parse_verb(verb: &str) -> Option<Self::Verb>;
    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError>;
    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError>;
    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError>;
    fn parse_polymorphic_owner(
        owner: &str,
    ) -> Result<<Self::Owner as OwnerPattern>::Polymorphic, CardParseError>;
    fn parse_polymorphic_recipient(
        recipient: &str,
    ) -> Result<<Self::Recipient as RecipientPattern>::Polymorphic, CardParseError>;
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

impl<C: PermissionClass> Subsumes for ClassPermissionPattern<C> {
    fn subsumes(&self, other: &Self) -> bool {
        let (self_verb, self_owner, self_recipient, self_resource) = self.parts();
        let (other_verb, other_owner, other_recipient, other_resource) = other.parts();
        self_owner.subsumes(other_owner)
            && self_recipient.subsumes(other_recipient)
            && (self_verb.is_none() || self_verb == other_verb)
            && self_resource.subsumes(other_resource)
    }
}

impl<C: PermissionClass> ClassPermissionPattern<C> {
    pub fn matches_holder(&self, holder: &RecipientPathPattern) -> bool {
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

pub mod filesystem;

pub mod network;

pub mod env;

pub mod oplog;

pub mod config;

pub mod secret;

pub mod agent;

pub mod tool;

pub mod kv;

pub mod blob;

pub mod rdbms;

pub mod card;

pub mod system;

pub mod plan;

pub mod account;

pub mod account_usage;

pub mod account_token;

pub mod account_plugin;

pub mod application;

pub mod environment;

pub mod environment_share;

pub mod environment_plugin_grant;

pub mod environment_domain_registration;

pub mod environment_security_scheme;

pub mod environment_http_api_deployment;

pub mod environment_mcp_deployment;

pub mod environment_agent_secret;

pub mod environment_resource_definition;

pub mod environment_retry_policy;

pub mod component;

pub mod account_oauth2_identity;

pub mod environment_initial_files;

pub mod environment_kv_bucket;

pub mod environment_blob_bucket;

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

    pub fn matches_recipient(&self, holder: &RecipientPathPattern) -> bool {
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

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

use crate::base_model::card::{CardBinaryCodec, SlotVariable, Subsumes};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

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

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

use serde::{Deserialize, Serialize};
use std::fmt::Debug;

mod account;
mod agent;
mod application;
mod component;
mod empty;
mod environment;
mod tool;

pub use account::*;
pub use agent::*;
pub use application::*;
pub use component::*;
pub use empty::*;
pub use environment::*;
pub use tool::*;

macro_rules! concrete_owner_conversion {
    ($concrete:ident, $poly:ident, $($slot:pat),* $(,)?) => {
        impl From<$concrete> for $poly {
            fn from(owner: $concrete) -> Self { Self::Concrete(owner) }
        }
        impl $poly {
            pub fn as_concrete(&self) -> Option<$concrete> {
                match self {
                    Self::Concrete(owner) => Some(owner.clone()),
                    $($slot => None,)*
                }
            }
        }
    };
}

concrete_owner_conversion!(EmptyOwnerPattern, PolymorphicEmptyOwnerPattern,);
concrete_owner_conversion!(
    AccountOwnerPattern,
    PolymorphicAccountOwnerPattern,
    Self::Account
);
concrete_owner_conversion!(
    ApplicationOwnerPattern,
    PolymorphicApplicationOwnerPattern,
    Self::AccountApplications,
    Self::AccountApplication { .. },
    Self::App
);
concrete_owner_conversion!(
    EnvironmentOwnerPattern,
    PolymorphicEnvironmentOwnerPattern,
    Self::AccountEnvironments,
    Self::AccountApplicationEnvironments { .. },
    Self::AccountEnvironment { .. },
    Self::ApplicationEnvironments,
    Self::ApplicationEnvironment { .. },
    Self::Env
);
concrete_owner_conversion!(
    ComponentOwnerPattern,
    PolymorphicComponentOwnerPattern,
    Self::AccountComponents,
    Self::AccountApplicationComponents { .. },
    Self::AccountEnvironmentComponents { .. },
    Self::AccountComponent { .. },
    Self::ApplicationComponents,
    Self::ApplicationEnvironmentComponents { .. },
    Self::ApplicationComponent { .. },
    Self::EnvComponents,
    Self::EnvComponent { .. },
    Self::Component
);
concrete_owner_conversion!(
    AgentOwnerPattern,
    PolymorphicAgentOwnerPattern,
    Self::AccountAgents,
    Self::AccountApplicationAgents { .. },
    Self::AccountEnvironmentAgents { .. },
    Self::AccountComponentAgents { .. },
    Self::AccountAgent { .. },
    Self::ApplicationAgents,
    Self::ApplicationEnvironmentAgents { .. },
    Self::ApplicationComponentAgents { .. },
    Self::ApplicationAgent { .. },
    Self::EnvAgents,
    Self::EnvComponentAgents { .. },
    Self::EnvAgent { .. },
    Self::ComponentAgents,
    Self::ComponentAgent { .. },
    Self::Agent
);
concrete_owner_conversion!(
    ToolOwnerPattern,
    PolymorphicToolOwnerPattern,
    Self::AccountTools,
    Self::AccountApplicationTools { .. },
    Self::AccountEnvironmentTools { .. },
    Self::AccountComponentTools { .. },
    Self::AccountTool { .. },
    Self::ApplicationTools,
    Self::ApplicationEnvironmentTools { .. },
    Self::ApplicationComponentTools { .. },
    Self::ApplicationTool { .. },
    Self::EnvTools,
    Self::EnvComponentTools { .. },
    Self::EnvTool { .. },
    Self::ComponentTools,
    Self::ComponentTool { .. }
);

#[cfg(feature = "full")]
pub trait OwnerPattern:
    Debug
    + Clone
    + PartialEq
    + Eq
    + std::hash::Hash
    + Serialize
    + for<'de> Deserialize<'de>
    + desert_rust::BinarySerializer
    + desert_rust::BinaryDeserializer
{
    type Polymorphic: Debug
        + Clone
        + PartialEq
        + Eq
        + std::hash::Hash
        + Serialize
        + for<'de> Deserialize<'de>
        + desert_rust::BinarySerializer
        + desert_rust::BinaryDeserializer;

    fn parse(value: &str) -> Result<Self, String>
    where
        Self: Sized;
    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String>
    where
        Self: Sized;

    fn subsumes(&self, other: &Self) -> bool;
}

#[cfg(not(feature = "full"))]
pub trait OwnerPattern:
    Debug + Clone + PartialEq + Eq + std::hash::Hash + Serialize + for<'de> Deserialize<'de>
{
    type Polymorphic: Debug
        + Clone
        + PartialEq
        + Eq
        + std::hash::Hash
        + Serialize
        + for<'de> Deserialize<'de>;

    fn parse(value: &str) -> Result<Self, String>
    where
        Self: Sized;
    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String>
    where
        Self: Sized;

    fn subsumes(&self, other: &Self) -> bool;
}

pub(super) fn split_leftmost_owner_slot(value: &str) -> Result<Option<(&str, Vec<&str>)>, String> {
    if value.is_empty() {
        return Err(value.to_string());
    }

    let segments = parse_segments(value)?;
    let Some((first, rest)) = segments.split_first() else {
        return Err(value.to_string());
    };

    if first.starts_with('?') {
        if !matches!(
            *first,
            "?account" | "?app" | "?env" | "?component" | "?agent"
        ) || rest.iter().any(|segment| segment.contains('?'))
        {
            Err(value.to_string())
        } else {
            Ok(Some((*first, rest.to_vec())))
        }
    } else if contains_slot_reference(value) {
        Err(value.to_string())
    } else {
        Ok(None)
    }
}

pub(super) fn parse_segments(value: &str) -> Result<Vec<&str>, String> {
    if value.is_empty() {
        return Err(value.to_string());
    }

    let segments: Vec<&str> = value.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        Err(value.to_string())
    } else {
        Ok(segments)
    }
}

pub(super) fn parse_concrete_segment(value: &str) -> Result<&str, String> {
    if value.is_empty() || value == "*" || value.starts_with('?') {
        Err(value.to_string())
    } else {
        Ok(value)
    }
}

fn contains_slot_reference(value: &str) -> bool {
    value.split('/').any(|segment| segment.starts_with('?'))
}

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

use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::EnvironmentOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentRetryPolicyResourcePattern {
    Any,
    Name(EnvironmentRetryPolicyName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct EnvironmentRetryPolicyName(pub String);

impl EnvironmentRetryPolicyName {
    fn parse(value: &str) -> Result<Self, String> {
        parse_environment_retry_policy_identifier(value).map(Self)
    }
}

impl EnvironmentRetryPolicyResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }
}

impl ResourcePattern for EnvironmentRetryPolicyResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentRetryPolicyResourcePattern::Any)
        } else {
            EnvironmentRetryPolicyName::parse(resource)
                .map(EnvironmentRetryPolicyResourcePattern::Name)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentRetryPolicyClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Name(a), Self::Name(b)) => a == b,
            (Self::Name(_), Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentRetryPolicyVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
}
impl VerbPattern for EnvironmentRetryPolicyVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "restore" => Some(Self::Restore),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentRetryPolicyClass;

impl PermissionClass for EnvironmentRetryPolicyClass {
    type Verb = EnvironmentRetryPolicyVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = EnvironmentRetryPolicyResourcePattern;
    const NAME: &'static str = "environment.retry-policy";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentRetryPolicy(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentRetryPolicy(pattern)
    }
}

fn parse_environment_retry_policy_identifier(value: &str) -> Result<String, String> {
    let mut chars = value.chars();
    if chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(value.to_string())
    } else {
        Err(value.to_string())
    }
}

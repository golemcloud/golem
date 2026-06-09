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
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern, glob_subsumes,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::EnvironmentOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentHttpApiDeploymentResourcePattern {
    Any,
    DomainPath { domain: String, path_glob: String },
}

impl EnvironmentHttpApiDeploymentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    fn parse_value(value: &str) -> Result<Self, String> {
        let Some((domain, path_glob)) = value.split_once('.') else {
            return Err(value.to_string());
        };
        if domain.is_empty() || !path_glob.starts_with('/') {
            return Err(value.to_string());
        }
        Ok(Self::DomainPath {
            domain: domain.to_string(),
            path_glob: path_glob.to_string(),
        })
    }
}

impl ResourcePattern for EnvironmentHttpApiDeploymentResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentHttpApiDeploymentResourcePattern::Any)
        } else {
            EnvironmentHttpApiDeploymentResourcePattern::parse_value(resource).map_err(|_| {
                CardParseError::InvalidResource {
                    class: EnvironmentHttpApiDeploymentClass::NAME.to_string(),
                    resource: resource.to_string(),
                }
            })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (
                Self::DomainPath {
                    domain: a_domain,
                    path_glob: a_path,
                },
                Self::DomainPath {
                    domain: b_domain,
                    path_glob: b_path,
                },
            ) => (a_domain == "*" || a_domain == b_domain) && glob_subsumes(a_path, b_path),
            (Self::DomainPath { .. }, Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentHttpApiDeploymentVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
}
impl VerbPattern for EnvironmentHttpApiDeploymentVerb {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentHttpApiDeploymentClass;

impl PermissionClass for EnvironmentHttpApiDeploymentClass {
    type Verb = EnvironmentHttpApiDeploymentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = EnvironmentHttpApiDeploymentResourcePattern;
    const NAME: &'static str = "environment.http-api-deployment";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentHttpApiDeployment(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentHttpApiDeployment(pattern)
    }
}

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
use combine::parser::char::string;
use combine::{EasyParser, Parser, eof, many1, optional, satisfy};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourcePattern {
    Any,
    Revision { revision: u64 },
}

impl ResourcePattern for EnvironmentResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource.is_empty() || resource == "*" {
            Ok(EnvironmentResourcePattern::Any)
        } else {
            parse_environment_resource(resource).map_err(|_| CardParseError::InvalidResource {
                class: EnvironmentClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Revision { revision: a }, Self::Revision { revision: b }) => a == b,
            (Self::Revision { .. }, Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentVerb {
    View,
    Create,
    Update,
    Delete,
    Deploy,
    Rollback,
    ViewDeployment,
    ViewDeploymentPlan,
    ViewAgentTypes,
    WriteDeploymentRecord,
}
impl VerbPattern for EnvironmentVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "deploy" => Some(Self::Deploy),
            "rollback" => Some(Self::Rollback),
            "view-deployment" => Some(Self::ViewDeployment),
            "view-deployment-plan" => Some(Self::ViewDeploymentPlan),
            "view-agent-types" => Some(Self::ViewAgentTypes),
            "write-deployment-record" => Some(Self::WriteDeploymentRecord),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentClass;

impl PermissionClass for EnvironmentClass {
    type Verb = EnvironmentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = EnvironmentResourcePattern;
    const NAME: &'static str = "environment";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Environment(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Environment(pattern)
    }
}

fn parse_environment_resource(resource: &str) -> Result<EnvironmentResourcePattern, String> {
    let parser = optional(string("@"))
        .with(string("rev=").with(many1(satisfy(|c: char| c.is_ascii_digit()))));
    let mut parser = parser.skip(eof());

    let (revision, _): (String, &str) = parser
        .easy_parse(resource)
        .map_err(|_| resource.to_string())?;

    Ok(EnvironmentResourcePattern::Revision {
        revision: revision.parse::<u64>().map_err(|_| resource.to_string())?,
    })
}

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
use crate::model::card::owner::ComponentOwnerPattern;
use combine::parser::char::string;
use combine::{EasyParser, Parser, eof, many1, optional, satisfy};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ComponentResourcePattern {
    Any,
    Revision { revision: u64 },
}

impl ResourcePattern for ComponentResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource.is_empty() || resource == "*" {
            Ok(ComponentResourcePattern::Any)
        } else {
            parse_component_resource(resource).map_err(|_| CardParseError::InvalidResource {
                class: ComponentClass::NAME.to_string(),
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
pub enum ComponentVerb {
    View,
    Create,
    Update,
    Delete,
}
impl VerbPattern for ComponentVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ComponentClass;

impl PermissionClass for ComponentClass {
    type Verb = ComponentVerb;
    type Owner = ComponentOwnerPattern;
    type Resource = ComponentResourcePattern;
    const NAME: &'static str = "component";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Component(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Component(pattern)
    }
}

fn parse_component_resource(resource: &str) -> Result<ComponentResourcePattern, String> {
    let parser = optional(string("@"))
        .with(string("rev=").with(many1(satisfy(|c: char| c.is_ascii_digit()))));
    let mut parser = parser.skip(eof());

    let (revision, _): (String, &str) = parser
        .easy_parse(resource)
        .map_err(|_| resource.to_string())?;

    Ok(ComponentResourcePattern::Revision {
        revision: revision.parse::<u64>().map_err(|_| resource.to_string())?,
    })
}

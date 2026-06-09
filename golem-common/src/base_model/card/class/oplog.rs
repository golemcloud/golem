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
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern, range_subsumes,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::AgentOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl ResourcePattern for OplogResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            return Ok(OplogResourcePattern::Any);
        }
        let mut start = None;
        let mut end = None;
        for part in resource.split(':') {
            if let Some(value) = part.strip_prefix("start=") {
                start = Some(value.parse().map_err(|_| CardParseError::InvalidResource {
                    class: OplogClass::NAME.to_string(),
                    resource: resource.to_string(),
                })?);
            } else if let Some(value) = part.strip_prefix("end=") {
                end = Some(value.parse().map_err(|_| CardParseError::InvalidResource {
                    class: OplogClass::NAME.to_string(),
                    resource: resource.to_string(),
                })?);
            } else {
                return Err(CardParseError::InvalidResource {
                    class: OplogClass::NAME.to_string(),
                    resource: resource.to_string(),
                });
            }
        }
        Ok(OplogResourcePattern::Range { start, end })
    }

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum OplogVerb {
    Read,
}
impl VerbPattern for OplogVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "read" => Some(Self::Read),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct OplogClass;

impl PermissionClass for OplogClass {
    type Verb = OplogVerb;
    type Owner = AgentOwnerPattern;
    type Resource = OplogResourcePattern;
    const NAME: &'static str = "oplog";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Oplog(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Oplog(pattern)
    }
}

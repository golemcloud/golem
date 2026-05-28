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
pub enum RdbmsResourcePattern {
    Table {
        database: String,
        schema: String,
        table: String,
    },
}

impl RdbmsResourcePattern {
    pub fn any() -> Self {
        Self::Table {
            database: "*".to_string(),
            schema: "*".to_string(),
            table: "*".to_string(),
        }
    }

    fn parse_value(value: &str) -> Result<Self, String> {
        let parts = value.split('.').collect::<Vec<_>>();
        if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
            return Err(value.to_string());
        }
        Ok(Self::Table {
            database: parts[0].to_string(),
            schema: parts[1].to_string(),
            table: parts[2].to_string(),
        })
    }
}

impl ResourcePattern for RdbmsResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        RdbmsResourcePattern::parse_value(resource).map_err(|_| CardParseError::InvalidResource {
            class: RdbmsClass::NAME.to_string(),
            resource: resource.to_string(),
        })
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Table {
                    database: a_database,
                    schema: a_schema,
                    table: a_table,
                },
                Self::Table {
                    database: b_database,
                    schema: b_schema,
                    table: b_table,
                },
            ) => {
                component_subsumes(a_database, b_database)
                    && component_subsumes(a_schema, b_schema)
                    && component_subsumes(a_table, b_table)
            }
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RdbmsVerb {
    Query,
    Mutate,
}
impl VerbPattern for RdbmsVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "query" => Some(Self::Query),
            "mutate" => Some(Self::Mutate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct RdbmsClass;

impl PermissionClass for RdbmsClass {
    type Verb = RdbmsVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = RdbmsResourcePattern;
    const NAME: &'static str = "rdbms";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Rdbms(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Rdbms(pattern)
    }
}

fn component_subsumes(left: &str, right: &str) -> bool {
    left == "*" || left == right
}

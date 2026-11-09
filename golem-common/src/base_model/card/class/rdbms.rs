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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RdbmsResourcePattern {
    Table {
        database: String,
        schema: String,
        table: String,
    },
    Database {
        database: String,
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
        if let Some(database) = value
            .strip_prefix("database(")
            .and_then(|value| value.strip_suffix(')'))
        {
            if database.is_empty()
                || database
                    .chars()
                    .any(|ch| ch.is_whitespace() || "()?".contains(ch))
                || (database.contains('*') && database != "*")
            {
                return Err(value.to_string());
            }
            return Ok(Self::Database {
                database: database.to_string(),
            });
        }
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
            (Self::Database { database: left }, Self::Database { database: right }) => {
                component_subsumes(left, right)
            }
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RdbmsVerb {
    Query,
    Mutate,
    Unanalyzed,
}
impl VerbPattern for RdbmsVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "query" => Some(Self::Query),
            "mutate" => Some(Self::Mutate),
            "unanalyzed" => Some(Self::Unanalyzed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct RdbmsClass;

impl PermissionClass for RdbmsClass {
    type Verb = RdbmsVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = RdbmsResourcePattern;
    const NAME: &'static str = "rdbms";

    fn verb_subsumes(grant: Option<Self::Verb>, target: Option<Self::Verb>) -> bool {
        if target == Some(RdbmsVerb::Unanalyzed) {
            grant == target
        } else {
            grant.is_none() || grant == target
        }
    }

    fn parse_resource(
        verb: Option<Self::Verb>,
        resource: &str,
    ) -> Result<Self::Resource, CardParseError> {
        let parsed = RdbmsResourcePattern::parse_resource(resource)?;
        if matches!(
            (verb, &parsed),
            (
                Some(RdbmsVerb::Unanalyzed),
                RdbmsResourcePattern::Database { .. }
            ) | (
                None | Some(RdbmsVerb::Query | RdbmsVerb::Mutate),
                RdbmsResourcePattern::Table { .. }
            )
        ) {
            Ok(parsed)
        } else {
            Err(CardParseError::InvalidResource {
                class: Self::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

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

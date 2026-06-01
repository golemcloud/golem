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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum KvResourcePattern {
    StoreKey { store: String, key_pattern: String },
}

impl KvResourcePattern {
    pub fn any() -> Self {
        Self::StoreKey {
            store: "*".to_string(),
            key_pattern: "**".to_string(),
        }
    }

    fn parse_value(value: &str) -> Result<Self, String> {
        let Some((store, key_pattern)) = value.split_once('.') else {
            return Err(value.to_string());
        };
        if store.is_empty() || key_pattern.is_empty() {
            return Err(value.to_string());
        }
        Ok(Self::StoreKey {
            store: store.to_string(),
            key_pattern: key_pattern.to_string(),
        })
    }
}

impl ResourcePattern for KvResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        KvResourcePattern::parse_value(resource).map_err(|_| CardParseError::InvalidResource {
            class: KvClass::NAME.to_string(),
            resource: resource.to_string(),
        })
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::StoreKey {
                    store: a_store,
                    key_pattern: a_key,
                },
                Self::StoreKey {
                    store: b_store,
                    key_pattern: b_key,
                },
            ) => (a_store == "*" || a_store == b_store) && glob_subsumes(a_key, b_key),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum KvVerb {
    Read,
    Write,
    Delete,
    List,
}
impl VerbPattern for KvVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "delete" => Some(Self::Delete),
            "list" => Some(Self::List),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct KvClass;

impl PermissionClass for KvClass {
    type Verb = KvVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = KvResourcePattern;
    const NAME: &'static str = "kv";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Kv(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Kv(pattern)
    }
}

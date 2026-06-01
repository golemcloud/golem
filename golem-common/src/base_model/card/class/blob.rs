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
pub enum BlobResourcePattern {
    BucketKey { bucket: String, key_pattern: String },
}

impl BlobResourcePattern {
    pub fn any() -> Self {
        Self::BucketKey {
            bucket: "*".to_string(),
            key_pattern: "**".to_string(),
        }
    }

    fn parse_value(value: &str) -> Result<Self, String> {
        let Some((bucket, key_pattern)) = value.split_once('.') else {
            return Err(value.to_string());
        };
        if bucket.is_empty() || key_pattern.is_empty() {
            return Err(value.to_string());
        }
        Ok(Self::BucketKey {
            bucket: bucket.to_string(),
            key_pattern: key_pattern.to_string(),
        })
    }
}

impl ResourcePattern for BlobResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        BlobResourcePattern::parse_value(resource).map_err(|_| CardParseError::InvalidResource {
            class: BlobClass::NAME.to_string(),
            resource: resource.to_string(),
        })
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::BucketKey {
                    bucket: a_bucket,
                    key_pattern: a_key,
                },
                Self::BucketKey {
                    bucket: b_bucket,
                    key_pattern: b_key,
                },
            ) => (a_bucket == "*" || a_bucket == b_bucket) && glob_subsumes(a_key, b_key),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum BlobVerb {
    Read,
    Write,
    Delete,
    List,
}
impl VerbPattern for BlobVerb {
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
pub struct BlobClass;

impl PermissionClass for BlobClass {
    type Verb = BlobVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = BlobResourcePattern;
    const NAME: &'static str = "blob";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Blob(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Blob(pattern)
    }
}

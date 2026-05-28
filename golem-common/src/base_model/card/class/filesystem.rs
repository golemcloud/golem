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
use crate::model::card::owner::AgentOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum FilesystemResourcePattern {
    Path(FilesystemPathPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct FilesystemPathPattern {
    pub segments: Vec<FilesystemPathSegmentPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum FilesystemPathSegmentPattern {
    Literal(String),
    Star,
    GlobStar,
}

impl FilesystemPathPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        let Some(path) = value.strip_prefix('/') else {
            return Err(value.to_string());
        };
        let segments = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/')
                .map(parse_filesystem_path_segment)
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Self { segments })
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        filesystem_path_segments_subsume(&self.segments, &other.segments)
    }
}

impl FilesystemResourcePattern {
    pub fn any() -> Self {
        Self::Path(FilesystemPathPattern {
            segments: vec![FilesystemPathSegmentPattern::GlobStar],
        })
    }
}

impl ResourcePattern for FilesystemResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        FilesystemPathPattern::parse(resource)
            .map(FilesystemResourcePattern::Path)
            .map_err(|_| CardParseError::InvalidResource {
                class: FilesystemClass::NAME.to_string(),
                resource: resource.to_string(),
            })
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Path(a), Self::Path(b)) => a.subsumes(b),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum FilesystemVerb {
    Read,
    Write,
    List,
    Stat,
    Delete,
}
impl VerbPattern for FilesystemVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "list" => Some(Self::List),
            "stat" => Some(Self::Stat),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct FilesystemClass;

impl PermissionClass for FilesystemClass {
    type Verb = FilesystemVerb;
    type Owner = AgentOwnerPattern;
    type Resource = FilesystemResourcePattern;
    const NAME: &'static str = "filesystem";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Filesystem(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Filesystem(pattern)
    }
}

fn parse_filesystem_path_segment(value: &str) -> Result<FilesystemPathSegmentPattern, String> {
    if value.is_empty() {
        Err(value.to_string())
    } else if value == "*" {
        Ok(FilesystemPathSegmentPattern::Star)
    } else if value == "**" {
        Ok(FilesystemPathSegmentPattern::GlobStar)
    } else if value.contains('*') || value.contains('/') {
        Err(value.to_string())
    } else {
        Ok(FilesystemPathSegmentPattern::Literal(value.to_string()))
    }
}

fn filesystem_path_segments_subsume(
    left: &[FilesystemPathSegmentPattern],
    right: &[FilesystemPathSegmentPattern],
) -> bool {
    filesystem_path_segments_subsume_from(left, 0, right, 0)
}

fn filesystem_path_segments_subsume_from(
    left: &[FilesystemPathSegmentPattern],
    left_index: usize,
    right: &[FilesystemPathSegmentPattern],
    right_index: usize,
) -> bool {
    let Some(left_segment) = left.get(left_index) else {
        return right_index == right.len();
    };

    match left_segment {
        FilesystemPathSegmentPattern::GlobStar => {
            left_index + 1 == left.len()
                || filesystem_path_segments_subsume_from(left, left_index + 1, right, right_index)
                || (right_index < right.len()
                    && filesystem_path_segments_subsume_from(
                        left,
                        left_index,
                        right,
                        right_index + 1,
                    ))
        }
        _ => {
            right
                .get(right_index)
                .is_some_and(|right_segment| segment_subsumes(left_segment, right_segment))
                && filesystem_path_segments_subsume_from(
                    left,
                    left_index + 1,
                    right,
                    right_index + 1,
                )
        }
    }
}

fn segment_subsumes(
    left: &FilesystemPathSegmentPattern,
    right: &FilesystemPathSegmentPattern,
) -> bool {
    match (left, right) {
        (FilesystemPathSegmentPattern::Star, FilesystemPathSegmentPattern::Literal(_)) => true,
        (FilesystemPathSegmentPattern::Star, FilesystemPathSegmentPattern::Star) => true,
        (
            FilesystemPathSegmentPattern::Literal(left),
            FilesystemPathSegmentPattern::Literal(right),
        ) => left == right,
        _ => false,
    }
}

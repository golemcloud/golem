use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::ComponentOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentInitialFilesResourcePattern {
    Path(EnvironmentInitialFilesPathPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentInitialFilesPathPattern {
    pub segments: Vec<EnvironmentInitialFilesPathSegmentPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentInitialFilesPathSegmentPattern {
    Literal(String),
    Star,
    GlobStar,
}

impl EnvironmentInitialFilesPathPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        let Some(path) = value.strip_prefix('/') else {
            return Err(value.to_string());
        };
        let segments = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/')
                .map(parse_environment_initial_files_path_segment)
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Self { segments })
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        environment_initial_files_path_segments_subsume(&self.segments, &other.segments)
    }
}

impl EnvironmentInitialFilesResourcePattern {
    pub fn any() -> Self {
        Self::Path(EnvironmentInitialFilesPathPattern {
            segments: vec![EnvironmentInitialFilesPathSegmentPattern::GlobStar],
        })
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Path(
            EnvironmentInitialFilesPathPattern::parse(&value.into()).expect("invalid IFS path"),
        )
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::exact(value)
    }
}

impl ResourcePattern for EnvironmentInitialFilesResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        EnvironmentInitialFilesPathPattern::parse(resource)
            .map(EnvironmentInitialFilesResourcePattern::Path)
            .map_err(|_| CardParseError::InvalidResource {
                class: EnvironmentInitialFilesClass::NAME.to_string(),
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
pub enum EnvironmentInitialFilesVerb {
    View,
    Update,
    Delete,
    List,
}
impl VerbPattern for EnvironmentInitialFilesVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "list" => Some(Self::List),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentInitialFilesClass;

impl PermissionClass for EnvironmentInitialFilesClass {
    type Verb = EnvironmentInitialFilesVerb;
    type Owner = ComponentOwnerPattern;
    type Resource = EnvironmentInitialFilesResourcePattern;
    const NAME: &'static str = "environment.initial-files";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentInitialFiles(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentInitialFiles(pattern)
    }
}


fn parse_environment_initial_files_path_segment(
    value: &str,
) -> Result<EnvironmentInitialFilesPathSegmentPattern, String> {
    if value.is_empty() {
        Err(value.to_string())
    } else if value == "*" {
        Ok(EnvironmentInitialFilesPathSegmentPattern::Star)
    } else if value == "**" {
        Ok(EnvironmentInitialFilesPathSegmentPattern::GlobStar)
    } else if value.contains('*') || value.contains('/') {
        Err(value.to_string())
    } else {
        Ok(EnvironmentInitialFilesPathSegmentPattern::Literal(
            value.to_string(),
        ))
    }
}

fn environment_initial_files_path_segments_subsume(
    left: &[EnvironmentInitialFilesPathSegmentPattern],
    right: &[EnvironmentInitialFilesPathSegmentPattern],
) -> bool {
    if left.first().is_some_and(|segment| {
        matches!(segment, EnvironmentInitialFilesPathSegmentPattern::GlobStar)
    }) {
        return true;
    }
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .all(|(left, right)| match (left, right) {
            (EnvironmentInitialFilesPathSegmentPattern::GlobStar, _) => true,
            (
                EnvironmentInitialFilesPathSegmentPattern::Star,
                EnvironmentInitialFilesPathSegmentPattern::Literal(_),
            ) => true,
            (
                EnvironmentInitialFilesPathSegmentPattern::Star,
                EnvironmentInitialFilesPathSegmentPattern::Star,
            ) => true,
            (
                EnvironmentInitialFilesPathSegmentPattern::Literal(left),
                EnvironmentInitialFilesPathSegmentPattern::Literal(right),
            ) => left == right,
            _ => false,
        })
}

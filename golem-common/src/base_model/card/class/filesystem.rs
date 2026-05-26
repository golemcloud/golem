use super::*;
use crate::base_model::card::parsing::CardParseError;

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

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Path(FilesystemPathPattern::parse(&value.into()).expect("invalid filesystem path"))
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::exact(value)
    }
}

impl Subsumes for FilesystemResourcePattern {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct FilesystemClass;

impl PermissionClass for FilesystemClass {
    type Verb = FilesystemVerb;
    type Owner = AgentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = FilesystemResourcePattern;
    const NAME: &'static str = "filesystem";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "read" => Some(Self::Verb::Read),
            "write" => Some(Self::Verb::Write),
            "list" => Some(Self::Verb::List),
            "stat" => Some(Self::Verb::Stat),
            "delete" => Some(Self::Verb::Delete),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Filesystem(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Filesystem(pattern)
    }
}

pub type FilesystemPermissionPattern = ClassPermissionPattern<FilesystemClass>;
pub type PolymorphicFilesystemPermissionPattern =
    PolymorphicClassPermissionPattern<FilesystemClass>;

impl FilesystemClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<FilesystemResourcePattern, CardParseError> {
        FilesystemPathPattern::parse(resource)
            .map(FilesystemResourcePattern::Path)
            .map_err(|_| CardParseError::InvalidResource {
                class: FilesystemClass::NAME.to_string(),
                resource: resource.to_string(),
            })
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
    if left
        .first()
        .is_some_and(|segment| matches!(segment, FilesystemPathSegmentPattern::GlobStar))
    {
        return true;
    }
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .all(|(left, right)| match (left, right) {
            (FilesystemPathSegmentPattern::GlobStar, _) => true,
            (FilesystemPathSegmentPattern::Star, FilesystemPathSegmentPattern::Literal(_)) => true,
            (FilesystemPathSegmentPattern::Star, FilesystemPathSegmentPattern::Star) => true,
            (
                FilesystemPathSegmentPattern::Literal(left),
                FilesystemPathSegmentPattern::Literal(right),
            ) => left == right,
            _ => false,
        })
}

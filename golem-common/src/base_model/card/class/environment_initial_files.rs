use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_component_owner, parse_environment_recipient,
    parse_polymorphic_component_owner, parse_polymorphic_environment_recipient,
};

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

impl Subsumes for EnvironmentInitialFilesResourcePattern {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentInitialFilesClass;

impl PermissionClass for EnvironmentInitialFilesClass {
    type Verb = EnvironmentInitialFilesVerb;
    type Owner = ComponentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentInitialFilesResourcePattern;
    const NAME: &'static str = "environment.initial-files";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            "list" => Some(Self::Verb::List),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_component_owner(Self::NAME, owner)
    }

    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError> {
        parse_environment_recipient(recipient)
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn parse_polymorphic_owner(
        owner: &str,
    ) -> Result<<Self::Owner as OwnerPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_component_owner(Self::NAME, owner)
    }

    fn parse_polymorphic_recipient(
        recipient: &str,
    ) -> Result<<Self::Recipient as RecipientPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_environment_recipient(recipient)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentInitialFiles(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentInitialFiles(pattern)
    }
}

pub type EnvironmentInitialFilesPermissionPattern =
    ClassPermissionPattern<EnvironmentInitialFilesClass>;
pub type PolymorphicEnvironmentInitialFilesPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentInitialFilesClass>;

impl EnvironmentInitialFilesClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentInitialFilesResourcePattern, CardParseError> {
        EnvironmentInitialFilesPathPattern::parse(resource)
            .map(EnvironmentInitialFilesResourcePattern::Path)
            .map_err(|_| CardParseError::InvalidResource {
                class: EnvironmentInitialFilesClass::NAME.to_string(),
                resource: resource.to_string(),
            })
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

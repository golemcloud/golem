use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_owner, parse_agent_recipient, parse_polymorphic_agent_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum FilesystemResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl FilesystemResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::Glob(value.into())
    }
}

impl Subsumes for FilesystemResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Glob(a), Self::Glob(b)) => glob_subsumes(a, b),
            (Self::Glob(a), Self::Exact(b)) => glob_matches(a, b),
            (Self::Glob(_), Self::Any) => false,
            (Self::Exact(_), Self::Any | Self::Glob(_)) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicFilesystemResourcePattern {
    Any,
    Exact(String),
    Glob(String),
    Slot(SlotVariable),
    Template(ResourceTemplate),
}

impl From<FilesystemResourcePattern> for PolymorphicFilesystemResourcePattern {
    fn from(value: FilesystemResourcePattern) -> Self {
        match value {
            FilesystemResourcePattern::Any => Self::Any,
            FilesystemResourcePattern::Exact(value) => Self::Exact(value),
            FilesystemResourcePattern::Glob(value) => Self::Glob(value),
        }
    }
}

impl ResourcePattern for FilesystemResourcePattern {
    type Polymorphic = PolymorphicFilesystemResourcePattern;
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

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_agent_owner(Self::NAME, owner)
    }

    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError> {
        parse_agent_recipient(recipient)
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn parse_polymorphic_owner(
        owner: &str,
    ) -> Result<<Self::Owner as OwnerPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_agent_owner(Self::NAME, owner)
    }

    fn parse_polymorphic_recipient(
        recipient: &str,
    ) -> Result<<Self::Recipient as RecipientPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_agent_recipient(recipient)
    }

    fn parse_polymorphic_resource(
        resource: &str,
    ) -> Result<<Self::Resource as ResourcePattern>::Polymorphic, CardParseError> {
        Self::parse_polymorphic_resource(Self::NAME, resource)
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
        if resource == "*" || resource == "**" {
            Ok(FilesystemResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(FilesystemResourcePattern::Glob(resource.to_string()))
        } else {
            Ok(FilesystemResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicFilesystemResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicFilesystemResourcePattern::from,
            PolymorphicFilesystemResourcePattern::Slot,
            PolymorphicFilesystemResourcePattern::Template,
        )
    }
}

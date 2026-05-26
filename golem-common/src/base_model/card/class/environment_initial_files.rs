use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_component_owner, parse_environment_recipient,
    parse_polymorphic_component_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentInitialFilesResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl EnvironmentInitialFilesResourcePattern {
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

impl Subsumes for EnvironmentInitialFilesResourcePattern {
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
pub enum PolymorphicEnvironmentInitialFilesResourcePattern {
    Concrete(EnvironmentInitialFilesResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentInitialFilesResourcePattern {
    type Polymorphic = PolymorphicEnvironmentInitialFilesResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentInitialFilesVerb {
    View,
    Update,
    Delete,
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

    fn parse_polymorphic_resource(
        resource: &str,
    ) -> Result<<Self::Resource as ResourcePattern>::Polymorphic, CardParseError> {
        Self::parse_polymorphic_resource(Self::NAME, resource)
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
        if resource == "*" || resource == "**" {
            Ok(EnvironmentInitialFilesResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(EnvironmentInitialFilesResourcePattern::Glob(
                resource.to_string(),
            ))
        } else {
            Ok(EnvironmentInitialFilesResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentInitialFilesResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentInitialFilesResourcePattern::Concrete,
            PolymorphicEnvironmentInitialFilesResourcePattern::Slot,
            PolymorphicEnvironmentInitialFilesResourcePattern::Template,
        )
    }
}

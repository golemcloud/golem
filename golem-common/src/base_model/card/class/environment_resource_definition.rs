use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourceDefinitionResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentResourceDefinitionResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentResourceDefinitionResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Exact(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentResourceDefinitionResourcePattern {
    Concrete(EnvironmentResourceDefinitionResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentResourceDefinitionResourcePattern {
    type Polymorphic = PolymorphicEnvironmentResourceDefinitionResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourceDefinitionVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentResourceDefinitionClass;

impl PermissionClass for EnvironmentResourceDefinitionClass {
    type Verb = EnvironmentResourceDefinitionVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentResourceDefinitionResourcePattern;
    const NAME: &'static str = "environment.resource-definition";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_environment_owner(Self::NAME, owner)
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
        parse_polymorphic_environment_owner(Self::NAME, owner)
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
        PermissionPattern::EnvironmentResourceDefinition(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentResourceDefinition(pattern)
    }
}

pub type EnvironmentResourceDefinitionPermissionPattern =
    ClassPermissionPattern<EnvironmentResourceDefinitionClass>;
pub type PolymorphicEnvironmentResourceDefinitionPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentResourceDefinitionClass>;

impl EnvironmentResourceDefinitionClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentResourceDefinitionResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentResourceDefinitionResourcePattern::Any)
        } else {
            Ok(EnvironmentResourceDefinitionResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentResourceDefinitionResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentResourceDefinitionResourcePattern::Concrete,
            PolymorphicEnvironmentResourceDefinitionResourcePattern::Slot,
            PolymorphicEnvironmentResourceDefinitionResourcePattern::Template,
        )
    }
}

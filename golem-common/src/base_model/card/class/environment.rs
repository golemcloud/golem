use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentResourcePattern;

impl Subsumes for EnvironmentResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentResourcePattern {
    Concrete(EnvironmentResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentResourcePattern {
    type Polymorphic = PolymorphicEnvironmentResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
    Deploy,
    Rollback,
    ViewDeploymentPlan,
    WriteDeploymentRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentClass;

impl PermissionClass for EnvironmentClass {
    type Verb = EnvironmentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentResourcePattern;
    const NAME: &'static str = "environment";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            "restore" => Some(Self::Verb::Restore),
            "deploy" => Some(Self::Verb::Deploy),
            "rollback" => Some(Self::Verb::Rollback),
            "view-deployment-plan" => Some(Self::Verb::ViewDeploymentPlan),
            "write-deployment-record" => Some(Self::Verb::WriteDeploymentRecord),
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
        PermissionPattern::Environment(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Environment(pattern)
    }
}

pub type EnvironmentPermissionPattern = ClassPermissionPattern<EnvironmentClass>;
pub type PolymorphicEnvironmentPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentClass>;

impl EnvironmentClass {
    fn parse_resource(
        class: &str,
        resource: &str,
    ) -> Result<EnvironmentResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(EnvironmentResourcePattern)
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentResourcePattern, CardParseError> {
        if let Ok(resource) = Self::parse_resource(class, resource) {
            Ok(PolymorphicEnvironmentResourcePattern::Concrete(resource))
        } else if let Ok(slot) = SlotVariable::parse(resource) {
            Ok(PolymorphicEnvironmentResourcePattern::Slot(slot))
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}

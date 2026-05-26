use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_recipient, parse_empty_owner,
    parse_polymorphic_account_recipient, parse_polymorphic_empty_owner, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PlanResourcePattern {
    Any,
    Exact(String),
}

impl PlanResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for PlanResourcePattern {
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
pub enum PolymorphicPlanResourcePattern {
    Concrete(PlanResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for PlanResourcePattern {
    type Polymorphic = PolymorphicPlanResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PlanVerb {
    View,
    Create,
    Update,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PlanClass;

impl PermissionClass for PlanClass {
    type Verb = PlanVerb;
    type Owner = EmptyOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = PlanResourcePattern;
    const NAME: &'static str = "plan";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_empty_owner(Self::NAME, owner)
    }

    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError> {
        parse_account_recipient(recipient)
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn parse_polymorphic_owner(
        owner: &str,
    ) -> Result<<Self::Owner as OwnerPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_empty_owner(Self::NAME, owner)
    }

    fn parse_polymorphic_recipient(
        recipient: &str,
    ) -> Result<<Self::Recipient as RecipientPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_account_recipient(recipient)
    }

    fn parse_polymorphic_resource(
        resource: &str,
    ) -> Result<<Self::Resource as ResourcePattern>::Polymorphic, CardParseError> {
        Self::parse_polymorphic_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Plan(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Plan(pattern)
    }
}

pub type PlanPermissionPattern = ClassPermissionPattern<PlanClass>;
pub type PolymorphicPlanPermissionPattern = PolymorphicClassPermissionPattern<PlanClass>;

impl PlanClass {
    fn parse_resource(_class: &str, resource: &str) -> Result<PlanResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(PlanResourcePattern::Any)
        } else {
            Ok(PlanResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicPlanResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicPlanResourcePattern::Concrete,
            PolymorphicPlanResourcePattern::Slot,
            PolymorphicPlanResourcePattern::Template,
        )
    }
}

use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::EmptyOwnerPattern;
use crate::model::card::recipient::AccountRecipientPattern;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PlanResourcePattern {
    Any,
    Plan(PlanIdPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PlanIdPattern {
    Identifier(PlanIdentifier),
    Uuid(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct PlanIdentifier(pub String);

impl PlanIdentifier {
    fn parse(value: &str) -> Result<Self, String> {
        let mut chars = value.chars();
        if chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            Ok(Self(value.to_string()))
        } else {
            Err(value.to_string())
        }
    }
}

impl PlanResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        let value = value.into();
        Self::Plan(parse_plan_id(&value).expect("invalid plan id"))
    }
}

impl ResourcePattern for PlanResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(PlanResourcePattern::Any)
        } else {
            parse_plan_id(resource)
                .map(PlanResourcePattern::Plan)
                .map_err(|_| CardParseError::InvalidResource {
                    class: PlanClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Plan(a), Self::Plan(b)) => a == b,
            (Self::Plan(_), Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PlanVerb {
    View,
    Create,
    Update,
}
impl VerbPattern for PlanVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            _ => None,
        }
    }
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

fn parse_plan_id(value: &str) -> Result<PlanIdPattern, String> {
    if let Ok(uuid) = Uuid::parse_str(value) {
        Ok(PlanIdPattern::Uuid(uuid))
    } else {
        PlanIdentifier::parse(value).map(PlanIdPattern::Identifier)
    }
}

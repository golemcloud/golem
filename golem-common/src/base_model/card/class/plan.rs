use super::*;
use crate::base_model::card::parsing::CardParseError;
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

impl Subsumes for PlanResourcePattern {
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

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
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
            parse_plan_id(resource)
                .map(PlanResourcePattern::Plan)
                .map_err(|_| CardParseError::InvalidResource {
                    class: PlanClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }
}

fn parse_plan_id(value: &str) -> Result<PlanIdPattern, String> {
    if let Ok(uuid) = Uuid::parse_str(value) {
        Ok(PlanIdPattern::Uuid(uuid))
    } else {
        PlanIdentifier::parse(value).map(PlanIdPattern::Identifier)
    }
}

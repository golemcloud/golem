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
}

pub type PlanPermissionPattern = ClassPermissionPattern<PlanClass>;
pub type PolymorphicPlanPermissionPattern = PolymorphicClassPermissionPattern<PlanClass>;

impl PlanClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_empty_owner(Self::NAME, owner)?;
        let recipient = parse_account_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Plan(match verb {
            "*" => PlanPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => PlanPermissionPattern::Verb {
                verb: PlanVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => PlanPermissionPattern::Verb {
                verb: PlanVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => PlanPermissionPattern::Verb {
                verb: PlanVerb::Update,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

    pub(crate) fn parse_polymorphic_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PolymorphicPermissionPattern, CardParseError> {
        let owner = parse_polymorphic_empty_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_account_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Plan(match verb {
            "*" => PolymorphicPlanPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => PolymorphicPlanPermissionPattern::Verb {
                verb: PlanVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => PolymorphicPlanPermissionPattern::Verb {
                verb: PlanVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => PolymorphicPlanPermissionPattern::Verb {
                verb: PlanVerb::Update,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

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

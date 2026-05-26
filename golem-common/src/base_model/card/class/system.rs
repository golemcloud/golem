use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_recipient, parse_empty_owner,
    parse_polymorphic_account_recipient, parse_polymorphic_empty_owner,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SystemResourcePattern;

impl Subsumes for SystemResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicSystemResourcePattern {
    Concrete(SystemResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for SystemResourcePattern {
    type Polymorphic = PolymorphicSystemResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum SystemVerb {
    CreateAccount,
    ViewDefaultPlan,
    ViewAccountSummariesReport,
    ViewAccountCountsReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SystemClass;

impl PermissionClass for SystemClass {
    type Verb = SystemVerb;
    type Owner = EmptyOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = SystemResourcePattern;
    const NAME: &'static str = "system";
}

pub type SystemPermissionPattern = ClassPermissionPattern<SystemClass>;
pub type PolymorphicSystemPermissionPattern = PolymorphicClassPermissionPattern<SystemClass>;

impl SystemClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_empty_owner(Self::NAME, owner)?;
        let recipient = parse_account_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::System(match verb {
            "*" => SystemPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "create-account" => SystemPermissionPattern::Verb {
                verb: SystemVerb::CreateAccount,
                owner,
                recipient,
                resource,
            },
            "view-default-plan" => SystemPermissionPattern::Verb {
                verb: SystemVerb::ViewDefaultPlan,
                owner,
                recipient,
                resource,
            },
            "view-account-summaries-report" => SystemPermissionPattern::Verb {
                verb: SystemVerb::ViewAccountSummariesReport,
                owner,
                recipient,
                resource,
            },
            "view-account-counts-report" => SystemPermissionPattern::Verb {
                verb: SystemVerb::ViewAccountCountsReport,
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
        Ok(PolymorphicPermissionPattern::System(match verb {
            "*" => PolymorphicSystemPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "create-account" => PolymorphicSystemPermissionPattern::Verb {
                verb: SystemVerb::CreateAccount,
                owner,
                recipient,
                resource,
            },
            "view-default-plan" => PolymorphicSystemPermissionPattern::Verb {
                verb: SystemVerb::ViewDefaultPlan,
                owner,
                recipient,
                resource,
            },
            "view-account-summaries-report" => PolymorphicSystemPermissionPattern::Verb {
                verb: SystemVerb::ViewAccountSummariesReport,
                owner,
                recipient,
                resource,
            },
            "view-account-counts-report" => PolymorphicSystemPermissionPattern::Verb {
                verb: SystemVerb::ViewAccountCountsReport,
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

    fn parse_resource(
        class: &str,
        resource: &str,
    ) -> Result<SystemResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(SystemResourcePattern)
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
    ) -> Result<PolymorphicSystemResourcePattern, CardParseError> {
        if let Ok(resource) = Self::parse_resource(class, resource) {
            Ok(PolymorphicSystemResourcePattern::Concrete(resource))
        } else if let Ok(slot) = SlotVariable::parse(resource) {
            Ok(PolymorphicSystemResourcePattern::Slot(slot))
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}

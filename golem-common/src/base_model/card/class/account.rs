use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_owner, parse_account_recipient, parse_polymorphic_account_owner,
    parse_polymorphic_account_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountResourcePattern;

impl Subsumes for AccountResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAccountResourcePattern {
    Concrete(AccountResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for AccountResourcePattern {
    type Polymorphic = PolymorphicAccountResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountVerb {
    View,
    Update,
    Delete,
    SetRoles,
    SetPlan,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountClass;

impl PermissionClass for AccountClass {
    type Verb = AccountVerb;
    type Owner = AccountOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = AccountResourcePattern;
    const NAME: &'static str = "account";
}

pub type AccountPermissionPattern = ClassPermissionPattern<AccountClass>;
pub type PolymorphicAccountPermissionPattern = PolymorphicClassPermissionPattern<AccountClass>;

impl AccountClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_account_owner(Self::NAME, owner)?;
        let recipient = parse_account_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Account(match verb {
            "*" => AccountPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => AccountPermissionPattern::Verb {
                verb: AccountVerb::View,
                owner,
                recipient,
                resource,
            },
            "update" => AccountPermissionPattern::Verb {
                verb: AccountVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => AccountPermissionPattern::Verb {
                verb: AccountVerb::Delete,
                owner,
                recipient,
                resource,
            },
            "set-roles" => AccountPermissionPattern::Verb {
                verb: AccountVerb::SetRoles,
                owner,
                recipient,
                resource,
            },
            "set-plan" => AccountPermissionPattern::Verb {
                verb: AccountVerb::SetPlan,
                owner,
                recipient,
                resource,
            },
            "restore" => AccountPermissionPattern::Verb {
                verb: AccountVerb::Restore,
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
        let owner = parse_polymorphic_account_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_account_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Account(match verb {
            "*" => PolymorphicAccountPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => PolymorphicAccountPermissionPattern::Verb {
                verb: AccountVerb::View,
                owner,
                recipient,
                resource,
            },
            "update" => PolymorphicAccountPermissionPattern::Verb {
                verb: AccountVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicAccountPermissionPattern::Verb {
                verb: AccountVerb::Delete,
                owner,
                recipient,
                resource,
            },
            "set-roles" => PolymorphicAccountPermissionPattern::Verb {
                verb: AccountVerb::SetRoles,
                owner,
                recipient,
                resource,
            },
            "set-plan" => PolymorphicAccountPermissionPattern::Verb {
                verb: AccountVerb::SetPlan,
                owner,
                recipient,
                resource,
            },
            "restore" => PolymorphicAccountPermissionPattern::Verb {
                verb: AccountVerb::Restore,
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
    ) -> Result<AccountResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(AccountResourcePattern)
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
    ) -> Result<PolymorphicAccountResourcePattern, CardParseError> {
        if let Ok(resource) = Self::parse_resource(class, resource) {
            Ok(PolymorphicAccountResourcePattern::Concrete(resource))
        } else if let Ok(slot) = SlotVariable::parse(resource) {
            Ok(PolymorphicAccountResourcePattern::Slot(slot))
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}

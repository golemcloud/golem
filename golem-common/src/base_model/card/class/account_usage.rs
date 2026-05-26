use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_owner, parse_account_recipient, parse_polymorphic_account_owner,
    parse_polymorphic_account_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountUsageResourcePattern;

impl Subsumes for AccountUsageResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAccountUsageResourcePattern {
    Concrete(AccountUsageResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for AccountUsageResourcePattern {
    type Polymorphic = PolymorphicAccountUsageResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountUsageVerb {
    View,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountUsageClass;

impl PermissionClass for AccountUsageClass {
    type Verb = AccountUsageVerb;
    type Owner = AccountOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = AccountUsageResourcePattern;
    const NAME: &'static str = "account.usage";
}

pub type AccountUsagePermissionPattern = ClassPermissionPattern<AccountUsageClass>;
pub type PolymorphicAccountUsagePermissionPattern =
    PolymorphicClassPermissionPattern<AccountUsageClass>;

impl AccountUsageClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_account_owner(Self::NAME, owner)?;
        let recipient = parse_account_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::AccountUsage(match verb {
            "*" => AccountUsagePermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => AccountUsagePermissionPattern::Verb {
                verb: AccountUsageVerb::View,
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
        Ok(PolymorphicPermissionPattern::AccountUsage(match verb {
            "*" => PolymorphicAccountUsagePermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => PolymorphicAccountUsagePermissionPattern::Verb {
                verb: AccountUsageVerb::View,
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
    ) -> Result<AccountUsageResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(AccountUsageResourcePattern)
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
    ) -> Result<PolymorphicAccountUsageResourcePattern, CardParseError> {
        if let Ok(resource) = Self::parse_resource(class, resource) {
            Ok(PolymorphicAccountUsageResourcePattern::Concrete(resource))
        } else if let Ok(slot) = SlotVariable::parse(resource) {
            Ok(PolymorphicAccountUsageResourcePattern::Slot(slot))
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}

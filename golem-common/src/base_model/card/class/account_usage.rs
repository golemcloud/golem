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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_account_owner(Self::NAME, owner)
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
        parse_polymorphic_account_owner(Self::NAME, owner)
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
        PermissionPattern::AccountUsage(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::AccountUsage(pattern)
    }
}

pub type AccountUsagePermissionPattern = ClassPermissionPattern<AccountUsageClass>;
pub type PolymorphicAccountUsagePermissionPattern =
    PolymorphicClassPermissionPattern<AccountUsageClass>;

impl AccountUsageClass {
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

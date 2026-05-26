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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            "set-roles" => Some(Self::Verb::SetRoles),
            "set-plan" => Some(Self::Verb::SetPlan),
            "restore" => Some(Self::Verb::Restore),
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
        PermissionPattern::Account(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Account(pattern)
    }
}

pub type AccountPermissionPattern = ClassPermissionPattern<AccountClass>;
pub type PolymorphicAccountPermissionPattern = PolymorphicClassPermissionPattern<AccountClass>;

impl AccountClass {
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

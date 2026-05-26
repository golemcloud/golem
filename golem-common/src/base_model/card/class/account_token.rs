use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_owner, parse_account_recipient, parse_polymorphic_account_owner,
    parse_polymorphic_account_recipient, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountTokenResourcePattern {
    Any,
    Exact(String),
}

impl AccountTokenResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for AccountTokenResourcePattern {
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
pub enum PolymorphicAccountTokenResourcePattern {
    Concrete(AccountTokenResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for AccountTokenResourcePattern {
    type Polymorphic = PolymorphicAccountTokenResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountTokenVerb {
    Create,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountTokenClass;

impl PermissionClass for AccountTokenClass {
    type Verb = AccountTokenVerb;
    type Owner = AccountOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = AccountTokenResourcePattern;
    const NAME: &'static str = "account.token";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "create" => Some(Self::Verb::Create),
            "delete" => Some(Self::Verb::Delete),
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
        PermissionPattern::AccountToken(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::AccountToken(pattern)
    }
}

pub type AccountTokenPermissionPattern = ClassPermissionPattern<AccountTokenClass>;
pub type PolymorphicAccountTokenPermissionPattern =
    PolymorphicClassPermissionPattern<AccountTokenClass>;

impl AccountTokenClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<AccountTokenResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(AccountTokenResourcePattern::Any)
        } else {
            Ok(AccountTokenResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicAccountTokenResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicAccountTokenResourcePattern::Concrete,
            PolymorphicAccountTokenResourcePattern::Slot,
            PolymorphicAccountTokenResourcePattern::Template,
        )
    }
}

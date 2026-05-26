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
}

pub type AccountTokenPermissionPattern = ClassPermissionPattern<AccountTokenClass>;
pub type PolymorphicAccountTokenPermissionPattern =
    PolymorphicClassPermissionPattern<AccountTokenClass>;

impl AccountTokenClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_account_owner(Self::NAME, owner)?;
        let recipient = parse_account_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::AccountToken(match verb {
            "*" => AccountTokenPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "create" => AccountTokenPermissionPattern::Verb {
                verb: AccountTokenVerb::Create,
                owner,
                recipient,
                resource,
            },
            "delete" => AccountTokenPermissionPattern::Verb {
                verb: AccountTokenVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::AccountToken(match verb {
            "*" => PolymorphicAccountTokenPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "create" => PolymorphicAccountTokenPermissionPattern::Verb {
                verb: AccountTokenVerb::Create,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicAccountTokenPermissionPattern::Verb {
                verb: AccountTokenVerb::Delete,
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

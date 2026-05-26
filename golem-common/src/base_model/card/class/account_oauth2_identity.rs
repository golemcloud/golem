use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_owner, parse_account_recipient, parse_polymorphic_account_owner,
    parse_polymorphic_account_recipient, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOauth2IdentityResourcePattern {
    Any,
    Exact(String),
}

impl AccountOauth2IdentityResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for AccountOauth2IdentityResourcePattern {
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
pub enum PolymorphicAccountOauth2IdentityResourcePattern {
    Concrete(AccountOauth2IdentityResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for AccountOauth2IdentityResourcePattern {
    type Polymorphic = PolymorphicAccountOauth2IdentityResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOauth2IdentityVerb {
    View,
    Link,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountOauth2IdentityClass;

impl PermissionClass for AccountOauth2IdentityClass {
    type Verb = AccountOauth2IdentityVerb;
    type Owner = AccountOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = AccountOauth2IdentityResourcePattern;
    const NAME: &'static str = "account.oauth2-identity";
}

pub type AccountOauth2IdentityPermissionPattern =
    ClassPermissionPattern<AccountOauth2IdentityClass>;
pub type PolymorphicAccountOauth2IdentityPermissionPattern =
    PolymorphicClassPermissionPattern<AccountOauth2IdentityClass>;

impl AccountOauth2IdentityClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_account_owner(Self::NAME, owner)?;
        let recipient = parse_account_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::AccountOauth2Identity(match verb {
            "*" => AccountOauth2IdentityPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => AccountOauth2IdentityPermissionPattern::Verb {
                verb: AccountOauth2IdentityVerb::View,
                owner,
                recipient,
                resource,
            },
            "link" => AccountOauth2IdentityPermissionPattern::Verb {
                verb: AccountOauth2IdentityVerb::Link,
                owner,
                recipient,
                resource,
            },
            "delete" => AccountOauth2IdentityPermissionPattern::Verb {
                verb: AccountOauth2IdentityVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::AccountOauth2Identity(
            match verb {
                "*" => PolymorphicAccountOauth2IdentityPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicAccountOauth2IdentityPermissionPattern::Verb {
                    verb: AccountOauth2IdentityVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "link" => PolymorphicAccountOauth2IdentityPermissionPattern::Verb {
                    verb: AccountOauth2IdentityVerb::Link,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicAccountOauth2IdentityPermissionPattern::Verb {
                    verb: AccountOauth2IdentityVerb::Delete,
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
            },
        ))
    }

    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<AccountOauth2IdentityResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(AccountOauth2IdentityResourcePattern::Any)
        } else {
            Ok(AccountOauth2IdentityResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicAccountOauth2IdentityResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicAccountOauth2IdentityResourcePattern::Concrete,
            PolymorphicAccountOauth2IdentityResourcePattern::Slot,
            PolymorphicAccountOauth2IdentityResourcePattern::Template,
        )
    }
}

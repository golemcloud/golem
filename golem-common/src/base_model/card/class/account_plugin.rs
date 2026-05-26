use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_owner, parse_account_recipient, parse_polymorphic_account_owner,
    parse_polymorphic_account_recipient, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountPluginResourcePattern {
    Any,
    Exact(String),
}

impl AccountPluginResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for AccountPluginResourcePattern {
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
pub enum PolymorphicAccountPluginResourcePattern {
    Concrete(AccountPluginResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for AccountPluginResourcePattern {
    type Polymorphic = PolymorphicAccountPluginResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountPluginVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountPluginClass;

impl PermissionClass for AccountPluginClass {
    type Verb = AccountPluginVerb;
    type Owner = AccountOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = AccountPluginResourcePattern;
    const NAME: &'static str = "account.plugin";
}

pub type AccountPluginPermissionPattern = ClassPermissionPattern<AccountPluginClass>;
pub type PolymorphicAccountPluginPermissionPattern =
    PolymorphicClassPermissionPattern<AccountPluginClass>;

impl AccountPluginClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_account_owner(Self::NAME, owner)?;
        let recipient = parse_account_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::AccountPlugin(match verb {
            "*" => AccountPluginPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => AccountPluginPermissionPattern::Verb {
                verb: AccountPluginVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => AccountPluginPermissionPattern::Verb {
                verb: AccountPluginVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => AccountPluginPermissionPattern::Verb {
                verb: AccountPluginVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => AccountPluginPermissionPattern::Verb {
                verb: AccountPluginVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::AccountPlugin(match verb {
            "*" => PolymorphicAccountPluginPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => PolymorphicAccountPluginPermissionPattern::Verb {
                verb: AccountPluginVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => PolymorphicAccountPluginPermissionPattern::Verb {
                verb: AccountPluginVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => PolymorphicAccountPluginPermissionPattern::Verb {
                verb: AccountPluginVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicAccountPluginPermissionPattern::Verb {
                verb: AccountPluginVerb::Delete,
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
    ) -> Result<AccountPluginResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(AccountPluginResourcePattern::Any)
        } else {
            Ok(AccountPluginResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicAccountPluginResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicAccountPluginResourcePattern::Concrete,
            PolymorphicAccountPluginResourcePattern::Slot,
            PolymorphicAccountPluginResourcePattern::Template,
        )
    }
}

use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_owner, parse_account_recipient, parse_polymorphic_account_owner,
    parse_polymorphic_account_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountPluginResourcePattern {
    Any,
    Name(AccountPluginName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AccountPluginName(pub String);

impl AccountPluginName {
    fn parse(value: &str) -> Result<Self, String> {
        parse_account_plugin_identifier(value).map(Self)
    }
}

impl AccountPluginResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Name(AccountPluginName::parse(&value.into()).expect("invalid plugin name"))
    }
}

impl Subsumes for AccountPluginResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Name(a), Self::Name(b)) => a == b,
            (Self::Name(_), Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountPluginVerb {
    View,
    Register,
    Delete,
    Restore,
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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "register" => Some(Self::Verb::Register),
            "delete" => Some(Self::Verb::Delete),
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

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::AccountPlugin(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::AccountPlugin(pattern)
    }
}

pub type AccountPluginPermissionPattern = ClassPermissionPattern<AccountPluginClass>;
pub type PolymorphicAccountPluginPermissionPattern =
    PolymorphicClassPermissionPattern<AccountPluginClass>;

impl AccountPluginClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<AccountPluginResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(AccountPluginResourcePattern::Any)
        } else {
            AccountPluginName::parse(resource)
                .map(AccountPluginResourcePattern::Name)
                .map_err(|_| CardParseError::InvalidResource {
                    class: AccountPluginClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }
}

fn parse_account_plugin_identifier(value: &str) -> Result<String, String> {
    let mut chars = value.chars();
    if chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(value.to_string())
    } else {
        Err(value.to_string())
    }
}

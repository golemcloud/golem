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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
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

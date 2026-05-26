use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_owner, parse_account_recipient, parse_polymorphic_account_owner,
    parse_polymorphic_account_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOauth2IdentityResourcePattern {
    Any,
    Identity { provider: String, external_id: String },
}

impl AccountOauth2IdentityResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        let value = value.into();
        let (provider, external_id) = value
            .split_once('/')
            .expect("oauth2 identity must be provider/external-id");
        Self::Identity {
            provider: provider.to_string(),
            external_id: external_id.to_string(),
        }
    }
}

impl Subsumes for AccountOauth2IdentityResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (
                Self::Identity {
                    provider: a_provider,
                    external_id: a_external_id,
                },
                Self::Identity {
                    provider: b_provider,
                    external_id: b_external_id,
                },
            ) => a_provider == b_provider && a_external_id == b_external_id,
            (Self::Identity { .. }, Self::Any) => false,
        }
    }
}

pub type PolymorphicAccountOauth2IdentityResourcePattern = AccountOauth2IdentityResourcePattern;

impl ResourcePattern for AccountOauth2IdentityResourcePattern {
    type Polymorphic = PolymorphicAccountOauth2IdentityResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOauth2IdentityVerb {
    View,
    Link,
    Unlink,
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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "link" => Some(Self::Verb::Link),
            "unlink" => Some(Self::Verb::Unlink),
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
        PermissionPattern::AccountOauth2Identity(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::AccountOauth2Identity(pattern)
    }
}

pub type AccountOauth2IdentityPermissionPattern =
    ClassPermissionPattern<AccountOauth2IdentityClass>;
pub type PolymorphicAccountOauth2IdentityPermissionPattern =
    PolymorphicClassPermissionPattern<AccountOauth2IdentityClass>;

impl AccountOauth2IdentityClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<AccountOauth2IdentityResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(AccountOauth2IdentityResourcePattern::Any)
        } else if let Some((provider, external_id)) = resource.split_once('/') {
            if provider.is_empty() || external_id.is_empty() {
                Err(CardParseError::InvalidResource {
                    class: AccountOauth2IdentityClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
            } else {
                Ok(AccountOauth2IdentityResourcePattern::Identity {
                    provider: provider.to_string(),
                    external_id: external_id.to_string(),
                })
            }
        } else {
            Err(CardParseError::InvalidResource {
                class: AccountOauth2IdentityClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicAccountOauth2IdentityResourcePattern, CardParseError> {
        Self::parse_resource(class, resource)
    }
}

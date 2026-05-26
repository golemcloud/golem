use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOauth2IdentityResourcePattern {
    Any,
    Identity {
        provider: String,
        external_id: String,
    },
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

impl ResourcePattern for AccountOauth2IdentityResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOauth2IdentityVerb {
    View,
    Link,
    Unlink,
}
impl VerbPattern for AccountOauth2IdentityVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "link" => Some(Self::Link),
            "unlink" => Some(Self::Unlink),
            _ => None,
        }
    }
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

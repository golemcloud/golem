use super::*;
use crate::base_model::card::parsing::CardParseError;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountTokenResourcePattern {
    Any,
    Token(Uuid),
}

impl AccountTokenResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn token(value: Uuid) -> Self {
        Self::Token(value)
    }
}

impl Subsumes for AccountTokenResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Token(a), Self::Token(b)) => a == b,
            (Self::Token(_), Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountTokenVerb {
    View,
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
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "delete" => Some(Self::Verb::Delete),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
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
        class: &str,
        resource: &str,
    ) -> Result<AccountTokenResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(AccountTokenResourcePattern::Any)
        } else {
            Uuid::parse_str(resource)
                .map(AccountTokenResourcePattern::Token)
                .map_err(|_| CardParseError::InvalidResource {
                    class: class.to_string(),
                    resource: resource.to_string(),
                })
        }
    }
}

use super::*;

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

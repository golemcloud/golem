use super::*;

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

use super::*;

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

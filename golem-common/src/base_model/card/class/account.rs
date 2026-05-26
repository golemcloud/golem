use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountResourcePattern;

impl Subsumes for AccountResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAccountResourcePattern {
    Concrete(AccountResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for AccountResourcePattern {
    type Polymorphic = PolymorphicAccountResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountVerb {
    View,
    Update,
    Delete,
    SetRoles,
    SetPlan,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountClass;

impl PermissionClass for AccountClass {
    type Verb = AccountVerb;
    type Owner = AccountOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = AccountResourcePattern;
    const NAME: &'static str = "account";
}

pub type AccountPermissionPattern = ClassPermissionPattern<AccountClass>;
pub type PolymorphicAccountPermissionPattern = PolymorphicClassPermissionPattern<AccountClass>;

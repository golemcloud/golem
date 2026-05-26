use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountUsageResourcePattern;

impl Subsumes for AccountUsageResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAccountUsageResourcePattern {
    Concrete(AccountUsageResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for AccountUsageResourcePattern {
    type Polymorphic = PolymorphicAccountUsageResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountUsageVerb {
    View,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountUsageClass;

impl PermissionClass for AccountUsageClass {
    type Verb = AccountUsageVerb;
    type Owner = AccountOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = AccountUsageResourcePattern;
    const NAME: &'static str = "account.usage";
}

pub type AccountUsagePermissionPattern = ClassPermissionPattern<AccountUsageClass>;
pub type PolymorphicAccountUsagePermissionPattern =
    PolymorphicClassPermissionPattern<AccountUsageClass>;

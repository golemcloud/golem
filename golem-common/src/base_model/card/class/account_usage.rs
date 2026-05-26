use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::AccountOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountUsageResourcePattern;

impl ResourcePattern for AccountUsageResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource.is_empty() {
            Ok(AccountUsageResourcePattern)
        } else {
            Err(CardParseError::InvalidResource {
                class: AccountUsageClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountUsageVerb {
    View,
}
impl VerbPattern for AccountUsageVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountUsageClass;

impl PermissionClass for AccountUsageClass {
    type Verb = AccountUsageVerb;
    type Owner = AccountOwnerPattern;
    type Resource = AccountUsageResourcePattern;
    const NAME: &'static str = "account.usage";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::AccountUsage(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::AccountUsage(pattern)
    }
}

pub type AccountUsagePermissionPattern = ClassPermissionPattern<AccountUsageClass>;
pub type PolymorphicAccountUsagePermissionPattern =
    PolymorphicClassPermissionPattern<AccountUsageClass>;

use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::AccountOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountResourcePattern;

impl ResourcePattern for AccountResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource.is_empty() {
            Ok(AccountResourcePattern)
        } else {
            Err(CardParseError::InvalidResource {
                class: AccountClass::NAME.to_string(),
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
pub enum AccountVerb {
    View,
    Update,
    Delete,
    SetRoles,
    SetPlan,
}
impl VerbPattern for AccountVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "set-roles" => Some(Self::SetRoles),
            "set-plan" => Some(Self::SetPlan),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountClass;

impl PermissionClass for AccountClass {
    type Verb = AccountVerb;
    type Owner = AccountOwnerPattern;
    type Resource = AccountResourcePattern;
    const NAME: &'static str = "account";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Account(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Account(pattern)
    }
}

pub type AccountPermissionPattern = ClassPermissionPattern<AccountClass>;
pub type PolymorphicAccountPermissionPattern = PolymorphicClassPermissionPattern<AccountClass>;

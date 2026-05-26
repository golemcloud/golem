use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SystemResourcePattern;

impl ResourcePattern for SystemResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource.is_empty() {
            Ok(SystemResourcePattern)
        } else {
            Err(CardParseError::InvalidResource {
                class: SystemClass::NAME.to_string(),
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
pub enum SystemVerb {
    CreateAccount,
    ViewDefaultPlan,
    ViewAccountSummariesReport,
    ViewAccountCountsReport,
}
impl VerbPattern for SystemVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "create-account" => Some(Self::CreateAccount),
            "view-default-plan" => Some(Self::ViewDefaultPlan),
            "view-account-summaries-report" => Some(Self::ViewAccountSummariesReport),
            "view-account-counts-report" => Some(Self::ViewAccountCountsReport),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SystemClass;

impl PermissionClass for SystemClass {
    type Verb = SystemVerb;
    type Owner = EmptyOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = SystemResourcePattern;
    const NAME: &'static str = "system";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::System(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::System(pattern)
    }
}

pub type SystemPermissionPattern = ClassPermissionPattern<SystemClass>;
pub type PolymorphicSystemPermissionPattern = PolymorphicClassPermissionPattern<SystemClass>;

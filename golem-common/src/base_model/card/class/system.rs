use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SystemResourcePattern;

impl Subsumes for SystemResourcePattern {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SystemClass;

impl PermissionClass for SystemClass {
    type Verb = SystemVerb;
    type Owner = EmptyOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = SystemResourcePattern;
    const NAME: &'static str = "system";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "create-account" => Some(Self::Verb::CreateAccount),
            "view-default-plan" => Some(Self::Verb::ViewDefaultPlan),
            "view-account-summaries-report" => Some(Self::Verb::ViewAccountSummariesReport),
            "view-account-counts-report" => Some(Self::Verb::ViewAccountCountsReport),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

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

impl SystemClass {
    fn parse_resource(
        class: &str,
        resource: &str,
    ) -> Result<SystemResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(SystemResourcePattern)
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}

use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourcePattern {
    Empty,
    AnyRevision,
    Revision(u64),
}

impl Subsumes for EnvironmentResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Empty, Self::Empty) => true,
            (Self::AnyRevision, Self::AnyRevision | Self::Revision(_)) => true,
            (Self::Revision(a), Self::Revision(b)) => a == b,
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
    Deploy,
    Rollback,
    ViewDeployment,
    ViewDeploymentPlan,
    ViewAgentTypes,
    WriteDeploymentRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentClass;

impl PermissionClass for EnvironmentClass {
    type Verb = EnvironmentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentResourcePattern;
    const NAME: &'static str = "environment";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            "restore" => Some(Self::Verb::Restore),
            "deploy" => Some(Self::Verb::Deploy),
            "rollback" => Some(Self::Verb::Rollback),
            "view-deployment" => Some(Self::Verb::ViewDeployment),
            "view-deployment-plan" => Some(Self::Verb::ViewDeploymentPlan),
            "view-agent-types" => Some(Self::Verb::ViewAgentTypes),
            "write-deployment-record" => Some(Self::Verb::WriteDeploymentRecord),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Environment(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Environment(pattern)
    }
}

pub type EnvironmentPermissionPattern = ClassPermissionPattern<EnvironmentClass>;
pub type PolymorphicEnvironmentPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentClass>;

impl EnvironmentClass {
    fn parse_resource(
        class: &str,
        resource: &str,
    ) -> Result<EnvironmentResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(EnvironmentResourcePattern::Empty)
        } else if resource == "rev=*" {
            Ok(EnvironmentResourcePattern::AnyRevision)
        } else if let Some(revision) = resource.strip_prefix("rev=") {
            revision
                .parse::<u64>()
                .map(EnvironmentResourcePattern::Revision)
                .map_err(|_| CardParseError::InvalidResource {
                    class: class.to_string(),
                    resource: resource.to_string(),
                })
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}

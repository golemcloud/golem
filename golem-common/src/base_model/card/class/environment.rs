use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::ApplicationOwnerPattern;
use crate::model::card::recipient::EnvironmentRecipientPattern;
use nom::IResult;
use nom::branch::alt;
use nom::bytes::complete::{tag, take_while1};
use nom::combinator::{all_consuming, map, map_res};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourcePattern {
    Any,
    Environment(EnvironmentName),
    Revision {
        environment: EnvironmentName,
        revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct EnvironmentName(pub String);

impl ResourcePattern for EnvironmentResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentResourcePattern::Any)
        } else {
            parse_environment_resource(resource).map_err(|_| CardParseError::InvalidResource {
                class: EnvironmentClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Environment(a), Self::Environment(b)) => a == b,
            (Self::Environment(a), Self::Revision { environment: b, .. }) => a == b,
            (
                Self::Revision {
                    environment: a,
                    revision: ar,
                },
                Self::Revision {
                    environment: b,
                    revision: br,
                },
            ) => a == b && ar == br,
            (Self::Environment(_) | Self::Revision { .. }, Self::Any) => false,
            (Self::Revision { .. }, Self::Environment(_)) => false,
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
    Deploy,
    Rollback,
    ViewDeployment,
    ViewDeploymentPlan,
    ViewAgentTypes,
    WriteDeploymentRecord,
}
impl VerbPattern for EnvironmentVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "deploy" => Some(Self::Deploy),
            "rollback" => Some(Self::Rollback),
            "view-deployment" => Some(Self::ViewDeployment),
            "view-deployment-plan" => Some(Self::ViewDeploymentPlan),
            "view-agent-types" => Some(Self::ViewAgentTypes),
            "write-deployment-record" => Some(Self::WriteDeploymentRecord),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentClass;

impl PermissionClass for EnvironmentClass {
    type Verb = EnvironmentVerb;
    type Owner = ApplicationOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentResourcePattern;
    const NAME: &'static str = "environment";

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

fn parse_environment_resource(resource: &str) -> Result<EnvironmentResourcePattern, String> {
    all_consuming(environment_resource)(resource)
        .map(|(_, resource)| resource)
        .map_err(|_| resource.to_string())
}

fn environment_resource(input: &str) -> IResult<&str, EnvironmentResourcePattern> {
    let (input, environment) = environment_name(input)?;
    alt((
        map(
            map_res(
                nom::sequence::preceded(tag("@rev="), take_while1(|c: char| c.is_ascii_digit())),
                |revision: &str| revision.parse::<u64>(),
            ),
            |revision| EnvironmentResourcePattern::Revision {
                environment: environment.clone(),
                revision,
            },
        ),
        map(nom::combinator::success(()), |_| {
            EnvironmentResourcePattern::Environment(environment.clone())
        }),
    ))(input)
}

fn environment_name(input: &str) -> IResult<&str, EnvironmentName> {
    map(
        take_while1(|c: char| c != '@' && c != ':' && c != '/' && !c.is_whitespace()),
        |value: &str| EnvironmentName(value.to_string()),
    )(input)
}

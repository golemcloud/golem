use super::*;
use crate::base_model::card::parsing::CardParseError;
use nom::IResult;
use nom::bytes::complete::take_while1;
use nom::combinator::{all_consuming, map};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ApplicationResourcePattern {
    Empty,
    Application(ApplicationName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct ApplicationName(pub String);

impl Subsumes for ApplicationResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Empty, Self::Empty) => true,
            (Self::Application(a), Self::Application(b)) => a == b,
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ApplicationVerb {
    View,
    Create,
    Update,
    Delete,
    ListAllEnvironments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ApplicationClass;

impl PermissionClass for ApplicationClass {
    type Verb = ApplicationVerb;
    type Owner = AccountOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = ApplicationResourcePattern;
    const NAME: &'static str = "application";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            "list-all-environments" => Some(Self::Verb::ListAllEnvironments),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Application(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Application(pattern)
    }
}

pub type ApplicationPermissionPattern = ClassPermissionPattern<ApplicationClass>;
pub type PolymorphicApplicationPermissionPattern =
    PolymorphicClassPermissionPattern<ApplicationClass>;

impl ApplicationClass {
    fn parse_resource(
        class: &str,
        resource: &str,
    ) -> Result<ApplicationResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(ApplicationResourcePattern::Empty)
        } else {
            parse_application_resource(resource).map_err(|_| CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}

fn parse_application_resource(resource: &str) -> Result<ApplicationResourcePattern, String> {
    all_consuming(application_resource)(resource)
        .map(|(_, resource)| resource)
        .map_err(|_| resource.to_string())
}

fn application_resource(input: &str) -> IResult<&str, ApplicationResourcePattern> {
    map(application_name, ApplicationResourcePattern::Application)(input)
}

fn application_name(input: &str) -> IResult<&str, ApplicationName> {
    map(
        take_while1(|c: char| c != ':' && c != '/' && !c.is_whitespace()),
        |value: &str| ApplicationName(value.to_string()),
    )(input)
}

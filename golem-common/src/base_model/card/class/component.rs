use super::*;
use crate::base_model::card::parsing::CardParseError;
use nom::IResult;
use nom::branch::alt;
use nom::bytes::complete::{tag, take_while1};
use nom::combinator::{all_consuming, map, map_res};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ComponentResourcePattern {
    Empty,
    Component(ComponentName),
    Revision {
        component: ComponentName,
        revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct ComponentName(pub String);

impl Subsumes for ComponentResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Empty, Self::Empty) => true,
            (Self::Component(a), Self::Component(b)) => a == b,
            (Self::Component(a), Self::Revision { component: b, .. }) => a == b,
            (
                Self::Revision {
                    component: a,
                    revision: ar,
                },
                Self::Revision {
                    component: b,
                    revision: br,
                },
            ) => a == b && ar == br,
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ComponentVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ComponentClass;

impl PermissionClass for ComponentClass {
    type Verb = ComponentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = ComponentResourcePattern;
    const NAME: &'static str = "component";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Component(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Component(pattern)
    }
}

pub type ComponentPermissionPattern = ClassPermissionPattern<ComponentClass>;
pub type PolymorphicComponentPermissionPattern = PolymorphicClassPermissionPattern<ComponentClass>;

impl ComponentClass {
    fn parse_resource(
        class: &str,
        resource: &str,
    ) -> Result<ComponentResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(ComponentResourcePattern::Empty)
        } else {
            parse_component_resource(resource).map_err(|_| CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}

fn parse_component_resource(resource: &str) -> Result<ComponentResourcePattern, String> {
    all_consuming(component_resource)(resource)
        .map(|(_, resource)| resource)
        .map_err(|_| resource.to_string())
}

fn component_resource(input: &str) -> IResult<&str, ComponentResourcePattern> {
    let (input, component) = component_name(input)?;
    alt((
        map(
            map_res(
                nom::sequence::preceded(tag("@rev="), take_while1(|c: char| c.is_ascii_digit())),
                |revision: &str| revision.parse::<u64>(),
            ),
            |revision| ComponentResourcePattern::Revision {
                component: component.clone(),
                revision,
            },
        ),
        map(nom::combinator::success(()), |_| {
            ComponentResourcePattern::Component(component.clone())
        }),
    ))(input)
}

fn component_name(input: &str) -> IResult<&str, ComponentName> {
    map(
        take_while1(|c: char| c != '@' && c != ':' && c != '/' && !c.is_whitespace()),
        |value: &str| ComponentName(value.to_string()),
    )(input)
}

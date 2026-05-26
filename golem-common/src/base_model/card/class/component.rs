use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::EnvironmentOwnerPattern;
use nom::IResult;
use nom::branch::alt;
use nom::bytes::complete::{tag, take_while1};
use nom::combinator::{all_consuming, map, map_res};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ComponentResourcePattern {
    Any,
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

impl ResourcePattern for ComponentResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(ComponentResourcePattern::Any)
        } else {
            parse_component_resource(resource).map_err(|_| CardParseError::InvalidResource {
                class: ComponentClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
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
            (Self::Component(_) | Self::Revision { .. }, Self::Any) => false,
            (Self::Revision { .. }, Self::Component(_)) => false,
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
impl VerbPattern for ComponentVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ComponentClass;

impl PermissionClass for ComponentClass {
    type Verb = ComponentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = ComponentResourcePattern;
    const NAME: &'static str = "component";

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

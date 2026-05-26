use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::ToolOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolResourcePattern {
    AnyInvocation,
    Invocation(ToolInvocationPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ToolInvocationPattern {
    pub command_path: Option<Vec<ToolIdentifier>>,
    pub args: Vec<ToolArgPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct ToolIdentifier(pub String);

impl ToolIdentifier {
    fn parse(value: &str) -> Result<Self, String> {
        let mut chars = value.chars();
        if chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            Ok(Self(value.to_string()))
        } else {
            Err(value.to_string())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolArgPattern {
    ShortFlags {
        flags: Vec<char>,
        value: Option<ToolValuePattern>,
    },
    LongFlag {
        name: ToolIdentifier,
        value: Option<ToolValuePattern>,
    },
    Positional(ToolValuePattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolValuePattern {
    Literal(ToolValueLiteral),
    Star,
    GlobStar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct ToolValueLiteral(pub String);

impl ToolResourcePattern {
    pub fn any() -> Self {
        Self::AnyInvocation
    }

    pub fn command(command: impl Into<String>) -> Self {
        Self::Invocation(
            parse_tool_invocation_pattern(&command.into()).expect("invalid tool command"),
        )
    }
}

impl ResourcePattern for ToolResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(ToolResourcePattern::AnyInvocation)
        } else if resource.is_empty() {
            Err(CardParseError::InvalidResource {
                class: ToolClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        } else {
            parse_tool_invocation_pattern(resource)
                .map(ToolResourcePattern::Invocation)
                .map_err(|_| CardParseError::InvalidResource {
                    class: ToolClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnyInvocation, _) => true,
            (Self::Invocation(a), Self::Invocation(b)) => a == b,
            (Self::Invocation(_), Self::AnyInvocation) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolVerb {
    Invoke,
}
impl VerbPattern for ToolVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "invoke" => Some(Self::Invoke),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ToolClass;

impl PermissionClass for ToolClass {
    type Verb = ToolVerb;
    type Owner = ToolOwnerPattern;
    type Resource = ToolResourcePattern;
    const NAME: &'static str = "tool";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Tool(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Tool(pattern)
    }
}

pub type ToolPermissionPattern = ClassPermissionPattern<ToolClass>;
pub type PolymorphicToolPermissionPattern = PolymorphicClassPermissionPattern<ToolClass>;

fn parse_tool_invocation_pattern(value: &str) -> Result<ToolInvocationPattern, String> {
    let mut tokens = value.split_whitespace().peekable();
    let command_path = match tokens.peek().copied() {
        Some(first) if !first.starts_with('-') => {
            let command = tokens.next().unwrap();
            Some(
                command
                    .split('.')
                    .map(ToolIdentifier::parse)
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        _ => None,
    };

    let mut args = Vec::new();
    while let Some(token) = tokens.next() {
        if let Some(long) = token.strip_prefix("--") {
            let (name, value) = split_flag_value(long, &mut tokens)?;
            args.push(ToolArgPattern::LongFlag {
                name: ToolIdentifier::parse(name)?,
                value,
            });
        } else if let Some(short) = token.strip_prefix('-') {
            if short.is_empty() || !short.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(value.to_string());
            }
            args.push(ToolArgPattern::ShortFlags {
                flags: short.chars().collect(),
                value: None,
            });
        } else {
            args.push(ToolArgPattern::Positional(parse_tool_value_pattern(token)?));
        }
    }

    Ok(ToolInvocationPattern { command_path, args })
}

fn split_flag_value<'a, I>(
    token: &'a str,
    _tokens: &mut std::iter::Peekable<I>,
) -> Result<(&'a str, Option<ToolValuePattern>), String>
where
    I: Iterator<Item = &'a str>,
{
    if let Some((name, value)) = token.split_once('=') {
        Ok((name, Some(parse_tool_value_pattern(value)?)))
    } else {
        Ok((token, None))
    }
}

fn parse_tool_value_pattern(value: &str) -> Result<ToolValuePattern, String> {
    if value.is_empty() {
        Err(value.to_string())
    } else if value == "*" {
        Ok(ToolValuePattern::Star)
    } else if value == "**" {
        Ok(ToolValuePattern::GlobStar)
    } else {
        Ok(ToolValuePattern::Literal(ToolValueLiteral(
            value.to_string(),
        )))
    }
}

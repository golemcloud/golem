use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_polymorphic_agent_recipient,
    parse_polymorphic_tool_owner, parse_tool_owner,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolResourcePattern {
    AnyInvocation,
    Invocation(ToolInvocationPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ToolInvocationPattern {
    pub command_path: Option<Vec<ResourceIdentifier>>,
    pub args: Vec<ToolArgPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolArgPattern {
    ShortFlags {
        flags: Vec<char>,
        value: Option<ToolValuePattern>,
    },
    LongFlag {
        name: ResourceIdentifier,
        value: Option<ToolValuePattern>,
    },
    Positional(ToolValuePattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolValuePattern {
    Literal(ResourceLiteral),
    Star,
    GlobStar,
}

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

impl Subsumes for ToolResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnyInvocation, _) => true,
            (Self::Invocation(a), Self::Invocation(b)) => a == b,
            (Self::Invocation(_), Self::AnyInvocation) => false,
        }
    }
}

pub type PolymorphicToolResourcePattern = ToolResourcePattern;

impl ResourcePattern for ToolResourcePattern {
    type Polymorphic = PolymorphicToolResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolVerb {
    Invoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ToolClass;

impl PermissionClass for ToolClass {
    type Verb = ToolVerb;
    type Owner = ToolOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = ToolResourcePattern;
    const NAME: &'static str = "tool";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "invoke" => Some(Self::Verb::Invoke),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_tool_owner(Self::NAME, owner)
    }

    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError> {
        parse_agent_recipient(recipient)
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn parse_polymorphic_owner(
        owner: &str,
    ) -> Result<<Self::Owner as OwnerPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_tool_owner(Self::NAME, owner)
    }

    fn parse_polymorphic_recipient(
        recipient: &str,
    ) -> Result<<Self::Recipient as RecipientPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_agent_recipient(recipient)
    }

    fn parse_polymorphic_resource(
        resource: &str,
    ) -> Result<<Self::Resource as ResourcePattern>::Polymorphic, CardParseError> {
        Self::parse_polymorphic_resource(Self::NAME, resource)
    }

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

impl ToolClass {
    fn parse_resource(_class: &str, resource: &str) -> Result<ToolResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(ToolResourcePattern::AnyInvocation)
        } else {
            parse_tool_invocation_pattern(resource)
                .map(ToolResourcePattern::Invocation)
                .map_err(|_| CardParseError::InvalidResource {
                    class: ToolClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicToolResourcePattern, CardParseError> {
        Self::parse_resource(class, resource)
    }
}

fn parse_tool_invocation_pattern(value: &str) -> Result<ToolInvocationPattern, String> {
    let mut tokens = value.split_whitespace().peekable();
    let command_path = match tokens.peek().copied() {
        Some(first) if !first.starts_with('-') => {
            let command = tokens.next().unwrap();
            Some(
                command
                    .split('.')
                    .map(ResourceIdentifier::parse)
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
                name: ResourceIdentifier::parse(name)?,
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
        Ok(ToolValuePattern::Literal(ResourceLiteral(
            value.to_string(),
        )))
    }
}

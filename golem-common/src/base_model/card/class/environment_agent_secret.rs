use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentAgentSecretResourcePattern {
    Any,
    Key(EnvironmentAgentSecretKeyPathPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentAgentSecretKeyPathPattern {
    pub segments: Vec<EnvironmentAgentSecretKeySegmentPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentAgentSecretKeySegmentPattern {
    Literal(String),
    Star,
    GlobStar,
}

impl EnvironmentAgentSecretKeyPathPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            return Err(value.to_string());
        }
        Ok(Self {
            segments: value
                .split('.')
                .map(parse_environment_agent_secret_key_segment)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        environment_agent_secret_key_segments_subsume(&self.segments, &other.segments)
    }
}

impl EnvironmentAgentSecretResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Key(
            EnvironmentAgentSecretKeyPathPattern::parse(&value.into())
                .expect("invalid agent-secret key path"),
        )
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::exact(value)
    }
}

impl Subsumes for EnvironmentAgentSecretResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Key(a), Self::Key(b)) => a.subsumes(b),
            (Self::Key(_), Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentAgentSecretVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentAgentSecretClass;

impl PermissionClass for EnvironmentAgentSecretClass {
    type Verb = EnvironmentAgentSecretVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentAgentSecretResourcePattern;
    const NAME: &'static str = "environment.agent-secret";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            "restore" => Some(Self::Verb::Restore),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentAgentSecret(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentAgentSecret(pattern)
    }
}

pub type EnvironmentAgentSecretPermissionPattern =
    ClassPermissionPattern<EnvironmentAgentSecretClass>;
pub type PolymorphicEnvironmentAgentSecretPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentAgentSecretClass>;

impl EnvironmentAgentSecretClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentAgentSecretResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentAgentSecretResourcePattern::Any)
        } else {
            EnvironmentAgentSecretKeyPathPattern::parse(resource)
                .map(EnvironmentAgentSecretResourcePattern::Key)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentAgentSecretClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }
}

fn parse_environment_agent_secret_key_segment(
    value: &str,
) -> Result<EnvironmentAgentSecretKeySegmentPattern, String> {
    if value.is_empty() {
        Err(value.to_string())
    } else if value == "*" {
        Ok(EnvironmentAgentSecretKeySegmentPattern::Star)
    } else if value == "**" {
        Ok(EnvironmentAgentSecretKeySegmentPattern::GlobStar)
    } else if value.contains('*') || value.contains('.') {
        Err(value.to_string())
    } else {
        Ok(EnvironmentAgentSecretKeySegmentPattern::Literal(
            value.to_string(),
        ))
    }
}

fn environment_agent_secret_key_segments_subsume(
    left: &[EnvironmentAgentSecretKeySegmentPattern],
    right: &[EnvironmentAgentSecretKeySegmentPattern],
) -> bool {
    if left
        .first()
        .is_some_and(|segment| matches!(segment, EnvironmentAgentSecretKeySegmentPattern::GlobStar))
    {
        return true;
    }
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .all(|(left, right)| match (left, right) {
            (EnvironmentAgentSecretKeySegmentPattern::GlobStar, _) => true,
            (
                EnvironmentAgentSecretKeySegmentPattern::Star,
                EnvironmentAgentSecretKeySegmentPattern::Literal(_),
            ) => true,
            (
                EnvironmentAgentSecretKeySegmentPattern::Star,
                EnvironmentAgentSecretKeySegmentPattern::Star,
            ) => true,
            (
                EnvironmentAgentSecretKeySegmentPattern::Literal(left),
                EnvironmentAgentSecretKeySegmentPattern::Literal(right),
            ) => left == right,
            _ => false,
        })
}

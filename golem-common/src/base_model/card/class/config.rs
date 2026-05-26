use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ConfigResourcePattern {
    Any,
    Key(ConfigKeyPathPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ConfigKeyPathPattern {
    pub segments: Vec<ConfigKeySegmentPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ConfigKeySegmentPattern {
    Literal(String),
    Star,
    GlobStar,
}

impl ConfigKeyPathPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            return Err(value.to_string());
        }
        Ok(Self {
            segments: value
                .split('.')
                .map(parse_config_key_segment)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        config_key_segments_subsume(&self.segments, &other.segments)
    }
}

impl ConfigResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Key(ConfigKeyPathPattern::parse(&value.into()).expect("invalid config key path"))
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::exact(value)
    }
}

impl ResourcePattern for ConfigResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(ConfigResourcePattern::Any)
        } else {
            ConfigKeyPathPattern::parse(resource)
                .map(ConfigResourcePattern::Key)
                .map_err(|_| CardParseError::InvalidResource {
                    class: ConfigClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }

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
pub enum ConfigVerb {
    Read,
}
impl VerbPattern for ConfigVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "read" => Some(Self::Read),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ConfigClass;

impl PermissionClass for ConfigClass {
    type Verb = ConfigVerb;
    type Owner = AgentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = ConfigResourcePattern;
    const NAME: &'static str = "config";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Config(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Config(pattern)
    }
}

pub type ConfigPermissionPattern = ClassPermissionPattern<ConfigClass>;
pub type PolymorphicConfigPermissionPattern = PolymorphicClassPermissionPattern<ConfigClass>;

fn parse_config_key_segment(value: &str) -> Result<ConfigKeySegmentPattern, String> {
    if value.is_empty() {
        Err(value.to_string())
    } else if value == "*" {
        Ok(ConfigKeySegmentPattern::Star)
    } else if value == "**" {
        Ok(ConfigKeySegmentPattern::GlobStar)
    } else if value.contains('*') || value.contains('.') {
        Err(value.to_string())
    } else {
        Ok(ConfigKeySegmentPattern::Literal(value.to_string()))
    }
}

fn config_key_segments_subsume(
    left: &[ConfigKeySegmentPattern],
    right: &[ConfigKeySegmentPattern],
) -> bool {
    if left
        .first()
        .is_some_and(|segment| matches!(segment, ConfigKeySegmentPattern::GlobStar))
    {
        return true;
    }
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .all(|(left, right)| match (left, right) {
            (ConfigKeySegmentPattern::GlobStar, _) => true,
            (ConfigKeySegmentPattern::Star, ConfigKeySegmentPattern::Literal(_)) => true,
            (ConfigKeySegmentPattern::Star, ConfigKeySegmentPattern::Star) => true,
            (ConfigKeySegmentPattern::Literal(left), ConfigKeySegmentPattern::Literal(right)) => {
                left == right
            }
            _ => false,
        })
}

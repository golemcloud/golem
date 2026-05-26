use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::EnvironmentOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum SecretResourcePattern {
    Any,
    Key(SecretKeyPathPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SecretKeyPathPattern {
    pub segments: Vec<SecretKeySegmentPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum SecretKeySegmentPattern {
    Literal(String),
    Star,
    GlobStar,
}

impl SecretKeyPathPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            return Err(value.to_string());
        }
        Ok(Self {
            segments: value
                .split('.')
                .map(parse_secret_key_segment)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        secret_key_segments_subsume(&self.segments, &other.segments)
    }
}

impl SecretResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Key(SecretKeyPathPattern::parse(&value.into()).expect("invalid secret key path"))
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::exact(value)
    }
}

impl ResourcePattern for SecretResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(SecretResourcePattern::Any)
        } else {
            SecretKeyPathPattern::parse(resource)
                .map(SecretResourcePattern::Key)
                .map_err(|_| CardParseError::InvalidResource {
                    class: SecretClass::NAME.to_string(),
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
pub enum SecretVerb {
    Hold,
    Mint,
    Reveal,
}
impl VerbPattern for SecretVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "hold" => Some(Self::Hold),
            "mint" => Some(Self::Mint),
            "reveal" => Some(Self::Reveal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SecretClass;

impl PermissionClass for SecretClass {
    type Verb = SecretVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = SecretResourcePattern;
    const NAME: &'static str = "secret";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Secret(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Secret(pattern)
    }
}


fn parse_secret_key_segment(value: &str) -> Result<SecretKeySegmentPattern, String> {
    if value.is_empty() {
        Err(value.to_string())
    } else if value == "*" {
        Ok(SecretKeySegmentPattern::Star)
    } else if value == "**" {
        Ok(SecretKeySegmentPattern::GlobStar)
    } else if value.contains('*') || value.contains('.') {
        Err(value.to_string())
    } else {
        Ok(SecretKeySegmentPattern::Literal(value.to_string()))
    }
}

fn secret_key_segments_subsume(
    left: &[SecretKeySegmentPattern],
    right: &[SecretKeySegmentPattern],
) -> bool {
    if left
        .first()
        .is_some_and(|segment| matches!(segment, SecretKeySegmentPattern::GlobStar))
    {
        return true;
    }
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .all(|(left, right)| match (left, right) {
            (SecretKeySegmentPattern::GlobStar, _) => true,
            (SecretKeySegmentPattern::Star, SecretKeySegmentPattern::Literal(_)) => true,
            (SecretKeySegmentPattern::Star, SecretKeySegmentPattern::Star) => true,
            (SecretKeySegmentPattern::Literal(left), SecretKeySegmentPattern::Literal(right)) => {
                left == right
            }
            _ => false,
        })
}

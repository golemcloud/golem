use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_environment_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_environment_owner,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RdbmsResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl RdbmsResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::Glob(value.into())
    }
}

impl Subsumes for RdbmsResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Glob(a), Self::Glob(b)) => glob_subsumes(a, b),
            (Self::Glob(a), Self::Exact(b)) => glob_matches(a, b),
            (Self::Glob(_), Self::Any) => false,
            (Self::Exact(_), Self::Any | Self::Glob(_)) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicRdbmsResourcePattern {
    Concrete(RdbmsResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for RdbmsResourcePattern {
    type Polymorphic = PolymorphicRdbmsResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RdbmsVerb {
    Query,
    Execute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct RdbmsClass;

impl PermissionClass for RdbmsClass {
    type Verb = RdbmsVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = RdbmsResourcePattern;
    const NAME: &'static str = "rdbms";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "query" => Some(Self::Verb::Query),
            "execute" => Some(Self::Verb::Execute),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_environment_owner(Self::NAME, owner)
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
        parse_polymorphic_environment_owner(Self::NAME, owner)
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
        PermissionPattern::Rdbms(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Rdbms(pattern)
    }
}

pub type RdbmsPermissionPattern = ClassPermissionPattern<RdbmsClass>;
pub type PolymorphicRdbmsPermissionPattern = PolymorphicClassPermissionPattern<RdbmsClass>;

impl RdbmsClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<RdbmsResourcePattern, CardParseError> {
        if resource == "*" || resource == "**" {
            Ok(RdbmsResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(RdbmsResourcePattern::Glob(resource.to_string()))
        } else {
            Ok(RdbmsResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicRdbmsResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicRdbmsResourcePattern::Concrete,
            PolymorphicRdbmsResourcePattern::Slot,
            PolymorphicRdbmsResourcePattern::Template,
        )
    }
}

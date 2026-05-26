use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentAgentSecretResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl EnvironmentAgentSecretResourcePattern {
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

impl Subsumes for EnvironmentAgentSecretResourcePattern {
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
pub enum PolymorphicEnvironmentAgentSecretResourcePattern {
    Concrete(EnvironmentAgentSecretResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentAgentSecretResourcePattern {
    type Polymorphic = PolymorphicEnvironmentAgentSecretResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentAgentSecretVerb {
    View,
    Create,
    Update,
    Delete,
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
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_environment_owner(Self::NAME, owner)
    }

    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError> {
        parse_environment_recipient(recipient)
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
        parse_polymorphic_environment_recipient(recipient)
    }

    fn parse_polymorphic_resource(
        resource: &str,
    ) -> Result<<Self::Resource as ResourcePattern>::Polymorphic, CardParseError> {
        Self::parse_polymorphic_resource(Self::NAME, resource)
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
        if resource == "*" || resource == "**" {
            Ok(EnvironmentAgentSecretResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(EnvironmentAgentSecretResourcePattern::Glob(
                resource.to_string(),
            ))
        } else {
            Ok(EnvironmentAgentSecretResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentAgentSecretResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentAgentSecretResourcePattern::Concrete,
            PolymorphicEnvironmentAgentSecretResourcePattern::Slot,
            PolymorphicEnvironmentAgentSecretResourcePattern::Template,
        )
    }
}

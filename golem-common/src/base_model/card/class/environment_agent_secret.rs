use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentAgentSecretResourcePattern {
    Any,
    Key(DotPathPattern),
}

impl EnvironmentAgentSecretResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Key(DotPathPattern::parse(&value.into()).expect("invalid agent-secret key path"))
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

pub type PolymorphicEnvironmentAgentSecretResourcePattern = EnvironmentAgentSecretResourcePattern;

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
        if resource == "*" {
            Ok(EnvironmentAgentSecretResourcePattern::Any)
        } else {
            DotPathPattern::parse(resource)
                .map(EnvironmentAgentSecretResourcePattern::Key)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentAgentSecretClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentAgentSecretResourcePattern, CardParseError> {
        Self::parse_resource(class, resource)
    }
}

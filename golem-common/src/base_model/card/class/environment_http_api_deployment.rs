use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentHttpApiDeploymentResourcePattern {
    Any,
    DomainPath { domain: String, path_glob: String },
}

impl EnvironmentHttpApiDeploymentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        let value = value.into();
        Self::parse_value(&value).unwrap_or(Self::DomainPath {
            domain: value,
            path_glob: String::new(),
        })
    }

    fn parse_value(value: &str) -> Result<Self, String> {
        let Some((domain, path_glob)) = value.split_once('.') else {
            return Err(value.to_string());
        };
        if domain.is_empty() || !path_glob.starts_with('/') {
            return Err(value.to_string());
        }
        Ok(Self::DomainPath {
            domain: domain.to_string(),
            path_glob: path_glob.to_string(),
        })
    }
}

impl Subsumes for EnvironmentHttpApiDeploymentResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (
                Self::DomainPath {
                    domain: a_domain,
                    path_glob: a_path,
                },
                Self::DomainPath {
                    domain: b_domain,
                    path_glob: b_path,
                },
            ) => (a_domain == "*" || a_domain == b_domain) && glob_subsumes(a_path, b_path),
            (Self::DomainPath { .. }, Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentHttpApiDeploymentResourcePattern {
    Any,
    DomainPath { domain: String, path_glob: String },
    Slot(SlotVariable),
    Template(ResourceTemplate),
}

impl From<EnvironmentHttpApiDeploymentResourcePattern>
    for PolymorphicEnvironmentHttpApiDeploymentResourcePattern
{
    fn from(value: EnvironmentHttpApiDeploymentResourcePattern) -> Self {
        match value {
            EnvironmentHttpApiDeploymentResourcePattern::Any => Self::Any,
            EnvironmentHttpApiDeploymentResourcePattern::DomainPath { domain, path_glob } => {
                Self::DomainPath { domain, path_glob }
            }
        }
    }
}

impl ResourcePattern for EnvironmentHttpApiDeploymentResourcePattern {
    type Polymorphic = PolymorphicEnvironmentHttpApiDeploymentResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentHttpApiDeploymentVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentHttpApiDeploymentClass;

impl PermissionClass for EnvironmentHttpApiDeploymentClass {
    type Verb = EnvironmentHttpApiDeploymentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentHttpApiDeploymentResourcePattern;
    const NAME: &'static str = "environment.http-api-deployment";

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
        PermissionPattern::EnvironmentHttpApiDeployment(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentHttpApiDeployment(pattern)
    }
}

pub type EnvironmentHttpApiDeploymentPermissionPattern =
    ClassPermissionPattern<EnvironmentHttpApiDeploymentClass>;
pub type PolymorphicEnvironmentHttpApiDeploymentPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentHttpApiDeploymentClass>;

impl EnvironmentHttpApiDeploymentClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentHttpApiDeploymentResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentHttpApiDeploymentResourcePattern::Any)
        } else {
            EnvironmentHttpApiDeploymentResourcePattern::parse_value(resource).map_err(|_| {
                CardParseError::InvalidResource {
                    class: EnvironmentHttpApiDeploymentClass::NAME.to_string(),
                    resource: resource.to_string(),
                }
            })
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentHttpApiDeploymentResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentHttpApiDeploymentResourcePattern::from,
            PolymorphicEnvironmentHttpApiDeploymentResourcePattern::Slot,
            PolymorphicEnvironmentHttpApiDeploymentResourcePattern::Template,
        )
    }
}

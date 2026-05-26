use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentMcpDeploymentResourcePattern {
    Any,
    Name(EnvironmentMcpDeploymentName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct EnvironmentMcpDeploymentName(pub String);

impl EnvironmentMcpDeploymentName {
    fn parse(value: &str) -> Result<Self, String> {
        parse_environment_mcp_deployment_identifier(value).map(Self)
    }
}

impl EnvironmentMcpDeploymentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Name(
            EnvironmentMcpDeploymentName::parse(&value.into())
                .expect("invalid MCP deployment name"),
        )
    }
}

impl Subsumes for EnvironmentMcpDeploymentResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Name(a), Self::Name(b)) => a == b,
            (Self::Name(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentMcpDeploymentVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentMcpDeploymentClass;

impl PermissionClass for EnvironmentMcpDeploymentClass {
    type Verb = EnvironmentMcpDeploymentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentMcpDeploymentResourcePattern;
    const NAME: &'static str = "environment.mcp-deployment";

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
        PermissionPattern::EnvironmentMcpDeployment(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentMcpDeployment(pattern)
    }
}

pub type EnvironmentMcpDeploymentPermissionPattern =
    ClassPermissionPattern<EnvironmentMcpDeploymentClass>;
pub type PolymorphicEnvironmentMcpDeploymentPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentMcpDeploymentClass>;

impl EnvironmentMcpDeploymentClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentMcpDeploymentResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentMcpDeploymentResourcePattern::Any)
        } else {
            EnvironmentMcpDeploymentName::parse(resource)
                .map(EnvironmentMcpDeploymentResourcePattern::Name)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentMcpDeploymentClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }
}

fn parse_environment_mcp_deployment_identifier(value: &str) -> Result<String, String> {
    let mut chars = value.chars();
    if chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(value.to_string())
    } else {
        Err(value.to_string())
    }
}

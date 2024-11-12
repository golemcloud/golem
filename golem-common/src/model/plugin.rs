use crate::model::{ComponentId, Empty};
use poem_openapi::types::{ParseError, ParseFromParameter, ParseResult};
use poem_openapi::{Object, Union};
use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentPluginScope {
    pub component_id: ComponentId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum DefaultPluginScope {
    Global(Empty),
    Component(ComponentPluginScope),
}

impl DefaultPluginScope {
    pub fn global() -> Self {
        DefaultPluginScope::Global(Empty {})
    }

    pub fn component(component_id: ComponentId) -> Self {
        DefaultPluginScope::Component(ComponentPluginScope { component_id })
    }
}

impl Default for DefaultPluginScope {
    fn default() -> Self {
        DefaultPluginScope::global()
    }
}

impl Display for DefaultPluginScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefaultPluginScope::Global(_) => write!(f, "global"),
            DefaultPluginScope::Component(scope) => write!(f, "component:{}", scope.component_id),
        }
    }
}

impl ParseFromParameter for DefaultPluginScope {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        if value == "global" {
            Ok(Self::global())
        } else if let Some(id_part) = value.strip_prefix("component:") {
            let component_id = ComponentId::try_from(id_part);
            match component_id {
                Ok(component_id) => Ok(Self::component(component_id)),
                Err(err) => Err(ParseError::<Self>::custom(err)),
            }
        } else {
            Err(ParseError::<Self>::custom("Unexpected representation of plugin scope - must be 'global' or 'component:<component_id>'".to_string()))
        }
    }
}

impl From<DefaultPluginScope> for golem_api_grpc::proto::golem::component::DefaultPluginScope {
    fn from(scope: DefaultPluginScope) -> Self {
        match scope {
            DefaultPluginScope::Global(_) => golem_api_grpc::proto::golem::component::DefaultPluginScope {
                scope: Some(golem_api_grpc::proto::golem::component::default_plugin_scope::Scope::Global(
                    golem_api_grpc::proto::golem::common::Empty {},
                )),
            },
            DefaultPluginScope::Component(scope) => golem_api_grpc::proto::golem::component::DefaultPluginScope {
                scope: Some(golem_api_grpc::proto::golem::component::default_plugin_scope::Scope::Component(
                    golem_api_grpc::proto::golem::component::ComponentPluginScope {
                        component_id: Some(scope.component_id.into()),
                    },
                )),
            },
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::DefaultPluginScope> for DefaultPluginScope {
    type Error = String;

    fn try_from(
        proto: golem_api_grpc::proto::golem::component::DefaultPluginScope,
    ) -> Result<Self, Self::Error> {
        match proto.scope {
            Some(golem_api_grpc::proto::golem::component::default_plugin_scope::Scope::Global(
                _,
            )) => Ok(Self::global()),
            Some(
                golem_api_grpc::proto::golem::component::default_plugin_scope::Scope::Component(
                    proto,
                ),
            ) => Ok(Self::component(
                proto
                    .component_id
                    .ok_or("Missing component_id".to_string())?
                    .try_into()?,
            )),
            None => Err("Missing scope".to_string()),
        }
    }
}

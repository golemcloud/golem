use std::result::Result;

use poem_openapi::*;
use serde::{Deserialize, Serialize};

use golem_api_grpc::proto::golem::apidefinition as grpc_apidefinition;
use golem_common::model::ComponentId;

use crate::api_definition::http::MethodPattern;
use crate::api_definition::{ApiDefinitionId, ApiSite, ApiVersion};
use rib::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ApiDeployment {
    pub api_definitions: Vec<ApiDefinitionInfo>,
    pub site: ApiSite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ApiDefinitionInfo {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
}

// Mostly this data structures that represents the actual incoming request
// exist due to the presence of complicated Expr data type in api_definition::ApiDefinition.
// Consider them to be otherwise same
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct HttpApiDefinition {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<Route>,
    #[serde(default)]
    pub draft: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct Route {
    pub method: MethodPattern,
    pub path: String,
    pub binding: GolemWorkerBinding,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemWorkerBinding {
    pub component_id: ComponentId,
    pub worker_name: String,
    pub idempotency_key: Option<String>,
    pub response: String,
}

impl<N> From<crate::api_definition::ApiDeployment<N>> for ApiDeployment {
    fn from(value: crate::api_definition::ApiDeployment<N>) -> Self {
        let api_definitions = value
            .api_definition_keys
            .into_iter()
            .map(|key| ApiDefinitionInfo {
                id: key.id,
                version: key.version,
            })
            .collect();

        Self {
            api_definitions,
            site: value.site,
        }
    }
}

impl TryFrom<crate::api_definition::http::HttpApiDefinition> for HttpApiDefinition {
    type Error = String;

    fn try_from(
        value: crate::api_definition::http::HttpApiDefinition,
    ) -> Result<Self, Self::Error> {
        let mut routes = Vec::new();
        for route in value.routes {
            let v = Route::try_from(route)?;
            routes.push(v);
        }

        Ok(Self {
            id: value.id,
            version: value.version,
            routes,
            draft: value.draft,
        })
    }
}

impl TryInto<crate::api_definition::http::HttpApiDefinition> for HttpApiDefinition {
    type Error = String;

    fn try_into(self) -> Result<crate::api_definition::http::HttpApiDefinition, Self::Error> {
        let mut routes = Vec::new();

        for route in self.routes {
            let v = route.try_into()?;
            routes.push(v);
        }

        Ok(crate::api_definition::http::HttpApiDefinition {
            id: self.id,
            version: self.version,
            routes,
            draft: self.draft,
        })
    }
}

impl TryFrom<crate::api_definition::http::Route> for Route {
    type Error = String;

    fn try_from(value: crate::api_definition::http::Route) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = GolemWorkerBinding::try_from(value.binding)?;

        Ok(Self {
            method: value.method,
            path,
            binding,
        })
    }
}

impl TryInto<crate::api_definition::http::Route> for Route {
    type Error = String;

    fn try_into(self) -> Result<crate::api_definition::http::Route, Self::Error> {
        let path = crate::api_definition::http::AllPathPatterns::parse(self.path.as_str())
            .map_err(|e| e.to_string())?;
        let binding = self.binding.try_into()?;

        Ok(crate::api_definition::http::Route {
            method: self.method,
            path,
            binding,
        })
    }
}

impl TryFrom<crate::worker_binding::GolemWorkerBinding> for GolemWorkerBinding {
    type Error = String;

    fn try_from(value: crate::worker_binding::GolemWorkerBinding) -> Result<Self, Self::Error> {
        let response: String = rib::to_string(&value.response.0).map_err(|e| e.to_string())?;

        let worker_id = rib::to_string(&value.worker_name).map_err(|e| e.to_string())?;

        let idempotency_key = if let Some(key) = &value.idempotency_key {
            Some(rib::to_string(key).map_err(|e| e.to_string())?)
        } else {
            None
        };

        Ok(Self {
            component_id: value.component_id,
            worker_name: worker_id,
            idempotency_key,
            response,
        })
    }
}

impl TryInto<crate::worker_binding::GolemWorkerBinding> for GolemWorkerBinding {
    type Error = String;

    fn try_into(self) -> Result<crate::worker_binding::GolemWorkerBinding, Self::Error> {
        let response: crate::worker_binding::ResponseMapping = {
            let r = rib::from_string(self.response.as_str()).map_err(|e| e.to_string())?;
            crate::worker_binding::ResponseMapping(r)
        };

        let worker_name: Expr =
            rib::from_string(self.worker_name.as_str()).map_err(|e| e.to_string())?;

        let idempotency_key = if let Some(key) = &self.idempotency_key {
            Some(rib::from_string(key).map_err(|e| e.to_string())?)
        } else {
            None
        };

        Ok(crate::worker_binding::GolemWorkerBinding {
            component_id: self.component_id,
            worker_name,
            idempotency_key,
            response,
        })
    }
}

impl TryFrom<crate::api_definition::http::HttpApiDefinition> for grpc_apidefinition::ApiDefinition {
    type Error = String;

    fn try_from(
        value: crate::api_definition::http::HttpApiDefinition,
    ) -> Result<Self, Self::Error> {
        let routes = value
            .routes
            .into_iter()
            .map(grpc_apidefinition::HttpRoute::try_from)
            .collect::<Result<Vec<grpc_apidefinition::HttpRoute>, String>>()?;

        let id = value.id.0;

        let definition = grpc_apidefinition::HttpApiDefinition { routes };

        let result = grpc_apidefinition::ApiDefinition {
            id: Some(grpc_apidefinition::ApiDefinitionId { value: id }),
            version: value.version.0,
            definition: Some(grpc_apidefinition::api_definition::Definition::Http(
                definition,
            )),
            draft: value.draft,
        };

        Ok(result)
    }
}

impl TryFrom<grpc_apidefinition::ApiDefinition> for crate::api_definition::http::HttpApiDefinition {
    type Error = String;

    fn try_from(value: grpc_apidefinition::ApiDefinition) -> Result<Self, Self::Error> {
        let routes = match value.definition.ok_or("definition is missing")? {
            grpc_apidefinition::api_definition::Definition::Http(http) => http
                .routes
                .into_iter()
                .map(crate::api_definition::http::Route::try_from)
                .collect::<Result<Vec<crate::api_definition::http::Route>, String>>()?,
        };

        let id = value.id.ok_or("Api Definition ID is missing")?;

        let result = crate::api_definition::http::HttpApiDefinition {
            id: crate::api_definition::ApiDefinitionId(id.value),
            version: crate::api_definition::ApiVersion(value.version),
            routes,
            draft: value.draft,
        };

        Ok(result)
    }
}

impl TryFrom<crate::api_definition::http::Route> for grpc_apidefinition::HttpRoute {
    type Error = String;

    fn try_from(value: crate::api_definition::http::Route) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = grpc_apidefinition::WorkerBinding::try_from(value.binding)?;
        let method: grpc_apidefinition::HttpMethod = value.method.into();

        let result = grpc_apidefinition::HttpRoute {
            method: method as i32,
            path,
            binding: Some(binding),
        };

        Ok(result)
    }
}

impl From<MethodPattern> for grpc_apidefinition::HttpMethod {
    fn from(value: MethodPattern) -> Self {
        match value {
            MethodPattern::Get => grpc_apidefinition::HttpMethod::Get,
            MethodPattern::Post => grpc_apidefinition::HttpMethod::Post,
            MethodPattern::Put => grpc_apidefinition::HttpMethod::Put,
            MethodPattern::Delete => grpc_apidefinition::HttpMethod::Delete,
            MethodPattern::Patch => grpc_apidefinition::HttpMethod::Patch,
            MethodPattern::Head => grpc_apidefinition::HttpMethod::Head,
            MethodPattern::Options => grpc_apidefinition::HttpMethod::Options,
            MethodPattern::Trace => grpc_apidefinition::HttpMethod::Trace,
            MethodPattern::Connect => grpc_apidefinition::HttpMethod::Connect,
        }
    }
}

impl TryFrom<grpc_apidefinition::HttpRoute> for crate::api_definition::http::Route {
    type Error = String;

    fn try_from(value: grpc_apidefinition::HttpRoute) -> Result<Self, Self::Error> {
        let path = crate::api_definition::http::AllPathPatterns::parse(value.path.as_str())
            .map_err(|e| e.to_string())?;
        let binding = value.binding.ok_or("binding is missing")?.try_into()?;

        let method: MethodPattern = value.method.try_into()?;

        let result = crate::api_definition::http::Route {
            method,
            path,
            binding,
        };

        Ok(result)
    }
}

impl TryFrom<crate::worker_binding::GolemWorkerBinding> for grpc_apidefinition::WorkerBinding {
    type Error = String;

    fn try_from(value: crate::worker_binding::GolemWorkerBinding) -> Result<Self, Self::Error> {
        let response: String = value.response.0.to_string();

        let worker_id = rib::to_string(&value.worker_name).map_err(|e| e.to_string())?;

        let idempotency_key = if let Some(key) = &value.idempotency_key {
            Some(rib::to_string(key).map_err(|e| e.to_string())?)
        } else {
            None
        };

        let result = grpc_apidefinition::WorkerBinding {
            component: Some(value.component_id.into()),
            worker_id,
            idempotency_key,
            response,
        };

        Ok(result)
    }
}

impl TryFrom<grpc_apidefinition::WorkerBinding> for crate::worker_binding::GolemWorkerBinding {
    type Error = String;

    fn try_from(value: grpc_apidefinition::WorkerBinding) -> Result<Self, Self::Error> {
        let response: crate::worker_binding::ResponseMapping = {
            let r: Expr = value.response.parse()?;
            crate::worker_binding::ResponseMapping(r)
        };

        let worker_name = value.worker_id.parse()?;

        let component_id = value.component.ok_or("component is missing")?.try_into()?;

        let idempotency_key = if let Some(key) = &value.idempotency_key {
            Some(key.parse()?)
        } else {
            None
        };

        let result = crate::worker_binding::GolemWorkerBinding {
            component_id,
            worker_name,
            idempotency_key,
            response,
        };

        Ok(result)
    }
}

#[test]
fn test_method_pattern() {
    for method in 0..8 {
        let method_pattern: MethodPattern = method.try_into().unwrap();
        let method_grpc: grpc_apidefinition::HttpMethod = method_pattern.into();
        assert_eq!(method, method_grpc as i32);
    }
}

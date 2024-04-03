use std::result::Result;

use poem_openapi::*;
use serde::{Deserialize, Serialize};

use golem_common::model::TemplateId;

use crate::api_definition::http::MethodPattern;
use crate::api_definition::{ApiDefinitionId, ApiVersion};
use crate::expression::Expr;

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
    pub template: TemplateId,
    pub worker_id: serde_json::value::Value,
    pub function_name: String,
    pub function_params: Vec<serde_json::value::Value>,
    pub response: Option<serde_json::value::Value>,
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
        let path = crate::api_definition::http::PathPattern::from(self.path.as_str())
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
        let response: Option<serde_json::value::Value> = match value.response {
            Some(v) => {
                let r = Expr::to_json_value(&v.0).map_err(|e| e.to_string())?;
                Some(r)
            }
            None => None,
        };
        let worker_id = serde_json::to_value(value.worker_id).map_err(|e| e.to_string())?;
        let mut function_params = Vec::new();
        for param in value.function_params {
            let v = serde_json::to_value(param).map_err(|e| e.to_string())?;
            function_params.push(v);
        }

        Ok(Self {
            template: value.template,
            worker_id,
            function_name: value.function_name,
            function_params,
            response,
        })
    }
}

impl TryInto<crate::worker_binding::GolemWorkerBinding> for GolemWorkerBinding {
    type Error = String;

    fn try_into(self) -> Result<crate::worker_binding::GolemWorkerBinding, Self::Error> {
        let response: Option<crate::worker_binding::ResponseMapping> = match self.response {
            Some(v) => {
                let r = Expr::from_json_value(&v).map_err(|e| e.to_string())?;
                Some(crate::worker_binding::ResponseMapping(r))
            }
            None => None,
        };

        let worker_id: Expr = serde_json::from_value(self.worker_id).map_err(|e| e.to_string())?;
        let mut function_params = Vec::new();

        for param in self.function_params {
            let v: Expr = serde_json::from_value(param).map_err(|e| e.to_string())?;
            function_params.push(v);
        }

        Ok(crate::worker_binding::GolemWorkerBinding {
            template: self.template,
            worker_id,
            function_name: self.function_name,
            function_params,
            response,
        })
    }
}

use golem_api_grpc::proto::golem::apidefinition as grpc_apidefinition;

impl TryFrom<crate::api_definition::http::HttpApiDefinition>
    for grpc_apidefinition::HttpApiDefinition
{
    type Error = String;

    fn try_from(
        value: crate::api_definition::http::HttpApiDefinition,
    ) -> Result<Self, Self::Error> {
        let routes = value
            .routes
            .into_iter()
            .map(grpc_apidefinition::HttpRoute::try_from)
            .collect::<Result<Vec<grpc_apidefinition::HttpRoute>, String>>()?;

        let result = grpc_apidefinition::HttpApiDefinition {
            id: value.id.0,
            version: value.version.0,
            routes,
        };

        Ok(result)
    }
}

impl TryFrom<grpc_apidefinition::HttpApiDefinition>
    for crate::api_definition::http::HttpApiDefinition
{
    type Error = String;

    fn try_from(value: grpc_apidefinition::HttpApiDefinition) -> Result<Self, Self::Error> {
        let routes = value
            .routes
            .into_iter()
            .map(crate::api_definition::http::Route::try_from)
            .collect::<Result<Vec<crate::api_definition::http::Route>, String>>()?;

        let result = crate::api_definition::http::HttpApiDefinition {
            id: crate::api_definition::ApiDefinitionId(value.id),
            version: crate::api_definition::ApiVersion(value.version),
            routes,
        };

        Ok(result)
    }
}

impl TryFrom<crate::api_definition::http::Route> for grpc_apidefinition::HttpRoute {
    type Error = String;

    fn try_from(value: crate::api_definition::http::Route) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = grpc_apidefinition::GolemWorkerBinding::try_from(value.binding)?;

        let result = grpc_apidefinition::HttpRoute {
            method: value.method.to_string(),
            path,
            binding: Some(binding),
        };

        Ok(result)
    }
}

impl TryFrom<grpc_apidefinition::HttpRoute> for crate::api_definition::http::Route {
    type Error = String;

    fn try_from(value: grpc_apidefinition::HttpRoute) -> Result<Self, Self::Error> {
        let path = crate::api_definition::http::PathPattern::from(value.path.as_str())
            .map_err(|e| e.to_string())?;
        let binding = value.binding.ok_or("binding is missing")?.try_into()?;

        let method = value.method.parse()?;

        let result = crate::api_definition::http::Route {
            method,
            path,
            binding,
        };

        Ok(result)
    }
}

impl TryFrom<crate::worker_binding::GolemWorkerBinding> for grpc_apidefinition::GolemWorkerBinding {
    type Error = String;

    fn try_from(value: crate::worker_binding::GolemWorkerBinding) -> Result<Self, Self::Error> {
        let response = match value.response {
            Some(v) => {
                let r = grpc_apidefinition::ResponseMapping::try_from(v)?;
                Some(r)
            }
            None => None,
        };

        let worker_id = serde_json::to_string(&value.worker_id).map_err(|e| e.to_string())?;
        let function_params = value
            .function_params
            .into_iter()
            .map(|p| serde_json::to_string(&p).map_err(|e| e.to_string()))
            .collect::<Result<Vec<String>, String>>()?;

        let result = golem_api_grpc::proto::golem::apidefinition::GolemWorkerBinding {
            template: Some(value.template.into()),
            worker_id,
            function_name: value.function_name,
            function_params,
            response,
        };

        Ok(result)
    }
}

impl TryFrom<grpc_apidefinition::GolemWorkerBinding> for crate::worker_binding::GolemWorkerBinding {
    type Error = String;

    fn try_from(value: grpc_apidefinition::GolemWorkerBinding) -> Result<Self, Self::Error> {
        let response = match value.response {
            Some(v) => {
                let r = v.try_into()?;
                Some(r)
            }
            None => None,
        };

        let worker_id = serde_json::from_str(&value.worker_id).map_err(|e| e.to_string())?;
        let function_params = value
            .function_params
            .into_iter()
            .map(|p| serde_json::from_str(&p).map_err(|e| e.to_string()))
            .collect::<Result<Vec<Expr>, String>>()?;

        let template_id = value.template.ok_or("template is missing")?.try_into()?;

        let result = crate::worker_binding::GolemWorkerBinding {
            template: template_id,
            worker_id,
            function_name: value.function_name,
            function_params,
            response,
        };

        Ok(result)
    }
}

impl TryFrom<crate::worker_binding::ResponseMapping> for grpc_apidefinition::ResponseMapping {
    type Error = String;

    fn try_from(value: crate::worker_binding::ResponseMapping) -> Result<Self, Self::Error> {
        let body = serde_json::to_string(&value.body).map_err(|e| e.to_string())?;
        let status = serde_json::to_string(&value.status).map_err(|e| e.to_string())?;
        let headers = value
            .headers
            .into_iter()
            .map(|(k, v)| {
                serde_json::to_string(&v)
                    .map(|v| (k, v))
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<HashMap<String, String>, String>>()?;

        let result = golem_api_grpc::proto::golem::apidefinition::ResponseMapping {
            body,
            status,
            headers,
        };

        Ok(result)
    }
}

impl TryFrom<grpc_apidefinition::ResponseMapping> for crate::worker_binding::ResponseMapping {
    type Error = String;

    fn try_from(value: grpc_apidefinition::ResponseMapping) -> Result<Self, Self::Error> {
        let body = serde_json::from_str(&value.body).map_err(|e| e.to_string())?;
        let status = serde_json::from_str(&value.status).map_err(|e| e.to_string())?;
        let headers = value
            .headers
            .into_iter()
            .map(|(k, v)| {
                serde_json::from_str(&v)
                    .map(|v| (k, v))
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<HashMap<String, Expr>, String>>()?;

        let result = crate::worker_binding::ResponseMapping {
            body,
            status,
            headers,
        };

        Ok(result)
    }
}

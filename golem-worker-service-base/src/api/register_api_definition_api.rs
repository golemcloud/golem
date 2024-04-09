use std::result::Result;

use poem_openapi::*;
use serde::{Deserialize, Serialize};

use golem_common::model::TemplateId;

use crate::api_definition::http::MethodPattern;
use crate::api_definition::{ApiDefinitionId, ApiVersion};
use crate::expression;
use crate::expression::Expr;
use crate::parser::ParseError;

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
    pub worker_id: String,
    pub function_name: String,
    pub function_params: Vec<String>,
    pub response: Option<String>,
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
        let response: Option<String> = match value.response {
            Some(v) => {
                let r = expression::to_string(&v.0).map_err(|e| e.to_string())?;
                Some(r)
            }
            None => None,
        };
        let worker_id = expression::to_string(&value.worker_id).map_err(|e| e.to_string())?;
        let mut function_params = Vec::new();
        for param in value.function_params {
            let v = expression::to_string(&param).map_err(|e| e.to_string())?;
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
                let r = expression::from_string(v).map_err(|e| e.to_string())?;
                Some(crate::worker_binding::ResponseMapping(r))
            }
            None => None,
        };

        let worker_id: Expr = expression::from_string(self.worker_id).map_err(|e| e.to_string())?;
        let mut function_params = Vec::new();

        for param in self.function_params {
            let v: Expr = expression::from_string(param).map_err(|e| e.to_string())?;
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

        let id = value.id.0;

        let result = grpc_apidefinition::HttpApiDefinition {
            id: Some(grpc_apidefinition::ApiDefinitionId { value: id }),
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

        let id = value.id.ok_or("Api Definition ID is missing")?;

        let result = crate::api_definition::http::HttpApiDefinition {
            id: crate::api_definition::ApiDefinitionId(id.value),
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
        let response: Option<String> = match value.response {
            Some(v) => {
                let r = v.0.to_string().map_err(|e| e.to_string())?;
                Some(r)
            }
            None => None,
        };

        let worker_id = expression::to_string(&value.worker_id).map_err(|e| e.to_string())?;
        let function_params = value
            .function_params
            .into_iter()
            .map(|p| expression::to_string(&p).map_err(|e| e.to_string()))
            .collect::<Result<Vec<String>, String>>()?;

        let result = grpc_apidefinition::WorkerBinding {
            template: Some(value.template.into()),
            worker_id,
            function_name: value.function_name,
            function_params,
            response,
        };

        Ok(result)
    }
}

impl TryFrom<grpc_apidefinition::WorkerBinding> for crate::worker_binding::GolemWorkerBinding {
    type Error = String;

    fn try_from(value: grpc_apidefinition::WorkerBinding) -> Result<Self, Self::Error> {
        let response: Option<crate::worker_binding::ResponseMapping> = match value.response {
            Some(v) => {
                let r: Expr = v.parse().map_err(|e: ParseError| e.to_string())?;
                Some(crate::worker_binding::ResponseMapping(r))
            }
            None => None,
        };

        let worker_id = value
            .worker_id
            .parse()
            .map_err(|e: ParseError| e.to_string())?;

        let function_params: Vec<Expr> = value
            .function_params
            .into_iter()
            .map(|p| p.parse().map_err(|e: ParseError| e.to_string()))
            .collect::<Result<_, String>>()?;

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

#[test]
fn test_method_pattern() {
    for method in 0..8 {
        let method_pattern: MethodPattern = method.try_into().unwrap();
        let method_grpc: grpc_apidefinition::HttpMethod = method_pattern.into();
        assert_eq!(method, method_grpc as i32);
    }
}

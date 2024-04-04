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

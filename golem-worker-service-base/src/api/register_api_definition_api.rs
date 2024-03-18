use std::collections::HashMap;
use std::result::Result;

use crate::api_definition;
use crate::api_definition::{ApiDefinitionId, MethodPattern, Version};
use crate::expr::Expr;
use golem_common::model::TemplateId;
use poem_openapi::*;
use serde::{Deserialize, Serialize};

// Mostly this data structures that represents the actual incoming request
// exist due to the presence of complicated Expr data type in api_definition::ApiDefinition.
// Consider them to be otherwise same
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
struct ApiDefinition {
    pub id: ApiDefinitionId,
    pub version: Version,
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
struct Route {
    pub method: MethodPattern,
    pub path: String,
    pub binding: GolemWorkerBinding,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
struct GolemWorkerBinding {
    pub template: TemplateId,
    pub worker_id: serde_json::value::Value,
    pub function_name: String,
    pub function_params: Vec<serde_json::value::Value>,
    pub response: Option<ResponseMapping>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
struct ResponseMapping {
    pub body: serde_json::value::Value,
    // ${function.return}
    pub status: serde_json::value::Value,
    // "200" or if ${response.body.id == 1} "200" else "400"
    pub headers: HashMap<String, serde_json::value::Value>,
}

impl TryFrom<api_definition::ApiDefinition> for ApiDefinition {
    type Error = String;

    fn try_from(value: api_definition::ApiDefinition) -> Result<Self, Self::Error> {
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

impl TryInto<api_definition::ApiDefinition> for ApiDefinition {
    type Error = String;

    fn try_into(self) -> Result<api_definition::ApiDefinition, Self::Error> {
        let mut routes = Vec::new();

        for route in self.routes {
            let v = route.try_into()?;
            routes.push(v);
        }

        Ok(api_definition::ApiDefinition {
            id: self.id,
            version: self.version,
            routes,
        })
    }
}

impl TryFrom<api_definition::Route> for Route {
    type Error = String;

    fn try_from(value: api_definition::Route) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = GolemWorkerBinding::try_from(value.binding)?;

        Ok(Self {
            method: value.method,
            path,
            binding,
        })
    }
}

impl TryInto<api_definition::Route> for Route {
    type Error = String;

    fn try_into(self) -> Result<api_definition::Route, Self::Error> {
        let path =
            api_definition::PathPattern::from(self.path.as_str()).map_err(|e| e.to_string())?;
        let binding = self.binding.try_into()?;

        Ok(api_definition::Route {
            method: self.method,
            path,
            binding,
        })
    }
}

impl TryFrom<api_definition::ResponseMapping> for ResponseMapping {
    type Error = String;

    fn try_from(value: api_definition::ResponseMapping) -> Result<Self, Self::Error> {
        let body = serde_json::to_value(value.body).map_err(|e| e.to_string())?;
        let status = serde_json::to_value(value.status).map_err(|e| e.to_string())?;
        let mut headers = HashMap::new();
        for (key, value) in value.headers {
            let v = serde_json::to_value(value).map_err(|e| e.to_string())?;
            headers.insert(key.to_string(), v);
        }
        Ok(Self {
            body,
            status,
            headers,
        })
    }
}

impl TryInto<api_definition::ResponseMapping> for ResponseMapping {
    type Error = String;

    fn try_into(self) -> Result<api_definition::ResponseMapping, Self::Error> {
        let body: Expr = serde_json::from_value(self.body).map_err(|e| e.to_string())?;
        let status: Expr = serde_json::from_value(self.status).map_err(|e| e.to_string())?;
        let mut headers = HashMap::new();
        for (key, value) in self.headers {
            let v: Expr = serde_json::from_value(value).map_err(|e| e.to_string())?;
            headers.insert(key.to_string(), v);
        }

        Ok(api_definition::ResponseMapping {
            body,
            status,
            headers,
        })
    }
}

impl TryFrom<api_definition::GolemWorkerBinding> for GolemWorkerBinding {
    type Error = String;

    fn try_from(value: api_definition::GolemWorkerBinding) -> Result<Self, Self::Error> {
        let response: Option<ResponseMapping> = match value.response {
            Some(v) => {
                let r = ResponseMapping::try_from(v)?;
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

impl TryInto<api_definition::GolemWorkerBinding> for GolemWorkerBinding {
    type Error = String;

    fn try_into(self) -> Result<api_definition::GolemWorkerBinding, Self::Error> {
        let response: Option<api_definition::ResponseMapping> = match self.response {
            Some(v) => {
                let r: api_definition::ResponseMapping = v.try_into()?;
                Some(r)
            }
            None => None,
        };

        let worker_id: Expr = serde_json::from_value(self.worker_id).map_err(|e| e.to_string())?;
        let mut function_params = Vec::new();

        for param in self.function_params {
            let v: Expr = serde_json::from_value(param).map_err(|e| e.to_string())?;
            function_params.push(v);
        }

        Ok(api_definition::GolemWorkerBinding {
            template: self.template,
            worker_id,
            function_name: self.function_name,
            function_params,
            response,
        })
    }
}

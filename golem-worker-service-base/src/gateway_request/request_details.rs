// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_api_definition::http::{QueryInfo, VarInfo};

use crate::gateway_api_deployment::ApiSiteString;
use crate::gateway_execution::gateway_session::{DataKey, GatewaySessionStore, SessionId};
use crate::gateway_execution::router::RouterPattern;
use crate::gateway_execution::RibInputTypeMismatch;
use crate::gateway_middleware::HttpMiddlewares;
use crate::gateway_request::http_request::ApiInputPath;
use golem_common::SafeDisplay;
use http::uri::Scheme;
use http::{HeaderMap, Method};
use poem::web::cookie::CookieJar;
use rib::{RibInput, RibInputTypeInfo};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::format;
use std::vec;
use url::Url;
use crate::gateway_request::http_request::router;
use super::http_request::router::PathParamExtractor;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;

/// A structure that holds the incoming request details
/// along with parameters that are required by the route in API definition
///
/// TODO: track consumption of the body using types,
/// something like
///   struct ConsumedBody(poem::Request);
///   struct IntactBody(poem::Request);
///
///   struct HttpRequestDetails<BodyState>
#[derive(Debug)]
pub struct HttpRequestDetails {
    pub underlying: poem::Request,
    pub path_param_extractors: Vec<PathParamExtractor>,
    pub query_info: Vec<QueryInfo>,
    pub request_custom_params: Option<HashMap<String, Value>>,
}

impl HttpRequestDetails {
    pub fn empty() -> Self {
        Self {
            underlying: poem::Request::default(),
            path_param_extractors: vec![],
            query_info: vec![],
            request_custom_params: None
        }
    }

    pub fn from_request(
        request: poem::Request,
        path_param_extractors: Vec<PathParamExtractor>,
        query_info: Vec<QueryInfo>
    ) -> Self {
        Self {
            underlying: request,
            path_param_extractors: path_param_extractors,
            query_info: query_info,
            request_custom_params: None
        }
    }

    pub fn api_site_string(&self) -> Result<ApiSiteString, String> {
        let host_header = self.underlying.header(http::header::HOST).map(|h| h.to_string())
            .ok_or("No host header provided".to_string())?;
        Ok(ApiSiteString(host_header))
    }

    pub async fn inject_auth_details(
        &mut self,
        session_id: &SessionId,
        gateway_session_store: &GatewaySessionStore,
    ) -> Result<(), String> {
        let claims = gateway_session_store
            .get(session_id, &DataKey::claims())
            .await
            .map_err(|err| err.to_safe_string())?;

        if let Some(custom_params) = self.request_custom_params.as_mut() {
            custom_params.insert("auth".to_string(), claims.0);
        } else {
            self.request_custom_params = Some(HashMap::from_iter([("auth".to_string(), claims.0)]));
        }

        Ok(())
    }

    pub fn url(&self) -> Result<url::Url, String> {
        url::Url::parse(&self.underlying.uri().to_string()).map_err(|e| format!("Failed parsing url: {e}"))
    }

    fn request_path_values(&self) -> RequestPathValues {
        let path: Vec<&str> = RouterPattern::split(self.underlying.uri().path()).collect();

        let path_param_values = self.path_param_extractors
            .iter()
            .map(|param| match param {
                router::PathParamExtractor::Single { var_info, index } => {
                    (var_info.clone(), path[*index].to_string())
                }
                router::PathParamExtractor::AllFollowing { var_info, index } => {
                    let value = path[*index..].join("/");
                    (var_info.clone(), value)
                }
            })
            .collect();

        RequestPathValues::from(&path_param_values)
    }

    fn request_query_values(&self) -> Result<RequestQueryValues, String> {
        let query_key_values = self.underlying.uri().query().map(query_components_from_str).unwrap_or_default();

        RequestQueryValues::from(&query_key_values, &self.query_info)
            .map_err(|e| format!("Failed to extract query values, missing: [{}]", e.join(",")))
    }

    fn request_header_values(&self) -> Result<RequestHeaderValues, String> {
        RequestHeaderValues::from(self.underlying.headers())
            .map_err(|e| format!("Found malformed headers: [{}]", e.join(",")))
    }

    /// consumes the body of the underlying request
    async fn request_body_value(&mut self) -> Result<RequestBodyValue, String> {
        let body = self.underlying.take_body();

        let json_request_body: Value = if body.is_empty() {
            Value::Null
        } else {
            match body.into_json().await {
                Ok(json_request_body) => json_request_body,
                Err(err) => {
                    tracing::error!("Failed reading http request body as json: {}", err);
                    Err(format!("Request body parse error: {err}"))?
                }
            }
        };

        Ok(RequestBodyValue(json_request_body))
    }

    // pub fn as_json(&self) -> Value {
    //     let typed_path_values = self.request_path_params.clone().0;
    //     let typed_query_values = self.request_query_params.clone().0;

    //     let mut path_values = serde_json::Map::new();

    //     for field in typed_path_values.fields.iter() {
    //         path_values.insert(field.name.clone(), field.value.clone());
    //     }

    //     for field in typed_query_values.fields.iter() {
    //         path_values.insert(field.name.clone(), field.value.clone());
    //     }

    //     let merged_request_path_and_query = Value::Object(path_values);

    //     let mut header_records = serde_json::Map::new();

    //     for field in self.request_headers.0.fields.iter() {
    //         header_records.insert(field.name.clone(), field.value.clone());
    //     }

    //     let header_value = Value::Object(header_records);

    //     let mut basic = serde_json::Map::from_iter(vec![
    //         ("path".to_string(), merged_request_path_and_query),
    //         ("body".to_string(), self.request_body_value.0.clone()),
    //         ("headers".to_string(), header_value),
    //     ]);

    //     let custom = self.request_custom_params.clone().unwrap_or_default();

    //     for (key, value) in custom.iter() {
    //         basic.insert(key.clone(), value.clone());
    //     }

    //     Value::Object(basic)
    // }

    /// will consume the underlying request body
    async fn as_value(&mut self) -> Result<Value, String> {
        let typed_path_values = self.request_path_values();
        let typed_query_values = self.request_query_values()?;
        let typed_header_values = self.request_header_values()?;
        let typed_body_value: RequestBodyValue = self.request_body_value().await?;

        let mut path_values = serde_json::Map::new();

        for field in typed_path_values.0.fields.into_iter() {
            path_values.insert(field.name, field.value);
        }

        for field in typed_query_values.0.fields.into_iter() {
            path_values.insert(field.name, field.value);
        }

        let merged_request_path_and_query = Value::Object(path_values);

        let mut header_records = serde_json::Map::new();

        for field in typed_header_values.0.fields.iter() {
            header_records.insert(field.name.clone(), field.value.clone());
        }

        let header_value = Value::Object(header_records);

        let mut basic = serde_json::Map::from_iter(vec![
            ("path".to_string(), merged_request_path_and_query),
            ("body".to_string(), typed_body_value.0),
            ("headers".to_string(), header_value),
        ]);

        let custom = self.request_custom_params.clone().unwrap_or_default();

        for (key, value) in custom.iter() {
            basic.insert(key.clone(), value.clone());
        }

        Ok(Value::Object(basic))
    }

    /// will consume the underlying request body
    pub async fn resolve_rib_input_value(
        &mut self,
        required_types: &RibInputTypeInfo,
    ) -> Result<RibInput, RibInputTypeMismatch> {

        let request_type_info = required_types.types.get("request");

        let rib_input_with_request_content = &self.as_value().await.map_err(|e|
            RibInputTypeMismatch(format!("Could not retrieve request details: {e}"))
        )?;

        match request_type_info {
            Some(request_type) => {
                tracing::debug!("received: {:?}", rib_input_with_request_content);
                let input = TypeAnnotatedValue::parse_with_type(rib_input_with_request_content, request_type)
                        .map_err(|err| RibInputTypeMismatch(format!("Input request details don't match the requirements for rib expression to execute: {}. Requirements. {:?}", err.join(", "), request_type)))?;
                let input = input.try_into().map_err(|err| {
                    RibInputTypeMismatch(format!(
                        "Internal error converting between value representations: {err}"
                    ))
                })?;

                let mut rib_input_map = HashMap::new();
                rib_input_map.insert("request".to_string(), input);
                Ok(RibInput {
                    input: rib_input_map,
                })
            }
            None => Ok(RibInput::default()),
        }
    }


    // pub fn empty() -> HttpRequestDetails {
    //     HttpRequestDetails {
    //         scheme: Scheme::HTTP,
    //         host: ApiSiteString("".to_string()),
    //         request_method: Method::GET,
    //         api_input_path: ApiInputPath {
    //             base_path: "".to_string(),
    //             query_path: None,
    //         },
    //         request_path_params: RequestPathValues(JsonKeyValues::default()),
    //         request_body_value: RequestBody(Value::Null),
    //         request_query_params: RequestQueryValues(JsonKeyValues::default()),
    //         request_headers: RequestHeaderValues(JsonKeyValues::default()),
    //         http_middlewares: None,
    //         request_custom_params: None,
    //     }
    // }

    // pub fn get_api_input_path(&self) -> String {
    //     self.api_input_path.to_string()
    // }

    // pub fn get_access_token_from_cookie(&self) -> Option<String> {
    //     self.request_headers
    //         .0
    //         .fields
    //         .iter()
    //         .find(|field| field.name == "Cookie" || field.name == "cookie")
    //         .and_then(|field| field.value.as_str().map(|x| x.to_string()))
    //         .and_then(|cookie_header| {
    //             let parts: Vec<&str> = cookie_header.split(';').collect();
    //             let access_token_part = parts.iter().find(|part| part.contains("access_token"));
    //             access_token_part.and_then(|part| {
    //                 let token_parts: Vec<&str> = part.split('=').collect();
    //                 if token_parts.len() == 2 {
    //                     Some(token_parts[1].to_string())
    //                 } else {
    //                     None
    //                 }
    //             })
    //         })
    // }

    // pub fn get_id_token_from_cookie(&self) -> Option<String> {
    //     self.request_headers
    //         .0
    //         .fields
    //         .iter()
    //         .find(|field| field.name == "Cookie" || field.name == "cookie")
    //         .and_then(|field| field.value.as_str().map(|x| x.to_string()))
    //         .and_then(|cookie_header| {
    //             let parts: Vec<&str> = cookie_header.split(';').collect();
    //             let id_token_part = parts.iter().find(|part| part.contains("id_token"));
    //             id_token_part.and_then(|part| {
    //                 let token_parts: Vec<&str> = part.split('=').collect();
    //                 if token_parts.len() == 2 {
    //                     Some(token_parts[1].to_string())
    //                 } else {
    //                     None
    //                 }
    //             })
    //         })
    // }

    pub fn get_cookie_jar(&self) -> &CookieJar {
        self.underlying.cookie()
    }

    // pub fn get_accept_content_type_header(&self) -> Option<String> {
    //     self.request_headers
    //         .0
    //         .fields
    //         .iter()
    //         .find(|field| field.name == http::header::ACCEPT.to_string())
    //         .and_then(|field| field.value.as_str().map(|x| x.to_string()))
    // }

    // pub fn from_input_http_request(
    //     scheme: &Scheme,
    //     host: &ApiSiteString,
    //     method: Method,
    //     api_input_path: &ApiInputPath,
    //     path_params: &HashMap<VarInfo, String>,
    //     query_variable_values: &HashMap<String, String>,
    //     query_variable_names: &[QueryInfo],
    //     request_body: &Value,
    //     all_headers: HeaderMap,
    //     http_middlewares: &Option<HttpMiddlewares>,
    // ) -> Result<Self, Vec<String>> {
    //     let request_body = RequestBody::from(request_body)?;
    //     let path_params = RequestPathValues::from(path_params);
    //     let query_params = RequestQueryValues::from(query_variable_values, query_variable_names)?;
    //     let header_params = RequestHeaderValues::from(&all_headers)?;

    //     Ok(Self {
    //         scheme: scheme.clone(),
    //         host: host.clone(),
    //         request_method: method,
    //         api_input_path: api_input_path.clone(),
    //         request_path_params: path_params,
    //         request_body_value: request_body,
    //         request_query_params: query_params,
    //         request_headers: header_params,
    //         http_middlewares: http_middlewares.clone(),
    //         request_custom_params: None,
    //     })
    // }
}

fn query_components_from_str(query_path: &str) -> HashMap<String, String> {
    let mut query_components: HashMap<String, String> = HashMap::new();
    let query_parts = query_path.split('&').map(|x| x.trim());

    for part in query_parts {
        let key_value: Vec<&str> = part.split('=').map(|x| x.trim()).collect();

        if let (Some(key), Some(value)) = (key_value.first(), key_value.get(1)) {
            query_components.insert(key.to_string(), value.to_string());
        }
    }

    query_components
}

#[derive(Clone, Debug, Default)]
pub struct RequestPathValues(pub JsonKeyValues);

impl RequestPathValues {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0
            .fields
            .iter()
            .find(|field| field.name == key)
            .map(|field| &field.value)
    }

    pub fn from(path_variables: HashMap<VarInfo, String>) -> RequestPathValues {
        let record_fields: Vec<JsonKeyValue> = path_variables
            .into_iter()
            .map(|(key, value)| JsonKeyValue {
                name: key.key_name,
                value: internal::refine_json_str_value(value),
            })
            .collect();

        RequestPathValues(JsonKeyValues {
            fields: record_fields,
        })
    }
}

#[derive(Debug, Clone)]
pub struct RequestQueryValues(pub JsonKeyValues);

impl RequestQueryValues {
    pub fn from(
        query_key_values: &HashMap<String, String>,
        query_keys: &[QueryInfo],
    ) -> Result<RequestQueryValues, Vec<String>> {
        let mut unavailable_query_variables: Vec<String> = vec![];
        let mut query_variable_map: JsonKeyValues = JsonKeyValues::default();

        for spec_query_variable in query_keys.iter() {
            let key = &spec_query_variable.key_name;
            if let Some(query_value) = query_key_values.get(key) {
                let typed_value = internal::refine_json_str_value(query_value);
                query_variable_map.push(key.clone(), typed_value);
            } else {
                unavailable_query_variables.push(spec_query_variable.to_string());
            }
        }

        if unavailable_query_variables.is_empty() {
            Ok(RequestQueryValues(query_variable_map))
        } else {
            Err(unavailable_query_variables)
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestHeaderValues(pub JsonKeyValues);

impl RequestHeaderValues {
    pub fn from(headers: &HeaderMap) -> Result<RequestHeaderValues, Vec<String>> {
        let mut headers_map: JsonKeyValues = JsonKeyValues::default();

        for (header_name, header_value) in headers {
            let header_value_str = header_value.to_str().map_err(|err| vec![err.to_string()])?;

            let typed_header_value = internal::refine_json_str_value(header_value_str);

            headers_map.push(header_name.to_string(), typed_header_value);
        }

        Ok(RequestHeaderValues(headers_map))
    }
}

#[derive(Debug, Clone)]
pub struct RequestBodyValue(pub Value);

#[derive(Clone, Debug, Default)]
pub struct JsonKeyValues {
    pub fields: Vec<JsonKeyValue>,
}

impl JsonKeyValues {
    pub fn push(&mut self, key: String, value: Value) {
        self.fields.push(JsonKeyValue { name: key, value });
    }
}

#[derive(Clone, Debug)]
pub struct JsonKeyValue {
    pub name: String,
    pub value: Value,
}

mod internal {
    use rib::{CoercedNumericValue, LiteralValue};
    use serde_json::Value;

    pub(crate) fn refine_json_str_value(value: impl AsRef<str>) -> Value {
        let primitive = LiteralValue::from(value.as_ref().to_string());
        match primitive {
            LiteralValue::Num(number) => match number {
                CoercedNumericValue::PosInt(value) => {
                    Value::Number(serde_json::Number::from(value))
                }
                CoercedNumericValue::NegInt(value) => {
                    Value::Number(serde_json::Number::from(value))
                }
                CoercedNumericValue::Float(value) => {
                    Value::Number(serde_json::Number::from_f64(value).unwrap())
                }
            },
            LiteralValue::String(value) => Value::String(value),
            LiteralValue::Bool(value) => Value::Bool(value),
        }
    }
}

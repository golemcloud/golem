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
use crate::gateway_middleware::HttpMiddlewares;
use crate::gateway_request::http_request::ApiInputPath;
use golem_common::SafeDisplay;
use http::uri::Scheme;
use http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;
use url::Url;

// https://github.com/golemcloud/golem/issues/1069
#[derive(Clone, Debug)]
pub enum GatewayRequestDetails {
    Http(HttpRequestDetails),
}
impl GatewayRequestDetails {
    // Form the HttpRequestDetails based on what's required by
    // ApiDefinition. If there are query or path parameters that are not required
    // by API definition, they will be discarded here.
    // If there is a need to fetch any query values or path values that are required
    // in the workflow but not through API definition, use poem::Request directly
    // as it will be better performing in the hot path
    pub fn from(
        scheme: &Option<Scheme>,
        host: &ApiSiteString,
        api_input_path: &ApiInputPath,
        path_params: &HashMap<VarInfo, String>,
        query_variable_values: &HashMap<String, String>,
        query_variable_names: &[QueryInfo],
        request_body: &Value,
        headers: HeaderMap,
        middlewares: &Option<HttpMiddlewares>,
    ) -> Result<Self, Vec<String>> {
        Ok(Self::Http(HttpRequestDetails::from_input_http_request(
            scheme,
            host,
            api_input_path,
            path_params,
            query_variable_values,
            query_variable_names,
            request_body,
            headers,
            middlewares,
        )?))
    }
}

// A structure that holds the incoming request details
// along with parameters that are required by the route in API definition
// Any extra query parameter values in the incoming request will not be available
// in query or path values. If we need to retrieve them at any stage, the original
// api_input_path is still available.
#[derive(Debug, Clone)]
pub struct HttpRequestDetails {
    pub scheme: Option<Scheme>,
    pub host: ApiSiteString,
    pub api_input_path: ApiInputPath,
    pub request_path_params: RequestPathValues,
    pub request_body_value: RequestBody,
    pub request_query_params: RequestQueryValues,
    pub request_headers: RequestHeaderValues,
    pub http_middlewares: Option<HttpMiddlewares>,
    pub request_custom_params: Option<HashMap<String, Value>>,
}

impl HttpRequestDetails {
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

    pub fn as_json(&self) -> Value {
        let typed_path_values = self.request_path_params.clone().0;
        let typed_query_values = self.request_query_params.clone().0;

        let mut path_values = serde_json::Map::new();

        for field in typed_path_values.fields.iter() {
            path_values.insert(field.name.clone(), field.value.clone());
        }

        for field in typed_query_values.fields.iter() {
            path_values.insert(field.name.clone(), field.value.clone());
        }

        let merged_request_path_and_query = Value::Object(path_values);

        let mut header_records = serde_json::Map::new();

        for field in self.request_headers.0.fields.iter() {
            header_records.insert(field.name.clone(), field.value.clone());
        }

        let header_value = Value::Object(header_records);

        let mut basic = serde_json::Map::from_iter(vec![
            ("path".to_string(), merged_request_path_and_query),
            ("body".to_string(), self.request_body_value.0.clone()),
            ("headers".to_string(), header_value),
        ]);

        let custom = self.request_custom_params.clone().unwrap_or_default();

        for (key, value) in custom.iter() {
            basic.insert(key.clone(), value.clone());
        }

        Value::Object(basic)
    }

    pub fn url(&self) -> Result<Url, String> {
        let url_str = if let Some(scheme) = &self.scheme {
            format!(
                "{}://{}{}",
                scheme,
                &self.host,
                &self.api_input_path.to_string()
            )
        } else {
            format!("{}{}", &self.host, &self.api_input_path.to_string())
        };

        Url::parse(&url_str).map_err(|err| err.to_string())
    }

    pub fn empty() -> HttpRequestDetails {
        HttpRequestDetails {
            scheme: Some(Scheme::HTTP),
            host: ApiSiteString("".to_string()),
            api_input_path: ApiInputPath {
                base_path: "".to_string(),
                query_path: None,
            },
            request_path_params: RequestPathValues(JsonKeyValues::default()),
            request_body_value: RequestBody(Value::Null),
            request_query_params: RequestQueryValues(JsonKeyValues::default()),
            request_headers: RequestHeaderValues(JsonKeyValues::default()),
            http_middlewares: None,
            request_custom_params: None,
        }
    }

    pub fn get_api_input_path(&self) -> String {
        self.api_input_path.to_string()
    }

    pub fn get_access_token_from_cookie(&self) -> Option<String> {
        self.request_headers
            .0
            .fields
            .iter()
            .find(|field| field.name == "Cookie" || field.name == "cookie")
            .and_then(|field| field.value.as_str().map(|x| x.to_string()))
            .and_then(|cookie_header| {
                let parts: Vec<&str> = cookie_header.split(';').collect();
                let access_token_part = parts.iter().find(|part| part.contains("access_token"));
                access_token_part.and_then(|part| {
                    let token_parts: Vec<&str> = part.split('=').collect();
                    if token_parts.len() == 2 {
                        Some(token_parts[1].to_string())
                    } else {
                        None
                    }
                })
            })
    }

    pub fn get_id_token_from_cookie(&self) -> Option<String> {
        self.request_headers
            .0
            .fields
            .iter()
            .find(|field| field.name == "Cookie" || field.name == "cookie")
            .and_then(|field| field.value.as_str().map(|x| x.to_string()))
            .and_then(|cookie_header| {
                let parts: Vec<&str> = cookie_header.split(';').collect();
                let id_token_part = parts.iter().find(|part| part.contains("id_token"));
                id_token_part.and_then(|part| {
                    let token_parts: Vec<&str> = part.split('=').collect();
                    if token_parts.len() == 2 {
                        Some(token_parts[1].to_string())
                    } else {
                        None
                    }
                })
            })
    }

    pub fn get_cookie_values(&self) -> HashMap<&str, &str> {
        let mut hash_map = HashMap::new();

        for json_key_value in &self.request_headers.0.fields {
            let field_name = &json_key_value.name;

            if field_name == "cookie" || field_name == "Cookie" {
                if let Some(cookie_header) = json_key_value.value.as_str() {
                    let parts: Vec<&str> = cookie_header.split(';').collect();
                    for part in parts {
                        let key_value: Vec<&str> = part.split('=').collect();
                        if let (Some(key), Some(value)) = (key_value.first(), key_value.get(1)) {
                            hash_map.insert(key.trim(), value.trim());
                        }
                    }
                }
            }
        }

        hash_map
    }

    pub fn get_accept_content_type_header(&self) -> Option<String> {
        self.request_headers
            .0
            .fields
            .iter()
            .find(|field| field.name == http::header::ACCEPT.to_string())
            .and_then(|field| field.value.as_str().map(|x| x.to_string()))
    }

    pub fn from_input_http_request(
        scheme: &Option<Scheme>,
        host: &ApiSiteString,
        api_input_path: &ApiInputPath,
        path_params: &HashMap<VarInfo, String>,
        query_variable_values: &HashMap<String, String>,
        query_variable_names: &[QueryInfo],
        request_body: &Value,
        all_headers: HeaderMap,
        http_middlewares: &Option<HttpMiddlewares>,
    ) -> Result<Self, Vec<String>> {
        let request_body = RequestBody::from(request_body)?;
        let path_params = RequestPathValues::from(path_params);
        let query_params = RequestQueryValues::from(query_variable_values, query_variable_names)?;
        let header_params = RequestHeaderValues::from(&all_headers)?;

        Ok(Self {
            scheme: scheme.clone(),
            host: host.clone(),
            api_input_path: api_input_path.clone(),
            request_path_params: path_params,
            request_body_value: request_body,
            request_query_params: query_params,
            request_headers: header_params,
            http_middlewares: http_middlewares.clone(),
            request_custom_params: None,
        })
    }
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

    fn from(path_variables: &HashMap<VarInfo, String>) -> RequestPathValues {
        let record_fields: Vec<JsonKeyValue> = path_variables
            .iter()
            .map(|(key, value)| JsonKeyValue {
                name: key.key_name.clone(),
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
    fn from(
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
pub struct RequestHeaderValues(JsonKeyValues);
impl RequestHeaderValues {
    fn from(headers: &HeaderMap) -> Result<RequestHeaderValues, Vec<String>> {
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
pub struct RequestBody(Value);

impl RequestBody {
    fn from(request_body: &Value) -> Result<RequestBody, Vec<String>> {
        Ok(RequestBody(request_body.clone()))
    }
}

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

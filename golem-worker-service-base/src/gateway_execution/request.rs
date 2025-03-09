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

use super::gateway_session::{DataKey, GatewaySessionStore, SessionId};
use crate::gateway_api_definition::http::{QueryInfo, VarInfo};
use crate::gateway_binding::{GatewayBindingCompiled, ResolvedRouteEntry};
use crate::gateway_middleware::HttpMiddlewares;
use crate::gateway_request::http_request::router::PathParamExtractor;
use bytes::Bytes;
use golem_common::SafeDisplay;
use http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;
use urlencoding::decode;

const COOKIE_HEADER_NAMES: [&str; 2] = ["cookie", "Cookie"];

/// Thin wrapper around a poem::Request that is used to evaluate all binding types when coming from an http gateway.
pub struct RichRequest {
    pub underlying: poem::Request,
    pub path_segments: Vec<String>,
    pub path_param_extractors: Vec<PathParamExtractor>,
    pub query_info: Vec<QueryInfo>,
    pub auth_data: Option<Value>,
}

impl RichRequest {
    pub fn query_params(&self) -> HashMap<String, String> {
        self.underlying
            .uri()
            .query()
            .map(query_components_from_str)
            .unwrap_or_default()
    }

    pub async fn add_auth_details(
        &mut self,
        session_id: &SessionId,
        gateway_session_store: &GatewaySessionStore,
    ) -> Result<(), String> {
        let claims = gateway_session_store
            .get(session_id, &DataKey::claims())
            .await
            .map_err(|err| err.to_safe_string())?;

        self.auth_data = Some(claims.0);

        Ok(())
    }

    fn path_and_query(&self) -> Result<String, String> {
        self.underlying
            .uri()
            .path_and_query()
            .map(|paq| paq.to_string())
            .ok_or("No path and query provided".to_string())
    }

    fn request_path_values(&self) -> RequestPathValues {
        use crate::gateway_request::http_request::router;

        let path_param_values: HashMap<VarInfo, String> = self
            .path_param_extractors
            .iter()
            .map(|param| match param {
                router::PathParamExtractor::Single { var_info, index } => {
                    (var_info.clone(), self.path_segments[*index].clone())
                }
                router::PathParamExtractor::AllFollowing { var_info, index } => {
                    let value = self.path_segments[*index..].join("/");
                    (var_info.clone(), value)
                }
            })
            .collect();

        RequestPathValues::from(path_param_values)
    }

    fn request_query_values(&self) -> Result<RequestQueryValues, String> {
        RequestQueryValues::from(&self.query_params(), &self.query_info)
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

    fn as_basic_json_hashmap(&self) -> Result<serde_json::Map<String, Value>, String> {
        let typed_path_values = self.request_path_values();
        let typed_query_values = self.request_query_values()?;
        let typed_header_values = self.request_header_values()?;

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
            ("headers".to_string(), header_value),
        ]);

        if let Some(auth_data) = self.auth_data.as_ref() {
            basic.insert("auth".to_string(), auth_data.clone());
        };

        Ok(basic)
    }

    pub fn as_json(&self) -> Result<Value, String> {
        Ok(Value::Object(self.as_basic_json_hashmap()?))
    }

    /// consumes the body of the underlying request
    pub async fn as_json_with_body(&mut self) -> Result<Value, String> {
        let mut basic = self.as_basic_json_hashmap()?;
        let body = self.request_body_value().await?;

        basic.insert("body".to_string(), body.0);

        Ok(Value::Object(basic))
    }

    /// consumes the body of the underlying request
    pub async fn as_wasi_http_input(
        &mut self,
    ) -> Result<golem_common::virtual_exports::http_incoming_handler::IncomingHttpRequest, String>
    {
        use golem_common::virtual_exports::http_incoming_handler as hic;

        let headers = {
            let mut acc = Vec::new();
            for (header_name, header_value) in self.underlying.headers().iter() {
                let header_bytes: Vec<u8> = header_value.as_bytes().into();
                acc.push((header_name.clone().to_string(), Bytes::from(header_bytes)));
            }
            hic::HttpFields(acc)
        };

        let body_bytes = self
            .underlying
            .take_body()
            .into_bytes()
            .await
            .map_err(|e| format!("Failed reading request body: ${e}"))?;

        let body = hic::HttpBodyAndTrailers {
            content: hic::HttpBodyContent(body_bytes),
            trailers: None,
        };

        let authority = authority_from_request(&self.underlying)?;

        let path_and_query = self.path_and_query()?;

        Ok(hic::IncomingHttpRequest {
            scheme: self.underlying.scheme().clone().into(),
            authority,
            path_and_query,
            method: hic::HttpMethod::from_http_method(self.underlying.method().into()),
            headers,
            body: Some(body),
        })
    }

    pub fn get_cookie_values(&self) -> HashMap<&str, &str> {
        let mut result = HashMap::new();

        for header_name in COOKIE_HEADER_NAMES.iter() {
            if let Some(value) = self.underlying.header(header_name) {
                let parts: Vec<&str> = value.split(';').collect();
                for part in parts {
                    let key_value: Vec<&str> = part.split('=').collect();
                    if let (Some(key), Some(value)) = (key_value.first(), key_value.get(1)) {
                        result.insert(key.trim(), value.trim());
                    }
                }
            }
        }

        result
    }
}

pub struct SplitResolvedRouteEntryResult<Namespace> {
    pub namespace: Namespace,
    pub binding: GatewayBindingCompiled,
    pub middlewares: Option<HttpMiddlewares>,
    pub rich_request: RichRequest,
}

pub fn split_resolved_route_entry<Namespace>(
    request: poem::Request,
    entry: ResolvedRouteEntry<Namespace>,
) -> SplitResolvedRouteEntryResult<Namespace> {
    // helper function to save a few clones

    let namespace = entry.route_entry.namespace;
    let binding = entry.route_entry.binding;
    let middlewares = entry.route_entry.middlewares;

    let rich_request = RichRequest {
        underlying: request,
        path_segments: entry.path_segments,
        path_param_extractors: entry.route_entry.path_params,
        query_info: entry.route_entry.query_params,
        auth_data: None,
    };

    SplitResolvedRouteEntryResult {
        namespace,
        binding,
        middlewares,
        rich_request,
    }
}

fn query_components_from_str(query_path: &str) -> HashMap<String, String> {
    let mut query_components: HashMap<String, String> = HashMap::new();
    let query_parts = query_path.split('&').map(|x| x.trim());

    for part in query_parts {
        let key_value: Vec<&str> = part.split('=').map(|x| x.trim()).collect();

        if let (Some(key), Some(value)) = (key_value.first(), key_value.get(1)) {
            decode(value)
                .map(|decoded_value| {
                    query_components.insert(key.to_string(), decoded_value.to_string())
                })
                .unwrap_or_else(|_| query_components.insert(key.to_string(), value.to_string()));
        }
    }

    query_components
}

pub fn authority_from_request(request: &poem::Request) -> Result<String, String> {
    request
        .header(http::header::HOST)
        .map(|h| h.to_string())
        .ok_or("No host header provided".to_string())
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

    fn from(path_variables: HashMap<VarInfo, String>) -> RequestPathValues {
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

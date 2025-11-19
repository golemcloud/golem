// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::gateway_binding_resolver::ResolvedRouteEntry;
use super::gateway_session_store::{DataKey, GatewaySessionStore, SessionId};
use crate::gateway_router::PathParamExtractor;
use bytes::Bytes;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::SafeDisplay;
use golem_service_base::worker_api::compiled_gateway_binding::GatewayBindingCompiled;
use golem_service_base::worker_api::http_middlewares::HttpMiddlewares;
use golem_service_base::worker_api::path_pattern::QueryInfo;
use http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

const COOKIE_HEADER_NAMES: [&str; 2] = ["cookie", "Cookie"];

/// Thin wrapper around a poem::Request that is used to evaluate all binding types when coming from an http gateway.
pub struct RichRequest {
    pub underlying: poem::Request,
    pub request_id: Uuid,
    path_segments: Vec<String>,
    path_param_extractors: Vec<PathParamExtractor>,
    auth_data: Option<Value>,
    cached_request_body: Value,
}

impl RichRequest {
    pub fn new(underlying: poem::Request) -> RichRequest {
        RichRequest {
            underlying,
            request_id: Uuid::new_v4(),
            path_segments: vec![],
            path_param_extractors: vec![],
            auth_data: None,
            cached_request_body: serde_json::Value::Null,
        }
    }

    pub fn auth_data(&self) -> Option<&Value> {
        self.auth_data.as_ref()
    }

    pub fn headers(&self) -> &HeaderMap {
        self.underlying.headers()
    }

    pub fn query_params(&self) -> HashMap<String, String> {
        self.underlying
            .uri()
            .query()
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn add_auth_details(
        &mut self,
        session_id: &SessionId,
        gateway_session_store: &Arc<dyn GatewaySessionStore>,
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

    pub fn path_params(&self) -> HashMap<String, String> {
        self.path_param_extractors
            .iter()
            .map(|param| match param {
                PathParamExtractor::Single { var_info, index } => (
                    var_info.key_name.clone(),
                    self.path_segments[*index].clone(),
                ),
                PathParamExtractor::AllFollowing { var_info, index } => {
                    let value = self.path_segments[*index..].join("/");
                    (var_info.key_name.clone(), value)
                }
            })
            .collect()
    }

    pub async fn request_body(&mut self) -> Result<&Value, String> {
        self.take_request_body().await?;

        Ok(self.cached_request_body())
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

    fn cached_request_body(&self) -> &Value {
        &self.cached_request_body
    }

    /// Consumes the body of the underlying request, and make it as part of RichRequest as `cached_request_body`.
    /// The following logic is subtle enough that it takes the following into consideration:
    /// 99% of the time, number of separate rib scripts in API definition that needs to look up request body is 1,
    /// and for that rib-script, there will be no extra logic to read the request body in the hot path.
    /// At the same, if by any chance, multiple rib scripts exist (within a request) that require to lookup the request body, `take_request_body`
    /// is idempotent, that it doesn't affect correctness.
    /// We intentionally don't consume the body if its not required in any Rib script.
    async fn take_request_body(&mut self) -> Result<(), String> {
        let body = self.underlying.take_body();

        if !body.is_empty() {
            match body.into_json().await {
                Ok(json_request_body) => {
                    self.cached_request_body = json_request_body;
                }
                Err(err) => {
                    tracing::error!("Failed reading http request body as json: {}", err);
                    return Err(format!("Request body parse error: {err}"))?;
                }
            }
        };

        Ok(())
    }
}

pub struct SplitResolvedRouteEntryResult {
    pub binding: GatewayBindingCompiled,
    pub middlewares: Option<HttpMiddlewares>,
    pub rich_request: RichRequest,
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
}

pub fn split_resolved_route_entry(
    request: poem::Request,
    entry: ResolvedRouteEntry,
) -> SplitResolvedRouteEntryResult {
    // helper function to save a few clones

    let binding = entry.route_entry.binding;
    let middlewares = entry.route_entry.middlewares;
    let account_id = entry.route_entry.account_id;
    let environment_id = entry.route_entry.environment_id;

    let rich_request = RichRequest {
        underlying: request,
        request_id: Uuid::new_v4(),
        path_segments: entry.path_segments,
        path_param_extractors: entry.route_entry.path_params,
        auth_data: None,
        cached_request_body: Value::Null,
    };

    SplitResolvedRouteEntryResult {
        binding,
        middlewares,
        rich_request,
        account_id,
        environment_id,
    }
}

pub fn authority_from_request(request: &poem::Request) -> Result<String, String> {
    request
        .header(http::header::HOST)
        .map(|h| h.to_string())
        .ok_or("No host header provided".to_string())
}

#[derive(Debug, Clone)]
pub struct RequestQueryValues(pub HashMap<String, String>);

impl RequestQueryValues {
    pub fn from(
        query_key_values: &HashMap<String, String>,
        query_keys: &[QueryInfo],
    ) -> Result<RequestQueryValues, Vec<String>> {
        let mut unavailable_query_variables: Vec<String> = vec![];
        let mut query_variable_map: HashMap<String, String> = HashMap::new();

        for spec_query_variable in query_keys.iter() {
            let key = &spec_query_variable.key_name;
            if let Some(query_value) = query_key_values.get(key) {
                query_variable_map.insert(key.clone(), query_value.to_string());
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
pub struct RequestHeaderValues(pub HashMap<String, String>);

impl RequestHeaderValues {
    pub fn from(headers: &HeaderMap) -> Result<RequestHeaderValues, Vec<String>> {
        let mut headers_map: HashMap<String, String> = HashMap::new();

        for (header_name, header_value) in headers {
            let header_value_str = header_value.to_str().map_err(|err| vec![err.to_string()])?;

            headers_map.insert(header_name.to_string(), header_value_str.to_string());
        }

        Ok(RequestHeaderValues(headers_map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem::http::Uri;
    use test_r::test;

    #[test]
    fn test_query_params_with_plus_encoded_spaces() -> anyhow::Result<()> {
        let uri: Uri = "/search?q=hello+world&lang=rust".parse()?;
        let req = poem::Request::builder().uri(uri).finish();
        let rich_req = RichRequest::new(req);
        let params = rich_req.query_params();

        assert_eq!(params.get("q"), Some(&"hello world".to_string())); // '+' decoded to space
        assert_eq!(params.get("lang"), Some(&"rust".to_string()));

        Ok(())
    }
}

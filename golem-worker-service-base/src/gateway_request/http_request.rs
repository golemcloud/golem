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

use crate::gateway_api_deployment::ApiSiteString;
use http::header::HOST;
use http::uri::Scheme;
use http::StatusCode;
use hyper::http::{HeaderMap, Method};
use poem::{Body, Response};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Display;
use tracing::error;

#[derive(Clone, Debug)]
pub struct InputHttpRequest {
    pub scheme: Option<Scheme>,
    pub host: ApiSiteString,
    pub api_input_path: ApiInputPath,
    pub headers: HeaderMap,
    pub req_method: Method,
    pub req_body: Value,
}

#[derive(Debug)]
pub struct ErrorResponse(Response);

impl From<ErrorResponse> for Response {
    fn from(value: ErrorResponse) -> Self {
        value.0
    }
}

impl InputHttpRequest {
    pub async fn from_request(request: poem::Request) -> Result<InputHttpRequest, ErrorResponse> {
        let (req_parts, body) = request.into_parts();
        let headers = req_parts.headers;
        let uri = req_parts.uri;
        let scheme = uri.scheme();

        let host = match headers.get(HOST).and_then(|h| h.to_str().ok()) {
            Some(host) => ApiSiteString(host.to_string()),
            None => {
                return Err(ErrorResponse(
                    Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::from_string("Missing host".to_string())),
                ));
            }
        };

        let json_request_body: Value = if body.is_empty() {
            Value::Null
        } else {
            match body.into_json().await {
                Ok(json_request_body) => json_request_body,
                Err(err) => {
                    error!("API request host: {} - error: {}", host, err);
                    return Err(ErrorResponse(
                        Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .body(Body::from_string("Request body parse error".to_string())),
                    ));
                }
            }
        };

        Ok(InputHttpRequest {
            scheme: scheme.cloned(),
            host,
            api_input_path: ApiInputPath {
                base_path: uri.path().to_string(),
                query_path: uri.query().map(|x| x.to_string()),
            },
            headers,
            req_method: req_parts.method,
            req_body: json_request_body,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ApiInputPath {
    pub base_path: String,
    pub query_path: Option<String>,
}

impl Display for ApiInputPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.query_path {
            Some(ref query_path) => write!(f, "{}?{}", self.base_path, query_path),
            None => write!(f, "{}", self.base_path),
        }
    }
}

impl ApiInputPath {
    // Return the value of each query variable in a HashMap
    pub fn query_components(&self) -> Option<HashMap<String, String>> {
        if let Some(query_path) = self.query_path.clone() {
            let query_components = Self::query_components_from_str(&query_path);

            Some(query_components)
        } else {
            None
        }
    }

    pub fn query_components_from_str(query_path: &str) -> HashMap<String, String> {
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
}

pub mod router {
    use crate::gateway_api_definition::http::CompiledRoute;
    use crate::gateway_api_definition::http::{PathPattern, QueryInfo, VarInfo};
    use crate::gateway_binding::GatewayBindingCompiled;
    use crate::gateway_execution::router::{Router, RouterPattern};
    use crate::gateway_middleware::HttpMiddlewares;

    #[derive(Debug, Clone)]
    pub enum PathParamExtractor {
        Single { var_info: VarInfo, index: usize },
        AllFollowing { var_info: VarInfo, index: usize },
    }

    #[derive(Debug, Clone)]
    pub struct RouteEntry<Namespace> {
        // size is the index of all path patterns.
        pub path_params: Vec<PathParamExtractor>,
        pub query_params: Vec<QueryInfo>,
        pub namespace: Namespace,
        pub binding: GatewayBindingCompiled,
        pub middlewares: Option<HttpMiddlewares>,
    }

    pub fn build<Namespace>(
        routes: Vec<(Namespace, CompiledRoute)>,
    ) -> Router<RouteEntry<Namespace>> {
        let mut router = Router::new();

        for (namespace, route) in routes {
            let method = route.method.into();
            let path = route.path;
            let binding = route.binding;

            let path_params = path
                .path_patterns
                .iter()
                .enumerate()
                .filter_map(|(i, x)| match x {
                    PathPattern::Var(var_info) => Some(PathParamExtractor::Single {
                        var_info: var_info.clone(),
                        index: i,
                    }),
                    PathPattern::CatchAllVar(var_info) => Some(PathParamExtractor::AllFollowing {
                        var_info: var_info.clone(),
                        index: i,
                    }),
                    _ => None,
                })
                .collect();

            let entry = RouteEntry {
                path_params,
                query_params: path.query_params,
                namespace,
                binding,
                middlewares: route.middlewares,
            };

            let path: Vec<RouterPattern> = path
                .path_patterns
                .iter()
                .map(|x| x.clone().into())
                .collect();

            router.add_route(method, path, entry);
        }

        router
    }
}

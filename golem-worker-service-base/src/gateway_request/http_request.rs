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

use poem::Response;
use std::collections::HashMap;
use std::fmt::Display;

#[derive(Debug)]
pub struct ErrorResponse(pub Response);

impl From<ErrorResponse> for Response {
    fn from(value: ErrorResponse) -> Self {
        value.0
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

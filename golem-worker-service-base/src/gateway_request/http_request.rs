use std::collections::HashMap;

use crate::gateway_api_definition::ApiSiteString;
use hyper::http::{HeaderMap, Method};
use serde_json::Value;

#[derive(Clone)]
pub struct InputHttpRequest {
    pub input_path: ApiInputPath,
    pub headers: HeaderMap,
    pub req_method: Method,
    pub req_body: Value,
}

impl InputHttpRequest {
    pub fn get_host(&self) -> Option<ApiSiteString> {
        self.headers
            .get("host")
            .and_then(|host| host.to_str().ok())
            .map(|host_str| ApiSiteString(host_str.to_string()))
    }
}

#[derive(Clone)]
pub struct ApiInputPath {
    pub base_path: String,
    pub query_path: Option<String>,
}

impl ApiInputPath {
    // Return the value of each query variable in a HashMap
    pub fn query_components(&self) -> Option<HashMap<String, String>> {
        if let Some(query_path) = self.query_path.clone() {
            let mut query_components: HashMap<String, String> = HashMap::new();
            let query_parts = query_path.split('&').map(|x| x.trim());

            for part in query_parts {
                let key_value: Vec<&str> = part.split('=').map(|x| x.trim()).collect();

                if let (Some(key), Some(value)) = (key_value.first(), key_value.get(1)) {
                    query_components.insert(key.to_string(), value.to_string());
                }
            }
            Some(query_components)
        } else {
            None
        }
    }
}

pub mod router {
    use crate::gateway_api_definition::http::CompiledRoute;
    use crate::gateway_api_definition::http::{PathPattern, QueryInfo, VarInfo};
    use crate::gateway_binding::GatewayBindingCompiled;
    use crate::gateway_execution::router::{Router, RouterPattern};

    #[derive(Debug, Clone)]
    pub struct RouteEntry<Namespace> {
        // size is the index of all path patterns.
        pub path_params: Vec<(VarInfo, usize)>,
        pub query_params: Vec<QueryInfo>,
        pub namespace: Namespace,
        pub binding: GatewayBindingCompiled,
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
                    PathPattern::Var(var_info) => Some((var_info.clone(), i)),
                    _ => None,
                })
                .collect();

            let entry = RouteEntry {
                path_params,
                query_params: path.query_params,
                namespace,
                binding,
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

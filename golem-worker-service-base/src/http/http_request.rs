use std::collections::HashMap;

use crate::api_definition::ApiSiteString;
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
    use crate::{
        api_definition::http::{PathPattern, QueryInfo, Route, VarInfo},
        http::router::{Router, RouterPattern},
        worker_binding::GolemWorkerBinding,
    };

    #[derive(Debug, Clone)]
    pub struct RouteEntry {
        // size is the index of all path patterns.
        pub path_params: Vec<(VarInfo, usize)>,
        pub query_params: Vec<QueryInfo>,
        pub binding: GolemWorkerBinding,
    }

    pub fn build(routes: Vec<Route>) -> Router<RouteEntry> {
        let mut router = Router::new();

        for route in routes {
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

#[cfg(test)]
mod tests {

    use http::{HeaderMap, HeaderName, HeaderValue, Method};

    use golem_common::model::ComponentId;

    use crate::api_definition::http::HttpApiDefinition;
    use crate::http::http_request::{ApiInputPath, InputHttpRequest};
    use crate::worker_binding::WorkerBindingResolver;
    use crate::worker_bridge_execution::WorkerRequest;

    #[test]
    fn test_worker_request_resolution() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);
        let function_params = "[\"a\", \"b\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![
                serde_json::Value::String("a".to_string()),
                serde_json::Value::String("b".to_string()),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_resolution_with_concrete_params() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

        let function_params = "[\"${{x : 'y'}}\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let mut expected_map = serde_json::Map::new();

        expected_map.insert("x".to_string(), serde_json::Value::String("y".to_string()));

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![serde_json::Value::Object(expected_map)],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_resolution_with_path_params() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

        let function_params = "[\"${{x : request.path.user-id}}\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let mut expected_map = serde_json::Map::new();

        expected_map.insert(
            "x".to_string(),
            serde_json::Value::Number(serde_json::Number::from(1)),
        );

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![serde_json::Value::Object(expected_map)],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_resolution_with_path_and_query_params() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request(
            "foo/1",
            Some("token-id=2"),
            &empty_headers,
            serde_json::Value::Null,
        );

        let function_params = "[\"${request.path.user-id}\", \"${request.path.token-id}\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let mut expected_map = serde_json::Map::new();

        expected_map.insert(
            "x".to_string(),
            serde_json::Value::Number(serde_json::Number::from(1)),
        );

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![
                serde_json::Value::Number(serde_json::Number::from(1)),
                serde_json::Value::Number(serde_json::Number::from(2)),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_resolution_with_path_and_query_body_params() {
        let mut request_body_amp = serde_json::Map::new();

        request_body_amp.insert(
            "age".to_string(),
            serde_json::Value::Number(serde_json::Number::from(10)),
        );

        let empty_headers = HeaderMap::new();
        let api_request = get_api_request(
            "foo/1",
            Some("token-id=2"),
            &empty_headers,
            serde_json::Value::Object(request_body_amp),
        );

        let function_params =
            "[\"${request.path.user-id}\", \"${request.path.token-id}\",  \"age-${request.body.age}\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![
                serde_json::Value::Number(serde_json::Number::from(1)),
                serde_json::Value::Number(serde_json::Number::from(2)),
                serde_json::Value::String("age-10".to_string()),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_resolution_with_record_params() {
        let mut request_body_amp = serde_json::Map::new();

        request_body_amp.insert(
            "age".to_string(),
            serde_json::Value::Number(serde_json::Number::from(10)),
        );

        let mut headers = HeaderMap::new();

        headers.insert(
            HeaderName::from_static("username"),
            HeaderValue::from_static("foo"),
        );

        let api_request = get_api_request(
            "foo/1",
            Some("token-id=2"),
            &headers,
            serde_json::Value::Object(request_body_amp),
        );

        let arg1 = "${{ user-id : request.path.user-id }}";
        let arg2 = "${request.path.token-id}";
        let arg3 = "age-${request.body.age}";
        let arg4 = "${{user-name : request.headers.username}}";

        let function_params = format!("[\"{}\", \"{}\", \"{}\", \"{}\"]", arg1, arg2, arg3, arg4);

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let mut user_id_map = serde_json::Map::new();

        user_id_map.insert(
            "user-id".to_string(),
            serde_json::Value::Number(serde_json::Number::from(1)),
        );

        let mut user_name_map = serde_json::Map::new();

        user_name_map.insert(
            "user-name".to_string(),
            serde_json::Value::String("foo".to_string()),
        );

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![
                serde_json::Value::Object(user_id_map),
                serde_json::Value::Number(serde_json::Number::from(2)),
                serde_json::Value::String("age-10".to_string()),
                serde_json::Value::Object(user_name_map),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_cond_expr_resolution() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/2", None, &empty_headers, serde_json::Value::Null);
        let function_params = "[\"a\", \"b\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![
                serde_json::Value::String("a".to_string()),
                serde_json::Value::String("b".to_string()),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_request_body_resolution() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::String("address".to_string()),
        );

        let function_params = "[\"${request.body}\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![serde_json::Value::String("address".to_string())],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_resolution_for_predicate_gives_bool() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::String("address".to_string()),
        );

        let function_params = "[\"${1 == 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![serde_json::Value::Bool(true)],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_resolution_for_predicate_gives_bool_greater() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::String("address".to_string()),
        );

        let function_params = "[\"${2 > 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![serde_json::Value::Bool(true)],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_resolution_for_cond_expr_fn_params() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::String("address".to_string()),
        );

        let function_params = "[\"${if (2 < 1) then 0 else 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![serde_json::Value::Number(serde_json::Number::from(1))],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_resolution_for_cond_expr_req_body_fn_params() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Object(serde_json::Map::from_iter(vec![(
                "number".to_string(),
                serde_json::Value::Number(serde_json::Number::from(10)),
            )])),
        );

        let function_params = "[\"${if (request.body.number < 11) then 0 else 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![serde_json::Value::Number(serde_json::Number::from(0))],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_resolution_for_cond_expr_req_body_direct_fn_params() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Number(serde_json::Number::from(10)),
        );

        let function_params = "[\"${if (request.body < 11) then request.path.user-id else 1}\", \"${if (request.body < 5) then ${request.path.user-id} else 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![
                serde_json::Value::Number(serde_json::Number::from(2)),
                serde_json::Value::Number(serde_json::Number::from(1)),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_map_request_body_resolution() {
        let empty_headers = HeaderMap::new();

        let mut request_body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            serde_json::Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            serde_json::Value::String("bar_value".to_string()),
        );

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Object(request_body),
        );

        let foo_key = "${request.body.foo_key}";
        let bar_key = "${request.body.bar_key}";

        let function_params = format!("[\"{}\", \"{}\"]", foo_key, bar_key);

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![
                serde_json::Value::String("foo_value".to_string()),
                serde_json::Value::String("bar_value".to_string()),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_map_list_request_body_resolution() {
        let empty_headers = HeaderMap::new();

        let mut request_body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            serde_json::Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String("bar_value".to_string())]),
        );

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Object(request_body),
        );

        let foo_key = "${request.body.foo_key}";
        let bar_key = "${request.body.bar_key[0]}";

        let function_params = format!("[\"{}\", \"{}\"]", foo_key, bar_key);

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![
                serde_json::Value::String("foo_value".to_string()),
                serde_json::Value::String("bar_value".to_string()),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_request_body_direct() {
        let empty_headers = HeaderMap::new();

        let mut request_body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            serde_json::Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String("bar_value".to_string())]),
        );

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Object(request_body.clone()),
        );

        let foo_key: &str = "${request.body}";

        let function_params = format!("[\"{}\"]", foo_key);

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![serde_json::Value::Object(request_body.clone())],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_with_request_header_resolution() {
        let mut headers = HeaderMap::new();

        headers.append(
            HeaderName::from_static("token"),
            HeaderValue::from_static("token_value"),
        );

        let mut request_body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            serde_json::Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String("bar_value".to_string())]),
        );

        let api_request = get_api_request(
            "/foo/2",
            None,
            &headers,
            serde_json::Value::Object(request_body),
        );

        let foo_key = "${request.body.foo_key}";
        let bar_key = "${request.body.bar_key[0]}";
        let token_key = "${request.headers.token}";

        let function_params = format!("[\"{}\", \"{}\", \"{}\"]", foo_key, bar_key, token_key);

        let api_specification: HttpApiDefinition = get_api_spec(
            "/foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = resolved_route.worker_request;

        let expected = WorkerRequest {
            component_id: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_name: "shopping-cart-1".to_string(),
            function_name: "golem:it/api/get-cart-contents".to_string(),
            function_params: vec![
                serde_json::Value::String("foo_value".to_string()),
                serde_json::Value::String("bar_value".to_string()),
                serde_json::Value::String("token_value".to_string()),
            ],
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_worker_request_resolution_paths() {
        fn test_paths(definition_path: &str, request_path: &str, ok: bool) {
            let empty_headers = HeaderMap::new();
            let api_request =
                get_api_request(request_path, None, &empty_headers, serde_json::Value::Null);

            let function_params = "[]";

            let api_specification: HttpApiDefinition = get_api_spec(
                definition_path,
                "shopping-cart-${request.path.cart-id}",
                function_params,
            );

            let resolved_route = api_request.resolve(&api_specification);

            let result = resolved_route.map(|x| x.worker_request);

            assert_eq!(result.is_ok(), ok);
        }

        test_paths("getcartcontent/{cart-id}", "/noexist", false);
        test_paths("/getcartcontent/{cart-id}", "noexist", false);
        test_paths("getcartcontent/{cart-id}", "noexist", false);
        test_paths("/getcartcontent/{cart-id}", "/noexist", false);
        test_paths("getcartcontent/{cart-id}", "/getcartcontent/1", true);
        test_paths("/getcartcontent/{cart-id}", "getcartcontent/1", true);
        test_paths("getcartcontent/{cart-id}", "getcartcontent/1", true);
        test_paths("/getcartcontent/{cart-id}", "/getcartcontent/1", true);
    }

    fn get_api_request(
        base_path: &str,
        query_path: Option<&str>,
        headers: &HeaderMap,
        req_body: serde_json::Value,
    ) -> InputHttpRequest {
        InputHttpRequest {
            input_path: ApiInputPath {
                base_path: base_path.to_string(),
                query_path: query_path.map(|x| x.to_string()),
            },
            headers: headers.clone(),
            req_method: Method::GET,
            req_body,
        }
    }

    fn get_api_spec(
        path_pattern: &str,
        worker_name: &str,
        function_params: &str,
    ) -> HttpApiDefinition {
        let yaml_string = format!(
            r#"
          id: users-api
          version: 0.0.1
          routes:
          - method: Get
            path: {}
            binding:
              type: wit-worker
              component: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
              workerId: '{}'
              functionName: golem:it/api/get-cart-contents
              functionParams: {}
        "#,
            path_pattern, worker_name, function_params
        );

        serde_yaml::from_str(yaml_string.as_str()).unwrap()
    }
}

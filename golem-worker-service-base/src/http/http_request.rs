use std::collections::HashMap;

use crate::api_definition::ApiSiteString;
use hyper::http::{HeaderMap, Method};
use serde_json::Value;

use crate::api_definition::http::{HttpApiDefinition, QueryInfo, VarInfo};
use crate::worker_binding::{WorkerBindingResolver};

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

mod internal {
    use crate::api_definition::http::{QueryInfo, VarInfo};
    use crate::http::http_request::internal;
    use crate::merge::Merge;
    use crate::primitive::{Number, Primitive};
    use golem_service_base::type_inference::infer_analysed_type;
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::json::get_typed_value_from_json;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use http::HeaderMap;
    use serde_json::Value;
    use std::collections::HashMap;

    pub(crate) fn get_typed_value_from_primitive(value: impl Into<String>) -> TypeAnnotatedValue {
        let query_value = Primitive::from(value.into());
        match query_value {
            Primitive::Num(number) => match number {
                Number::PosInt(value) => TypeAnnotatedValue::U64(value),
                Number::NegInt(value) => TypeAnnotatedValue::S64(value),
                Number::Float(value) => TypeAnnotatedValue::F64(value),
            },
            Primitive::String(value) => TypeAnnotatedValue::Str(value),
            Primitive::Bool(value) => TypeAnnotatedValue::Bool(value),
        }
    }

    #[derive(Clone, Debug)]
    pub struct RecordField {
        pub name: String,
        pub typ: AnalysedType,
        pub value: TypeAnnotatedValue,
    }

    impl RecordField {
        pub(crate) fn merge_all(records: Vec<RecordField>) -> TypeAnnotatedValue {
            let mut typ: Vec<(String, AnalysedType)> = vec![];
            let mut value: Vec<(String, TypeAnnotatedValue)> = vec![];

            for record in records {
                typ.push((record.name.clone(), record.typ));
                value.push((record.name, record.value));
            }

            TypeAnnotatedValue::Record { typ, value }
        }
    }

    pub(crate) fn get_request_body(request_body: &Value) -> Result<RecordField, Vec<String>> {
        let inferred_type = infer_analysed_type(request_body);
        let typed_value = get_typed_value_from_json(request_body, &inferred_type)?;

        Ok(RecordField {
            name: "body".into(),
            typ: inferred_type,
            value: typed_value,
        })
    }

    pub(crate) fn get_headers(headers: &HeaderMap) -> Result<RecordField, Vec<String>> {
        let mut headers_map: Vec<(String, TypeAnnotatedValue)> = vec![];

        for (header_name, header_value) in headers {
            let header_value_str = header_value.to_str().map_err(|err| vec![err.to_string()])?;

            let typed_header_value = internal::get_typed_value_from_primitive(header_value_str);

            headers_map.push((header_name.to_string(), typed_header_value));
        }

        let type_annotated_value = TypeAnnotatedValue::Record {
            typ: headers_map
                .iter()
                .map(|(key, v)| (key.clone(), AnalysedType::from(v)))
                .collect(),
            value: headers_map,
        };
        Ok(RecordField {
            name: "header".into(),
            typ: AnalysedType::from(&type_annotated_value),
            value: type_annotated_value,
        })
    }

    pub(crate) fn get_request_path_query_values(
        request_query_variables: HashMap<String, String>,
        spec_query_variables: &[QueryInfo],
        path_variables: HashMap<VarInfo, &str>,
    ) -> Result<RecordField, Vec<String>> {
        let mut request_query_values =
            get_request_query_values(request_query_variables, spec_query_variables)?;

        let request_path_values = get_request_path_values(path_variables);

        request_query_values.merge(&request_path_values);

        let merged = request_query_values;

        Ok(RecordField {
            name: "path".into(),
            typ: AnalysedType::from(&merged),
            value: merged,
        })
    }

    fn get_request_path_values(path_variables: HashMap<VarInfo, &str>) -> TypeAnnotatedValue {
        let value: Vec<(String, TypeAnnotatedValue)> = path_variables
            .into_iter()
            .map(|(key, value)| (key.key_name, get_typed_value_from_primitive(value)))
            .collect();

        let typ = value
            .iter()
            .map(|(key, v)| (key.clone(), AnalysedType::from(v)))
            .collect();

        TypeAnnotatedValue::Record { typ, value }
    }

    fn get_request_query_values(
        request_query_variables: HashMap<String, String>,
        spec_query_variables: &[QueryInfo],
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let mut unavailable_query_variables: Vec<String> = vec![];
        let mut query_variable_map: Vec<(String, TypeAnnotatedValue)> = vec![];

        for spec_query_variable in spec_query_variables.iter() {
            let key = &spec_query_variable.key_name;
            if let Some(query_value) = request_query_variables.get(key) {
                let typed_value = internal::get_typed_value_from_primitive(query_value);
                query_variable_map.push((key.clone(), typed_value));
            } else {
                unavailable_query_variables.push(spec_query_variable.to_string());
            }
        }

        if unavailable_query_variables.is_empty() {
            let type_annotated_value = TypeAnnotatedValue::Record {
                typ: query_variable_map
                    .iter()
                    .map(|(key, v)| (key.clone(), AnalysedType::from(v)))
                    .collect(),
                value: query_variable_map.clone(),
            };
            Ok(type_annotated_value)
        } else {
            Err(unavailable_query_variables)
        }
    }
}

#[cfg(test)]
mod tests {

    use http::{HeaderMap, HeaderName, HeaderValue, Method};

    use golem_common::model::ComponentId;

    use crate::api_definition::http::HttpApiDefinition;
    use crate::http::http_request::{ApiInputPath, InputHttpRequest};
    use crate::worker_bridge_execution::WorkerRequest;

    use super::*;

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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("a".to_string()),
                serde_json::Value::String("b".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let mut expected_map = serde_json::Map::new();

        expected_map.insert("x".to_string(), serde_json::Value::String("y".to_string()));

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Object(
                expected_map,
            )]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let mut expected_map = serde_json::Map::new();

        expected_map.insert(
            "x".to_string(),
            serde_json::Value::Number(serde_json::Number::from(1)),
        );

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Object(
                expected_map,
            )]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let mut expected_map = serde_json::Map::new();

        expected_map.insert(
            "x".to_string(),
            serde_json::Value::Number(serde_json::Number::from(1)),
        );

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::Number(serde_json::Number::from(1)),
                serde_json::Value::Number(serde_json::Number::from(2)),
            ]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::Number(serde_json::Number::from(1)),
                serde_json::Value::Number(serde_json::Number::from(2)),
                serde_json::Value::String("age-10".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
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
        let arg4 = "${{user-name : request.header.username}}";

        let function_params = format!("[\"{}\", \"{}\", \"{}\", \"{}\"]", arg1, arg2, arg3, arg4);

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

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
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::Object(user_id_map),
                serde_json::Value::Number(serde_json::Number::from(2)),
                serde_json::Value::String("age-10".to_string()),
                serde_json::Value::Object(user_name_map),
            ]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("a".to_string()),
                serde_json::Value::String("b".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::String(
                "address".to_string(),
            )]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Bool(true)]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Bool(true)]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(1),
            )]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(0),
            )]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::Number(serde_json::Number::from(2)),
                serde_json::Value::Number(serde_json::Number::from(1)),
            ]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("foo_value".to_string()),
                serde_json::Value::String("bar_value".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("foo_value".to_string()),
                serde_json::Value::String("bar_value".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
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

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Object(
                request_body.clone(),
            )]),
        };

        assert_eq!(result, Ok(expected));
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
        let token_key = "${request.header.token}";

        let function_params = format!("[\"{}\", \"{}\", \"{}\"]", foo_key, bar_key, token_key);

        let api_specification: HttpApiDefinition = get_api_spec(
            "/foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            component: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<ComponentId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("foo_value".to_string()),
                serde_json::Value::String("bar_value".to_string()),
                serde_json::Value::String("token_value".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
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

            let result = match resolved_route {
                Some(resolved_route) => WorkerRequest::from_resolved_route(resolved_route)
                    .map_err(|err| err.to_string()),
                None => Err("not found".to_string()),
            };

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
        worker_id: &str,
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
            path_pattern, worker_id, function_params
        );

        serde_yaml::from_str(yaml_string.as_str()).unwrap()
    }
}

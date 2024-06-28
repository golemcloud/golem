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
    use async_trait::async_trait;
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::json::get_json_from_typed_value;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use http::{HeaderMap, HeaderName, HeaderValue, Method};
    use serde_json::Value;
    use std::sync::Arc;

    use golem_common::model::IdempotencyKey;
    use golem_service_base::model::{
        ComponentMetadata, Export, ExportFunction, ExportInstance, FunctionResult, WorkerId,
    };

    use crate::api_definition::http::HttpApiDefinition;
    use crate::evaluator::getter::Getter;
    use crate::evaluator::path::Path;
    use crate::evaluator::{
        DefaultEvaluator, EvaluationError, Evaluator, ExprEvaluationResult, MetadataFetchError,
        WorkerMetadataFetcher, FQN,
    };
    use crate::http::http_request::{ApiInputPath, InputHttpRequest};
    use crate::merge::Merge;
    use crate::primitive::GetPrimitive;
    use crate::worker_binding::{RequestDetails, WorkerBindingResolver};
    use crate::worker_bridge_execution::to_response::ToResponse;
    use crate::worker_bridge_execution::{
        WorkerRequest, WorkerRequestExecutor, WorkerRequestExecutorError, WorkerResponse,
    };

    struct TestWorkerRequestExecutor {}

    #[async_trait]
    impl WorkerRequestExecutor for TestWorkerRequestExecutor {
        // This test executor simply returns the worker request details itself to a type-annotated-value
        async fn execute(
            &self,
            resolved_worker_request: WorkerRequest,
        ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
            let function_result_type = FunctionResult {
                name: None,
                typ: AnalysedType::Str.into(),
            };

            let response = convert_to_worker_response(&resolved_worker_request);

            let response_dummy = TypeAnnotatedValue::Tuple {
                typ: vec![AnalysedType::from(&response)],
                value: vec![response],
            };

            Ok(WorkerResponse::new(
                response_dummy,
                vec![function_result_type],
            ))
        }
    }

    fn convert_to_worker_response(worker_request: &WorkerRequest) -> TypeAnnotatedValue {
        let mut required = TypeAnnotatedValue::Record {
            typ: vec![
                ("component_id".to_string(), AnalysedType::Str),
                ("name".to_string(), AnalysedType::Str),
                ("function_name".to_string(), AnalysedType::Str),
            ],
            value: vec![
                (
                    "component_id".to_string(),
                    TypeAnnotatedValue::Str(worker_request.component_id.0.to_string()),
                ),
                (
                    "name".to_string(),
                    TypeAnnotatedValue::Str(worker_request.worker_name.clone()),
                ),
                (
                    "function_name".to_string(),
                    TypeAnnotatedValue::Str(worker_request.function_name.to_string()),
                ),
                (
                    "function_params".to_string(),
                    TypeAnnotatedValue::Tuple {
                        typ: worker_request
                            .function_params
                            .iter()
                            .map(AnalysedType::from)
                            .collect(),
                        value: worker_request.function_params.clone(),
                    },
                ),
            ],
        };

        let optional_idempotency_key =
            worker_request
                .clone()
                .idempotency_key
                .map(|x| TypeAnnotatedValue::Record {
                    // Idempotency key can exist in header of the request in which case users can refer to it as
                    // request.headers.idempotency-key. In order to keep some consistency, we are keeping the same key name here,
                    // if it exists as part of the API definition
                    typ: vec![("idempotency-key".to_string(), AnalysedType::Str)],
                    value: vec![(
                        "idempotency-key".to_string(),
                        TypeAnnotatedValue::Str(x.to_string()),
                    )],
                });

        if let Some(idempotency_key) = optional_idempotency_key {
            required = required.merge(&idempotency_key).clone();
        }

        required
    }

    fn get_test_evaluator() -> Arc<dyn Evaluator + Sync + Send> {
        Arc::new(DefaultEvaluator::from_worker_request_executor(Arc::new(
            TestWorkerRequestExecutor {},
        )))
    }

    struct TestMetadataFetcher {
        test_fqn: FQN,
    }

    #[async_trait]
    impl WorkerMetadataFetcher for TestMetadataFetcher {
        async fn get_worker_metadata(
            &self,
            _worker_id: &WorkerId,
        ) -> Result<ComponentMetadata, MetadataFetchError> {
            Ok(ComponentMetadata {
                exports: vec![Export::Instance(ExportInstance {
                    name: self
                        .test_fqn
                        .clone()
                        .parsed_function_name
                        .site()
                        .interface_name()
                        .unwrap(),
                    functions: vec![ExportFunction {
                        name: self
                            .test_fqn
                            .parsed_function_name
                            .function()
                            .function_name()
                            .clone(),
                        parameters: vec![],
                        results: vec![],
                    }],
                })],
                producers: vec![],
                memories: vec![],
            })
        }
    }

    fn get_test_metadata_fetcher(
        function_name: &str,
    ) -> Arc<dyn WorkerMetadataFetcher + Sync + Send> {
        Arc::new(TestMetadataFetcher {
            test_fqn: FQN::try_from(function_name).unwrap(),
        })
    }

    #[derive(Debug)]
    struct TestResponse {
        worker_name: String,
        function_name: String,
        function_params: Value,
    }

    impl ToResponse<TestResponse> for ExprEvaluationResult {
        fn to_response(&self, _request_details: &RequestDetails) -> TestResponse {
            let function_name = self
                .get_value()
                .map(|x| x.get(&Path::from_key("function_name")).unwrap())
                .unwrap()
                .get_primitive()
                .unwrap()
                .as_string();

            let function_params = {
                let params = self
                    .get_value()
                    .map(|x| x.get(&Path::from_key("function_params")).unwrap())
                    .unwrap();
                get_json_from_typed_value(&params)
            };

            let worker_name = self
                .get_value()
                .map(|x| x.get(&Path::from_key("name")).unwrap())
                .unwrap()
                .get_primitive()
                .unwrap()
                .as_string();

            TestResponse {
                worker_name,
                function_name,
                function_params,
            }
        }
    }

    impl ToResponse<TestResponse> for EvaluationError {
        fn to_response(&self, _request_details: &RequestDetails) -> TestResponse {
            panic!("{}", self.to_string())
        }
    }

    impl ToResponse<TestResponse> for MetadataFetchError {
        fn to_response(&self, _request_details: &RequestDetails) -> TestResponse {
            panic!("{}", self.to_string())
        }
    }

    async fn execute(
        api_request: &InputHttpRequest,
        api_specification: &HttpApiDefinition,
    ) -> TestResponse {
        let evaluator = get_test_evaluator();
        let worker_metadata_fetcher = get_test_metadata_fetcher("golem:it/api.{get-cart-contents}");

        let resolved_route = api_request
            .resolve(vec![api_specification.clone()])
            .await
            .unwrap();

        resolved_route
            .execute_with(&evaluator, &worker_metadata_fetcher)
            .await
    }

    #[tokio::test]
    async fn test_end_to_end_evaluation_simple() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);
        let expression = r#"let response = golem:it/api.{get-cart-contents}("a", "b"); response"#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${request.path.user-id}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let result = (test_response.function_name, test_response.function_params);

        let expected = (
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![
                Value::String("a".to_string()),
                Value::String("b".to_string()),
            ]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_resolution_with_concrete_params() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

        let expression = r#"
          let response = golem:it/api.{get-cart-contents}({x : "y"});
          response
        "#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${request.path.user-id}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let mut expected_map = serde_json::Map::new();

        expected_map.insert("x".to_string(), Value::String("y".to_string()));

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart-1".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![Value::Object(expected_map)]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_resolution_with_path_params() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

        let expression = r#"
          let response = golem:it/api.{get-cart-contents}({x : request.path.user-id});
          response
        "#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${request.path.user-id}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let mut expected_map = serde_json::Map::new();

        expected_map.insert("x".to_string(), Value::Number(serde_json::Number::from(1)));

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart-1".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![Value::Object(expected_map)]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_resolution_with_path_and_query_params() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request(
            "foo/1",
            Some("token-id=2"),
            &empty_headers,
            serde_json::Value::Null,
        );

        let expression = r#"let response = golem:it/api.{get-cart-contents}(request.path.user-id, request.path.token-id); response"#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart-1".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![
                Value::Number(serde_json::Number::from(1)),
                Value::Number(serde_json::Number::from(2)),
            ]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_resolution_with_path_and_query_body_params() {
        let mut request_body_amp = serde_json::Map::new();

        request_body_amp.insert(
            "age".to_string(),
            Value::Number(serde_json::Number::from(10)),
        );

        let empty_headers = HeaderMap::new();
        let api_request = get_api_request(
            "foo/1",
            Some("token-id=2"),
            &empty_headers,
            serde_json::Value::Object(request_body_amp),
        );

        let expression = r#"
          let response = golem:it/api.{get-cart-contents}(request.path.user-id, request.path.token-id, "age-${request.body.age}");
          response
        "#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart-1".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![
                Value::Number(serde_json::Number::from(1)),
                Value::Number(serde_json::Number::from(2)),
                Value::String("age-10".to_string()),
            ]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_resolution_with_record_params() {
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
            Value::Object(request_body_amp),
        );

        let expression = r#"
          let response = golem:it/api.{get-cart-contents}({ user-id : request.path.user-id }, request.path.token-id, "age-${request.body.age}", {user-name : request.headers.username});
          response
        "#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let mut user_id_map = serde_json::Map::new();

        user_id_map.insert(
            "user-id".to_string(),
            Value::Number(serde_json::Number::from(1)),
        );

        let mut user_name_map = serde_json::Map::new();

        user_name_map.insert("user-name".to_string(), Value::String("foo".to_string()));

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart-1".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![
                Value::Object(user_id_map),
                Value::Number(serde_json::Number::from(2)),
                Value::String("age-10".to_string()),
                Value::Object(user_name_map),
            ]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_cond_expr_resolution() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/2", None, &empty_headers, Value::Null);
        let expression = r#"let response = golem:it/api.{get-cart-contents}("a", "b"); response"#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart-1".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![
                Value::String("a".to_string()),
                Value::String("b".to_string()),
            ]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_request_body_resolution() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            Value::String("address".to_string()),
        );

        let expression =
            r#"let response = golem:it/api.{get-cart-contents}(request.body); response"#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart-1".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![Value::String("address".to_string())]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_resolution_for_predicate_gives_bool() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            Value::String("address".to_string()),
        );

        let expression = r#"let response = golem:it/api.{get-cart-contents}(1 == 1); response"#;

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", expression);

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![Value::Bool(true)]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_resolution_for_predicate_gives_bool_greater() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            Value::String("address".to_string()),
        );

        let expression = r#"let response = golem:it/api.{get-cart-contents}(2 > 1); response"#;

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", expression);

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![Value::Bool(true)]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_resolution_for_cond_expr_fn_params() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            Value::String("address".to_string()),
        );

        let expression = r#"
          let response = golem:it/api.{get-cart-contents}(if (2 < 1) then 0 else 1);
          response
        "#;

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", expression);

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![Value::Number(serde_json::Number::from(1))]),
        );

        assert_eq!(result, expected);
    }
    //
    #[tokio::test]
    async fn test_worker_resolution_for_cond_expr_req_body_fn_params() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            Value::Object(serde_json::Map::from_iter(vec![(
                "number".to_string(),
                Value::Number(serde_json::Number::from(10)),
            )])),
        );

        let expression = r#"
          let response = golem:it/api.{get-cart-contents}(if (request.body.number < 11) then 0 else 1);
          response
        "#;

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", expression);

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![Value::Number(serde_json::Number::from(0))]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_resolution_for_cond_expr_req_body_direct_fn_params() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            Value::Object(serde_json::Map::from_iter(vec![(
                "number".to_string(),
                Value::Number(serde_json::Number::from(10)),
            )])),
        );

        let expression = r#"
          let condition1 = if (request.body.number < 11) then request.path.user-id else 1;
          let condition2 = if (request.body.number < 5) then request.path.user-id else 1;
          let response = golem:it/api.{get-cart-contents}(condition1, condition2);
          response
        "#;

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", expression);

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![
                Value::Number(serde_json::Number::from(2)),
                Value::Number(serde_json::Number::from(1)),
            ]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_map_list_request_body_resolution() {
        let empty_headers = HeaderMap::new();

        let mut request_body: serde_json::Map<String, Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            Value::Array(vec![Value::String("bar_value".to_string())]),
        );

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Object(request_body),
        );

        let expression = r#"
          let param1 = request.body.foo_key;
          let param2 = request.body.bar_key[0];
          let response = golem:it/api.{get-cart-contents}(param1, param2);
          response
        "#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart-1".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![
                Value::String("foo_value".to_string()),
                Value::String("bar_value".to_string()),
            ]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_request_body_direct() {
        let empty_headers = HeaderMap::new();

        let mut request_body: serde_json::Map<String, Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            Value::Array(vec![Value::String("bar_value".to_string())]),
        );

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            Value::Object(request_body.clone()),
        );

        let expression = r#"
          let param = request.body;
          let response = golem:it/api.{get-cart-contents}(param);
          response
        "#;

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if request.path.user-id>100 then 0 else 1}",
            expression,
        );

        let test_response = execute(&api_request, &api_specification).await;

        let result = (
            test_response.worker_name,
            test_response.function_name,
            test_response.function_params,
        );

        let expected = (
            "shopping-cart-1".to_string(),
            "golem:it/api.{get-cart-contents}".to_string(),
            Value::Array(vec![Value::Object(request_body)]),
        );

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_worker_request_resolution_paths() {
        async fn test_paths(definition_path: &str, request_path: &str, ok: bool) {
            let empty_headers = HeaderMap::new();
            let api_request =
                get_api_request(request_path, None, &empty_headers, serde_json::Value::Null);

            let function_params = "[]";

            let api_specification: HttpApiDefinition = get_api_spec(
                definition_path,
                "shopping-cart-${request.path.cart-id}",
                function_params,
            );

            let resolved_route = api_request.resolve(vec![api_specification]).await;

            let result = resolved_route.map(|x| x.worker_detail);

            assert_eq!(result.is_ok(), ok);
        }

        test_paths("getcartcontent/{cart-id}", "/noexist", false).await;
        test_paths("/getcartcontent/{cart-id}", "noexist", false).await;
        test_paths("getcartcontent/{cart-id}", "noexist", false).await;
        test_paths("/getcartcontent/{cart-id}", "/noexist", false).await;
        test_paths("getcartcontent/{cart-id}", "/getcartcontent/1", true).await;
        test_paths("/getcartcontent/{cart-id}", "getcartcontent/1", true).await;
        test_paths("getcartcontent/{cart-id}", "getcartcontent/1", true).await;
        test_paths("/getcartcontent/{cart-id}", "/getcartcontent/1", true).await;
    }

    #[tokio::test]
    async fn test_worker_idempotency_key_header() {
        async fn test_key(header_map: &HeaderMap, idempotency_key: Option<IdempotencyKey>) {
            let api_request = get_api_request("/getcartcontent/1", None, header_map, Value::Null);

            let expression = r#"
            let param = request.body;
            let response = golem:it/api/get-cart-contents();
            response
            "#;

            let api_specification: HttpApiDefinition = get_api_spec(
                "getcartcontent/{cart-id}",
                "shopping-cart-${request.path.cart-id}",
                expression,
            );

            let resolved_route = api_request.resolve(vec![api_specification]).await.unwrap();

            assert_eq!(
                resolved_route.worker_detail.idempotency_key,
                idempotency_key
            );
        }

        test_key(&HeaderMap::new(), None).await;
        let mut headers = HeaderMap::new();
        headers.insert("Idempotency-Key", HeaderValue::from_str("foo").unwrap());
        test_key(&headers, Some(IdempotencyKey::new("foo".to_string()))).await;
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", HeaderValue::from_str("bar").unwrap());
        test_key(&headers, Some(IdempotencyKey::new("bar".to_string()))).await;
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
        rib_expression: &str,
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
              componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
              workerName: '{}'
              response: '${{{}}}'

        "#,
            path_pattern, worker_name, rib_expression
        );

        let http_api_definition: HttpApiDefinition =
            serde_yaml::from_str(yaml_string.as_str()).unwrap();
        http_api_definition
    }
}

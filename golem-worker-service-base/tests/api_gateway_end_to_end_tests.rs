// Copyright 2024 Golem Cloud
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

use std::sync::Arc;
use test_r::test;

test_r::enable!();

use golem_service_base::auth::DefaultNamespace;

use crate::gateway_api_definition::http::{CompiledHttpApiDefinition, HttpApiDefinition};

use crate::internal::TestResponse;
use chrono::{DateTime, Utc};
use golem_common::model::IdempotencyKey;
use golem_worker_service_base::gateway_execution::auth_call_back_binding_handler::DefaultAuthCallBack;
use golem_worker_service_base::gateway_execution::gateway_binding_resolver::GatewayBindingResolver;
use golem_worker_service_base::gateway_execution::gateway_input_executor::{
    DefaultGatewayInputExecutor, GatewayInputExecutor, Input,
};
use golem_worker_service_base::gateway_execution::gateway_session::GatewaySessionStore;
use golem_worker_service_base::gateway_middleware::Cors;
use golem_worker_service_base::gateway_request::http_request::{ApiInputPath, InputHttpRequest};
use golem_worker_service_base::gateway_security::{
    DefaultIdentityProviderResolver, Provider, SecurityScheme, SecuritySchemeIdentifier,
};
use golem_worker_service_base::{api, gateway_api_definition};
use http::{HeaderMap, HeaderValue, Method};
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use serde_json::Value;

// The tests that focus on end to end workflow of API Gateway, without involving any real workers,
// and stays independent of other modules.
// Workflow: Given an API request and an API specification,
// execute the API request and return the TestResponse (instead of poem::Response)
// Similar to types having ToResponse<poem::Response>
// there are instances of ToResponse<TestResponse> for them in the internal module of tests.
// Example: RibResult has an instance of `ToResponse<TestResponse>`.
// The tests skips validation and transformations done at the service side.
async fn execute(
    api_request: &InputHttpRequest,
    api_specification: &HttpApiDefinition,
) -> TestResponse {
    let compiled = CompiledHttpApiDefinition::from_http_api_definition(
        api_specification,
        &internal::get_component_metadata(),
        &DefaultNamespace::default(),
    )
    .unwrap();

    let resolved_gateway_binding = api_request
        .resolve_gateway_binding(vec![compiled])
        .await
        .unwrap();

    let test_executor = DefaultGatewayInputExecutor::new(
        internal::get_test_rib_interpreter(),
        internal::get_test_file_server_binding_handler(),
        Arc::new(DefaultAuthCallBack),
    );

    let input = Input::new(
        &resolved_gateway_binding,
        &GatewaySessionStore::in_memory(),
        Arc::new(DefaultIdentityProviderResolver),
    );

    let poem_response: poem::Response = test_executor.execute_binding(&input).await;
    TestResponse::from_poem_response(poem_response, &resolved_gateway_binding).await
}

#[test]
async fn test_end_to_end_api_gateway_simple_worker() {
    let empty_headers = HeaderMap::new();
    let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

    let worker_name = r#"
      let id: u64 = request.path.user-id;
      "shopping-cart-${id}"
    "#;

    let response_mapping = r#"
      let response = golem:it/api.{get-cart-contents}("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_spec_worker_binding("foo/{user-id}", worker_name, response_mapping).await;

    let test_response = execute(&api_request, &api_specification).await;

    let result = (
        test_response.get_function_name().unwrap(),
        test_response.get_function_params().unwrap(),
    );

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        Value::Array(vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_end_to_end_api_gateway_with_security() {
    let empty_headers = HeaderMap::new();
    let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

    let worker_name = r#"
      let id: u64 = request.path.user-id;
      "shopping-cart-${id}"
    "#;

    let response_mapping = r#"
      let response = golem:it/api.{get-cart-contents}("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_spec_worker_binding_with_security("foo/{user-id}", worker_name, response_mapping)
            .await;

    let test_response = execute(&api_request, &api_specification).await;

    let result = (
        test_response.get_function_name().unwrap(),
        test_response.get_function_params().unwrap(),
    );

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        Value::Array(vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_end_to_end_api_gateway_with_multiple_security() {
    let empty_headers = HeaderMap::new();
    let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

    let worker_name = r#"
      let id: u64 = request.path.user-id;
      "shopping-cart-${id}"
    "#;

    let response_mapping = r#"
      let response = golem:it/api.{get-cart-contents}("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_spec_worker_binding_with_multiple_securities(
            "foo/{user-id}",
            worker_name,
            response_mapping,
        )
        .await;

    let test_response = execute(&api_request, &api_specification).await;

    dbg!(test_response.clone());

    let result = (
        test_response.get_function_name().unwrap(),
        test_response.get_function_params().unwrap(),
    );

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        Value::Array(vec![
            Value::String("a".to_string()),
            Value::String("a".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_end_to_end_api_gateway_cors_preflight() {
    let empty_headers = HeaderMap::new();
    let api_request =
        get_preflight_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

    let cors = Cors::from_parameters(
        Some("http://example.com".to_string()),
        Some("GET, POST, PUT, DELETE, OPTIONS".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some(true),
        Some(3600),
    )
    .unwrap();

    let api_specification: HttpApiDefinition =
        get_api_spec_cors_preflight_binding("foo/{user-id}", &cors).await;

    let test_response = execute(&api_request, &api_specification).await;

    let result = test_response.get_cors_preflight().unwrap();
    assert_eq!(result, cors);
}

#[test]
async fn test_end_to_end_api_gateway_cors_preflight_default() {
    let empty_headers = HeaderMap::new();
    let api_request =
        get_preflight_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

    let api_specification: HttpApiDefinition =
        get_api_spec_cors_preflight_binding_default_response("foo/{user-id}").await;

    let test_response = execute(&api_request, &api_specification).await;

    let result = test_response.get_cors_preflight().unwrap();

    let expected = Cors::default();

    assert_eq!(result, expected);
}

#[test]
async fn test_end_to_end_api_gateway_cors_with_preflight_default_and_actual_request() {
    let empty_headers = HeaderMap::new();
    let preflight_request =
        get_preflight_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

    let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

    let worker_name = r#"
      let id: u64 = request.path.user-id;
      "shopping-cart-${id}"
    "#;

    let response_mapping = r#"
      let response = golem:it/api.{get-cart-contents}("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_spec_for_cors_preflight_default_and_actual_endpoint(
            "foo/{user-id}",
            worker_name,
            response_mapping,
        )
        .await;

    let preflight_response = execute(&preflight_request, &api_specification).await;
    let actual_response = execute(&api_request, &api_specification).await;

    let pre_flight_response = preflight_response.get_cors_preflight().unwrap();

    let expected_cors_preflight = Cors::default();

    let allow_origin_in_actual_response = actual_response.get_cors_allow_origin().unwrap();

    assert_eq!(pre_flight_response, expected_cors_preflight);
    assert_eq!(
        allow_origin_in_actual_response,
        expected_cors_preflight.get_allow_origin()
    );
}

#[test]
async fn test_end_to_end_api_gateway_cors_with_preflight_and_actual_request() {
    let empty_headers = HeaderMap::new();
    let preflight_request =
        get_preflight_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

    let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

    let cors = Cors::from_parameters(
        Some("http://example.com".to_string()),
        Some("GET, POST, PUT, DELETE, OPTIONS".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some(true),
        Some(3600),
    )
    .unwrap();

    let worker_name = r#"
      let id: u64 = request.path.user-id;
      "shopping-cart-${id}"
    "#;

    let response_mapping = r#"
      let response = golem:it/api.{get-cart-contents}("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition = get_api_spec_for_cors_preflight_and_actual_endpoint(
        "foo/{user-id}",
        worker_name,
        response_mapping,
        &cors,
    )
    .await;

    let preflight_response = execute(&preflight_request, &api_specification).await;
    let actual_response = execute(&api_request, &api_specification).await;

    let pre_flight_response = preflight_response.get_cors_preflight().unwrap();

    let allow_origin_in_actual_response = actual_response.get_cors_allow_origin().unwrap();

    let expose_headers_in_actual_response = actual_response.get_expose_headers().unwrap();

    let allow_credentials_in_actual_response = actual_response.get_allow_credentials().unwrap();

    assert_eq!(pre_flight_response, cors);

    // In the actual response other than preflight we expect only allow_origin, expose_headers, and allow_credentials
    assert_eq!(allow_origin_in_actual_response, cors.get_allow_origin());
    assert_eq!(
        expose_headers_in_actual_response,
        cors.get_expose_headers().unwrap()
    );
    assert_eq!(
        allow_credentials_in_actual_response,
        cors.get_allow_credentials().unwrap()
    );
}

#[test]
async fn test_end_to_end_api_gateway_with_request_path_and_query_lookup() {
    let empty_headers = HeaderMap::new();
    let api_request = get_api_request("foo/1", Some("token-id=jon"), &empty_headers, Value::Null);

    let worker_name = r#"
        let x: u64 = request.path.user-id;
        "shopping-cart-${x}"
    "#;

    let response_mapping = r#"
        let response = golem:it/api.{get-cart-contents}(request.path.token-id, request.path.token-id);
        response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_spec_worker_binding("foo/{user-id}?{token-id}", worker_name, response_mapping)
            .await;

    let test_response = execute(&api_request, &api_specification).await;

    let result = (
        test_response.get_worker_name().unwrap(),
        test_response.get_function_name().unwrap(),
        test_response.get_function_params().unwrap(),
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{get-cart-contents}".to_string(),
        Value::Array(vec![
            Value::String("jon".to_string()),
            Value::String("jon".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_end_to_end_api_gateway_with_request_path_and_query_lookup_complex() {
    let empty_headers = HeaderMap::new();
    let api_request = get_api_request(
        "foo/1",
        None,
        &empty_headers,
        Value::Object(serde_json::Map::from_iter(vec![(
            "age".to_string(),
            Value::Number(serde_json::Number::from(10)),
        )])),
    );

    let response_mapping = r#"
      let response = golem:it/api.{get-cart-contents}("a", "b");
      response
    "#;

    let worker_name = r#"
      let n: u64 = 100;
      let age: u64 = request.body.age;
      let zero: u64 = 0; let one: u64 = 1;
      let res = if age > n then zero else one;
      "shopping-cart-${res}"
    "#;

    let api_specification: HttpApiDefinition =
        get_api_spec_worker_binding("foo/{user-id}", worker_name, response_mapping).await;

    let test_response = execute(&api_request, &api_specification).await;

    let result = (
        test_response.get_worker_name().unwrap(),
        test_response.get_function_name().unwrap(),
        test_response.get_function_params().unwrap(),
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

#[test]
async fn test_end_to_end_api_gateway_with_with_request_body_lookup1() {
    let empty_headers = HeaderMap::new();

    let api_request = get_api_request(
        "foo/2",
        None,
        &empty_headers,
        Value::String("address".to_string()),
    );

    let worker_name = r#"
        let userid: u64 = request.path.user-id;
        let max: u64 = 100;
        let zero: u64 = 0;
        let one: u64 = 1;
        let res = if userid > max then zero else one;
        "shopping-cart-${res}"
    "#;

    let response_mapping = r#"
        let response = golem:it/api.{get-cart-contents}(request.body, request.body);
        response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_spec_worker_binding("foo/{user-id}", worker_name, response_mapping).await;

    let test_response = execute(&api_request, &api_specification).await;

    let result = (
        test_response.get_worker_name().unwrap(),
        test_response.get_function_name().unwrap(),
        test_response.get_function_params().unwrap(),
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{get-cart-contents}".to_string(),
        Value::Array(vec![
            Value::String("address".to_string()),
            Value::String("address".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_end_to_end_api_gateway_with_with_request_body_lookup2() {
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
        "foo/bar",
        None,
        &empty_headers,
        serde_json::Value::Object(request_body),
    );

    let worker_name = r#"
        let userid: str = request.path.user-id;
        let max: u64 = 100;
        let zero: u64 = 0;
        let one: u64 = 1;
        let res = if userid == "bar" then one else zero;
        "shopping-cart-${res}"
    "#;

    let response_mapping = r#"
          let param1 = request.body.foo_key;
          let param2 = request.body.bar_key[0];
          let response = golem:it/api.{get-cart-contents}(param1, param2);
          response
        "#;

    let api_specification: HttpApiDefinition =
        get_api_spec_worker_binding("foo/{user-id}", worker_name, response_mapping).await;

    let test_response = execute(&api_request, &api_specification).await;

    let result = (
        test_response.get_worker_name().unwrap(),
        test_response.get_function_name().unwrap(),
        test_response.get_function_params().unwrap(),
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

#[test]
async fn test_end_to_end_api_gateway_with_with_request_body_lookup3() {
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

    let worker_name = r#"
        let userid: u64 = request.path.user-id;
        let max: u64 = 100;
        let zero: u64 = 0;
        let one: u64 = 1;
        let res = if userid > max then zero else one;
        "shopping-cart-${res}"
    "#;

    let response_mapping = r#"
          let response = golem:it/api.{get-cart-contents}(request.body.foo_key, request.body.bar_key[0]);
          response
        "#;

    let api_specification: HttpApiDefinition =
        get_api_spec_worker_binding("foo/{user-id}", worker_name, response_mapping).await;

    let test_response = execute(&api_request, &api_specification).await;

    let result = (
        test_response.get_worker_name().unwrap(),
        test_response.get_function_name().unwrap(),
        test_response.get_function_params().unwrap(),
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

#[test]
async fn test_api_gateway_rib_input_from_request_details() {
    async fn test_paths(definition_path: &str, request_path: &str, ok: bool) {
        let empty_headers = HeaderMap::new();
        let api_request =
            get_api_request(request_path, None, &empty_headers, serde_json::Value::Null);

        let worker_name = r#"
            let x: u64 = request.path.cart-id;
            "shopping-cart-${x}"
        "#;

        let response_mapping = r#"
            let response = golem:it/api.{get-cart-contents}("foo", "bar");
            response
            "#;

        let api_specification: HttpApiDefinition =
            get_api_spec_worker_binding(definition_path, worker_name, response_mapping).await;

        let compiled_api_spec = CompiledHttpApiDefinition::from_http_api_definition(
            &api_specification,
            &internal::get_component_metadata(),
            &DefaultNamespace::default(),
        )
        .unwrap();

        let resolved_route = api_request
            .resolve_gateway_binding(vec![compiled_api_spec])
            .await;

        let result =
            resolved_route.map(|x| x.get_worker_detail().expect("Tests expect worker detail"));

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

#[test]
async fn test_api_gateway_idempotency_key_resolution() {
    async fn test_key(header_map: &HeaderMap, idempotency_key: Option<IdempotencyKey>) {
        let api_request = get_api_request("/getcartcontent/1", None, header_map, Value::Null);

        let expression = r#"
            let response = golem:it/api.{get-cart-contents}("foo", "bar");
            response
            "#;

        let api_specification: HttpApiDefinition = get_api_spec_worker_binding(
            "getcartcontent/{cart-id}",
            "${let x: u64 = request.path.cart-id; \"shopping-cart-${x}\"}",
            expression,
        )
        .await;

        let compiled_api_spec = CompiledHttpApiDefinition::from_http_api_definition(
            &api_specification,
            &internal::get_component_metadata(),
            &DefaultNamespace::default(),
        )
        .unwrap();

        let resolved_route = api_request
            .resolve_gateway_binding(vec![compiled_api_spec])
            .await
            .unwrap();

        assert_eq!(
            resolved_route.get_worker_detail().unwrap().idempotency_key,
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

fn get_preflight_api_request(
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
        req_method: Method::OPTIONS,
        req_body,
    }
}

async fn get_api_spec_worker_binding(
    path_pattern: &str,
    worker_name: &str,
    rib_expression: &str,
) -> HttpApiDefinition {
    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Get
            path: {}
            binding:
              type: wit-worker
              componentId:
                componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
                version: 0
              workerName: '{}'
              response: '${{{}}}'

        "#,
        path_pattern, worker_name, rib_expression
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request.try_into().unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();

    HttpApiDefinition::from_http_api_definition_request(
        &DefaultNamespace(),
        core_request,
        create_at,
        &internal::get_security_scheme_service(),
    )
    .await
    .unwrap()
}

// https://swagger.io/docs/specification/v3_0/authentication/openid-connect-discovery/
async fn get_api_spec_worker_binding_with_security(
    path_pattern: &str,
    worker_name: &str,
    rib_expression: &str,
) -> HttpApiDefinition {
    let security_scheme_identifier = SecuritySchemeIdentifier::new("openId1".to_string());

    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          security:
          - openId1
          routes:
          - method: Get
            path: {}
            security: {}
            binding:
              type: wit-worker
              componentId:
                componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
                version: 0
              workerName: '{}'
              response: '${{{}}}'

        "#,
        path_pattern, security_scheme_identifier, worker_name, rib_expression
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request.try_into().unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    let security_scheme_service = internal::get_security_scheme_service();

    let security_scheme = SecurityScheme::new(
        Provider::Google,
        security_scheme_identifier,
        ClientId::new("foo".to_string()),
        ClientSecret::new("bar".to_string()),
        RedirectUrl::new("http://abc.com/d/e".to_string()).unwrap(),
        vec![
            Scope::new("user".to_string()),
            Scope::new("email".to_string()),
        ],
    );

    // Make sure security scheme is added to golem
    security_scheme_service
        .create(&DefaultNamespace(), &security_scheme)
        .await
        .unwrap();

    HttpApiDefinition::from_http_api_definition_request(
        &DefaultNamespace(),
        core_request,
        create_at,
        &security_scheme_service,
    )
    .await
    .unwrap()
}

// https://swagger.io/docs/specification/v3_0/authentication/openid-connect-discovery/
async fn get_api_spec_worker_binding_with_multiple_securities(
    path_pattern: &str,
    worker_name: &str,
    rib_expression: &str,
) -> HttpApiDefinition {
    let security_scheme_identifier1 = SecuritySchemeIdentifier::new("openId1".to_string());
    let security_scheme_identifier2 = SecuritySchemeIdentifier::new("openId2".to_string());

    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          security:
          - {}
          - {}
          routes:
          - method: Get
            path: {}
            security: {}
            binding:
              type: wit-worker
              componentId:
                componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
                version: 0
              workerName: '{}'
              response: '${{{}}}'
          - method: Post
            path: {}
            security: {}
            binding:
              type: wit-worker
              componentId:
                componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
                version: 0
              workerName: '{}'
              response: '${{{}}}'

        "#,
        security_scheme_identifier1, security_scheme_identifier2,
        path_pattern, security_scheme_identifier1, worker_name, rib_expression, path_pattern, security_scheme_identifier2, worker_name, rib_expression
    );

    let user_facing_definition_request: api::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_definition_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        user_facing_definition_request.try_into().unwrap();

    let security_scheme_service = internal::get_security_scheme_service();

    let security_scheme1 = SecurityScheme::new(
        Provider::Google,
        security_scheme_identifier1,
        ClientId::new("foo1".to_string()),
        ClientSecret::new("bar1".to_string()),
        RedirectUrl::new("http://abc.com/d/e".to_string()).unwrap(),
        vec![
            Scope::new("user".to_string()),
            Scope::new("email".to_string()),
        ],
    );

    let security_scheme2 = SecurityScheme::new(
        Provider::Google,
        security_scheme_identifier2,
        ClientId::new("foo2".to_string()),
        ClientSecret::new("bar2".to_string()),
        RedirectUrl::new("http://abc.com/d/e".to_string()).unwrap(),
        vec![
            Scope::new("user".to_string()),
            Scope::new("email".to_string()),
        ],
    );

    // Make sure security scheme 1 is added to golem
    security_scheme_service
        .create(&DefaultNamespace(), &security_scheme1)
        .await
        .unwrap();

    // Make sure security scheme 2 is added to golem
    security_scheme_service
        .create(&DefaultNamespace(), &security_scheme2)
        .await
        .unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();

    HttpApiDefinition::from_http_api_definition_request(
        &DefaultNamespace(),
        core_definition_request,
        create_at,
        &security_scheme_service,
    )
    .await
    .unwrap()
}

async fn get_api_spec_cors_preflight_binding_default_response(
    path_pattern: &str,
) -> HttpApiDefinition {
    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Options
            path: {}
            binding:
              bindingType: cors-preflight
        "#,
        path_pattern,
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request.try_into().unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    HttpApiDefinition::from_http_api_definition_request(
        &DefaultNamespace(),
        core_request,
        create_at,
        &internal::get_security_scheme_service(),
    )
    .await
    .unwrap()
}

async fn get_api_spec_cors_preflight_binding(path_pattern: &str, cors: &Cors) -> HttpApiDefinition {
    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Options
            path: {}
            binding:
              bindingType: cors-preflight
              response: |
                {{
                  Access-Control-Allow-Origin: "{}",
                  Access-Control-Allow-Methods: "{}",
                  Access-Control-Allow-Headers: "{}",
                  Access-Control-Expose-Headers: "{}",
                  Access-Control-Allow-Credentials: {},
                  Access-Control-Max-Age: {}u64
                }}
        "#,
        path_pattern,
        cors.get_allow_origin(),
        cors.get_allow_methods(),
        cors.get_allow_headers(),
        cors.get_expose_headers().clone().unwrap_or_default(),
        cors.get_allow_credentials().unwrap_or_default(),
        cors.get_max_age().unwrap_or_default()
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        { http_api_definition_request.try_into().unwrap() };

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    HttpApiDefinition::from_http_api_definition_request(
        &DefaultNamespace(),
        core_request,
        create_at,
        &internal::get_security_scheme_service(),
    )
    .await
    .unwrap()
}

async fn get_api_spec_for_cors_preflight_and_actual_endpoint(
    path_pattern: &str,
    worker_name: &str,
    rib_expression: &str,
    cors: &Cors,
) -> HttpApiDefinition {
    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Options
            path: {}
            binding:
              bindingType: cors-preflight
              response: |
                {{
                  Access-Control-Allow-Origin: "{}",
                  Access-Control-Allow-Methods: "{}",
                  Access-Control-Allow-Headers: "{}",
                  Access-Control-Expose-Headers: "{}",
                  Access-Control-Allow-Credentials: {},
                  Access-Control-Max-Age: {}u64
                }}
          - method: Get
            path: {}
            binding:
              type: wit-worker
              componentId:
                componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
                version: 0
              workerName: '{}'
              response: '${{{}}}'

        "#,
        path_pattern,
        cors.get_allow_origin(),
        cors.get_allow_methods(),
        cors.get_allow_headers(),
        cors.get_expose_headers().clone().unwrap_or_default(),
        cors.get_allow_credentials().unwrap_or_default(),
        cors.get_max_age().unwrap_or_default(),
        path_pattern,
        worker_name,
        rib_expression
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request.try_into().unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    HttpApiDefinition::from_http_api_definition_request(
        &DefaultNamespace(),
        core_request,
        create_at,
        &internal::get_security_scheme_service(),
    )
    .await
    .unwrap()
}

async fn get_api_spec_for_cors_preflight_default_and_actual_endpoint(
    path_pattern: &str,
    worker_name: &str,
    rib_expression: &str,
) -> HttpApiDefinition {
    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Options
            path: {}
            binding:
              bindingType: cors-preflight
          - method: Get
            path: {}
            binding:
              type: wit-worker
              componentId:
                componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
                version: 0
              workerName: '{}'
              response: '${{{}}}'

        "#,
        path_pattern, path_pattern, worker_name, rib_expression
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request.try_into().unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    HttpApiDefinition::from_http_api_definition_request(
        &DefaultNamespace(),
        core_request,
        create_at,
        &internal::get_security_scheme_service(),
    )
    .await
    .unwrap()
}

mod internal {
    use async_trait::async_trait;
    use chrono::{Duration, TimeZone, Utc};
    use golem_common::model::ComponentId;
    use golem_service_base::auth::DefaultNamespace;
    use golem_service_base::model::VersionedComponentId;
    use golem_service_base::repo::RepoError;
    use golem_wasm_ast::analysis::analysed_type::{field, record, str, tuple};
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
        AnalysedInstance,
    };
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::{NameTypePair, NameValuePair, Type, TypedRecord, TypedTuple};
    use golem_worker_service_base::gateway_api_definition::http::ComponentMetadataDictionary;
    use golem_worker_service_base::gateway_execution::file_server_binding_handler::{
        FileServerBindingHandler, FileServerBindingResult,
    };
    use golem_worker_service_base::gateway_execution::gateway_binding_resolver::{ResolvedBinding, ResolvedGatewayBinding, WorkerDetail};
    use golem_worker_service_base::gateway_execution::{
        GatewayResolvedWorkerRequest, GatewayWorkerRequestExecutor, WorkerRequestExecutorError,
        WorkerResponse,
    };
    use golem_worker_service_base::gateway_middleware::Cors;
    use golem_worker_service_base::gateway_rib_interpreter::{
        DefaultRibInterpreter, EvaluationError, WorkerServiceRibInterpreter,
    };
    use golem_worker_service_base::gateway_security::{
        AuthorizationUrl, GolemIdentityProviderMetadata, IdentityProvider, IdentityProviderError,
        IdentityProviderResolver, OpenIdClient, Provider, SecuritySchemeWithProviderMetadata,
    };
    use golem_worker_service_base::repo::security_scheme::{
        SecuritySchemeRecord, SecuritySchemeRepo,
    };
    use golem_worker_service_base::service::gateway::security_scheme::{
        DefaultSecuritySchemeService, SecuritySchemeService,
    };
    use http::header::{
        ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS,
        ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS,
        ACCESS_CONTROL_MAX_AGE,
    };
    use openidconnect::core::{
        CoreClaimName, CoreClaimType, CoreClient, CoreClientAuthMethod, CoreGrantType, CoreIdToken,
        CoreIdTokenClaims, CoreIdTokenFields, CoreJweContentEncryptionAlgorithm,
        CoreJweKeyManagementAlgorithm, CoreJwsSigningAlgorithm, CoreProviderMetadata,
        CoreResponseMode, CoreResponseType, CoreRsaPrivateSigningKey, CoreSubjectIdentifierType,
        CoreTokenResponse, CoreTokenType,
    };
    use openidconnect::{
        AccessToken, Audience, AuthUrl, AuthenticationContextClass, AuthorizationCode, ClientId,
        ClientSecret, CsrfToken, EmptyAdditionalClaims, EmptyExtraTokenFields, EndUserEmail,
        IssuerUrl, JsonWebKeyId, JsonWebKeySet, JsonWebKeySetUrl, Nonce, RegistrationUrl,
        ResponseTypes, Scope, StandardClaims, SubjectIdentifier, TokenUrl, UserInfoUrl,
    };
    use rib::RibResult;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use url::Url;
    use golem_worker_service_base::gateway_binding::StaticBinding;

    struct TestSecuritySchemeRepo {
        security_scheme: Arc<Mutex<HashMap<String, SecuritySchemeRecord>>>,
    }

    #[async_trait]
    impl SecuritySchemeRepo for TestSecuritySchemeRepo {
        async fn create(
            &self,
            security_scheme_record: &SecuritySchemeRecord,
        ) -> Result<(), RepoError> {
            self.security_scheme.lock().await.insert(
                security_scheme_record.security_scheme_id.clone(),
                security_scheme_record.clone(),
            );
            Ok(())
        }

        async fn get(
            &self,
            security_scheme_id: &str,
        ) -> Result<Option<SecuritySchemeRecord>, RepoError> {
            Ok(self
                .security_scheme
                .lock()
                .await
                .get(security_scheme_id)
                .cloned())
        }
    }

    struct TestIdentityProvider {}

    #[async_trait]
    impl IdentityProvider for TestIdentityProvider {
        async fn get_provider_metadata(
            &self,
            _provider: &Provider,
        ) -> Result<GolemIdentityProviderMetadata, IdentityProviderError> {
            Ok(get_test_provider_metadata())
        }

        async fn exchange_code_for_tokens(
            &self,
            _client: &OpenIdClient,
            _code: &AuthorizationCode,
        ) -> Result<CoreTokenResponse, IdentityProviderError> {
            Ok(CoreTokenResponse::new(
                AccessToken::new("some_secret".to_string()),
                CoreTokenType::Bearer,
                CoreIdTokenFields::new(Some(get_id_token()), EmptyExtraTokenFields {}),
            ))
        }

        fn get_client(
            &self,
            _security_scheme: &SecuritySchemeWithProviderMetadata,
        ) -> Result<OpenIdClient, IdentityProviderError> {
            let core_client = CoreClient::new(
                ClientId::new("aaa".to_string()),
                Some(ClientSecret::new("bbb".to_string())),
                IssuerUrl::new("https://example".to_string()).unwrap(),
                AuthUrl::new("https://example/authorize".to_string()).unwrap(),
                Some(TokenUrl::new("https://example/token".to_string()).unwrap()),
                None,
                JsonWebKeySet::default(),
            );
            Ok(OpenIdClient::new(core_client))
        }

        fn get_claims(
            &self,
            _client: &OpenIdClient,
            _core_token_response: CoreTokenResponse,
            _nonce: &Nonce,
        ) -> Result<CoreIdTokenClaims, IdentityProviderError> {
            Ok(CoreIdTokenClaims::new(
                IssuerUrl::new("https://server.example.com".to_string()).unwrap(),
                vec![Audience::new("s6BhdRkqt3".to_string())],
                Utc.timestamp_opt(1311281970, 0)
                    .single()
                    .expect("valid timestamp"),
                Utc.timestamp_opt(1311280970, 0)
                    .single()
                    .expect("valid timestamp"),
                StandardClaims::new(SubjectIdentifier::new("24400320".to_string())),
                EmptyAdditionalClaims {},
            ))
        }

        fn get_authorization_url(
            &self,
            _client: &OpenIdClient,
            _scopes: Vec<Scope>,
        ) -> AuthorizationUrl {
            AuthorizationUrl {
                url: Url::parse("https:// example. net").unwrap(),
                csrf_state: CsrfToken::new_random(),
                nonce: Nonce::new("nonce".to_string()),
            }
        }
    }

    struct TestIdentityProviderResolver;

    impl IdentityProviderResolver for TestIdentityProviderResolver {
        fn resolve(&self, _provider_type: &Provider) -> Arc<dyn IdentityProvider + Sync + Send> {
            Arc::new(TestIdentityProvider {})
        }
    }

    pub(crate) fn get_security_scheme_service(
    ) -> Arc<dyn SecuritySchemeService<DefaultNamespace> + Send + Sync> {
        let repo = Arc::new(TestSecuritySchemeRepo {
            security_scheme: Arc::new(Mutex::new(HashMap::new())),
        });

        let identity_provider_resolver = Arc::new(TestIdentityProviderResolver);

        let default = DefaultSecuritySchemeService::new(repo, identity_provider_resolver);

        Arc::new(default)
    }

    pub(crate) struct TestApiGatewayWorkerRequestExecutor {}

    #[async_trait]
    impl GatewayWorkerRequestExecutor<DefaultNamespace> for TestApiGatewayWorkerRequestExecutor {
        // This test executor simply returns the worker request details itself as a type-annotated-value
        async fn execute(
            &self,
            resolved_worker_request: GatewayResolvedWorkerRequest<DefaultNamespace>,
        ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
            let type_annotated_value = convert_to_worker_response(&resolved_worker_request);
            let worker_response = create_tuple(vec![type_annotated_value]);

            Ok(WorkerResponse::new(worker_response))
        }
    }

    struct TestFileServerBindingHandler {}
    #[async_trait]
    impl<Namespace> FileServerBindingHandler<Namespace> for TestFileServerBindingHandler {
        async fn handle_file_server_binding_result(
            &self,
            _namespace: &Namespace,
            _worker_detail: &WorkerDetail,
            _original_result: RibResult,
        ) -> FileServerBindingResult {
            unimplemented!()
        }
    }

    #[derive(Debug, Clone)]
    pub(crate) enum TestResponse {
        WorkerResponse {
            worker_name: String,
            function_name: String,
            function_params: Value,
            cors_middleware_headers: Option<CorsMiddlewareHeadersInResponse> // if binding has cors middleware configured
        },
        CorsPreflightResponse(CorsPreflightResponseHeaders), // preflight test response
        RedirectResponse {
            redirect_url: Url
        } // If any middleware resulted in redirect
    }

    #[derive(Debug, Clone)]
    pub struct CorsMiddlewareHeadersInResponse {
        cors_header_allow_origin: String,
        cors_header_allow_credentials: Option<String>, // If cors middleware is applied
        cors_header_expose_headers: Option<String> // If cors middleware is applied
    }

    #[derive(Debug, Clone)]
    pub struct CorsPreflightResponseHeaders {
        cors_header_allow_origin: String,
        cors_header_allow_methods: String,
        cors_header_allow_headers: String,
        cors_header_max_age: Option<u64>,
        cors_header_allow_credentials: Option<bool>,
        cors_header_expose_headers: Option<String>,
    }

    impl TestResponse {
        pub async fn from_poem_response(response: poem::Response, resolved_gateway_binding: &ResolvedGatewayBinding<DefaultNamespace>) -> Self {

            match resolved_gateway_binding.resolved_binding {
                ResolvedBinding::Static(static_binding) =>  {
                    match static_binding  {
                        StaticBinding::HttpCorsPreflight(_) => {
                            let headers = response.headers();

                            let allow_headers = headers
                                .get(ACCESS_CONTROL_ALLOW_HEADERS)
                                .map(|x| x.to_str().unwrap().to_string()).expect("Cors preflight response expects allow_headers");

                            let allow_origin = headers
                                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                                .map(|x| x.to_str().unwrap().to_string()).expect("Cors preflight response expects allow_origin");

                            let allow_methods = headers
                                .get(ACCESS_CONTROL_ALLOW_METHODS)
                                .map(|x| x.to_str().unwrap().to_string()).expect("Cors preflight response expects allow_method");

                            let expose_headers = headers
                                .get(ACCESS_CONTROL_EXPOSE_HEADERS)
                                .map(|x| x.to_str().unwrap().to_string());

                            let max_age = headers
                                .get(ACCESS_CONTROL_MAX_AGE)
                                .map(|x| x.to_str().unwrap().parse::<u64>().unwrap());

                            let allow_credentials = headers
                                .get(ACCESS_CONTROL_ALLOW_CREDENTIALS)
                                .map(|x| x.to_str().unwrap().parse::<bool>().unwrap());

                            TestResponse::CorsPreflightResponse(CorsPreflightResponseHeaders {
                                cors_header_allow_origin: allow_origin,
                                cors_header_allow_methods: allow_methods,
                                cors_header_allow_headers: allow_headers,
                                cors_header_expose_headers: expose_headers,
                                cors_header_allow_credentials: allow_credentials,
                                cors_header_max_age: max_age
                            })
                        }

                        // If binding was http auth call back, we expect a redirect to the original Url
                        StaticBinding::HttpAuthCallBack(_) => {
                           unimplemented!("Http auth call back test response is not handled")
                        }
                    }
                }

                ResolvedBinding::Worker(binding) => {
                    let bytes = response.into_body().into_bytes().await.ok().expect("TestResponse for worker-binding expects a response body");

                    let body_json: Value = serde_json::from_slice(&bytes).expect("Failed to read the response body");

                    let worker_name = body_json
                        .get("worker_name")
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    let function_name = body_json
                        .get("function_name")
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    let function_params = body_json.get("function_params").cloned();

                    TestResponse::WorkerResponse {
                        worker_name: worker_name.expect("Worker response expects worker_name"),
                        function_name: function_name.expect("Worker response expects function_name"),
                        function_params: function_params.expect("Worker response expects function_params"),
                        cors_middleware_headers: {
                            if binding.middlewares.get_cors() {

                            }
                        }

                    }
                }
            }

            let headers = response.headers();

            let allow_headers = headers
                .get(ACCESS_CONTROL_ALLOW_HEADERS)
                .map(|x| x.to_str().unwrap().to_string());

            let allow_origin = headers
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .map(|x| x.to_str().unwrap().to_string());

            let allow_methods = headers
                .get(ACCESS_CONTROL_ALLOW_METHODS)
                .map(|x| x.to_str().unwrap().to_string());

            let expose_headers = headers
                .get(ACCESS_CONTROL_EXPOSE_HEADERS)
                .map(|x| x.to_str().unwrap().to_string());

            let max_age = headers
                .get(ACCESS_CONTROL_MAX_AGE)
                .map(|x| x.to_str().unwrap().parse::<u64>().unwrap());

            let allow_credentials = headers
                .get(ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .map(|x| x.to_str().unwrap().parse::<bool>().unwrap());

            let bytes = response.into_body().into_bytes().await.ok();

            if let Some(bytes) = bytes {
                let body_json: Value = serde_json::from_slice(&bytes).unwrap_or_default();

                let worker_name = body_json
                    .get("worker_name")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                let function_name = body_json
                    .get("function_name")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                let function_params = body_json.get("function_params").cloned();

                TestResponse {
                    worker_name,
                    function_name,
                    function_params,
                    cors_header_allow_credentials: allow_credentials,
                    cors_header_allow_origin: allow_origin,
                    cors_header_expose_headers: expose_headers,
                    cors_header_allow_methods: allow_methods,
                    cors_header_allow_headers: allow_headers,
                    cors_header_max_age: max_age,
                }
            } else {
                TestResponse {
                    worker_name: None,
                    function_name: None,
                    function_params: None,
                    cors_header_allow_credentials: allow_credentials,
                    cors_header_allow_origin: allow_origin,
                    cors_header_expose_headers: expose_headers,
                    cors_header_allow_methods: allow_methods,
                    cors_header_allow_headers: allow_headers,
                    cors_header_max_age: max_age,
                }
            }
        }

        pub fn get_cors_preflight(&self) -> Option<Cors> {
            Cors::from_parameters(
                self.cors_header_allow_origin.clone(),
                self.cors_header_allow_methods.clone(),
                self.cors_header_allow_headers.clone(),
                self.cors_header_expose_headers.clone(),
                self.cors_header_allow_credentials,
                self.cors_header_max_age,
            )
            .ok()
        }

        pub fn get_cors_allow_origin(&self) -> Option<String> {
            self.cors_header_allow_origin.clone()
        }

        pub fn get_allow_credentials(&self) -> Option<bool> {
            self.cors_header_allow_credentials
        }

        pub fn get_expose_headers(&self) -> Option<String> {
            self.cors_header_expose_headers.clone()
        }

        pub fn get_worker_name(&self) -> Option<String> {
            self.worker_name.clone()
        }

        pub fn get_function_name(&self) -> Option<String> {
            self.function_name.clone()
        }

        pub fn get_function_params(&self) -> Option<Value> {
            self.function_params.clone()
        }
    }

    pub(crate) fn create_tuple(
        type_annotated_value: Vec<TypeAnnotatedValue>,
    ) -> TypeAnnotatedValue {
        let root = type_annotated_value
            .iter()
            .map(|x| golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(x.clone()),
            })
            .collect::<Vec<_>>();

        let types = type_annotated_value
            .iter()
            .map(|x| golem_wasm_rpc::protobuf::Type::try_from(x).unwrap())
            .collect::<Vec<_>>();

        TypeAnnotatedValue::Tuple(TypedTuple {
            value: root,
            typ: types,
        })
    }

    pub(crate) fn create_record(
        values: Vec<(String, TypeAnnotatedValue)>,
    ) -> Result<TypeAnnotatedValue, EvaluationError> {
        let mut name_type_pairs = vec![];
        let mut name_value_pairs = vec![];

        for (key, value) in values.iter() {
            let typ = Type::try_from(value)
                .map_err(|_| EvaluationError("Failed to get type".to_string()))?;
            name_type_pairs.push(NameTypePair {
                name: key.to_string(),
                typ: Some(typ),
            });

            name_value_pairs.push(NameValuePair {
                name: key.to_string(),
                value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(value.clone()),
                }),
            });
        }

        Ok(TypeAnnotatedValue::Record(TypedRecord {
            typ: name_type_pairs,
            value: name_value_pairs,
        }))
    }

    pub(crate) fn convert_to_worker_response(
        worker_request: &GatewayResolvedWorkerRequest<DefaultNamespace>,
    ) -> TypeAnnotatedValue {
        let mut record_elems = vec![
            (
                "component_id".to_string(),
                TypeAnnotatedValue::Str(worker_request.component_id.0.to_string()),
            ),
            (
                "function_name".to_string(),
                TypeAnnotatedValue::Str(worker_request.function_name.to_string()),
            ),
            (
                "function_params".to_string(),
                create_tuple(worker_request.function_params.clone()),
            ),
        ];

        if let Some(worker_name) = worker_request.clone().worker_name {
            record_elems.push((
                "worker_name".to_string(),
                TypeAnnotatedValue::Str(worker_name),
            ))
        };

        if let Some(idempotency_key) = worker_request.clone().idempotency_key {
            record_elems.push((
                "idempotency-key".to_string(),
                TypeAnnotatedValue::Str(idempotency_key.to_string()),
            ))
        };

        create_record(record_elems).unwrap()
    }

    pub(crate) fn get_component_metadata() -> ComponentMetadataDictionary {
        let versioned_component_id = VersionedComponentId {
            component_id: ComponentId::try_from("0b6d9cd8-f373-4e29-8a5a-548e61b868a5").unwrap(),
            version: 0,
        };

        let mut metadata_dict = HashMap::new();

        let analysed_export = AnalysedExport::Instance(AnalysedInstance {
            name: "golem:it/api".to_string(),
            functions: vec![AnalysedFunction {
                name: "get-cart-contents".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "a".to_string(),
                        typ: str(),
                    },
                    AnalysedFunctionParameter {
                        name: "b".to_string(),
                        typ: str(),
                    },
                ],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: record(vec![
                        field("component_id", str()),
                        field("name", str()),
                        field("function_name", str()),
                        field("function_params", tuple(vec![str()])),
                    ]),
                }],
            }],
        });

        let metadata = vec![analysed_export];

        metadata_dict.insert(versioned_component_id, metadata);

        ComponentMetadataDictionary {
            metadata: metadata_dict,
        }
    }

    pub(crate) fn get_test_rib_interpreter(
    ) -> Arc<dyn WorkerServiceRibInterpreter<DefaultNamespace> + Sync + Send> {
        Arc::new(DefaultRibInterpreter::from_worker_request_executor(
            Arc::new(TestApiGatewayWorkerRequestExecutor {}),
        ))
    }

    pub(crate) fn get_test_file_server_binding_handler<Namespace>(
    ) -> Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send> {
        Arc::new(TestFileServerBindingHandler {})
    }

    fn get_test_provider_metadata() -> GolemIdentityProviderMetadata {
        let all_signing_algs = vec![
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512,
            CoreJwsSigningAlgorithm::EcdsaP256Sha256,
            CoreJwsSigningAlgorithm::EcdsaP384Sha384,
            CoreJwsSigningAlgorithm::EcdsaP521Sha512,
            CoreJwsSigningAlgorithm::HmacSha256,
            CoreJwsSigningAlgorithm::HmacSha384,
            CoreJwsSigningAlgorithm::HmacSha512,
            CoreJwsSigningAlgorithm::RsaSsaPssSha256,
            CoreJwsSigningAlgorithm::RsaSsaPssSha384,
            CoreJwsSigningAlgorithm::RsaSsaPssSha512,
            CoreJwsSigningAlgorithm::None,
        ];
        let all_encryption_algs = vec![
            CoreJweKeyManagementAlgorithm::RsaPkcs1V15,
            CoreJweKeyManagementAlgorithm::RsaOaep,
            CoreJweKeyManagementAlgorithm::RsaOaepSha256,
            CoreJweKeyManagementAlgorithm::AesKeyWrap128,
            CoreJweKeyManagementAlgorithm::AesKeyWrap192,
            CoreJweKeyManagementAlgorithm::AesKeyWrap256,
            CoreJweKeyManagementAlgorithm::EcdhEs,
            CoreJweKeyManagementAlgorithm::EcdhEsAesKeyWrap128,
            CoreJweKeyManagementAlgorithm::EcdhEsAesKeyWrap192,
            CoreJweKeyManagementAlgorithm::EcdhEsAesKeyWrap256,
        ];
        let new_provider_metadata = CoreProviderMetadata::new(
            IssuerUrl::new(
                "https://rp.certification.openid.net:8080/openidconnect-rs/rp-response_type-code"
                    .to_string(),
            )
            .unwrap(),
            AuthUrl::new(
                "https://rp.certification.openid.net:8080/openidconnect-rs/\
                 rp-response_type-code/authorization"
                    .to_string(),
            )
            .unwrap(),
            JsonWebKeySetUrl::new(
                "https://rp.certification.openid.net:8080/static/jwks_3INbZl52IrrPCp2j.json"
                    .to_string(),
            )
            .unwrap(),
            vec![ResponseTypes::new(vec![CoreResponseType::Code])],
            vec![
                CoreSubjectIdentifierType::Public,
                CoreSubjectIdentifierType::Pairwise,
            ],
            all_signing_algs.clone(),
            Default::default(),
        )
        .set_request_object_signing_alg_values_supported(Some(all_signing_algs.clone()))
        .set_token_endpoint_auth_signing_alg_values_supported(Some(vec![
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512,
            CoreJwsSigningAlgorithm::EcdsaP256Sha256,
            CoreJwsSigningAlgorithm::EcdsaP384Sha384,
            CoreJwsSigningAlgorithm::EcdsaP521Sha512,
            CoreJwsSigningAlgorithm::HmacSha256,
            CoreJwsSigningAlgorithm::HmacSha384,
            CoreJwsSigningAlgorithm::HmacSha512,
            CoreJwsSigningAlgorithm::RsaSsaPssSha256,
            CoreJwsSigningAlgorithm::RsaSsaPssSha384,
            CoreJwsSigningAlgorithm::RsaSsaPssSha512,
        ]))
        .set_scopes_supported(Some(vec![
            Scope::new("email".to_string()),
            Scope::new("phone".to_string()),
            Scope::new("profile".to_string()),
            Scope::new("openid".to_string()),
            Scope::new("address".to_string()),
            Scope::new("offline_access".to_string()),
            Scope::new("openid".to_string()),
        ]))
        .set_userinfo_signing_alg_values_supported(Some(all_signing_algs))
        .set_id_token_encryption_enc_values_supported(Some(vec![
            CoreJweContentEncryptionAlgorithm::Aes128CbcHmacSha256,
            CoreJweContentEncryptionAlgorithm::Aes192CbcHmacSha384,
            CoreJweContentEncryptionAlgorithm::Aes256CbcHmacSha512,
            CoreJweContentEncryptionAlgorithm::Aes128Gcm,
            CoreJweContentEncryptionAlgorithm::Aes192Gcm,
            CoreJweContentEncryptionAlgorithm::Aes256Gcm,
        ]))
        .set_grant_types_supported(Some(vec![
            CoreGrantType::AuthorizationCode,
            CoreGrantType::Implicit,
            CoreGrantType::JwtBearer,
            CoreGrantType::RefreshToken,
        ]))
        .set_response_modes_supported(Some(vec![
            CoreResponseMode::Query,
            CoreResponseMode::Fragment,
            CoreResponseMode::FormPost,
        ]))
        .set_require_request_uri_registration(Some(true))
        .set_registration_endpoint(Some(
            RegistrationUrl::new(
                "https://rp.certification.openid.net:8080/openidconnect-rs/\
                 rp-response_type-code/registration"
                    .to_string(),
            )
            .unwrap(),
        ))
        .set_claims_parameter_supported(Some(true))
        .set_request_object_encryption_enc_values_supported(Some(vec![
            CoreJweContentEncryptionAlgorithm::Aes128CbcHmacSha256,
            CoreJweContentEncryptionAlgorithm::Aes192CbcHmacSha384,
            CoreJweContentEncryptionAlgorithm::Aes256CbcHmacSha512,
            CoreJweContentEncryptionAlgorithm::Aes128Gcm,
            CoreJweContentEncryptionAlgorithm::Aes192Gcm,
            CoreJweContentEncryptionAlgorithm::Aes256Gcm,
        ]))
        .set_userinfo_endpoint(Some(
            UserInfoUrl::new(
                "https://rp.certification.openid.net:8080/openidconnect-rs/\
                 rp-response_type-code/userinfo"
                    .to_string(),
            )
            .unwrap(),
        ))
        .set_token_endpoint_auth_methods_supported(Some(vec![
            CoreClientAuthMethod::ClientSecretPost,
            CoreClientAuthMethod::ClientSecretBasic,
            CoreClientAuthMethod::ClientSecretJwt,
            CoreClientAuthMethod::PrivateKeyJwt,
        ]))
        .set_claims_supported(Some(
            vec![
                "name",
                "given_name",
                "middle_name",
                "picture",
                "email_verified",
                "birthdate",
                "sub",
                "address",
                "zoneinfo",
                "email",
                "gender",
                "preferred_username",
                "family_name",
                "website",
                "profile",
                "phone_number_verified",
                "nickname",
                "updated_at",
                "phone_number",
                "locale",
            ]
            .iter()
            .map(|claim| CoreClaimName::new((*claim).to_string()))
            .collect(),
        ))
        .set_request_object_encryption_alg_values_supported(Some(all_encryption_algs.clone()))
        .set_claim_types_supported(Some(vec![
            CoreClaimType::Normal,
            CoreClaimType::Aggregated,
            CoreClaimType::Distributed,
        ]))
        .set_request_uri_parameter_supported(Some(true))
        .set_request_parameter_supported(Some(true))
        .set_token_endpoint(Some(
            TokenUrl::new(
                "https://rp.certification.openid.net:8080/openidconnect-rs/\
                 rp-response_type-code/token"
                    .to_string(),
            )
            .unwrap(),
        ))
        .set_id_token_encryption_alg_values_supported(Some(all_encryption_algs.clone()))
        .set_userinfo_encryption_alg_values_supported(Some(all_encryption_algs))
        .set_userinfo_encryption_enc_values_supported(Some(vec![
            CoreJweContentEncryptionAlgorithm::Aes128CbcHmacSha256,
            CoreJweContentEncryptionAlgorithm::Aes192CbcHmacSha384,
            CoreJweContentEncryptionAlgorithm::Aes256CbcHmacSha512,
            CoreJweContentEncryptionAlgorithm::Aes128Gcm,
            CoreJweContentEncryptionAlgorithm::Aes192Gcm,
            CoreJweContentEncryptionAlgorithm::Aes256Gcm,
        ]))
        .set_acr_values_supported(Some(vec![AuthenticationContextClass::new(
            "PASSWORD".to_string(),
        )]));

        new_provider_metadata
    }

    pub fn get_id_token() -> CoreIdToken {
        CoreIdToken::new(
            CoreIdTokenClaims::new(
                IssuerUrl::new("https://accounts.example.com".to_string()).unwrap(),
                vec![Audience::new("client-id-123".to_string())],
                Utc::now() + Duration::seconds(300),
                Utc::now(),
                StandardClaims::new(SubjectIdentifier::new(
                    "5f83e0ca-2b8e-4e8c-ba0a-f80fe9bc3632".to_string(),
                ))
                .set_email(Some(EndUserEmail::new("bob@example.com".to_string())))
                .set_email_verified(Some(true)),
                EmptyAdditionalClaims {},
            ),
            &CoreRsaPrivateSigningKey::from_pem("", Some(JsonWebKeyId::new("key1".to_string())))
                .expect("Invalid RSA private key"),
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            Some(&AccessToken::new("access_token".to_string())),
            None,
        )
        .unwrap()
    }
}

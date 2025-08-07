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

use crate::gateway_api_definition::http::{CompiledHttpApiDefinition, HttpApiDefinition};
use crate::internal::get_preflight_from_response;
use crate::security::TestIdentityProvider;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::auth::Namespace;
use golem_common::model::{AccountId, ComponentId, IdempotencyKey, ProjectId};
use golem_service_base::model::ComponentName;
use golem_worker_service::gateway_api_definition::http::RouteCompilationErrors;
use golem_worker_service::gateway_execution::auth_call_back_binding_handler::DefaultAuthCallBack;
use golem_worker_service::gateway_execution::gateway_http_input_executor::{
    DefaultGatewayInputExecutor, GatewayHttpInputExecutor,
};
use golem_worker_service::gateway_execution::gateway_session::{
    GatewaySession, GatewaySessionStore,
};
use golem_worker_service::gateway_middleware::HttpCors;
use golem_worker_service::gateway_request::http_request::ApiInputPath;
use golem_worker_service::gateway_security::{Provider, SecurityScheme, SecuritySchemeIdentifier};
use golem_worker_service::service::gateway::api_definition_validator::ValidationErrors;
use golem_worker_service::service::gateway::{ComponentView, ConversionContext};
use golem_worker_service::{api, gateway_api_definition};
use http::header::{HOST, LOCATION, ORIGIN};
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use poem::{Request, Response};
use serde_json::{Number, Value as JsonValue};
use std::sync::Arc;
use test_r::test;
use url::Url;
use uuid::uuid;

test_r::enable!();

// The tests that focus on end to end workflow of API Gateway, without involving any real workers,
// and stays independent of other modules.
// Workflow: Given an API request and an API specification,
// execute the API request and return the TestResponse (instead of poem::Response)
// Similar to types having ToResponse<poem::Response>
// there are instances of ToResponse<TestResponse> for them in the internal module of tests.
// Example: RibResult has an instance of `ToResponse<TestResponse>`.
// The tests skips validation and transformations done at the service side.
async fn execute(
    api_request: Request,
    api_specification: &HttpApiDefinition,
    session_store: &GatewaySessionStore,
    test_identity_provider: &TestIdentityProvider,
) -> Response {
    // Compile the API definition
    let compiled = CompiledHttpApiDefinition::from_http_api_definition(
        api_specification,
        &internal::get_component_metadata(),
        &test_namespace(),
        &(Box::new(TestConversionContext) as Box<dyn ConversionContext>),
    )
    .expect("Failed to compile API definition");

    let test_executor = DefaultGatewayInputExecutor::new(
        internal::get_test_rib_interpreter(),
        internal::get_test_file_server_binding_handler(),
        Arc::new(DefaultAuthCallBack),
        internal::get_test_http_handler_binding_handler(),
        internal::get_test_swagger_binding_handler(),
        Arc::new(internal::TestApiDefinitionLookup::new(compiled)),
        Arc::clone(session_store),
        Arc::new(test_identity_provider.clone()),
    );

    test_executor.execute_http_request(api_request).await
}

struct TestConversionContext;

#[async_trait]
impl ConversionContext for TestConversionContext {
    async fn component_by_name(&self, name: &ComponentName) -> Result<ComponentView, String> {
        if name.0 == "test-component" {
            Ok(ComponentView {
                name: ComponentName("test-component".to_string()),
                id: ComponentId(uuid!("0b6d9cd8-f373-4e29-8a5a-548e61b868a5")),
                latest_version: 0,
            })
        } else {
            Err("component not found".to_string())
        }
    }
    async fn component_by_id(&self, component_id: &ComponentId) -> Result<ComponentView, String> {
        match component_id.to_string().as_str() {
            "0b6d9cd8-f373-4e29-8a5a-548e61b868a5" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("test-component".to_string()),
                latest_version: 0,
            }),
            _ => Err("component not found".to_string()),
        }
    }
}

struct EmptyTestConversionContext;

#[async_trait]
impl ConversionContext for EmptyTestConversionContext {
    async fn component_by_name(&self, _name: &ComponentName) -> Result<ComponentView, String> {
        unimplemented!()
    }
    async fn component_by_id(&self, _component_id: &ComponentId) -> Result<ComponentView, String> {
        unimplemented!()
    }
}

fn test_namespace() -> Namespace {
    Namespace {
        account_id: AccountId {
            value: uuid!("a92803c1-186a-4367-bc00-23faffb5c932").to_string(),
        },
        project_id: ProjectId(uuid!("44f28456-d0c2-45d2-aaad-6e85462b6f18")),
    }
}

#[test]
async fn test_api_def_with_resource_1() {
    let api_request = get_gateway_request("/foo/mystore", None, &HeaderMap::new(), JsonValue::Null);

    // These functions get-user-name and get-currency produce the same result
    let response_mapping = r#"
        let email = "user@test.com";
        let temp_worker = instance("accounts_proxy");
        let user = temp_worker.get-user-name(email);
        let store_name = request.path.store;
        let store_worker_name = "${user}_${store_name}";
        let worker = instance(store_worker_name);
        let store = worker.store(user);
        let generated_user_name = store.add-user(user);
        let result = store.get-currency();

        match result {
          ok(currency) => { status: 200, body: currency, headers: { user: generated_user_name } },
          err(error) => { status: 400, body: "Failed to get currency: ${error}" }
        }
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{store}", response_mapping).await;

    let session_store: Arc<dyn GatewaySession + Sync + Send> = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let status = response.status();
    let user_name = response.headers().get("user").unwrap().to_str().unwrap();
    assert_eq!(user_name, "test-user-generated");

    let message = response.into_body().into_string().await.unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(message, "USD");
}

#[test]
async fn test_api_def_with_single_query_param() {
    let api_request =
        get_gateway_request("/foo?userid=jon", None, &HeaderMap::new(), JsonValue::Null);

    let response_mapping = r#"
       let user_id = request.query.userid;
       let worker-name = "shopping-cart-${user_id}";
       let worker-instance = instance(worker-name);
       let response = worker-instance.get-cart-contents(user_id, "bar");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo?{userid}", response_mapping).await;

    let session_store: Arc<dyn GatewaySession + Sync + Send> = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (test_response.function_name, test_response.function_params);

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("jon".to_string()),
            JsonValue::String("bar".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_multiple_query_params() {
    let api_request = get_gateway_request(
        "/foo?userid=jon&country=usa",
        None,
        &HeaderMap::new(),
        JsonValue::Null,
    );

    let response_mapping = r#"
       let user_id = request.query.userid;
       let country = request.query.country;
       let worker-name = "shopping-cart-${user_id}";
       let worker-instance = instance(worker-name);
       let response = worker-instance.get-cart-contents(user_id, country);
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo?{userid}&{country}", response_mapping).await;

    let session_store: Arc<dyn GatewaySession + Sync + Send> = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (test_response.function_name, test_response.function_params);

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("jon".to_string()),
            JsonValue::String("usa".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_query_and_path_params() {
    let api_request = get_gateway_request(
        "/foo/jon?country=usa",
        None,
        &HeaderMap::new(),
        JsonValue::Null,
    );

    let response_mapping = r#"
       let user_id = request.path.user-id;
       let country = request.query.country;
       let worker-name = "shopping-cart-${user_id}-${country}";
       let worker-instance = instance(worker-name);
       let response = worker-instance.get-cart-contents(user_id, country);
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}?{country}", response_mapping).await;

    let session_store: Arc<dyn GatewaySession + Sync + Send> = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (test_response.function_name, test_response.function_params);

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("jon".to_string()),
            JsonValue::String("usa".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_invalid_path_lookup() {
    let response_mapping = r#"
       let user-id = request.path.user-id;
       let worker-name = "shopping-cart-${user-id}";
       let worker-instance = instance(worker-name);
       let response = worker-instance.get-cart-contents(user-id, "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/bar", response_mapping).await;

    let result = CompiledHttpApiDefinition::from_http_api_definition(
        &api_specification,
        &internal::get_component_metadata(),
        &test_namespace(),
        &(Box::new(TestConversionContext) as Box<dyn ConversionContext>),
    )
    .unwrap_err();

    let expected = RouteCompilationErrors::ValidationError(
        ValidationErrors {
            errors: vec!["Following request.path lookups in response mapping rib script is not present in API route: user-id".to_string()],
        }
    );

    assert_eq!(result, expected)
}

#[test]
async fn test_api_def_with_invalid_query_and_path_lookup() {
    let response_mapping = r#"
       let user-id = request.path.user-id;
       let country = request.query.country;
       let worker-name = "shopping-cart-${user-id}-${country}";
       let worker-instance = instance(worker-name);
       let response = worker-instance.get-cart-contents(user-id, country);
      response
    "#;

    let api_specification1: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/bar", response_mapping).await;

    let result1 = CompiledHttpApiDefinition::from_http_api_definition(
        &api_specification1,
        &internal::get_component_metadata(),
        &test_namespace(),
        &(Box::new(TestConversionContext) as Box<dyn ConversionContext>),
    )
    .unwrap_err();

    let expected = RouteCompilationErrors::ValidationError(
        ValidationErrors {
            errors: vec![
                "Following request.query lookups in response mapping rib script is not present in API route: country".to_string(),
                "Following request.path lookups in response mapping rib script is not present in API route: user-id".to_string(),
            ],
        }
    );

    assert_eq!(&result1, &expected);

    let api_specification2 =
        get_api_def_with_worker_binding("/foo/{user  -  id}?{cou ntry}", response_mapping).await;

    let result2 = CompiledHttpApiDefinition::from_http_api_definition(
        &api_specification2,
        &internal::get_component_metadata(),
        &test_namespace(),
        &(Box::new(TestConversionContext) as Box<dyn ConversionContext>),
    )
    .unwrap_err();

    assert_eq!(&result2, &expected)
}

#[test]
async fn test_api_def_with_path_parameters_in_space() {
    let response_mapping = r#"
       let user_id = request.path.user-id;
       let country = request.query.country;
       let worker-name = "shopping-cart-${user_id}-${country}";
       let worker-instance = instance(worker-name);
       let response = worker-instance.get-cart-contents(user_id, country);
      response
    "#;

    // user-id and country is having space before and after
    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{ user-id   }?{    country }", response_mapping)
            .await;

    let api_request = get_gateway_request(
        "/foo/jon?country=usa",
        None,
        &HeaderMap::new(),
        JsonValue::Null,
    );

    let session_store: Arc<dyn GatewaySession + Sync + Send> = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (test_response.function_name, test_response.function_params);

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("jon".to_string()),
            JsonValue::String("usa".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_invalid_query_lookup() {
    let response_mapping = r#"
       let user-id = request.query.user-id;
       let worker-name = "shopping-cart-${user-id}";
       let worker-instance = instance(worker-name);
       let response = worker-instance.get-cart-contents(user-id, "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{userid}", response_mapping).await;

    let result = CompiledHttpApiDefinition::from_http_api_definition(
        &api_specification,
        &internal::get_component_metadata(),
        &test_namespace(),
        &(Box::new(TestConversionContext) as Box<dyn ConversionContext>),
    )
    .unwrap_err();

    let expected = RouteCompilationErrors::ValidationError(
        ValidationErrors {
            errors: vec!["Following request.query lookups in response mapping rib script is not present in API route: user-id".to_string()],
        }
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_request_path_0() {
    let empty_headers = HeaderMap::new();

    let api_request =
        get_gateway_request("/foo/foo_value/1", None, &empty_headers, JsonValue::Null);

    let response_mapping = r#"
         let bar-value: u32 = request.path.bar;
         let worker = instance(request.path.foo);
         let response = worker.add-item(bar-value, "bar");
         response
        "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{foo}/{bar}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "foo_value".to_string(),
        "golem:it/api.{add-item}".to_string(),
        JsonValue::Array(vec![
            JsonValue::Number(1.into()),
            JsonValue::String("bar".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

// A test where the input path is a number, and the rib script refers to request.path.* as a u64
#[test]
async fn test_api_def_with_request_path_only_1() {
    let api_request = get_gateway_request("/foo/1", None, &HeaderMap::new(), JsonValue::Null);

    let response_mapping = r#"
       let id: u64 = request.path.user-id;
       let worker-name = "shopping-cart-${id}";
       let worker-instance = instance(worker-name);
       let response = worker-instance.get-cart-contents("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store: Arc<dyn GatewaySession + Sync + Send> = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (test_response.function_name, test_response.function_params);

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("a".to_string()),
            JsonValue::String("b".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

// A test where the input path, query and header values are numbers,
// but the rib script requires these inputs to be string
#[test]
async fn test_api_def_with_request_path_only_2() {
    let mut header_map = HeaderMap::new();
    header_map.insert("project-id", HeaderValue::from_static("4"));

    let api_request =
        get_gateway_request("/foo/1/2?account-id=3", None, &header_map, JsonValue::Null);

    let response_mapping = r#"
       let user-id = request.path.user-id;
       let cart-id: string = request.path.cart-id;
       let account-id: string = request.query.account-id;
       let project-id: string = request.headers.project-id;
       let id = "${user-id}-${cart-id}-${account-id}-${project-id}";
       let worker-name = "shopping-cart-${id}";
       let worker-instance = instance(worker-name);
       let response = worker-instance.get-cart-contents(id, "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}/{cart-id}?{account-id}", response_mapping)
            .await;

    let session_store: Arc<dyn GatewaySession + Sync + Send> = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (test_response.function_name, test_response.function_params);

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("1-2-3-4".to_string()),
            JsonValue::String("b".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

// A test where the path parameter is a string,
// but the rib script refers to request.path.* as a number
#[test]
async fn test_legacy_api_def_with_invalid_input_1() {
    // RibInput (request.path.user-id) to form worker name is expected to be a u64
    let response_mapping = r#"
      let id: u64 = request.path.user-id;
      let worker-name = "shopping-cart-${id}";
      let worker = instance(worker-name);
      let response = worker.get-cart-contents("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store: Arc<dyn GatewaySession + Sync + Send> = internal::get_session_store();

    // user-id is a string, but the worker-name rib expressions expects it to be a u64
    let api_request =
        get_gateway_request("/foo/bar", None, &HeaderMap::new(), serde_json::Value::Null);

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// A test where the path parameter is a string,
// but the rib script refers to request.path.* as a number
#[test]
async fn test_api_def_with_invalid_input_1() {
    let response_mapping = r#"
      let id: u64 = request.path.user-id;
      let worker = instance("shopping-cart-${id}");
      let response = worker.get-cart-contents("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store: Arc<dyn GatewaySession + Sync + Send> = internal::get_session_store();

    // user-id is a string, but the worker-name rib expressions expects it to be a u64
    let api_request =
        get_gateway_request("/foo/bar", None, &HeaderMap::new(), serde_json::Value::Null);

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[test]
async fn test_api_def_with_request_body_0() {
    let empty_headers = HeaderMap::new();

    let mut request_body: serde_json::Map<String, JsonValue> = serde_json::Map::new();

    request_body.insert("foo_key".to_string(), JsonValue::Number(Number::from(1)));

    request_body.insert(
        "bar_key".to_string(),
        JsonValue::String("bar_value".to_string()),
    );

    let api_request = get_gateway_request(
        "/foo/john",
        None,
        &empty_headers,
        JsonValue::Object(request_body),
    );

    let response_mapping = r#"
         let worker = instance(request.body.bar_key);
         let response = worker.add-item(request.body.foo_key, "bar");
         response
        "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "bar_value".to_string(),
        "golem:it/api.{add-item}".to_string(),
        JsonValue::Array(vec![
            JsonValue::Number(1.into()),
            JsonValue::String("bar".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_request_body_1() {
    let empty_headers = HeaderMap::new();

    let mut request_body: serde_json::Map<String, JsonValue> = serde_json::Map::new();

    request_body.insert("foo_key".to_string(), JsonValue::Number(Number::from(1)));

    request_body.insert(
        "bar_key".to_string(),
        JsonValue::String("bar_value".to_string()),
    );

    let api_request = get_gateway_request(
        "/foo/john",
        None,
        &empty_headers,
        JsonValue::Object(request_body),
    );

    let response_mapping = r#"
         let userid: string = request.path.user-id;
         let res = if userid == "john" then 1:u64 else 0: u64;
         let worker = instance("shopping-cart-${res}");
         let param1 = request.body.foo_key;
         let param2 = request.body.bar_key;
         let response = worker.add-item(param1, param2);

         response
        "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{add-item}".to_string(),
        JsonValue::Array(vec![
            JsonValue::Number(1.into()),
            JsonValue::String("bar_value".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_request_body_2() {
    let empty_headers = HeaderMap::new();

    let api_request = get_gateway_request(
        "/foo/2",
        None,
        &empty_headers,
        JsonValue::String("address".to_string()),
    );

    let response_mapping = r#"
        let userid: u64 = request.path.user-id;
        let max: u64 = 100;
        let zero: u64 = 0;
        let one: u64 = 1;
        let res = if userid > max then zero else one;
        let worker-name = "shopping-cart-${res}";
        let worker = instance(worker-name);
        let response = worker.get-cart-contents(request.body, request.body);
        response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store = internal::get_session_store();

    let test_response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(test_response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("address".to_string()),
            JsonValue::String("address".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

// A test where one of the keys in the request body is expected to be a number
// based on rib script, but the actual value is a string
#[test]
async fn test_api_def_with_invalid_request_body() {
    let empty_headers = HeaderMap::new();

    let mut request_body: serde_json::Map<String, JsonValue> = serde_json::Map::new();

    request_body.insert("foo_key".to_string(), JsonValue::String("1".to_string()));

    request_body.insert(
        "bar_key".to_string(),
        JsonValue::String("bar_value".to_string()),
    );

    let api_request = get_gateway_request(
        "/foo/john",
        None,
        &empty_headers,
        JsonValue::Object(request_body),
    );

    let response_mapping = r#"
         let userid: string = request.path.user-id;
         let res = if userid == "john" then 1:u64 else 0: u64;
         let worker = instance("shopping-cart-${res}");
         let param1 = request.body.foo_key;
         let param2 = request.body.bar_key;
         let response = worker.add-item(param1, param2);

         response
        "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let status = response.status();

    let body = response.into_body().into_string().await.unwrap();

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, "invalid http request body\ninvalid value for key foo_key: expected number, found string\nexpected request body: record { bar_key: string, foo_key: u32 }");
}

#[test]
async fn test_api_def_with_security() {
    let empty_headers = HeaderMap::new();
    let api_request = get_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let response_mapping = r#"
      let id: u64 = request.path.user-id;
      let worker = instance("shopping-cart-${id}");
      let response = worker.get-cart-contents("a", "b");
      let email: string = request.auth.email;
      { body: response, headers: {email: email} }
    "#;

    let identity_provider = TestIdentityProvider::get_provider_with_valid_id_token();

    let auth_call_back_url =
        RedirectUrl::new("http://localhost/auth/callback".to_string()).unwrap();

    let api_specification: HttpApiDefinition = get_api_def_with_security(
        "/foo/{user-id}",
        response_mapping,
        &auth_call_back_url,
        &identity_provider,
    )
    .await;

    let session_store = internal::get_session_store();

    let initial_response_to_identity_provider = execute(
        api_request,
        &api_specification,
        &session_store,
        &identity_provider,
    )
    .await;

    let initial_redirect_response_headers = initial_response_to_identity_provider.headers();

    let initial_redirect_location = initial_redirect_response_headers
        .get(LOCATION)
        .expect("Expecting location")
        .to_str()
        .expect("Location should be a string");

    let url = Url::parse(initial_redirect_location)
        .expect("Expect the initial redirection to be a full URL");

    let query_components = ApiInputPath::query_components_from_str(url.query().unwrap_or_default());

    let initial_redirect_data = security::get_initial_redirect_data(&query_components);

    let actual_auth_call_back_url = internal::decode_url(&initial_redirect_data.auth_call_back_url);

    assert_eq!(initial_redirect_data.response_type, "code");
    assert_eq!(initial_redirect_data.client_id, "client_id_foo");
    assert_eq!(initial_redirect_data.scope, "openid+openid+user+email");
    assert_eq!(initial_redirect_data.state, "token"); // only for testing
    assert_eq!(initial_redirect_data.nonce, "nonce"); // only for testing
    assert_eq!(
        // The url embedded in the initial redirect should be the same as the redirect url
        // specified in the security scheme. Note that security scheme will have a full
        // redirect URL (auth call back URL)
        Url::parse(&actual_auth_call_back_url)
            .expect("Auth call back URL should be a full valid URL"),
        auth_call_back_url.url().clone()
    );

    // Manually create the request to hit auth_call_back endpoint by assuming we are identity-provider
    let call_back_request_from_identity_provider =
        security::request_from_identity_provider_to_auth_call_back_endpoint(
            initial_redirect_data.state.as_str(),
            "foo_code", // Decided by IdentityProvider
            initial_redirect_data.scope.as_str(),
            &auth_call_back_url.to_string(),
            "localhost",
        );

    // Execute it against the API Gateway
    // If successful, then it implies auth call back is successful and we get another redirect response.
    // This time, the redirect response will have a location that points to the original protected resource.
    let final_redirect_response = execute(
        call_back_request_from_identity_provider,
        &api_specification,
        &session_store,
        &identity_provider,
    )
    .await;

    let redirect_response_headers = final_redirect_response.headers();

    // Manually calling it back as we are the browser
    let api_request = security::create_request_from_redirect(redirect_response_headers).await;

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &identity_provider,
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.function_name,
        test_response.function_params,
        test_response.user_email,
    );

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("a".to_string()),
            JsonValue::String("b".to_string()),
        ]),
        Some("bob@example.com".to_string()),
    );

    assert_eq!(result, expected);
}

// A test where security is enabled but the signatures don't match
#[test]
async fn test_api_def_with_security_with_invalid_signatures() {
    let empty_headers = HeaderMap::new();
    let api_request = get_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let response_mapping = r#"
      let id: u64 = request.path.user-id;
      let worker = instance("shopping-cart-${id}");
      let response = worker.get-cart-contents("a", "b");
      response
    "#;

    let auth_call_back_url =
        RedirectUrl::new("http://localhost/auth/callback".to_string()).unwrap();

    let invalid_identity_provider_resolver =
        TestIdentityProvider::get_provider_with_invalid_signatures();

    let api_specification: HttpApiDefinition = get_api_def_with_security(
        "/foo/{user-id}",
        response_mapping,
        &auth_call_back_url,
        &invalid_identity_provider_resolver,
    )
    .await;

    let session_store = internal::get_session_store();

    let initial_redirect_response = execute(
        api_request,
        &api_specification,
        &session_store,
        &invalid_identity_provider_resolver,
    )
    .await;

    let initial_redirect_response_headers = initial_redirect_response.headers();

    let location = initial_redirect_response_headers
        .get(LOCATION)
        .expect("Expecting location")
        .to_str()
        .expect("Location should be a string");

    let url = Url::parse(location).expect("Expect the initial redirection to be a full URL");

    let query_components = ApiInputPath::query_components_from_str(url.query().unwrap_or_default());

    let initial_redirect_response_info = security::get_initial_redirect_data(&query_components);

    let actual_auth_call_back_url =
        internal::decode_url(&initial_redirect_response_info.auth_call_back_url);

    let call_back_request_from_identity_provider =
        security::request_from_identity_provider_to_auth_call_back_endpoint(
            initial_redirect_response_info.state.as_str(),
            "foo_code", // Decided by IdentityProvider
            initial_redirect_response_info.scope.as_str(),
            &actual_auth_call_back_url.to_string(),
            "localhost",
        );

    let auth_call_back_response = execute(
        call_back_request_from_identity_provider,
        &api_specification,
        &session_store,
        &invalid_identity_provider_resolver,
    )
    .await;

    // The auth call back endpoint results in another redirect response
    // which will now have the actual URL to the original protected resource
    let redirect_response_headers = auth_call_back_response.headers();

    // Manually creating the request to hit the original endpoint, as a browser
    let input_http_request =
        security::create_request_from_redirect(redirect_response_headers).await;

    // Hitting the endpoint with an expired token
    let test_response_from_actual_endpoint = execute(
        input_http_request,
        &api_specification,
        &session_store,
        &invalid_identity_provider_resolver,
    )
    .await;

    // The final redirect from the protected endpoint should be the same as
    // the initial redirect for unauthenticated request
    assert_eq!(
        test_response_from_actual_endpoint.status(),
        StatusCode::UNAUTHORIZED
    )
}

// A test where security is enabled but the sessions is expired
#[test]
async fn test_api_def_with_security_with_expired_session() {
    let empty_headers = HeaderMap::new();
    let api_request = get_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let response_mapping = r#"
      let id: u64 = request.path.user-id;
      let worker = instance("shopping-cart-${id}");
      let response = worker.get-cart-contents("a", "b");
      response
    "#;

    let auth_call_back_url =
        RedirectUrl::new("http://localhost/auth/callback".to_string()).unwrap();

    let invalid_identity_provider = TestIdentityProvider::get_provider_with_valid_id_token();

    let api_specification: HttpApiDefinition = get_api_def_with_security(
        "/foo/{user-id}",
        response_mapping,
        &auth_call_back_url,
        &invalid_identity_provider,
    )
    .await;

    // Until auth call back, we will use a valid session store
    let session_store = internal::get_session_store();

    let initial_response_to_identity_provider = execute(
        api_request,
        &api_specification,
        &session_store,
        &invalid_identity_provider,
    )
    .await;

    let initial_redirect_response_headers = initial_response_to_identity_provider.headers();

    let initial_redirect_location = initial_redirect_response_headers
        .get(LOCATION)
        .expect("Expecting location")
        .to_str()
        .expect("Location should be a string");

    let initial_redirect_url = Url::parse(initial_redirect_location)
        .expect("Expect the initial redirection to be a full URL");

    let initial_redirect_data = security::get_initial_redirect_data(
        &ApiInputPath::query_components_from_str(initial_redirect_url.query().unwrap_or_default()),
    );

    let actual_auth_call_back_url = internal::decode_url(&initial_redirect_data.auth_call_back_url);

    let call_back_request_from_identity_provider =
        security::request_from_identity_provider_to_auth_call_back_endpoint(
            initial_redirect_data.state.as_str(),
            "foo_code", // Decided by IdentityProvider
            initial_redirect_data.scope.as_str(),
            &actual_auth_call_back_url.to_string(),
            "localhost",
        );

    let auth_call_back_response = execute(
        call_back_request_from_identity_provider,
        &api_specification,
        &session_store,
        &invalid_identity_provider,
    )
    .await;

    // The auth call back endpoint results in another redirect response
    // which will now have the actual URL to the original protected resource
    let redirect_response_headers = auth_call_back_response.headers();

    // Manually calling it back as we are the browser
    let api_request_from_browser =
        security::create_request_from_redirect(redirect_response_headers).await;

    // Post auth call, we simulate the situation of an expired session
    let session_store = internal::get_session_store_with_zero_ttl();

    // Hitting the protected resource with an expired token
    let test_response_from_actual_endpoint = execute(
        api_request_from_browser,
        &api_specification,
        &session_store,
        &invalid_identity_provider,
    )
    .await;

    // And it should be a redirect which is same as the initial redirect to identity provider
    let final_redirect = test_response_from_actual_endpoint.headers();

    // The final redirect from the protected endpoint should be the same as
    // the initial redirect for unauthenticated request
    assert!(final_redirect.contains_key(LOCATION));
    assert_eq!(final_redirect, initial_redirect_response_headers)
}

// A test where security is enabled but the id-token is expired in the session
#[test]
async fn test_api_def_with_security_with_expired_token() {
    let empty_headers = HeaderMap::new();
    let api_request = get_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let response_mapping = r#"
      let id: u64 = request.path.user-id;
      let worker = instance("shopping-cart-${id}");
      let response = worker.get-cart-contents("a", "b");
      response
    "#;

    let auth_call_back_url =
        RedirectUrl::new("http://localhost/auth/callback".to_string()).unwrap();

    let invalid_identity_provider_resolver =
        TestIdentityProvider::get_provider_with_expired_id_token();

    let api_specification: HttpApiDefinition = get_api_def_with_security(
        "/foo/{user-id}",
        response_mapping,
        &auth_call_back_url,
        &invalid_identity_provider_resolver,
    )
    .await;

    let session_store = internal::get_session_store();

    let initial_response_to_identity_provider = execute(
        api_request,
        &api_specification,
        &session_store,
        &invalid_identity_provider_resolver,
    )
    .await;

    let initial_redirect_response_headers = initial_response_to_identity_provider.headers();

    let initial_redirect_location = initial_redirect_response_headers
        .get(LOCATION)
        .expect("Expecting location")
        .to_str()
        .expect("Location should be a string");

    let initial_redirect_url = Url::parse(initial_redirect_location)
        .expect("Expect the initial redirection to be a full URL");

    let initial_redirect_data = security::get_initial_redirect_data(
        &ApiInputPath::query_components_from_str(initial_redirect_url.query().unwrap_or_default()),
    );

    let actual_auth_call_back_url = internal::decode_url(&initial_redirect_data.auth_call_back_url);

    let call_back_request_from_identity_provider =
        security::request_from_identity_provider_to_auth_call_back_endpoint(
            initial_redirect_data.state.as_str(),
            "foo_code", // Decided by IdentityProvider
            initial_redirect_data.scope.as_str(),
            &actual_auth_call_back_url.to_string(),
            "localhost",
        );

    let auth_call_back_response = execute(
        call_back_request_from_identity_provider,
        &api_specification,
        &session_store,
        &invalid_identity_provider_resolver,
    )
    .await;

    // The auth call back endpoint results in another redirect response
    // which will now have the actual URL to the original protected resource
    let redirect_response_headers = auth_call_back_response.headers();

    // Manually calling it back as we are the browser
    let api_request_from_browser =
        security::create_request_from_redirect(redirect_response_headers).await;

    // Hitting the protected resource with an expired token
    let test_response_from_actual_endpoint = execute(
        api_request_from_browser,
        &api_specification,
        &session_store,
        &invalid_identity_provider_resolver,
    )
    .await;

    // And it should be a redirect which is same as the initial redirect to identity provider
    let final_redirect = test_response_from_actual_endpoint.headers();

    // The final redirect from the protected endpoint should be the same as
    // the initial redirect for unauthenticated request
    assert!(final_redirect.contains_key(LOCATION));
    assert_eq!(final_redirect, initial_redirect_response_headers)
}

/// regression test for: https://zivergeteam.slack.com/archives/C057S2E4XT5/p1741430776997879
#[test]
async fn test_api_def_with_security_with_relative_callback() {
    let empty_headers = HeaderMap::new();
    let api_request = get_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let response_mapping = r#"
      let id: u64 = request.path.user-id;
      let worker = instance("shopping-cart-${id}");
      let response = worker.get-cart-contents("a", "b");
      let email: string = request.auth.email;
      { body: response, headers: {email: email} }
    "#;

    let identity_provider = TestIdentityProvider::get_provider_with_valid_id_token();

    let base_url = Url::parse("https://localhost").unwrap();
    let auth_call_back_url = "/auth/callback";
    let absolute_auth_call_back_url =
        RedirectUrl::from_url(base_url.join(auth_call_back_url).unwrap());

    let api_specification: HttpApiDefinition = get_api_def_with_security(
        "/foo/{user-id}",
        response_mapping,
        &absolute_auth_call_back_url,
        &identity_provider,
    )
    .await;

    let session_store = internal::get_session_store();

    let initial_response_to_identity_provider = execute(
        api_request,
        &api_specification,
        &session_store,
        &identity_provider,
    )
    .await;

    let initial_redirect_response_headers = initial_response_to_identity_provider.headers();

    let initial_redirect_location = initial_redirect_response_headers
        .get(LOCATION)
        .expect("Expecting location")
        .to_str()
        .expect("Location should be a string");

    let url = Url::parse(initial_redirect_location)
        .expect("Expect the initial redirection to be a full URL");

    let query_components = ApiInputPath::query_components_from_str(url.query().unwrap_or_default());

    let initial_redirect_data = security::get_initial_redirect_data(&query_components);

    // Manually create the request to hit auth_call_back endpoint by assuming we are identity-provider
    let call_back_request_from_identity_provider =
        security::request_from_identity_provider_to_auth_call_back_endpoint(
            initial_redirect_data.state.as_str(),
            "foo_code", // Decided by IdentityProvider
            initial_redirect_data.scope.as_str(),
            auth_call_back_url,
            "localhost",
        );

    // Execute it against the API Gateway
    // If successful, then it implies auth call back is successful and we get another redirect response.
    // This time, the redirect response will have a location that points to the original protected resource.
    let final_redirect_response = execute(
        call_back_request_from_identity_provider,
        &api_specification,
        &session_store,
        &identity_provider,
    )
    .await;

    let redirect_response_headers = final_redirect_response.headers();

    // Manually calling it back as we are the browser
    let api_request = security::create_request_from_redirect(redirect_response_headers).await;

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &identity_provider,
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.function_name,
        test_response.function_params,
        test_response.user_email,
    );

    let expected = (
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("a".to_string()),
            JsonValue::String("b".to_string()),
        ]),
        Some("bob@example.com".to_string()),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_default_cors_preflight_1() {
    let empty_headers = HeaderMap::new();
    let api_request =
        get_preflight_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let api_specification: HttpApiDefinition =
        get_api_def_with_default_cors_preflight("/foo/{user-id}").await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let result = internal::get_preflight_from_response(response);

    let expected = HttpCors::default();

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_default_cors_preflight_2() {
    let empty_headers = HeaderMap::new();
    let preflight_request =
        get_preflight_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let api_request = get_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let response_mapping = r#"
       let id: u64 = request.path.user-id;
       let worker-name = "shopping-cart-${id}";
      let worker = instance(worker-name);
      let response = worker.get-cart-contents("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_with_default_cors_preflight_for_get_endpoint_resource(
            "/foo/{user-id}",
            response_mapping,
        )
        .await;

    let session_store = internal::get_session_store();

    let preflight_response = execute(
        preflight_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;
    let response_from_other_endpoint = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let expected_cors_preflight = HttpCors::default();

    let preflight_response = get_preflight_from_response(preflight_response);

    let actual_test_response =
        internal::get_test_response_for_worker_binding_with_cors(response_from_other_endpoint)
            .await;

    let allow_origin_in_actual_response = actual_test_response.cors_middleware_headers.unwrap();

    assert_eq!(preflight_response, expected_cors_preflight);
    assert_eq!(
        allow_origin_in_actual_response.cors_header_allow_origin,
        expected_cors_preflight.get_allow_origin()
    );
}

#[test]
async fn test_api_def_with_custom_cors_preflight_1() {
    let empty_headers = HeaderMap::new();
    let api_request =
        get_preflight_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let cors = HttpCors::from_parameters(
        Some("http://example.com".to_string()),
        Some("GET, POST, PUT, DELETE, OPTIONS".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some(true),
        Some(3600),
    )
    .unwrap();

    let api_specification: HttpApiDefinition =
        get_api_def_with_cors_preflight_route_only("/foo/{user-id}", &cors).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::get_provider_with_valid_id_token(),
    )
    .await;

    let result = internal::get_preflight_from_response(response);

    assert_eq!(result, cors);
}

#[test]
async fn test_api_def_with_custom_cors_preflight_2() {
    let empty_headers = HeaderMap::new();
    let preflight_request =
        get_preflight_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let api_request = get_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    let cors = HttpCors::from_parameters(
        Some("http://example.com".to_string()),
        Some("GET, POST, PUT, DELETE, OPTIONS".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some(true),
        Some(3600),
    )
    .unwrap();

    let response_mapping = r#"
      let id: u64 = request.path.user-id;
      let worker-name = "shopping-cart-${id}";
      let worker = instance(worker-name);
      let response = worker.get-cart-contents("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_preflight_and_get_endpoint("/foo/{user-id}", response_mapping, &cors)
            .await;

    let session_store = internal::get_session_store();

    let preflight_response = execute(
        preflight_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;
    let actual_response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let pre_flight_response = get_preflight_from_response(preflight_response);

    let actual_response =
        internal::get_test_response_for_worker_binding_with_cors(actual_response).await;

    let cors_headers_in_actual_response = actual_response
        .cors_middleware_headers
        .expect("Expecting cors headers in response");

    let allow_origin_in_actual_response = cors_headers_in_actual_response.cors_header_allow_origin;

    let expose_headers_in_actual_response = cors_headers_in_actual_response
        .cors_header_expose_headers
        .expect("Cors expose header missing in actual response");

    let allow_credentials_in_actual_response = cors_headers_in_actual_response
        .cors_header_allow_credentials
        .expect("Cors allow credentials missing in actual response");

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
async fn test_api_def_with_custom_cors_preflight_3() {
    let empty_headers = HeaderMap::new();
    let preflight_request =
        get_preflight_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    // We intentionally make the origin different for the get endpoint
    let mut api_request =
        get_gateway_request("/foo/1", None, &empty_headers, serde_json::Value::Null);

    api_request
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_static("http://wrongdomain.com"));

    let cors = HttpCors::from_parameters(
        Some("http://example.com".to_string()),
        Some("GET, POST, PUT, DELETE, OPTIONS".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some("Content-Type, Authorization".to_string()),
        Some(true),
        Some(3600),
    )
    .unwrap();

    let response_mapping = r#"
      let id: u64 = request.path.user-id;
      let worker-name = "shopping-cart-${id}";
      let worker = instance(worker-name);
      let response = worker.get-cart-contents("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_preflight_and_get_endpoint("/foo/{user-id}", response_mapping, &cors)
            .await;

    let session_store = internal::get_session_store();

    let preflight_response = execute(
        preflight_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;
    let actual_response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let pre_flight_response = get_preflight_from_response(preflight_response);

    assert_eq!(pre_flight_response, cors);

    // Since the origin is different for the get endpoint, we expect a forbidden response
    assert_eq!(actual_response.status(), StatusCode::FORBIDDEN);
}

#[test]
async fn test_api_def_with_path_and_query_1() {
    let empty_headers = HeaderMap::new();
    let api_request = get_gateway_request(
        "/foo/1",
        Some("token-id=jon"),
        &empty_headers,
        JsonValue::Null,
    );

    let response_mapping = r#"
        let x: u64 = request.path.user-id;
        let my-instance = instance("shopping-cart-${x}");
        let response = my-instance.get-cart-contents(request.query.token-id, request.query.token-id);
        response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}?{token-id}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("jon".to_string()),
            JsonValue::String("jon".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

// A test where input requirement of path and query is a string, but the actual inputs are numbers
#[test]
async fn test_api_def_with_path_and_query_2() {
    let empty_headers = HeaderMap::new();
    let api_request = get_gateway_request(
        "/foo/1",
        Some("token-id=2"),
        &empty_headers,
        JsonValue::Null,
    );

    // Default types for path and query parameters are string
    let response_mapping = r#"
        let user_id_from_path = request.path.user-id;
        let token_id_from_query = request.query.token-id;
        let my-instance = instance("shopping-cart-${user_id_from_path}");
        let response = my-instance.get-cart-contents(user_id_from_path, token_id_from_query);
        response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}?{token-id}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("1".to_string()),
            JsonValue::String("2".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_path_and_query_3() {
    let empty_headers = HeaderMap::new();
    let api_request = get_gateway_request(
        "/foo/1",
        None,
        &empty_headers,
        JsonValue::Object(serde_json::Map::from_iter(vec![(
            "age".to_string(),
            JsonValue::Number(serde_json::Number::from(10)),
        )])),
    );

    let response_mapping = r#"
      let n: u64 = 100;
      let age: u64 = request.body.age;
      let zero: u64 = 0; let one: u64 = 1;
      let res = if age > n then zero else one;
      let worker = instance("shopping-cart-${res}");
      let response = worker.get-cart-contents("a", "b");
      response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("a".to_string()),
            JsonValue::String("b".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_path_and_request_body_1() {
    let empty_headers = HeaderMap::new();

    let mut request_body: serde_json::Map<String, JsonValue> = serde_json::Map::new();

    request_body.insert(
        "foo_key".to_string(),
        JsonValue::String("foo_value".to_string()),
    );

    request_body.insert(
        "bar_key".to_string(),
        JsonValue::Array(vec![JsonValue::String("bar_value".to_string())]),
    );

    let api_request = get_gateway_request(
        "/foo/bar",
        None,
        &empty_headers,
        JsonValue::Object(request_body),
    );

    let response_mapping = r#"
         let userid: string = request.path.user-id;
         let res = if userid == "bar" then 1:u64 else 0: u64;
         let worker = instance("shopping-cart-${res}");
         let param1 = request.body.foo_key;
         let param2 = request.body.bar_key[0];
         let response = worker.get-cart-contents(param1, param2);

         response
        "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("foo_value".to_string()),
            JsonValue::String("bar_value".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_path_and_request_body_2() {
    let empty_headers = HeaderMap::new();

    let mut request_body: serde_json::Map<String, JsonValue> = serde_json::Map::new();

    request_body.insert(
        "foo_key".to_string(),
        JsonValue::String("foo_value".to_string()),
    );

    request_body.insert(
        "bar_key".to_string(),
        JsonValue::Array(vec![JsonValue::String("bar_value".to_string())]),
    );

    let api_request = get_gateway_request(
        "/foo/2",
        None,
        &empty_headers,
        JsonValue::Object(request_body.clone()),
    );

    let response_mapping = r#"
        let userid: u64 = request.path.user-id;
        let max: u64 = 100;
        let res = if userid > max then 0:u64 else 1:u64;
        let worker = instance("shopping-cart-${res}");
        let response = worker.get-cart-contents(request.body.foo_key, request.body.bar_key[0]);
        response
        "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{user-id}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("foo_value".to_string()),
            JsonValue::String("bar_value".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

// Test that ensures that the input requirement of path and query is a number, but the actual inputs are strings
// Also both request.header.value and request.headers.value works
// Along with these, it also makes use of parameters from request body
#[test]
async fn test_api_def_with_path_and_query_and_header_and_body() {
    let mut headers = HeaderMap::new();
    headers.insert("baz", HeaderValue::from_static("42"));
    headers.insert("qux", HeaderValue::from_static("qux_value"));

    let body = serde_json::json!({
        "quux": "quux_value"
    });

    let api_request = get_gateway_request("/foo/1", Some("bar=2"), &headers, body);

    // Default types for path and query parameters are string
    let response_mapping = r#"
        let path_foo = request.path.foo;
        let query_bar = request.query.bar;
        let header_baz = request.headers.baz;
        let header_qux = request.header.qux;
        let body_quux: string = request.body.quux;
        let arg1 = "${path_foo}-${query_bar}";
        let arg2 = "${header_baz}-${header_qux}-${body_quux}";
        let my-instance = instance("shopping-cart-${path_foo}");
        let response = my-instance.get-cart-contents(arg1, arg2);
        response
    "#;

    let api_specification: HttpApiDefinition =
        get_api_def_with_worker_binding("/foo/{foo}?{bar}", response_mapping).await;

    let session_store = internal::get_session_store();

    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    let test_response = internal::get_details_from_response(response).await;

    let result = (
        test_response.worker_name,
        test_response.function_name,
        test_response.function_params,
    );

    let expected = (
        "shopping-cart-1".to_string(),
        "golem:it/api.{get-cart-contents}".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("1-2".to_string()),
            JsonValue::String("42-qux_value-quux_value".to_string()),
        ]),
    );

    assert_eq!(result, expected);
}

#[test]
async fn test_api_def_with_idempotency_key() {
    async fn test_key(header_map: &HeaderMap, idempotency_key: Option<IdempotencyKey>) {
        let api_request =
            get_gateway_request("/getcartcontent/1", None, header_map, JsonValue::Null);

        let expression = r#"
            let x: u64 = request.path.cart-id;
            let my-instance = instance("shopping-cart-${x}");
            let response = my-instance.get-cart-contents("foo", "bar");
            response
            "#;

        let api_specification: HttpApiDefinition =
            get_api_def_with_worker_binding("/getcartcontent/{cart-id}", expression).await;

        let session_store = internal::get_session_store();

        let response = execute(
            api_request,
            &api_specification,
            &session_store,
            &TestIdentityProvider::default(),
        )
        .await;

        let test_response = internal::get_details_from_response(response).await;

        assert_eq!(test_response.idempotency_key, idempotency_key);
    }

    test_key(&HeaderMap::new(), None).await;
    let mut headers = HeaderMap::new();
    headers.insert("Idempotency-Key", HeaderValue::from_str("foo").unwrap());
    test_key(&headers, Some(IdempotencyKey::new("foo".to_string()))).await;
    let mut headers = HeaderMap::new();
    headers.insert("idempotency-key", HeaderValue::from_str("bar").unwrap());
    test_key(&headers, Some(IdempotencyKey::new("bar".to_string()))).await;
}

fn get_gateway_request(
    base_path: &str,
    query_path: Option<&str>,
    headers: &HeaderMap,
    req_body: serde_json::Value,
) -> Request {
    let full_uri = match query_path {
        Some(query) => format!("{}?{}", base_path.trim_end_matches('/'), query),
        None => base_path.to_string(),
    };

    // Construct the URI object

    let uri = Uri::builder()
        .scheme("http")
        .authority("localhost")
        .path_and_query(full_uri)
        .build()
        .unwrap();
    // Create the request
    let mut request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(req_body.to_string());

    // Add headers
    for (key, value) in headers.iter() {
        request.headers_mut().insert(key.clone(), value.clone());
    }

    request
        .headers_mut()
        .insert("host", "localhost".parse().unwrap());

    request
}

fn get_preflight_gateway_request(
    base_path: &str,
    query_path: Option<&str>,
    headers: &HeaderMap,
    req_body: JsonValue,
) -> Request {
    let full_uri = match query_path {
        Some(query) => format!("{}?{}", base_path.trim_end_matches('/'), query),
        None => base_path.to_string(),
    };

    let uri = Uri::builder()
        .scheme("http")
        .authority("localhost")
        .path_and_query(full_uri)
        .build()
        .unwrap();

    let mut request: Request = Request::builder()
        .method(Method::OPTIONS)
        .uri(uri)
        .body(req_body.to_string());

    for (key, value) in headers.iter() {
        request.headers_mut().insert(key.clone(), value.clone());
    }

    request
        .headers_mut()
        .insert("host", "localhost".parse().unwrap());

    request
}

async fn get_api_def_with_worker_binding(
    path_pattern: &str,
    rib_expression: &str,
) -> HttpApiDefinition {
    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Get
            path: {path_pattern}
            binding:
              type: wit-worker
              component:
                name: test-component
                version: 0
              response: '${{{rib_expression}}}'

        "#
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::dto::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request
            .into_core(&TestConversionContext.boxed())
            .await
            .unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();

    HttpApiDefinition::from_http_api_definition_request(
        &test_namespace(),
        core_request,
        create_at,
        &security::get_test_security_scheme_service(TestIdentityProvider::default()),
    )
    .await
    .unwrap()
}

// https://swagger.io/docs/specification/v3_0/authentication/openid-connect-discovery/
async fn get_api_def_with_security(
    path_pattern: &str,
    rib_expression: &str,
    auth_call_back_url: &RedirectUrl,
    test_identity_provider: &TestIdentityProvider,
) -> HttpApiDefinition {
    let security_scheme_identifier = SecuritySchemeIdentifier::new("openId1".to_string());

    let security_scheme_service =
        security::get_test_security_scheme_service(test_identity_provider.clone());

    let security_scheme = SecurityScheme::new(
        Provider::Google,
        security_scheme_identifier.clone(),
        ClientId::new("client_id_foo".to_string()),
        ClientSecret::new("client_secret_foo".to_string()),
        auth_call_back_url.clone(),
        vec![
            Scope::new("openid".to_string()),
            Scope::new("user".to_string()),
            Scope::new("email".to_string()),
        ],
    );

    // Make sure security scheme 1 is added to golem
    security_scheme_service
        .create(&test_namespace(), &security_scheme)
        .await
        .unwrap();

    let api_definition_yaml = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          security:
          - {security_scheme_identifier}
          routes:
          - method: Get
            path: {path_pattern}
            security: {security_scheme_identifier}
            binding:
              type: wit-worker
              component:
                name: test-component
                version: 0
              response: '${{{rib_expression}}}'
        "#
    );

    let user_facing_definition_request: api::dto::HttpApiDefinitionRequest =
        serde_yaml::from_str(api_definition_yaml.as_str()).unwrap();

    let core_definition_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        user_facing_definition_request
            .into_core(&TestConversionContext.boxed())
            .await
            .unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();

    HttpApiDefinition::from_http_api_definition_request(
        &test_namespace(),
        core_definition_request,
        create_at,
        &security_scheme_service,
    )
    .await
    .expect("Conversion of an HttpApiDefinitionRequest to HttpApiDefinition failed")
}

async fn get_api_def_with_default_cors_preflight(path_pattern: &str) -> HttpApiDefinition {
    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Options
            path: {path_pattern}
            binding:
              bindingType: cors-preflight
        "#,
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::dto::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request
            .into_core(&TestConversionContext.boxed())
            .await
            .unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    HttpApiDefinition::from_http_api_definition_request(
        &test_namespace(),
        core_request,
        create_at,
        &security::get_test_security_scheme_service(TestIdentityProvider::default()),
    )
    .await
    .unwrap()
}

async fn get_api_def_with_cors_preflight_route_only(
    path_pattern: &str,
    cors: &HttpCors,
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
    let http_api_definition_request: api::dto::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest = {
        http_api_definition_request
            .into_core(&EmptyTestConversionContext.boxed())
            .await
            .unwrap()
    };

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    HttpApiDefinition::from_http_api_definition_request(
        &test_namespace(),
        core_request,
        create_at,
        &security::get_test_security_scheme_service(TestIdentityProvider::default()),
    )
    .await
    .unwrap()
}

async fn get_api_def_with_preflight_and_get_endpoint(
    path_pattern: &str,
    rib_expression: &str,
    cors: &HttpCors,
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
              component:
                name: test-component
                version: 0
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
        rib_expression
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::dto::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request
            .into_core(&TestConversionContext.boxed())
            .await
            .unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    HttpApiDefinition::from_http_api_definition_request(
        &test_namespace(),
        core_request,
        create_at,
        &security::get_test_security_scheme_service(TestIdentityProvider::default()),
    )
    .await
    .unwrap()
}

async fn get_api_def_with_with_default_cors_preflight_for_get_endpoint_resource(
    path_pattern: &str,
    rib_expression: &str,
) -> HttpApiDefinition {
    let yaml_string = format!(
        r#"
          id: users-api
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Options
            path: {path_pattern}
            binding:
              bindingType: cors-preflight
          - method: Get
            path: {path_pattern}
            binding:
              type: wit-worker
              component:
                name: test-component
                version: 0
              response: '${{{rib_expression}}}'

        "#
    );

    // Serde is available only for user facing HttpApiDefinition
    let http_api_definition_request: api::dto::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request
            .into_core(&TestConversionContext.boxed())
            .await
            .unwrap();

    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    HttpApiDefinition::from_http_api_definition_request(
        &test_namespace(),
        core_request,
        create_at,
        &security::get_test_security_scheme_service(TestIdentityProvider::default()),
    )
    .await
    .unwrap()
}

mod internal {
    use async_trait::async_trait;
    use golem_common::model::auth::Namespace;
    use golem_common::model::component::VersionedComponentId;
    use golem_common::model::{ComponentId, IdempotencyKey};
    use golem_common::virtual_exports::http_incoming_handler::IncomingHttpRequest;
    use golem_wasm_ast::analysis::analysed_type::{field, handle, record, result, str, tuple, u32};
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
        AnalysedInstance, AnalysedResourceId, AnalysedResourceMode, AnalysedType, TypeHandle,
    };
    use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
    use golem_worker_service::gateway_api_definition::http::{
        CompiledHttpApiDefinition, ComponentDetails, ComponentMetadataDictionary,
    };
    use golem_worker_service::gateway_api_deployment::ApiSiteString;
    use golem_worker_service::gateway_binding::SwaggerUiBinding;
    use golem_worker_service::gateway_execution::api_definition_lookup::{
        ApiDefinitionLookupError, HttpApiDefinitionsLookup,
    };
    use golem_worker_service::gateway_execution::file_server_binding_handler::{
        FileServerBindingHandler, FileServerBindingResult,
    };
    use golem_worker_service::gateway_execution::gateway_session::{
        DataKey, DataValue, GatewaySession, GatewaySessionError, GatewaySessionStore, SessionId,
    };
    use golem_worker_service::gateway_execution::http_handler_binding_handler::{
        HttpHandlerBindingHandler, HttpHandlerBindingResult,
    };
    use golem_worker_service::gateway_execution::swagger_binding_handler::{
        SwaggerBindingHandler, SwaggerBindingResult, SwaggerBindingSuccess,
    };
    use golem_worker_service::gateway_execution::WorkerDetails;
    use golem_worker_service::gateway_execution::{
        GatewayResolvedWorkerRequest, GatewayWorkerRequestExecutor, WorkerRequestExecutorError,
        WorkerResponse,
    };
    use golem_worker_service::gateway_middleware::HttpCors;
    use golem_worker_service::gateway_rib_interpreter::{
        DefaultRibInterpreter, WorkerServiceRibInterpreter,
    };
    use http::header::{
        ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS,
        ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS,
        ACCESS_CONTROL_MAX_AGE,
    };
    use poem::Response;
    use rib::{ComponentDependencyKey, RibResult};
    use serde_json::Value as JsonValue;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    pub struct TestApiDefinitionLookup {
        pub api_definition: CompiledHttpApiDefinition,
    }

    impl TestApiDefinitionLookup {
        pub fn new(api_definition: CompiledHttpApiDefinition) -> Self {
            Self { api_definition }
        }
    }

    #[async_trait]
    impl HttpApiDefinitionsLookup for TestApiDefinitionLookup {
        async fn get(
            &self,
            _input: &ApiSiteString,
        ) -> Result<Vec<CompiledHttpApiDefinition>, ApiDefinitionLookupError> {
            Ok(vec![self.api_definition.clone()])
        }
    }

    // This worker-request-executor simply returns the same response to any worker function
    // which is record of the details of the worker request such as function name, arguments, worker name etc,
    // except for some specific functions which will return a specific valid response corresponding
    // to that function. This way, most of the tests can validate the correctness of the worker request
    // while others can validate the correctness of the real function result.
    pub struct TestApiGatewayWorkerRequestExecutor {}

    #[async_trait]
    impl GatewayWorkerRequestExecutor for TestApiGatewayWorkerRequestExecutor {
        // This test executor simply returns the worker request details itself as a type-annotated-value
        async fn execute(
            &self,
            resolved_worker_request: GatewayResolvedWorkerRequest,
        ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
            let function_name = resolved_worker_request.function_name.clone();

            if function_name == "bigw:shopping/api.{store.new}" {
                let uri = format!(
                    "urn:worker:71a31a33-28a5-4978-8a58-83424a149c8b/{}",
                    resolved_worker_request.worker_name.unwrap_or_default()
                );
                let handle = golem_wasm_rpc::Value::Handle {
                    uri,
                    resource_id: 0,
                };

                let handle_type = AnalysedType::Handle(TypeHandle {
                    resource_id: AnalysedResourceId(0),
                    mode: AnalysedResourceMode::Owned,
                    name: None,
                    owner: None,
                });

                let value_and_type = ValueAndType::new(handle, handle_type);

                return Ok(WorkerResponse::new(Some(value_and_type)));
            }

            if function_name.clone() == "bigw:shopping/api.{get-user-name}" {
                let value_and_type = ValueAndType::new(
                    golem_wasm_rpc::Value::String("test-user".to_string()),
                    str(),
                );

                return Ok(WorkerResponse::new(Some(value_and_type)));
            }

            if function_name == "bigw:shopping/api.{store.get-currency}" {
                let value_and_type = ValueAndType::new(
                    golem_wasm_rpc::Value::Result(Ok(Some(Box::new(
                        golem_wasm_rpc::Value::String("USD".to_string()),
                    )))),
                    result(str(), str()),
                );

                return Ok(WorkerResponse::new(Some(value_and_type)));
            }

            if function_name == "bigw:shopping/api.{store.add-user}" {
                let value_and_type = ValueAndType::new(
                    golem_wasm_rpc::Value::String("test-user-generated".to_string()),
                    str(),
                );

                return Ok(WorkerResponse::new(Some(value_and_type)));
            }

            let type_annotated_value = convert_to_worker_response(&resolved_worker_request);

            Ok(WorkerResponse::new(Some(type_annotated_value)))
        }
    }

    struct TestFileServerBindingHandler {}
    #[async_trait]
    impl FileServerBindingHandler for TestFileServerBindingHandler {
        async fn handle_file_server_binding_result(
            &self,
            _namespace: Namespace,
            _worker_name: Option<&str>,
            _component_id: &ComponentId,
            _original_result: RibResult,
        ) -> FileServerBindingResult {
            unimplemented!()
        }
    }

    struct TestHttpHandlerBindingHandler {}

    #[async_trait]
    impl HttpHandlerBindingHandler for TestHttpHandlerBindingHandler {
        async fn handle_http_handler_binding(
            &self,
            _namespace: &Namespace,
            _worker_detail: &WorkerDetails,
            _request_details: IncomingHttpRequest,
        ) -> HttpHandlerBindingResult {
            unimplemented!()
        }
    }

    #[derive(Debug, Clone)]
    pub struct DefaultResult {
        pub worker_name: String,
        pub function_name: String,
        pub function_params: JsonValue,
        pub user_email: Option<String>,
        pub cors_middleware_headers: Option<CorsMiddlewareHeadersInResponse>, // if binding has cors middleware configured,
        pub idempotency_key: Option<IdempotencyKey>,
    }

    #[derive(Debug, Clone)]
    pub struct CorsMiddlewareHeadersInResponse {
        pub cors_header_allow_origin: String,
        pub cors_header_allow_credentials: Option<bool>, // If cors middleware is applied
        pub cors_header_expose_headers: Option<String>,  // If cors middleware is applied
    }

    pub fn create_tuple(items: Vec<ValueAndType>) -> ValueAndType {
        ValueAndType::new(
            Value::Tuple(items.iter().map(|item| item.value.clone()).collect()),
            tuple(
                items
                    .iter()
                    .map(|item| item.typ.clone())
                    .collect::<Vec<AnalysedType>>(),
            ),
        )
    }

    pub fn create_record(values: Vec<(&str, ValueAndType)>) -> ValueAndType {
        ValueAndType::new(
            Value::Record(values.iter().map(|(_, vnt)| vnt.value.clone()).collect()),
            record(
                values
                    .iter()
                    .map(|(name, vnt)| field(name, vnt.typ.clone()))
                    .collect::<Vec<_>>(),
            ),
        )
    }

    pub fn convert_to_worker_response(
        worker_request: &GatewayResolvedWorkerRequest,
    ) -> ValueAndType {
        let mut record_elems = vec![
            (
                "component_id",
                worker_request
                    .component_id
                    .0
                    .to_string()
                    .into_value_and_type(),
            ),
            (
                "function_name",
                worker_request
                    .function_name
                    .to_string()
                    .into_value_and_type(),
            ),
            (
                "function_params",
                create_tuple(worker_request.function_params.clone()),
            ),
        ];

        if let Some(worker_name) = worker_request.clone().worker_name {
            record_elems.push(("worker_name", worker_name.into_value_and_type()))
        };

        if let Some(idempotency_key) = worker_request.clone().idempotency_key {
            record_elems.push((
                "idempotency_key",
                idempotency_key.to_string().into_value_and_type(),
            ))
        };

        create_record(record_elems)
    }

    pub(crate) fn get_bigw_shopping_metadata() -> Vec<AnalysedExport> {
        // Exist in only amazon:shopping-cart/api1
        let analysed_function_in_api1 = AnalysedFunction {
            name: "get-user-name".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "arg1".to_string(),
                typ: str(),
            }],
            result: Some(AnalysedFunctionResult { typ: str() }),
        };

        let analysed_export1 = AnalysedExport::Instance(AnalysedInstance {
            name: "bigw:shopping/api".to_string(),
            functions: vec![analysed_function_in_api1],
        });

        vec![analysed_export1]
    }

    pub(crate) fn get_component_metadata() -> ComponentMetadataDictionary {
        let versioned_component_id = VersionedComponentId {
            component_id: ComponentId::try_from("0b6d9cd8-f373-4e29-8a5a-548e61b868a5").unwrap(),
            version: 0,
        };

        let mut metadata_dict = HashMap::new();
        let mut exports = get_bigw_shopping_metadata();
        exports.extend(get_bigw_shopping_metadata_with_resource());
        exports.extend(get_golem_shopping_cart_metadata());

        let component_details = ComponentDetails {
            component_info: ComponentDependencyKey {
                component_name: "test-component".to_string(),
                component_id: Uuid::new_v4(),
                root_package_name: None,
                root_package_version: None,
            },
            metadata: exports,
        };

        metadata_dict.insert(versioned_component_id, component_details);

        ComponentMetadataDictionary {
            metadata: metadata_dict,
        }
    }

    fn get_bigw_shopping_metadata_with_resource() -> Vec<AnalysedExport> {
        let instance = AnalysedExport::Instance(AnalysedInstance {
            name: "bigw:shopping/api".to_string(),
            functions: vec![
                AnalysedFunction {
                    name: "[constructor]store".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "initial".to_string(),
                        typ: str(),
                    }],
                    result: Some(AnalysedFunctionResult {
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                    }),
                },
                AnalysedFunction {
                    name: "[method]store.get-currency".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    }],
                    result: Some(AnalysedFunctionResult {
                        typ: result(str(), str()),
                    }),
                },
                AnalysedFunction {
                    name: "[method]store.add-user".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "user".to_string(),
                            typ: str(),
                        },
                    ],
                    result: Some(AnalysedFunctionResult {
                        typ: result(str(), str()),
                    }),
                },
                AnalysedFunction {
                    name: "[drop]store".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                    }],
                    result: None,
                },
            ],
        });

        vec![instance]
    }

    pub fn get_golem_shopping_cart_metadata() -> Vec<AnalysedExport> {
        let analysed_export = AnalysedExport::Instance(AnalysedInstance {
            name: "golem:it/api".to_string(),
            functions: vec![
                AnalysedFunction {
                    name: "add-item".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "a".to_string(),
                            typ: u32(),
                        },
                        AnalysedFunctionParameter {
                            name: "b".to_string(),
                            typ: str(),
                        },
                    ],
                    result: Some(AnalysedFunctionResult { typ: str() }),
                },
                AnalysedFunction {
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
                    result: Some(AnalysedFunctionResult {
                        typ: record(vec![
                            field("component_id", str()),
                            field("function_name", str()),
                            field("function_params", tuple(vec![str(), str()])),
                            field("worker_name", str()),
                        ]),
                    }),
                },
            ],
        });

        vec![analysed_export]
    }

    pub fn get_test_rib_interpreter() -> Arc<dyn WorkerServiceRibInterpreter> {
        Arc::new(DefaultRibInterpreter::from_worker_request_executor(
            Arc::new(TestApiGatewayWorkerRequestExecutor {}),
        ))
    }

    pub fn get_test_file_server_binding_handler() -> Arc<dyn FileServerBindingHandler> {
        Arc::new(TestFileServerBindingHandler {})
    }

    pub fn get_test_http_handler_binding_handler() -> Arc<dyn HttpHandlerBindingHandler> {
        Arc::new(TestHttpHandlerBindingHandler {})
    }

    pub fn get_preflight_from_response(response: Response) -> HttpCors {
        let headers = response.headers();

        let allow_headers = headers
            .get(ACCESS_CONTROL_ALLOW_HEADERS)
            .map(|x| x.to_str().unwrap())
            .expect("Cors preflight response expects allow_headers");

        let allow_origin = headers
            .get(ACCESS_CONTROL_ALLOW_ORIGIN)
            .map(|x| x.to_str().unwrap())
            .expect("Cors preflight response expects allow_origin");

        let allow_methods = headers
            .get(ACCESS_CONTROL_ALLOW_METHODS)
            .map(|x| x.to_str().unwrap())
            .expect("Cors preflight response expects allow_method");

        let expose_headers = headers
            .get(ACCESS_CONTROL_EXPOSE_HEADERS)
            .map(|x| x.to_str().unwrap());

        let max_age = headers
            .get(ACCESS_CONTROL_MAX_AGE)
            .map(|x| x.to_str().unwrap().parse::<u64>().unwrap());

        let allow_credentials = headers
            .get(ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .map(|x| x.to_str().unwrap().parse::<bool>().unwrap());

        HttpCors::new(
            allow_origin,
            allow_methods,
            allow_headers,
            expose_headers,
            allow_credentials,
            max_age,
        )
    }

    pub async fn get_details_from_response(response: Response) -> DefaultResult {
        let user_email = &response
            .headers()
            .get("email")
            .map(|x| x.to_str().unwrap().to_string());

        let bytes = response
            .into_body()
            .into_bytes()
            .await
            .expect("TestResponse for worker-binding expects a response body");

        let body_json: JsonValue =
            serde_json::from_slice(&bytes).expect("Failed to read the response body");

        let worker_name = body_json
            .get("worker_name")
            .and_then(|v| v.as_str())
            .map(String::from);

        let function_name = body_json
            .get("function_name")
            .and_then(|v| v.as_str())
            .map(String::from);

        let function_params = body_json.get("function_params").cloned();

        let idempotency_key = body_json
            .get("idempotency_key")
            .and_then(|v| v.as_str())
            .map(String::from)
            .map(IdempotencyKey::new);

        DefaultResult {
            worker_name: worker_name.expect("Worker response expects worker_name"),
            function_name: function_name.expect("Worker response expects function_name"),
            function_params: function_params.expect("Worker response expects function_params"),
            user_email: user_email.clone(),
            cors_middleware_headers: None,
            idempotency_key,
        }
    }

    pub async fn get_test_response_for_worker_binding_with_cors(
        response: Response,
    ) -> DefaultResult {
        let headers = response.headers().clone();

        let bytes = response
            .into_body()
            .into_bytes()
            .await
            .expect("TestResponse for worker-binding expects a response body");

        let body_json: JsonValue =
            serde_json::from_slice(&bytes).expect("Failed to read the response body");

        let worker_name = body_json
            .get("worker_name")
            .and_then(|v| v.as_str())
            .map(String::from);

        let function_name = body_json
            .get("function_name")
            .and_then(|v| v.as_str())
            .map(String::from);

        let user_email = headers
            .get("email")
            .map(|x| x.to_str().unwrap().to_string());

        let function_params = body_json.get("function_params").cloned();

        let idempotency_key = body_json
            .get("idempotency_key")
            .and_then(|v| v.as_str())
            .map(String::from)
            .map(IdempotencyKey::new);

        DefaultResult {
            worker_name: worker_name.expect("Worker response expects worker_name"),
            function_name: function_name.expect("Worker response expects function_name"),
            function_params: function_params.expect("Worker response expects function_params"),
            user_email,
            cors_middleware_headers: {
                let cors_header_allow_origin = headers
                    .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                    .map(|x| x.to_str().unwrap().to_string())
                    .expect("Cors preflight response expects allow_origin");

                let cors_header_allow_credentials = headers
                    .get(ACCESS_CONTROL_ALLOW_CREDENTIALS)
                    .map(|x| x.to_str().unwrap().parse::<bool>().unwrap());

                let cors_header_expose_headers = headers
                    .get(ACCESS_CONTROL_EXPOSE_HEADERS)
                    .map(|x| x.to_str().unwrap().to_string());

                Some(CorsMiddlewareHeadersInResponse {
                    cors_header_allow_origin,
                    cors_header_allow_credentials,
                    cors_header_expose_headers,
                })
            },
            idempotency_key,
        }
    }

    // Simple decoder only for test
    pub fn decode_url(encoded: &str) -> String {
        let mut decoded = String::new();
        let mut chars = encoded.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '%' {
                if let (Some(hex1), Some(hex2)) = (chars.next(), chars.next()) {
                    if let Ok(byte) = u8::from_str_radix(&format!("{hex1}{hex2}"), 16) {
                        decoded.push(byte as char);
                    } else {
                        decoded.push('%');
                        decoded.push(hex1);
                        decoded.push(hex2);
                    }
                }
            } else {
                decoded.push(c);
            }
        }

        decoded
    }

    // This redirection is to offload eviction policy testing to the inbuilt cache
    // mechanism of golem, and doesn't test the eviction policy itself
    #[derive(Debug, Clone)]
    pub struct TestSessionBackEnd {
        pub inner: Arc<Mutex<HashMap<(SessionId, DataKey), DataValue>>>,
    }

    impl TestSessionBackEnd {
        pub fn new() -> Self {
            TestSessionBackEnd {
                inner: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }
    #[async_trait]
    impl GatewaySession for TestSessionBackEnd {
        async fn insert(
            &self,
            session_id: SessionId,
            data_key: DataKey,
            data_value: DataValue,
        ) -> Result<(), GatewaySessionError> {
            let mut data = self.inner.lock().unwrap();
            data.insert((session_id, data_key), data_value);
            Ok(())
        }

        async fn get(
            &self,
            session_id: &SessionId,
            data_key: &DataKey,
        ) -> Result<DataValue, GatewaySessionError> {
            let data = self.inner.lock().unwrap();
            let value = data.get(&(session_id.clone(), data_key.clone()));
            value.cloned().ok_or(GatewaySessionError::MissingValue {
                session_id: session_id.clone(),
                data_key: data_key.clone(),
            })
        }
    }

    pub fn get_session_store() -> GatewaySessionStore {
        Arc::new(TestSessionBackEnd::new())
    }

    struct NoopTestSessionBackend;

    #[async_trait]
    impl GatewaySession for NoopTestSessionBackend {
        async fn insert(
            &self,
            _session_id: SessionId,
            _data_key: DataKey,
            _data_value: DataValue,
        ) -> Result<(), GatewaySessionError> {
            Ok(())
        }

        async fn get(
            &self,
            session_id: &SessionId,
            data_key: &DataKey,
        ) -> Result<DataValue, GatewaySessionError> {
            Err(GatewaySessionError::MissingValue {
                session_id: session_id.clone(),
                data_key: data_key.clone(),
            })
        }
    }

    pub fn get_session_store_with_zero_ttl() -> GatewaySessionStore {
        Arc::new(NoopTestSessionBackend)
    }

    struct TestSwaggerBindingHandler {}
    // Create a test swagger binding handler which outputs a static html page
    #[async_trait]
    impl SwaggerBindingHandler for TestSwaggerBindingHandler {
        async fn handle_swagger_binding_request(
            &self,
            _authority: &str,
            _swagger_binding: &SwaggerUiBinding,
        ) -> SwaggerBindingResult {
            Ok(SwaggerBindingSuccess {
                html_content: "<html><body>Test Swagger UI</body></html>".to_string(),
            })
        }
    }

    pub fn get_test_swagger_binding_handler() -> Arc<dyn SwaggerBindingHandler + Sync + Send> {
        Arc::new(TestSwaggerBindingHandler {})
    }
}

pub mod security {
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use golem_service_base::repo::RepoError;
    use golem_worker_service::gateway_security::{
        AuthorizationUrl, DefaultIdentityProvider, GolemIdentityProviderMetadata, IdentityProvider,
        IdentityProviderError, OpenIdClient, Provider, SecurityScheme,
    };
    use golem_worker_service::repo::security_scheme::{SecuritySchemeRecord, SecuritySchemeRepo};
    use golem_worker_service::service::gateway::security_scheme::{
        DefaultSecuritySchemeService, SecuritySchemeService,
    };
    use http::header::{COOKIE, HOST};
    use http::{HeaderMap, HeaderValue, Method, Uri};
    use openidconnect::core::{
        CoreClaimName, CoreClaimType, CoreClient, CoreClientAuthMethod, CoreGrantType, CoreIdToken,
        CoreIdTokenClaims, CoreIdTokenFields, CoreIdTokenVerifier, CoreJsonWebKey,
        CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreJwsSigningAlgorithm,
        CoreProviderMetadata, CoreResponseMode, CoreResponseType, CoreRsaPrivateSigningKey,
        CoreSubjectIdentifierType, CoreTokenResponse, CoreTokenType,
    };
    use openidconnect::{
        AccessToken, Audience, AuthUrl, AuthenticationContextClass, AuthorizationCode, ClientId,
        ClientSecret, CsrfToken, EmptyAdditionalClaims, EmptyExtraTokenFields, EndUserEmail,
        IdTokenVerifier, IssuerUrl, JsonWebKeyId, JsonWebKeySet, JsonWebKeySetUrl, Nonce,
        RegistrationUrl, ResponseTypes, Scope, StandardClaims, SubjectIdentifier, TokenUrl,
        UserInfoUrl,
    };
    use poem::Request;
    use rsa::pkcs8::DecodePublicKey;
    use rsa::traits::PublicKeyParts;

    use std::collections::HashMap;
    use std::ops::Sub;
    use std::str::FromStr;
    use std::sync::Arc;

    use tokio::sync::Mutex;

    // These keys are used over the default JwkKeySet of the actual client
    // only for testing purposes, to verify jwt signature verifications
    const TEST_PUBLIC_KEY: &str = "\
       -----BEGIN PUBLIC KEY-----\n\
       MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAsRMj0YYjy7du6v1gWyKS\n\
       TJx3YjBzZTG0XotRP0IaObw0k+6830dXadjL5jVhSWNdcg9OyMyTGWfdNqfdrS6p\n\
       pBqlQNgjZJdloIqL9zOLBZrDm7G4+qN4KeZ4/5TyEilq2zOHHGFEzXpOq/UxqVnm\n\
       3J4fhjqCNaS2nKd7HVVXGBQQ+4+FdVT+MyJXemw5maz2F/h324TQi6XoUPEwUddx\n\
       BwLQFSOlzWnHYMc4/lcyZJ8MpTXCMPe/YJFNtb9CaikKUdf8x4mzwH7usSf8s2d6\n\
       R4dQITzKrjrEJ0u3w3eGkBBapoMVFBGPjP3Haz5FsVtHc5VEN3FZVIDF6HrbJH1C\n\
       4QIDAQAB\n\
       -----END PUBLIC KEY-----";

    const TEST_PRIVATE_KEY: &str = "\
      -----BEGIN RSA PRIVATE KEY-----\n\
       MIIEowIBAAKCAQEAsRMj0YYjy7du6v1gWyKSTJx3YjBzZTG0XotRP0IaObw0k+68\n\
       30dXadjL5jVhSWNdcg9OyMyTGWfdNqfdrS6ppBqlQNgjZJdloIqL9zOLBZrDm7G4\n\
       +qN4KeZ4/5TyEilq2zOHHGFEzXpOq/UxqVnm3J4fhjqCNaS2nKd7HVVXGBQQ+4+F\n\
       dVT+MyJXemw5maz2F/h324TQi6XoUPEwUddxBwLQFSOlzWnHYMc4/lcyZJ8MpTXC\n\
       MPe/YJFNtb9CaikKUdf8x4mzwH7usSf8s2d6R4dQITzKrjrEJ0u3w3eGkBBapoMV\n\
       FBGPjP3Haz5FsVtHc5VEN3FZVIDF6HrbJH1C4QIDAQABAoIBAHSS3izM+3nc7Bel\n\
       8S5uRxRKmcm5je6b11u6qiVUFkHWJmMRc6QmqmSThkCq+b4/vUAe1cYZ7+l02Exo\n\
       HOcrZiEULaDP6hUKGqyjKVv3wdlRtt8kFFxlC/HBufzAiNDuFVvzw0oquwnvMCXC\n\
       yQvtlK+/JY/PqvM32cSt+b4o9apySsHqAtdsoHHohK82jsQqIfCi1v8XYV/xRBJB\n\
       cQMCaA0Ls3tFpmJv3JdikyyQxio4kZ5tswghC63znCp1iL+qDq1wjjKzjick9MDb\n\
       Qzb95X09QQP201l1FPWN7Kbhj4ybg6PJGz/VHQcvILcBCoYIc0UY/OMSBt9VN9yD\n\
       wr1WlbECgYEA37difsTMcLmUEN57sicFe1q4lxH6eqnUBjmoKBflx4oMIIyRnfjF\n\
       Jwsu9yIiBkJfBCP85nl2tZdcV0wfZLf6amxB/KMtdfW6r8eoTDzE472OYxSIg1F5\n\
       dI4qn2nBI0Dou0g58xj+Kv0iLaym0pxtyJkSg/rxZGwKb9a+x5WAs50CgYEAyqC0\n\
       NcZs2BRIiT5kEOF6+MeUvarbKh1mangKHKcTdXRrvoJ+Z5izm7FifBixo/79MYpt\n\
       0VofW0IzYKtAI9KZDq2JcozEbZ+lt/ZPH5QEXO4T39QbDoAG8BbOmEP7l+6m+7QO\n\
       PiQ0WSNjDnwk3W7Zihgg31DH7hyxsxQCapKLcxUCgYAwERXPiPcoDSd8DGFlYK7z\n\
       1wUsKEe6DT0p7T9tBd1v5wA+ChXLbETn46Y+oQ3QbHg/yn+vAU/5KkFD3G4uVL0w\n\
       Gnx/DIxa+OYYmHxXjQL8r6ClNycxl9LRsS4FPFKsAWk/u///dFI/6E1spNjfDY8k\n\
       94ab5tHwsqn3Z5tsBHo3nQKBgFUmxbSXh2Qi2fy6+GhTqU7k6G/wXhvLsR9rBKzX\n\
       1YiVfTXZNu+oL0ptd/q4keZeIN7x0oaY/fZm0pp8PP8Q4HtXmBxIZb+/yG+Pld6q\n\
       YE8BSd7VDu3ABapdm0JHx3Iou4mpOBcLNeiDw3vx1bgsfkTXMPFHzE0XR+H+tak9\n\
       nlalAoGBALAmAF7WBGdOt43Rj8hPaKOM/ahj+6z3CNwVreToNsVBHoyNmiO8q7MC\n\
       +tRo4jgdrzk1pzs66OIHfbx5P1mXKPtgPZhvI5omAY8WqXEgeNqSL1Ksp6LZ2ql/\n\
       ouZns5xwKc9+aRL+GWoAGNzwzcjE8cP52sBy/r0rYXTs/sZo5kgV\n\
       -----END RSA PRIVATE KEY-----\
       ";

    const TEST_PRIVATE_KEY_INVALID: &str = "
-----BEGIN RSA PRIVATE KEY-----
MIICXAIBAAKBgHzeh/kp0qhpMU3zYHSdP/v2YGMLRg2MQMEGLrm8zDFPYu5/h9jx
jYOmSafqSIQ2wsoYSEsLHPzcrbxn/QK8lw8pv7/lT6S9WoFZh5IECjv8qc16wEMm
uV8hMQ+q32bKJ85HUGGls7xwwgNbU02byGawneZDYgaQSf0YyvR0+hDvAgMBAAEC
gYAhuOtZ0/yrNLV3Cso61Bjqj/AZ73p8wCrgti26RY2Tp4/dPx3N7fY83pS3v2LU
KgHTGFm1wouSknptDlxa99mHf21ZVfOMZsqy9PAHwdOsVXQTTxQbqkfF76V2/cWP
2kk4SnpqUmAfmuDV45VUUyh6ZQEx9Sv34eXZ91/5DGzDWQJBAOx1/tB8C/ZgcK+w
qccCRnhKy82Xs1kn4M3wZgcuYPGsv2rruNnR6BClBf7l9R8fBcQ90XUqb6+V9M+w
yfmTlEMCQQCHL/faP6cmTuaQT3+t9eQdbJsyMXJGsuNOtH8Rc5W1LDwfmP5L9Ivh
ogE+7oLnMUoqEhOzcfc15o6ydaUs2TvlAkEAyOOeLoWUrCZW1gxi0N4P/Jae+vXM
uwv0I0H/jE5MskWvAiltjDCW1b09kMbRLzVt8sEKOF5rMGNX7zMlBNHqfQJAPhsP
Zrivda3xxoJ/SdAZSFtIwTpuIYS1YMcTSVUshdsYk/uZeBOU38epgtvYT5YZWFKN
sna894pALERX6c6y8QJBAOTRthvuxZO7dEQ9/F2DGEJZyiEFBIEtB+vmnmXwXr+H
nUhg4edJVHjqxYyoQT+YSPLlHl6AkLZt9/n1NJ+bft0=
-----END RSA PRIVATE KEY-----
";

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

    // A simple testable identity provider
    // which piggybacks on DefaultIdentityProvider for all non side effecting
    // functionalities
    #[derive(Clone)]
    pub struct TestIdentityProvider {
        static_provider_metadata: GolemIdentityProviderMetadata,
        static_id_token: CoreIdToken,
    }

    impl Default for TestIdentityProvider {
        fn default() -> Self {
            TestIdentityProvider::get_provider_with_valid_id_token()
        }
    }

    impl TestIdentityProvider {
        pub fn get_provider_with_valid_id_token() -> TestIdentityProvider {
            TestIdentityProvider {
                static_provider_metadata: get_test_provider_metadata(),
                static_id_token: get_non_expiring_id_token(),
            }
        }

        pub fn get_provider_with_expired_id_token() -> TestIdentityProvider {
            TestIdentityProvider {
                static_provider_metadata: get_test_provider_metadata(),
                static_id_token: get_expired_id_token(),
            }
        }

        pub fn get_provider_with_invalid_signatures() -> TestIdentityProvider {
            TestIdentityProvider {
                static_provider_metadata: get_test_provider_metadata(),
                static_id_token: get_id_token_with_invalid_signatures(),
            }
        }
    }

    #[async_trait]
    impl IdentityProvider for TestIdentityProvider {
        async fn get_provider_metadata(
            &self,
            _provider: &Provider,
        ) -> Result<GolemIdentityProviderMetadata, IdentityProviderError> {
            Ok(self.static_provider_metadata.clone())
        }

        async fn exchange_code_for_tokens(
            &self,
            _client: &OpenIdClient,
            _code: &AuthorizationCode,
        ) -> Result<CoreTokenResponse, IdentityProviderError> {
            Ok(CoreTokenResponse::new(
                AccessToken::new("secret_access_token".to_string()),
                CoreTokenType::Bearer,
                CoreIdTokenFields::new(
                    Some(self.static_id_token.clone()),
                    EmptyExtraTokenFields {},
                ),
            ))
        }

        async fn get_client(
            &self,
            security_scheme: &SecurityScheme,
        ) -> Result<OpenIdClient, IdentityProviderError> {
            let provider_metadata = self
                .get_provider_metadata(&security_scheme.provider_type())
                .await?;

            let client = CoreClient::from_provider_metadata(
                provider_metadata,
                security_scheme.client_id().clone(),
                Some(security_scheme.client_secret().clone()),
            )
            .set_redirect_uri(security_scheme.redirect_url());

            Ok(OpenIdClient { client })
        }

        // In real, this token verifier depends on the provider metadata
        // however, we simply use our own public key for testing
        // instead of relying providers public key.
        fn get_id_token_verifier<'a>(&self, _client: &'a OpenIdClient) -> CoreIdTokenVerifier<'a> {
            let public_key = rsa::RsaPublicKey::from_public_key_pem(TEST_PUBLIC_KEY)
                .expect("Failed to parse public key");

            // Extract modulus and exponent
            let n = public_key.n().to_bytes_be();
            let e = public_key.e().to_bytes_be();
            let kid = JsonWebKeyId::new("my-key-id".to_string()); // Use a unique key ID

            let jwks = JsonWebKeySet::new(vec![CoreJsonWebKey::new_rsa(n, e, Some(kid))]);

            IdTokenVerifier::new_confidential_client(
                ClientId::new("client_id_foo".to_string()),
                ClientSecret::new("client_secret_foo".to_string()),
                IssuerUrl::new("https://accounts.google.com".to_string()).unwrap(),
                jwks,
            )
        }

        fn get_claims(
            &self,
            id_token_verifier: &CoreIdTokenVerifier,
            core_token_response: CoreTokenResponse,
            nonce: &Nonce,
        ) -> Result<CoreIdTokenClaims, IdentityProviderError> {
            let identity_provider = DefaultIdentityProvider;

            identity_provider.get_claims(id_token_verifier, core_token_response, nonce)
        }

        fn get_authorization_url(
            &self,
            client: &OpenIdClient,
            scopes: Vec<Scope>,
            _state: Option<CsrfToken>,
            _nonce: Option<Nonce>,
        ) -> AuthorizationUrl {
            let identity_provider = DefaultIdentityProvider;
            identity_provider.get_authorization_url(
                client,
                scopes,
                Some(CsrfToken::new("token".to_string())),
                Some(Nonce::new("nonce".to_string())),
            )
        }
    }

    pub fn get_test_security_scheme_service(
        identity_provider: TestIdentityProvider,
    ) -> Arc<dyn SecuritySchemeService> {
        let repo = Arc::new(TestSecuritySchemeRepo {
            security_scheme: Arc::new(Mutex::new(HashMap::new())),
        });

        let default = DefaultSecuritySchemeService::new(repo, Arc::new(identity_provider));

        Arc::new(default)
    }

    pub fn get_non_expiring_id_token() -> CoreIdToken {
        CoreIdToken::new(
            CoreIdTokenClaims::new(
                IssuerUrl::new("https://accounts.google.com".to_string()).unwrap(),
                vec![Audience::new("client_id_foo".to_string())],
                Utc.with_ymd_and_hms(9999, 1, 1, 0, 0, 0).unwrap(),
                Utc::now(),
                StandardClaims::new(SubjectIdentifier::new(
                    "5f83e0ca-2b8e-4e8c-ba0a-f80fe9bc3632".to_string(),
                ))
                .set_email(Some(EndUserEmail::new("bob@example.com".to_string())))
                .set_email_verified(Some(true)),
                EmptyAdditionalClaims {},
            )
            .set_nonce(Some(Nonce::new("nonce".to_string()))),
            &CoreRsaPrivateSigningKey::from_pem(
                TEST_PRIVATE_KEY,
                Some(JsonWebKeyId::new("my-key-id".to_string())),
            )
            .expect("Invalid RSA private key"),
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            Some(&AccessToken::new("secret_access_token".to_string())),
            None,
        )
        .unwrap()
    }

    pub fn get_expired_id_token() -> CoreIdToken {
        CoreIdToken::new(
            CoreIdTokenClaims::new(
                IssuerUrl::new("https://accounts.google.com".to_string()).unwrap(),
                vec![Audience::new("client_id_foo".to_string())],
                Utc::now().sub(chrono::Duration::days(1)),
                Utc::now().sub(chrono::Duration::days(2)),
                StandardClaims::new(SubjectIdentifier::new(
                    "5f83e0ca-2b8e-4e8c-ba0a-f80fe9bc3632".to_string(),
                ))
                .set_email(Some(EndUserEmail::new("bob@example.com".to_string())))
                .set_email_verified(Some(true)),
                EmptyAdditionalClaims {},
            )
            .set_nonce(Some(Nonce::new("nonce".to_string()))),
            &CoreRsaPrivateSigningKey::from_pem(
                TEST_PRIVATE_KEY,
                Some(JsonWebKeyId::new("my-key-id".to_string())),
            )
            .expect("Invalid RSA private key"),
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            Some(&AccessToken::new("secret_access_token".to_string())),
            None,
        )
        .unwrap()
    }

    pub fn get_id_token_with_invalid_signatures() -> CoreIdToken {
        CoreIdToken::new(
            CoreIdTokenClaims::new(
                IssuerUrl::new("https://accounts.google.com".to_string()).unwrap(),
                vec![Audience::new("client_id_foo".to_string())],
                Utc.with_ymd_and_hms(9999, 1, 1, 0, 0, 0).unwrap(),
                Utc::now(),
                StandardClaims::new(SubjectIdentifier::new(
                    "5f83e0ca-2b8e-4e8c-ba0a-f80fe9bc3632".to_string(),
                ))
                .set_email(Some(EndUserEmail::new("bob@example.com".to_string())))
                .set_email_verified(Some(true)),
                EmptyAdditionalClaims {},
            )
            .set_nonce(Some(Nonce::new("nonce".to_string()))),
            &CoreRsaPrivateSigningKey::from_pem(
                TEST_PRIVATE_KEY_INVALID,
                Some(JsonWebKeyId::new("my-key-id".to_string())),
            )
            .expect("Invalid RSA private key"),
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            Some(&AccessToken::new("secret_access_token".to_string())),
            None,
        )
        .unwrap()
    }

    // A simulated auth call back from identity provider
    // Example:
    //  Request {
    //     method: GET,
    //     uri: /auth/callback?state=Iy3GSF&code=4%2F0AeanPOWQlsww.googleapis.com%2Fauth%2Fuserinfo.profile+https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fuserinfo.email&authuser=0&hd=ziverge.com&prompt=consent,
    //     version: HTTP/1.1,
    //     headers: {
    //         "host": "127.0.0.1:5000",
    //         "connection": "keep-alive",
    //     },
    // }
    pub fn request_from_identity_provider_to_auth_call_back_endpoint(
        state: &str,
        code: &str,
        scope: &str,
        redirect_path: &str,
        redirect_host: &str,
    ) -> Request {
        let uri = Uri::from_str(
            format!("{redirect_path}?state={state}&code={code}&scope={scope}&prompt=consent")
                .as_str(),
        )
        .unwrap();

        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("host", redirect_host)
            .header("connection", "keep-alive")
            .header("referer", "https://accounts.google.com/");

        request.finish()
    }

    #[derive(Debug, Clone)]
    pub struct InitialRedirectData {
        pub response_type: String,
        pub client_id: String,
        pub state: String,
        pub auth_call_back_url: String,
        pub scope: String,
        pub nonce: String,
    }

    pub fn get_initial_redirect_data(
        query_components: &HashMap<String, String>,
    ) -> InitialRedirectData {
        let response_type = query_components
            .get("response_type")
            .expect("response_type is missing");
        let client_id = query_components
            .get("client_id")
            .expect("client_id is missing");
        let state = query_components.get("state").expect("state is missing");
        let redirect_uri = query_components
            .get("redirect_uri")
            .expect("redirect_uri is missing");
        let scope = query_components.get("scope").expect("scope is missing");
        let nonce = query_components.get("nonce").expect("nonce is missing");

        InitialRedirectData {
            response_type: response_type.to_string(),
            client_id: client_id.to_string(),
            state: state.to_string(),
            auth_call_back_url: redirect_uri.to_string(),
            scope: scope.to_string(),
            nonce: nonce.to_string(),
        }
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

        CoreProviderMetadata::new(
            IssuerUrl::new("https://accounts.google.com".to_string()).unwrap(),
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string()).unwrap(),
            JsonWebKeySetUrl::new("https://www.googleapis.com/oauth2/v3/certs".to_string())
                .unwrap(),
            vec![ResponseTypes::new(vec![CoreResponseType::Code])],
            vec![
                CoreSubjectIdentifierType::Public,
                CoreSubjectIdentifierType::Pairwise,
            ],
            all_signing_algs.clone(),
            Default::default(),
        )
        .set_jwks({
            let public_key = rsa::RsaPublicKey::from_public_key_pem(TEST_PUBLIC_KEY)
                .expect("Failed to parse public key");

            let n = public_key.n().to_bytes_be();
            let e = public_key.e().to_bytes_be();
            let kid = JsonWebKeyId::new("my-key-id".to_string());
            JsonWebKeySet::new(vec![CoreJsonWebKey::new_rsa(n, e, Some(kid))])
        })
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
                "https://accounts.google.com/openidconnect-rs/\
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
            UserInfoUrl::new("https://openidconnect.googleapis.com/v1/userinfo".to_string())
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
            TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).unwrap(),
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
        )]))
    }

    pub async fn create_request_from_redirect(headers: &HeaderMap) -> Request {
        let cookies = extract_cookies_from_redirect(headers);

        let cookie_header = cookies
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<String>>()
            .join("; ");

        let mut request_headers = HeaderMap::new();

        request_headers.insert(COOKIE, HeaderValue::from_str(&cookie_header).unwrap());

        let location = headers
            .get("location")
            .and_then(|loc| loc.to_str().ok())
            .unwrap_or("/");

        Request::builder()
            .method(Method::GET)
            .uri(Uri::from_str(format!("http://localhost/{location}").as_str()).unwrap()) // Use the "Location" header as the URL
            .header(HOST, "localhost")
            .header(COOKIE, cookie_header)
            .finish()
    }

    fn extract_cookies_from_redirect(headers: &HeaderMap) -> HashMap<String, String> {
        let mut cookies = HashMap::new();

        let view = headers.get_all("set-cookie");
        for cookie in view.iter() {
            if let Ok(cookie_str) = cookie.to_str() {
                let parts: Vec<&str> = cookie_str.split(';').collect();

                if let Some(cookie_value) = parts.first() {
                    let cookie_parts: Vec<&str> = cookie_value.splitn(2, '=').collect();
                    if cookie_parts.len() == 2 {
                        cookies.insert(cookie_parts[0].to_string(), cookie_parts[1].to_string());
                    }
                }
            }
        }

        cookies
    }
}

#[test]
async fn test_swagger_ui_binding() {
    // Create a Swagger UI API definition
    let api_specification = get_api_def_with_swagger_ui("/swagger-ui").await;

    // Create a request to the Swagger UI endpoint
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("localhost:8080"));
    let api_request = get_gateway_request("/swagger-ui", None, &headers, JsonValue::Null);

    // Create a session store
    let session_store = internal::get_session_store();

    // Execute the request
    let response = execute(
        api_request,
        &api_specification,
        &session_store,
        &TestIdentityProvider::default(),
    )
    .await;

    // Verify response status code
    assert_eq!(response.status(), StatusCode::OK);

    // Get response body
    let body = match response.into_body().into_bytes().await {
        Ok(b) => b,
        Err(_) => panic!("Failed to read body"),
    };
    let html_content = String::from_utf8(body.to_vec()).unwrap();

    // Verify the HTML contains the expected Swagger UI content
    // The test checks whether the static html page
    // is properly returned by the swagger binding handler
    assert!(html_content.contains("<html><body>Test Swagger UI</body></html>"));
}

// Helper function to create an API definition with a Swagger UI binding
async fn get_api_def_with_swagger_ui(path_pattern: &str) -> HttpApiDefinition {
    let yaml_string = format!(
        r#"
          id: api-docs
          version: 0.0.1
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Get
            path: {path_pattern}
            binding:
              bindingType: swagger-ui
        "#,
    );

    // Parse the YAML into an API definition request
    let http_api_definition_request: api::dto::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();

    // Convert to core request
    let core_request: gateway_api_definition::http::HttpApiDefinitionRequest =
        http_api_definition_request
            .into_core(&TestConversionContext.boxed())
            .await
            .unwrap();

    // Create the API definition
    let create_at: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
    HttpApiDefinition::from_http_api_definition_request(
        &test_namespace(),
        core_request,
        create_at,
        &security::get_test_security_scheme_service(TestIdentityProvider::default()),
    )
    .await
    .unwrap()
}

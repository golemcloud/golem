use async_trait::async_trait;
use futures_util::FutureExt;
use std::fmt::Display;
use std::sync::Arc;

use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

use golem_common::model::{ComponentId, IdempotencyKey};

use crate::worker_binding::RibInputValue;
use rib::{ParsedFunctionName, RibByteCode, RibFunctionInvoke, RibInterpreterResult};

use crate::worker_bridge_execution::{
    NoopWorkerRequestExecutor, WorkerRequest, WorkerRequestExecutor,
};

// A wrapper service over original RibInterpreter concerning
// the details of the worker service.
#[async_trait]
pub trait WorkerServiceRibInterpreter {
    async fn evaluate(
        &self,
        worker_name: &str,
        component_id: &ComponentId,
        idempotency_key: &Option<IdempotencyKey>,
        rib_byte_code: &RibByteCode,
        rib_input: &RibInputValue,
    ) -> Result<RibInterpreterResult, EvaluationError>;

    async fn evaluate_pure(
        &self,
        expr: &RibByteCode,
        rib_input: &RibInputValue,
    ) -> Result<RibInterpreterResult, EvaluationError>;
}

#[derive(Debug, PartialEq)]
pub struct EvaluationError(pub String);

impl Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for EvaluationError {
    fn from(err: String) -> Self {
        EvaluationError(err)
    }
}

pub struct DefaultEvaluator {
    worker_request_executor: Arc<dyn WorkerRequestExecutor + Sync + Send>,
}

impl DefaultEvaluator {
    pub fn noop() -> Self {
        DefaultEvaluator {
            worker_request_executor: Arc::new(NoopWorkerRequestExecutor),
        }
    }

    pub fn from_worker_request_executor(
        worker_request_executor: Arc<dyn WorkerRequestExecutor + Sync + Send>,
    ) -> Self {
        DefaultEvaluator {
            worker_request_executor,
        }
    }
}

#[async_trait]
impl WorkerServiceRibInterpreter for DefaultEvaluator {
    async fn evaluate(
        &self,
        worker_name: &str,
        component_id: &ComponentId,
        idempotency_key: &Option<IdempotencyKey>,
        expr: &RibByteCode,
        rib_input: &RibInputValue,
    ) -> Result<RibInterpreterResult, EvaluationError> {
        let executor = self.worker_request_executor.clone();

        let worker_name = worker_name.to_string();
        let component_id = component_id.clone();
        let idempotency_key = idempotency_key.clone();

        let worker_invoke_function: RibFunctionInvoke = Arc::new(
            move |function_name: ParsedFunctionName, parameters: Vec<TypeAnnotatedValue>| {
                let worker_name = worker_name.to_string();
                let component_id = component_id.clone();
                let worker_name = worker_name.clone();
                let idempotency_key = idempotency_key.clone();
                let executor = executor.clone();

                async move {
                    let worker_request = WorkerRequest {
                        component_id,
                        worker_name,
                        function_name,
                        function_params: parameters,
                        idempotency_key,
                    };

                    executor
                        .execute(worker_request)
                        .await
                        .map(|v| v.result)
                        .map_err(|e| e.to_string())
                }
                .boxed() // This ensures the future is boxed with the correct type
            },
        );
        rib::interpret(expr, rib_input.value.clone(), worker_invoke_function)
            .await
            .map_err(EvaluationError)
    }

    async fn evaluate_pure(
        &self,
        expr: &RibByteCode,
        rib_input: &RibInputValue,
    ) -> Result<RibInterpreterResult, EvaluationError> {
        let worker_invoke_function: RibFunctionInvoke = Arc::new(|_, _| {
            Box::pin(
                async move {
                    Err("Worker invoke function is not allowed in pure evaluation".to_string())
                }
                .boxed(),
            )
        });

        rib::interpret(expr, rib_input.value.clone(), worker_invoke_function)
            .await
            .map_err(EvaluationError)
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::Arc;

    use golem_service_base::type_inference::infer_analysed_type;
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedType, NameTypePair, TypeList, TypeOption, TypeRecord, TypeStr,
        TypeU32, TypeU64,
    };

    use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::{NameOptionTypePair, TypeVariant, TypedTuple, TypedVariant};
    use http::{HeaderMap, Uri};
    use poem_openapi::types::ToJSON;
    use rib::{
        Expr, FunctionTypeRegistry, RibFunctionInvoke, RibInputTypeInfo, RibInterpreterResult,
    };
    use serde_json::json;

    use crate::api_definition::http::AllPathPatterns;
    use crate::worker_binding::{RequestDetails, RibInputValue, RibInputValueResolver};

    use crate::worker_bridge_execution::to_response::ToResponse;
    use crate::worker_service_rib_interpreter::{
        DefaultEvaluator, EvaluationError, WorkerServiceRibInterpreter,
    };
    use test_utils::*;

    // An extension to over worker-service specific rib-interpreter
    // to make testing easy. Especially the compilation of Expr is done as we go
    // instead of a separate compilation and invoking use byte-code
    #[async_trait]
    trait WorkerServiceRibInterpreterTestExt {
        async fn evaluate_pure_expr_with_request_details(
            &self,
            expr: &Expr,
            input: &RequestDetails,
        ) -> Result<TypeAnnotatedValue, EvaluationError>;

        async fn evaluate_with_worker_response(
            &self,
            expr: &Expr,
            worker_bridge_response: Option<TypeAnnotatedValue>,
            metadata: Vec<AnalysedExport>,
            input: Option<(RequestDetails, AnalysedType)>,
        ) -> Result<TypeAnnotatedValue, EvaluationError>;

        async fn evaluate_with_worker_response_as_rib_result(
            &self,
            expr: &Expr,
            worker_response: Option<TypeAnnotatedValue>,
            metadata: Vec<AnalysedExport>,
            request_input: Option<(RequestDetails, AnalysedType)>,
        ) -> Result<RibInterpreterResult, EvaluationError>;

        async fn evaluate_pure_expr(
            &self,
            expr: &Expr,
        ) -> Result<RibInterpreterResult, EvaluationError>;
    }

    #[async_trait]
    impl<T: WorkerServiceRibInterpreter + Send + Sync> WorkerServiceRibInterpreterTestExt for T {
        async fn evaluate_pure_expr_with_request_details(
            &self,
            expr: &Expr,
            input: &RequestDetails,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            let rib_input_json = input.as_json(); // Simply convert to json and try and infer the analysed type
            let analysed_type = infer_analysed_type(&rib_input_json);
            let mut type_info = HashMap::new();
            type_info.insert("request".to_string(), analysed_type);

            let rib_input_value = input
                .resolve_rib_input_value(&RibInputTypeInfo { types: type_info })
                .unwrap();

            let mut expr = expr.clone();
            let _ = expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

            let compiled_expr = rib::compile(&expr, &vec![]).unwrap();

            let eval_result = self
                .evaluate_pure(&compiled_expr.byte_code, &rib_input_value)
                .await?;

            Ok(eval_result.get_val().ok_or(EvaluationError(
                "The text is evaluated to unit and doesn't have a value".to_string(),
            ))?)
        }

        async fn evaluate_with_worker_response_as_rib_result(
            &self,
            expr: &Expr,
            worker_response: Option<TypeAnnotatedValue>,
            metadata: Vec<AnalysedExport>,
            request_input: Option<(RequestDetails, AnalysedType)>,
        ) -> Result<RibInterpreterResult, EvaluationError> {
            let expr = expr.clone();
            let compiled = rib::compile(&expr, &metadata)?;

            let mut type_info = HashMap::new();
            let mut rib_input = HashMap::new();

            if let Some(worker_response) = worker_response.clone() {
                // Collect worker details and request details into rib-input value
                let worker_response_analysed_type =
                    AnalysedType::try_from(&worker_response).unwrap();
                type_info.insert("worker".to_string(), worker_response_analysed_type);
                rib_input.insert("worker".to_string(), worker_response.clone());
            }

            if let Some((request_details, analysed_type)) = request_input {
                let mut type_info = HashMap::new();
                type_info.insert("request".to_string(), analysed_type);
                let rib_input_type_info = RibInputTypeInfo { types: type_info };
                let request_rib_input_value = request_details
                    .resolve_rib_input_value(&rib_input_type_info)
                    .unwrap();
                rib_input.insert(
                    "request".to_string(),
                    request_rib_input_value
                        .value
                        .get("request")
                        .unwrap()
                        .clone(),
                );
            }

            let invoke_result = match worker_response {
                Some(ref result) => TypeAnnotatedValue::Tuple(TypedTuple {
                    value: vec![golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(result.clone()),
                    }],
                    typ: vec![],
                }),
                None => TypeAnnotatedValue::Tuple(TypedTuple {
                    value: vec![],
                    typ: vec![],
                }),
            };

            let worker_invoke_function: RibFunctionInvoke = Arc::new(move |_, _| {
                Box::pin({
                    let value = invoke_result.clone();
                    async move { Ok(value) }
                })
            });

            let result =
                rib::interpret(&compiled.byte_code, rib_input, worker_invoke_function).await?;

            Ok(result)
        }

        // This will invoke worker
        async fn evaluate_with_worker_response(
            &self,
            expr: &Expr,
            worker_response: Option<TypeAnnotatedValue>,
            metadata: Vec<AnalysedExport>,
            request_input: Option<(RequestDetails, AnalysedType)>,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            let eval_result = self
                .evaluate_with_worker_response_as_rib_result(
                    expr,
                    worker_response,
                    metadata,
                    request_input,
                )
                .await?;

            eval_result.get_val().ok_or(EvaluationError(
                "The text is evaluated to unit and doesn't have a value".to_string(),
            ))
        }

        async fn evaluate_pure_expr(
            &self,
            expr: &Expr,
        ) -> Result<RibInterpreterResult, EvaluationError> {
            let compiled = rib::compile(expr, &vec![]).unwrap();

            self.evaluate_pure(&compiled.byte_code, &RibInputValue::empty())
                .await
        }
    }

    #[tokio::test]
    async fn test_evaluation_with_request_path() {
        let noop_executor = DefaultEvaluator::noop();
        let uri = Uri::builder().path_and_query("/pId/items").build().unwrap();

        let path_pattern = AllPathPatterns::from_str("/{id}/items").unwrap();

        let request_details = request_details_from_request_path_variables(uri, path_pattern);

        // The spec that will become part of the component metadata
        let request_path_type =
            get_analysed_type_record(vec![("id".to_string(), AnalysedType::Str(TypeStr))]);

        let request_type =
            get_analysed_type_record(vec![("path".to_string(), request_path_type.clone())]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"${
              let x = request;
              let result = foo(x);
              request.path.id
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("pId".to_string());

        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_id() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = get_request_details(
            r#"
                    {


                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }

                    }"#,
            &HeaderMap::new(),
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
            (
                "titles".to_string(),
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            ),
            (
                "address".to_string(),
                get_analysed_type_record(vec![
                    ("street".to_string(), AnalysedType::Str(TypeStr)),
                    ("city".to_string(), AnalysedType::Str(TypeStr)),
                ]),
            ),
        ]);

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        // TODO; result2 should be automatically inferred
        let expr_str = r#"${
              let x = { body : { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let result = foo(x);
              let result2: str = request.body.id;
              match result {  some(value) => "personal-id", none => result2 }
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await
            .unwrap();

        assert_eq!(result, TypeAnnotatedValue::Str("bId".to_string()));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_select_index() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = get_request_details(
            r#"
                    {


                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }

                    }"#,
            &HeaderMap::new(),
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
            (
                "titles".to_string(),
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            ),
            (
                "address".to_string(),
                get_analysed_type_record(vec![
                    ("street".to_string(), AnalysedType::Str(TypeStr)),
                    ("city".to_string(), AnalysedType::Str(TypeStr)),
                ]),
            ),
        ]);

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"${
              let x = { body : { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let result = foo(x);
              match result {  some(value) => "personal-id", none =>  x.body.titles[1] }
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await
            .unwrap();

        assert_eq!(result, TypeAnnotatedValue::Str("bTitle2".to_string()));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_select_from_object() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = get_request_details(
            r#"
                    {

                     "address": {
                         "street": "bStreet",
                         "city": "bCity"
                      }

                    }"#,
            &HeaderMap::new(),
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![(
            "address".to_string(),
            get_analysed_type_record(vec![
                ("street".to_string(), AnalysedType::Str(TypeStr)),
                ("city".to_string(), AnalysedType::Str(TypeStr)),
            ]),
        )]);

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);
        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr = rib::from_string(r#"${foo(request); request.body.address.street}"#).unwrap();

        let expected_evaluated_result = TypeAnnotatedValue::Str("bStreet".to_string());
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr,
                Some(worker_response),
                component_metadata,
                Some((request_details, request_type)),
            )
            .await;
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_if_condition() {
        let noop_executor = DefaultEvaluator::noop();

        let mut header_map = HeaderMap::new();
        header_map.insert("authorisation", "admin".parse().unwrap());

        let resolved_variables = get_request_details(
            r#"
                    {
                        "id": "bId"
                    }"#,
            &header_map,
        );

        let expr =
            rib::from_string(r#"${let input: str = request.headers.authorisation; let x: u64 = 200; let y: u64 = 401; if input == "admin" then x else y}"#)
                .unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::U64("200".parse().unwrap());
        let result = noop_executor
            .evaluate_pure_expr_with_request_details(&expr, &resolved_variables)
            .await;
        assert_eq!(result, Ok(expected_evaluated_result));

        let noop_executor = DefaultEvaluator::noop();

        let mut header_map = HeaderMap::new();
        header_map.insert("authorisation", "admin".parse().unwrap());

        let request_details = get_request_details(
            r#"
                    {
                           "id": "bId",
                           "name": "bName"
                    }"#,
            &header_map,
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
        ]);

        let request_type = get_analysed_type_record(vec![
            ("body".to_string(), request_body_type.clone()),
            (
                "headers".to_string(),
                get_analysed_type_record(vec![(
                    "authorisation".to_string(),
                    AnalysedType::Str(TypeStr),
                )]),
            ),
        ]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"${
              let x = request;
              let result = foo(x);
              let success: u64 = 200;
              let failure: u64 = 401;
              let auth = request.headers.authorisation;
              if auth == "admin" then success else failure
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await
            .unwrap();

        let expected_evaluated_result = TypeAnnotatedValue::U64("200".parse().unwrap());

        assert_eq!(result, expected_evaluated_result);
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_select_unknown_field() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = get_request_details(
            r#"
                    {


                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }

                    }"#,
            &HeaderMap::new(),
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
            (
                "titles".to_string(),
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            ),
            (
                "address".to_string(),
                get_analysed_type_record(vec![
                    ("street".to_string(), AnalysedType::Str(TypeStr)),
                    ("city".to_string(), AnalysedType::Str(TypeStr)),
                ]),
            ),
        ]);

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"${
              let x = { body : { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let result = foo(x);
              match result {  some(value) => "personal-id", none =>  x.body.address.street2 }
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await;

        // A compile time failure
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_select_invalid_index() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = get_request_details(
            r#"
                    {


                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }

                    }"#,
            &HeaderMap::new(),
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
            (
                "titles".to_string(),
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            ),
            (
                "address".to_string(),
                get_analysed_type_record(vec![
                    ("street".to_string(), AnalysedType::Str(TypeStr)),
                    ("city".to_string(), AnalysedType::Str(TypeStr)),
                ]),
            ),
        ]);

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"${
              let x = { body : { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let result = foo(x);
              match result {  some(value) => "personal-id", none =>  x.body.titles[4] }
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await
            .unwrap_err()
            .0;

        assert!(result.contains("Index 4 not found in the list"));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_index_of_object() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = get_request_details(
            r#"
                    {


                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }

                    }"#,
            &HeaderMap::new(),
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
            (
                "titles".to_string(),
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            ),
            (
                "address".to_string(),
                get_analysed_type_record(vec![
                    ("street".to_string(), AnalysedType::Str(TypeStr)),
                    ("city".to_string(), AnalysedType::Str(TypeStr)),
                ]),
            ),
        ]);

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"${
              let x = { body : { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let result = foo(x);
              match result {  some(value) => "personal-id", none =>  x.body.address[4] }
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await
            .unwrap_err()
            .0;

        let _expected = TypeAnnotatedValue::Str("bStreet".to_string());

        assert!(result.contains("Types do not match. Inferred to be both Record([(\"street\", Str), (\"city\", Str)]) and List(Unknown)"));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_invalid_type_comparison() {
        let noop_executor = DefaultEvaluator::noop();

        let mut header_map = HeaderMap::new();
        header_map.insert("authorisation", "admin".parse().unwrap());

        let request_details = get_request_details(
            r#"
                    {
                           "id": "bId",
                           "name": "bName"
                    }"#,
            &header_map,
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
        ]);

        let request_type = get_analysed_type_record(vec![
            ("body".to_string(), request_body_type.clone()),
            (
                "headers".to_string(),
                get_analysed_type_record(vec![(
                    "authorisation".to_string(),
                    AnalysedType::Str(TypeStr),
                )]),
            ),
        ]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"${
              let x = request;
              let result = foo(x);
              if request.headers.authorisation then 200 else 401
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let error_message = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await
            .unwrap_err()
            .0;

        let _expected = TypeAnnotatedValue::Str("bName".to_string());

        assert!(error_message.contains("Types do not match. Inferred to be both Str and Bool"));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_info_inference() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = get_request_details(
            r#"
                    {
                           "id": "bId",
                           "name": "bName"
                    }"#,
            &HeaderMap::new(),
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
        ]);

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"${
              let x = request;
              let result = foo(x);
              match result {  some(value) => "personal-id", none => x.body.name }
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("bName".to_string());

        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_for_no_arg_unit_function() {
        let noop_executor = DefaultEvaluator::noop();

        let component_metadata = get_analysed_export_for_no_arg_unit_function("foo");

        let expr_str = r#"${
              foo();
              "foo executed"
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(&expr1, None, component_metadata.clone(), None)
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("foo executed".to_string());

        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_for_no_arg_function() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata = get_analysed_export_for_no_arg_function("foo", return_type);

        let expr_str = r#"${
              let result = foo();
              match result { some(value) => "ok", none =>  "err" }
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                None,
            )
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("err".to_string());

        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_for_function_returning_unit() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = get_request_details(
            r#"
                    {
                           "id": "bId",
                           "name": "bName"

                    }"#,
            &HeaderMap::new(),
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
        ]);

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        let component_metadata =
            get_analysed_export_for_unit_function("foo", vec![request_type.clone()]);

        let expr_str = r#"${
              foo( { id: "bId", name: "bName" });
              let result = foo( { id: "bId", name: "bName" });
              result
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response_as_rib_result(
                &expr1,
                None,
                component_metadata.clone(),
                Some((request_details.clone(), request_type)),
            )
            .await
            .unwrap();

        let response: poem::Response = value1.to_response(&request_details);

        assert!(response.into_body().is_empty());
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_with_select_fields() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = get_request_details(
            r#"
                    {


                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }

                    }"#,
            &HeaderMap::new(),
        );

        // The spec that will become part of the component metadata
        let request_body_type = get_analysed_type_record(vec![
            ("id".to_string(), AnalysedType::Str(TypeStr)),
            ("name".to_string(), AnalysedType::Str(TypeStr)),
            (
                "titles".to_string(),
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            ),
            (
                "address".to_string(),
                get_analysed_type_record(vec![
                    ("street".to_string(), AnalysedType::Str(TypeStr)),
                    ("city".to_string(), AnalysedType::Str(TypeStr)),
                ]),
            ),
        ]);

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        // Output from worker - doesn't matter
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"${
              let input = { body: { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let result = foo(input);
              match result {  some(value) => "personal-id", none =>  request.body.address.street }
            }"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                Some((request_details, request_type)),
            )
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("bStreet".to_string());

        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_zero_worker_response() {
        let noop_executor = DefaultEvaluator::noop();

        let resolved_variables = get_request_details(
            r#"
                    {
                        "path": {
                           "id": "pId"
                        }
                    }"#,
            &HeaderMap::new(),
        );

        let expr = rib::from_string("${let s: str = worker.response.address.street; s}").unwrap();
        let result = noop_executor
            .evaluate_pure_expr_with_request_details(&expr, &resolved_variables)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_some() {
        let noop_executor = DefaultEvaluator::noop();

        // Output from worker
        let record = create_record(vec![(
            "id".to_string(),
            TypeAnnotatedValue::Str("pId".to_string()),
        )])
        .unwrap();
        let worker_response = create_option(record).unwrap();

        // Output from worker
        let return_type = AnalysedType::try_from(&worker_response).unwrap();

        let component_metadata =
            get_analysed_exports("foo", vec![AnalysedType::U64(TypeU64)], return_type);

        let expr_str = r#"${let result = foo(1); match result {  some(value) => "personal-id", none => "not found" }}"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                None,
            )
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("personal-id".to_string());

        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_none() {
        let noop_executor = DefaultEvaluator::noop();

        // Output from worker
        let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

        // Output from worker
        let return_type = AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
        });

        let component_metadata =
            get_analysed_exports("foo", vec![AnalysedType::U64(TypeU64)], return_type);

        let expr_str = r#"${let result = foo(1); match result {  some(value) => "personal-id", none => "not found" }}"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                None,
            )
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("not found".to_string());

        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_if_else() {
        let noop_executor = DefaultEvaluator::noop();

        let uri = Uri::builder()
            .path_and_query("/shopping-cart/foo")
            .build()
            .unwrap();

        let path_pattern = AllPathPatterns::from_str("/shopping-cart/{id}").unwrap();

        let request_details =
            request_details_from_request_path_variables(uri.clone(), path_pattern.clone());

        let worker_response_inner = create_record(vec![(
            "id".to_string(),
            TypeAnnotatedValue::Str("baz".to_string()),
        )])
        .unwrap();

        let worker_response =
            create_ok_result(worker_response_inner, Some(AnalysedType::Str(TypeStr))).unwrap();

        let return_type = AnalysedType::try_from(&worker_response).unwrap();

        // The spec that will become part of the component metadata
        let request_path_type =
            get_analysed_type_record(vec![("id".to_string(), AnalysedType::Str(TypeStr))]);

        let request_type =
            get_analysed_type_record(vec![("path".to_string(), request_path_type.clone())]);

        let metadata = get_analysed_exports("foo", vec![request_type.clone()], return_type);

        // TODO; inlining request.path.id all over should work too
        let expr1 = rib::from_string(
            r#"${
              let x = request;
              let foo_result = foo(x);
              let txt = request.path.id;
              if txt == "foo" then "bar" else match foo_result { ok(value) => txt, err(msg) => "empty" }
             }"#,
        )
            .unwrap();

        let result1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                metadata.clone(),
                Some((request_details.clone(), request_type.clone())),
            )
            .await;

        // TODO; inlining request.path.id all over should work too
        let expr2 = rib::from_string(
            r#"${
              let x = request;
              let foo_result = foo(x);
              let txt = request.path.id;
              if txt == "bar" then "foo" else match foo_result { ok(foo) => foo.id, err(msg) => "empty" }
          }"#,

        ).unwrap();

        let result2 = noop_executor
            .evaluate_with_worker_response(
                &expr2,
                Some(worker_response),
                metadata.clone(),
                Some((request_details.clone(), request_type.clone())),
            )
            .await;

        let error_worker_response =
            create_error_result(TypeAnnotatedValue::Str("Error".to_string()), None).unwrap();

        let _new_request_details = request_details_from_request_path_variables(uri, path_pattern);

        let _expr3 = rib::from_string(
            r#"${if request.path.id == "bar" then "foo" else match worker.response { ok(foo) => foo.id, err(msg) => "empty" }}"#,

        ).unwrap();

        let result3 = noop_executor
            .evaluate_with_worker_response(
                &expr2,
                Some(error_worker_response),
                metadata,
                Some((request_details, request_type)),
            )
            .await;

        assert_eq!(
            (result1, result2, result3),
            (
                Ok(TypeAnnotatedValue::Str("bar".to_string())),
                Ok(TypeAnnotatedValue::Str("baz".to_string())),
                Ok(TypeAnnotatedValue::Str("empty".to_string()))
            )
        );
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_unused_variables() {
        let noop_executor = DefaultEvaluator::noop();

        // Output from worker
        let field_value = TypeAnnotatedValue::Str("pId".to_string());

        let record_value = create_singleton_record("id", &field_value).unwrap();

        let worker_response =
            create_ok_result(record_value.clone(), Some(AnalysedType::Str(TypeStr))).unwrap();

        // Output from worker
        let return_type = AnalysedType::try_from(&worker_response).unwrap();

        let component_metadata =
            get_analysed_exports("foo", vec![AnalysedType::U64(TypeU64)], return_type);

        let expr_str = r#"${let result = foo(1); match result {  ok(value) => "personal-id", err(msg) => "not found" }}"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                None,
            )
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("personal-id".to_string());

        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_use_success_variable() {
        let noop_executor = DefaultEvaluator::noop();

        // Output from worker
        let field_value = TypeAnnotatedValue::Str("pId".to_string());

        let record_value = create_singleton_record("id", &field_value).unwrap();

        let worker_response =
            create_ok_result(record_value.clone(), Some(AnalysedType::Str(TypeStr))).unwrap();

        // Output from worker
        let return_type = AnalysedType::try_from(&worker_response).unwrap();

        let component_metadata =
            get_analysed_exports("foo", vec![AnalysedType::U64(TypeU64)], return_type);

        let expr_str = r#"${let result = foo(1); match result { ok(value) => value.id, err(msg) => "not found" }}"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(&value1, &TypeAnnotatedValue::Str("pId".to_string())); // The value is the same as the worker response unwrapping ok result
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_select_field() {
        let noop_executor = DefaultEvaluator::noop();

        // Output from worker
        let field_value = TypeAnnotatedValue::Str("pId".to_string());

        let record_value = create_singleton_record("id", &field_value).unwrap();

        let worker_response = create_ok_result(record_value, None).unwrap();

        // Output from worker
        let return_type = AnalysedType::try_from(&worker_response).unwrap();

        let component_metadata =
            get_analysed_exports("foo", vec![AnalysedType::U64(TypeU64)], return_type);

        let expr_str = r#"${let result = foo(1); match result { ok(value) => value.id, err(_) => "not found" }}"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                None,
            )
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("pId".to_string());
        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_select_from_array() {
        let noop_executor = DefaultEvaluator::noop();

        // Output from worker
        let sequence_value = create_list(vec![
            TypeAnnotatedValue::Str("id1".to_string()),
            TypeAnnotatedValue::Str("id2".to_string()),
        ])
        .unwrap();

        let record_value = create_singleton_record("ids", &sequence_value).unwrap();

        let worker_response = create_ok_result(record_value, None).unwrap();

        // Output from worker
        let return_type = AnalysedType::try_from(&worker_response).unwrap();

        let component_metadata =
            get_analysed_exports("foo", vec![AnalysedType::U64(TypeU64)], return_type);

        let expr_str = r#"${let result = foo(1); match result { ok(value) => value.ids[0], err(_) => "not found" }}"#;

        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                None,
            )
            .await
            .unwrap();

        let expected = TypeAnnotatedValue::Str("id1".to_string());
        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_some_construction() {
        let noop_executor = DefaultEvaluator::noop();

        let return_type =
            get_result_type_fully_formed(AnalysedType::U32(TypeU32), AnalysedType::Str(TypeStr));

        let worker_response = create_ok_result(TypeAnnotatedValue::U32(10), None).unwrap();

        let component_metadata =
            get_analysed_exports("foo", vec![AnalysedType::U64(TypeU64)], return_type);

        let expr_str =
            r#"${let result = foo(1); match result { ok(x) => some(1u64), err(_) => none }}"#;
        let expr1 = rib::from_string(expr_str).unwrap();
        let value1 = noop_executor
            .evaluate_with_worker_response(
                &expr1,
                Some(worker_response.clone()),
                component_metadata.clone(),
                None,
            )
            .await
            .unwrap();

        let expected = create_option(TypeAnnotatedValue::U64(1)).unwrap();
        assert_eq!(&value1, &expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_none_construction() {
        let noop_executor = DefaultEvaluator::noop();

        let expr =
            rib::from_string(r#"${match ok(1u64) { ok(value) => none, err(_) => some(1u64) }}"#)
                .unwrap();
        let result = noop_executor
            .evaluate_pure_expr(&expr)
            .await
            .map(|v| v.get_val().unwrap());

        let expected = create_none(Some(&AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::U64(TypeU64)),
        })));

        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_ok_construction() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string(
            "${match ok(\"afsal\") { ok(value) => ok(1u64), err(_) => err(2u64) }}",
        )
        .unwrap();
        let result = noop_executor
            .evaluate_pure_expr(&expr)
            .await
            .map(|v| v.get_val().unwrap());
        let expected =
            create_ok_result(TypeAnnotatedValue::U64(1), Some(AnalysedType::U64(TypeU64))).unwrap();
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_err_construction() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string(
            "${let err_n: u64 = 2; match err(\"afsal\") { ok(_) => ok(\"1\"), err(msg) => err(err_n) }}",
        )
        .unwrap();

        let result = noop_executor
            .evaluate_pure_expr(&expr)
            .await
            .map(|v| v.get_val().unwrap());

        let expected =
            create_error_result(TypeAnnotatedValue::U64(2), Some(AnalysedType::Str(TypeStr)))
                .unwrap();

        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_wild_card() {
        let noop_executor = DefaultEvaluator::noop();

        let expr =
            rib::from_string("${let x: u64 = 10; let lef: u64 = 1; let rig: u64 = 2; match err(x) { ok(_) => ok(lef), err(_) => err(rig) }}").unwrap();

        let result = noop_executor
            .evaluate_pure_expr(&expr)
            .await
            .map(|v| v.get_val().unwrap());

        let expected =
            create_error_result(TypeAnnotatedValue::U64(2), Some(AnalysedType::U64(TypeU64)))
                .unwrap();
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_name_alias() {
        let noop_executor = DefaultEvaluator::noop();

        let uri = Uri::builder()
            .path_and_query("/shopping-cart/foo")
            .build()
            .unwrap();

        let path_pattern = AllPathPatterns::from_str("/shopping-cart/{id}").unwrap();

        let request_details =
            request_details_from_request_path_variables(uri.clone(), path_pattern.clone());

        let request_type = get_analysed_type_record(vec![(
            "path".to_string(),
            get_analysed_type_record(vec![("id".to_string(), AnalysedType::Str(TypeStr))]),
        )]);

        let worker_response = create_error_result(
            create_ok_result(
                create_record(vec![("id".to_string(), TypeAnnotatedValue::U64(1))]).unwrap(),
                Some(AnalysedType::Str(TypeStr)),
            )
            .unwrap(),
            Some(AnalysedType::Str(TypeStr)),
        )
        .unwrap();

        let function_return_type = AnalysedType::try_from(&worker_response).unwrap();
        let metadata =
            get_analysed_exports("foo", vec![request_type.clone()], function_return_type);

        // TODO; a @ err(value) => a should work
        let expr = rib::from_string(
            r#"${
            let x = request;
            let y = foo(x);
            match y  { err(value) => err(value) }
          }"#,
        )
        .unwrap();

        let result = noop_executor
            .evaluate_with_worker_response(
                &expr,
                Some(worker_response),
                metadata,
                Some((request_details, request_type)),
            )
            .await
            .unwrap();

        let output_json = result.to_json_value();

        let expected_json = json!({
                "err": {
                    "ok": {
                        "id": 1
                    }
                },
        });
        assert_eq!(output_json, expected_json);
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_variant_positive() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = TypeAnnotatedValue::Variant(Box::new(TypedVariant {
            case_name: "Foo".to_string(),
            case_value: Some(Box::new(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(
                    create_singleton_record("id", &TypeAnnotatedValue::Str("pId".to_string()))
                        .unwrap(),
                ),
            })),
            typ: Some(TypeVariant {
                cases: vec![NameOptionTypePair {
                    name: "Foo".to_string(),
                    typ: Some(
                        (&AnalysedType::Record(TypeRecord {
                            fields: vec![NameTypePair {
                                name: "id".to_string(),
                                typ: AnalysedType::Str(TypeStr),
                            }],
                        }))
                            .into(),
                    ),
                }],
            }),
        }));

        let variant_analysed_type = AnalysedType::try_from(&worker_response).unwrap();

        let json = worker_response.to_json_value();

        let request_input = get_request_details(&json.to_json_string(), &HeaderMap::new());

        let request_body_type = variant_analysed_type.clone();

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        let component_metadata = get_analysed_exports(
            "foo",
            vec![request_type.clone()],
            variant_analysed_type.clone(),
        );

        let expr =
            rib::from_string(r#"${let x = foo(request); match x { Foo(value) => ok(value.id) }}"#)
                .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr,
                Some(worker_response),
                component_metadata,
                Some((request_input, request_type)),
            )
            .await;

        let expected = create_ok_result(TypeAnnotatedValue::Str("pId".to_string()), None).unwrap();

        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_variant_nested_some_type() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = TypeAnnotatedValue::Variant(Box::new(TypedVariant {
            case_name: "Foo".to_string(),
            case_value: Some(Box::new(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(
                    test_utils::create_option(
                        create_record(vec![(
                            "id".to_string(),
                            TypeAnnotatedValue::Str("pId".to_string()),
                        )])
                        .unwrap(),
                    )
                    .unwrap(),
                ),
            })),
            typ: Some(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "Foo".to_string(),
                        typ: Some(
                            (&AnalysedType::Option(TypeOption {
                                inner: Box::new(AnalysedType::Record(TypeRecord {
                                    fields: vec![NameTypePair {
                                        name: "id".to_string(),
                                        typ: AnalysedType::Str(TypeStr),
                                    }],
                                })),
                            }))
                                .into(),
                        ),
                    },
                    NameOptionTypePair {
                        name: "Bar".to_string(),
                        typ: Some(
                            (&AnalysedType::Option(TypeOption {
                                inner: Box::new(AnalysedType::Record(TypeRecord {
                                    fields: vec![NameTypePair {
                                        name: "id".to_string(),
                                        typ: AnalysedType::Str(TypeStr),
                                    }],
                                })),
                            }))
                                .into(),
                        ),
                    },
                ],
            }),
        }));

        let variant_analysed_type = AnalysedType::try_from(&worker_response).unwrap();

        let json = worker_response.to_json_value();

        let request_input = get_request_details(&json.to_json_string(), &HeaderMap::new());

        let request_body_type = variant_analysed_type.clone();

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        let component_metadata = get_analysed_exports(
            "foo",
            vec![request_type.clone()],
            variant_analysed_type.clone(),
        );

        let expr = rib::from_string(
            r#"${let x = foo(request); match x { Foo(none) => "not found",  Foo(some(value)) => value.id }}"#,
        )
            .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr,
                Some(worker_response),
                component_metadata,
                Some((request_input, request_type)),
            )
            .await;

        let expected = TypeAnnotatedValue::Str("pId".to_string());

        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_variant_nested_with_none() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = TypeAnnotatedValue::Variant(Box::new(TypedVariant {
            case_name: "Foo".to_string(),
            case_value: Some(Box::new(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(test_utils::create_none(Some(&AnalysedType::Record(
                    TypeRecord {
                        fields: vec![NameTypePair {
                            name: "id".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        }],
                    },
                )))),
            })),
            typ: Some(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "Foo".to_string(),
                        typ: Some(
                            (&AnalysedType::Option(TypeOption {
                                inner: Box::new(AnalysedType::Record(TypeRecord {
                                    fields: vec![NameTypePair {
                                        name: "id".to_string(),
                                        typ: AnalysedType::Str(TypeStr),
                                    }],
                                })),
                            }))
                                .into(),
                        ),
                    },
                    NameOptionTypePair {
                        name: "Bar".to_string(),
                        typ: Some(
                            (&AnalysedType::Option(TypeOption {
                                inner: Box::new(AnalysedType::Record(TypeRecord {
                                    fields: vec![NameTypePair {
                                        name: "id".to_string(),
                                        typ: AnalysedType::Str(TypeStr),
                                    }],
                                })),
                            }))
                                .into(),
                        ),
                    },
                ],
            }),
        }));

        let variant_analysed_type = AnalysedType::try_from(&worker_response).unwrap();

        let json = worker_response.to_json_value();

        let request_input = get_request_details(&json.to_json_string(), &HeaderMap::new());

        let request_body_type = variant_analysed_type.clone();

        let request_type =
            get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);

        let component_metadata = get_analysed_exports(
            "foo",
            vec![request_type.clone()],
            variant_analysed_type.clone(),
        );

        let expr = rib::from_string(
            r#"${let x = foo(request); match x { Foo(none) => "not found",  Foo(some(value)) => value.id }}"#,
        )
        .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(
                &expr,
                Some(worker_response),
                component_metadata,
                Some((request_input, request_type)),
            )
            .await;

        let expected = TypeAnnotatedValue::Str("not found".to_string());

        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_ok_record() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${let x: u64 = 1; {a : ok(x)}}").unwrap();

        let result = noop_executor.evaluate_pure_expr(&expr).await;

        let inner_record_ok = create_ok_result(TypeAnnotatedValue::U64(1), None).unwrap();
        let record = create_record(vec![("a".to_string(), inner_record_ok)]).unwrap();

        let expected = Ok(RibInterpreterResult::Val(record));
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_err_record() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${let n: u64 = 1; {a : err(n)}}").unwrap();

        let result = noop_executor.evaluate_pure_expr(&expr).await;

        let inner_result = create_error_result(TypeAnnotatedValue::U64(1), None).unwrap();

        let expected = RibInterpreterResult::Val(
            create_record(vec![("a".to_string(), inner_result)]).unwrap(),
        );

        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_simple_list() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${let x: list<u64> = [1,2,3]; x}").unwrap();

        let result = noop_executor.evaluate_pure_expr(&expr).await;

        let list = create_list(vec![
            TypeAnnotatedValue::U64(1),
            TypeAnnotatedValue::U64(2),
            TypeAnnotatedValue::U64(3),
        ])
        .unwrap();

        let expected = Ok(RibInterpreterResult::Val(list));

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_simple_tuple() {
        let noop_executor = DefaultEvaluator::noop();

        let expr =
            rib::from_string("${let x: tuple<option<u64>, u64, u64> = (some(1),2,3); x}").unwrap();

        let result = noop_executor.evaluate_pure_expr(&expr).await;

        let optional_value = create_option(TypeAnnotatedValue::U64(1)).unwrap();

        let expected = create_tuple(vec![
            optional_value,
            TypeAnnotatedValue::U64(2),
            TypeAnnotatedValue::U64(3),
        ])
        .unwrap();

        assert_eq!(result, Ok(RibInterpreterResult::Val(expected)));
    }

    #[tokio::test]
    async fn test_evaluation_wave_like_syntax_flag() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${{A, B, C}}").unwrap();

        let result = noop_executor.evaluate_pure_expr(&expr).await;

        let flags = create_flags(vec!["A".to_string(), "B".to_string(), "C".to_string()]);

        let expected = Ok(RibInterpreterResult::Val(flags));

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_result_list() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${let x: u64 = 1; let y: u64 = 2; [ok(x),ok(y)]}").unwrap();

        let result = noop_executor.evaluate_pure_expr(&expr).await;

        let list = create_list(vec![
            create_ok_result(TypeAnnotatedValue::U64(1), None).unwrap(),
            create_ok_result(TypeAnnotatedValue::U64(2), None).unwrap(),
        ])
        .unwrap();

        let expected = Ok(RibInterpreterResult::Val(list));

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_multiple_lines() {
        let noop_executor = DefaultEvaluator::noop();

        let program = r"
            let n1: u64 = 1;
            let n2: u64 = 2;
            let x = { a : n1 };
            let y = { b : n2 };
            let z = x.a > y.b;
            z
          ";

        let expr = rib::from_string(format!("${{{}}}", program)).unwrap();

        let result = noop_executor.evaluate_pure_expr(&expr).await;

        let expected = Ok(RibInterpreterResult::Val(TypeAnnotatedValue::Bool(false)));

        assert_eq!(result, expected);
    }

    mod test_utils {
        use crate::api_definition::http::{AllPathPatterns, PathPattern, VarInfo};
        use crate::http::router::RouterPattern;
        use crate::worker_binding::RequestDetails;

        use crate::worker_service_rib_interpreter::tests::WorkerServiceRibInterpreterTestExt;
        use crate::worker_service_rib_interpreter::{DefaultEvaluator, EvaluationError};
        use golem_service_base::type_inference::infer_analysed_type;
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            AnalysedType, NameTypePair, TypeOption, TypeRecord, TypeResult, TypeStr, TypeU32,
            TypeU64,
        };
        use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
        use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
        use golem_wasm_rpc::protobuf::{
            NameOptionTypePair, TypeVariant, TypedOption, TypedVariant,
        };
        use http::{HeaderMap, Uri};
        use serde_json::{json, Value};
        use std::collections::HashMap;

        use golem_wasm_ast::analysis::TypeTuple;
        use golem_wasm_rpc::protobuf::typed_result::ResultValue;
        use golem_wasm_rpc::protobuf::NameValuePair;
        use golem_wasm_rpc::protobuf::Type;
        use golem_wasm_rpc::protobuf::TypeAnnotatedValue as RootTypeAnnotatedValue;
        use golem_wasm_rpc::protobuf::{TypedFlags, TypedTuple};
        use golem_wasm_rpc::protobuf::{TypedList, TypedRecord, TypedResult};

        pub(crate) fn create_tuple(
            value: Vec<TypeAnnotatedValue>,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            let mut types = vec![];

            for value in value.iter() {
                let typ = Type::try_from(value)
                    .map_err(|_| EvaluationError("Failed to get type".to_string()))?;
                types.push(typ);
            }

            Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                value: value
                    .into_iter()
                    .map(|result| golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(result.clone()),
                    })
                    .collect(),
                typ: types,
            }))
        }

        pub(crate) fn create_flags(value: Vec<String>) -> TypeAnnotatedValue {
            TypeAnnotatedValue::Flags(TypedFlags {
                values: value.clone(),
                typ: value.clone(),
            })
        }

        pub(crate) fn create_ok_result(
            value: TypeAnnotatedValue,
            error_type: Option<AnalysedType>,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            let typ = golem_wasm_rpc::protobuf::Type::try_from(&value)
                .map_err(|_| EvaluationError("Failed to get type".to_string()))?;

            let typed_value = TypeAnnotatedValue::Result(Box::new(TypedResult {
                result_value: Some(ResultValue::OkValue(Box::new(
                    golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(value),
                    },
                ))),
                ok: Some(typ),
                error: error_type.map(|x| (&x).into()),
            }));

            Ok(typed_value)
        }

        pub(crate) fn create_error_result(
            value: TypeAnnotatedValue,
            optional_ok_type: Option<AnalysedType>,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            let typ = golem_wasm_rpc::protobuf::Type::try_from(&value)
                .map_err(|_| EvaluationError("Failed to get type".to_string()))?;

            let typed_value = TypeAnnotatedValue::Result(Box::new(TypedResult {
                result_value: Some(ResultValue::ErrorValue(Box::new(
                    golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(value),
                    },
                ))),
                ok: optional_ok_type.map(|x| (&x).into()),
                error: Some(typ),
            }));

            Ok(typed_value)
        }

        pub(crate) fn create_option(
            value: TypeAnnotatedValue,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            let typ = Type::try_from(&value)
                .map_err(|_| EvaluationError("Failed to get analysed type".to_string()))?;

            Ok(TypeAnnotatedValue::Option(Box::new(TypedOption {
                value: Some(Box::new(RootTypeAnnotatedValue {
                    type_annotated_value: Some(value),
                })),
                typ: Some(typ),
            })))
        }

        pub(crate) fn create_list(
            values: Vec<TypeAnnotatedValue>,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            match values.first() {
                Some(value) => {
                    let typ = Type::try_from(value)
                        .map_err(|_| EvaluationError("Failed to get analysed type".to_string()))?;

                    Ok(TypeAnnotatedValue::List(TypedList {
                        values: values
                            .into_iter()
                            .map(|result| RootTypeAnnotatedValue {
                                type_annotated_value: Some(result.clone()),
                            })
                            .collect(),
                        typ: Some(typ),
                    }))
                }
                None => Ok(TypeAnnotatedValue::List(TypedList {
                    values: vec![],
                    typ: Some((&AnalysedType::Tuple(TypeTuple { items: vec![] })).into()),
                })),
            }
        }
        pub(crate) fn create_singleton_record(
            binding_variable: &str,
            value: &TypeAnnotatedValue,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            create_record(vec![(binding_variable.to_string(), value.clone())])
        }

        pub(crate) fn create_record(
            values: Vec<(String, TypeAnnotatedValue)>,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            let mut name_type_pairs = vec![];
            let mut name_value_pairs = vec![];

            for (key, value) in values.iter() {
                let typ = value
                    .try_into()
                    .map_err(|_| EvaluationError("Failed to get type".to_string()))?;
                name_type_pairs.push(NameTypePair {
                    name: key.to_string(),
                    typ,
                });

                name_value_pairs.push(NameValuePair {
                    name: key.to_string(),
                    value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(value.clone()),
                    }),
                });
            }

            Ok(TypeAnnotatedValue::Record(TypedRecord {
                typ: name_type_pairs
                    .iter()
                    .map(|x| golem_wasm_ast::analysis::protobuf::NameTypePair {
                        name: x.name.clone(),
                        typ: Some(golem_wasm_ast::analysis::protobuf::Type::from(&x.typ)),
                    })
                    .collect(),
                value: name_value_pairs,
            }))
        }

        pub(crate) fn create_none(typ: Option<&AnalysedType>) -> TypeAnnotatedValue {
            TypeAnnotatedValue::Option(Box::new(TypedOption {
                value: None,
                typ: typ.map(|t| t.into()),
            }))
        }

        #[allow(dead_code)]
        pub(crate) fn get_complex_variant_typed_value() -> TypeAnnotatedValue {
            let record =
                create_singleton_record("id", &TypeAnnotatedValue::Str("pId".to_string())).unwrap();

            let result = create_ok_result(record, None).unwrap();

            let optional = create_option(result).unwrap();

            let variant_type = TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "Foo".to_string(),
                        typ: Some(
                            (&AnalysedType::Option(TypeOption {
                                inner: Box::new(AnalysedType::Result(TypeResult {
                                    ok: Some(Box::new(AnalysedType::Record(TypeRecord {
                                        fields: vec![NameTypePair {
                                            name: "id".to_string(),
                                            typ: AnalysedType::Str(TypeStr),
                                        }],
                                    }))),
                                    err: None,
                                })),
                            }))
                                .into(),
                        ),
                    },
                    NameOptionTypePair {
                        name: "Bar".to_string(),
                        typ: Some(
                            (&AnalysedType::Option(TypeOption {
                                inner: Box::new(AnalysedType::Result(TypeResult {
                                    ok: Some(Box::new(AnalysedType::Record(TypeRecord {
                                        fields: vec![NameTypePair {
                                            name: "id".to_string(),
                                            typ: AnalysedType::Str(TypeStr),
                                        }],
                                    }))),
                                    err: None,
                                })),
                            }))
                                .into(),
                        ),
                    },
                ],
            };

            TypeAnnotatedValue::Variant(Box::new(TypedVariant {
                case_name: "Foo".to_string(),
                case_value: Some(Box::new(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(optional),
                })),
                typ: Some(variant_type),
            }))
        }

        pub(crate) fn get_result_type_fully_formed(
            ok_type: AnalysedType,
            err_type: AnalysedType,
        ) -> AnalysedType {
            AnalysedType::Result(TypeResult {
                ok: Some(Box::new(ok_type)),
                err: Some(Box::new(err_type)),
            })
        }

        pub(crate) fn get_analysed_type_record(
            record_type: Vec<(String, AnalysedType)>,
        ) -> AnalysedType {
            let record = TypeRecord {
                fields: record_type
                    .into_iter()
                    .map(|(name, typ)| NameTypePair { name, typ })
                    .collect(),
            };
            AnalysedType::Record(record)
        }

        pub(crate) fn get_analysed_exports(
            function_name: &str,
            input_types: Vec<AnalysedType>,
            output: AnalysedType,
        ) -> Vec<AnalysedExport> {
            let analysed_function_parameters = input_types
                .into_iter()
                .enumerate()
                .map(|(index, typ)| AnalysedFunctionParameter {
                    name: format!("param{}", index),
                    typ,
                })
                .collect();

            vec![AnalysedExport::Function(AnalysedFunction {
                name: function_name.to_string(),
                parameters: analysed_function_parameters,
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: output,
                }],
            })]
        }

        pub(crate) fn get_analysed_export_for_unit_function(
            function_name: &str,
            input_types: Vec<AnalysedType>,
        ) -> Vec<AnalysedExport> {
            let analysed_function_parameters = input_types
                .into_iter()
                .enumerate()
                .map(|(index, typ)| AnalysedFunctionParameter {
                    name: format!("param{}", index),
                    typ,
                })
                .collect();

            vec![AnalysedExport::Function(AnalysedFunction {
                name: function_name.to_string(),
                parameters: analysed_function_parameters,
                results: vec![],
            })]
        }

        pub(crate) fn get_analysed_export_for_no_arg_function(
            function_name: &str,
            output: AnalysedType,
        ) -> Vec<AnalysedExport> {
            vec![AnalysedExport::Function(AnalysedFunction {
                name: function_name.to_string(),
                parameters: vec![],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: output,
                }],
            })]
        }

        pub(crate) fn get_analysed_export_for_no_arg_unit_function(
            function_name: &str,
        ) -> Vec<AnalysedExport> {
            vec![AnalysedExport::Function(AnalysedFunction {
                name: function_name.to_string(),
                parameters: vec![],
                results: vec![],
            })]
        }

        pub(crate) fn get_function_response_analysed_type_result() -> Vec<AnalysedExport> {
            vec![
                AnalysedExport::Function(AnalysedFunction {
                    name: "foo".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "my_parameter".to_string(),
                        typ: AnalysedType::U64(TypeU64),
                    }],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: AnalysedType::Result(TypeResult {
                            ok: Some(Box::new(AnalysedType::Str(TypeStr))),
                            err: Some(Box::new(AnalysedType::Str(TypeStr))),
                        }),
                    }],
                }),
                AnalysedExport::Function(AnalysedFunction {
                    name: "baz".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "my_parameter".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    }],
                    results: vec![],
                }),
            ]
        }

        pub(crate) fn get_simple_worker_response_err() -> TypeAnnotatedValue {
            TypeAnnotatedValue::parse_with_type(
                &json!({"err": "afsal" }),
                &AnalysedType::Result(TypeResult {
                    ok: None,
                    err: Some(Box::new(AnalysedType::Str(TypeStr))),
                }),
            )
            .unwrap()
        }

        #[allow(dead_code)]
        pub(crate) fn get_err_worker_response() -> TypeAnnotatedValue {
            TypeAnnotatedValue::parse_with_type(
                &json!({"err": { "id" : "afsal"} }),
                &AnalysedType::Result(TypeResult {
                    err: Some(Box::new(AnalysedType::Record(TypeRecord {
                        fields: vec![NameTypePair {
                            name: "id".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        }],
                    }))),
                    ok: None,
                }),
            )
            .unwrap()
        }

        #[allow(dead_code)]
        pub(crate) fn get_worker_response(input: &str) -> TypeAnnotatedValue {
            let value: Value = serde_json::from_str(input).expect("Failed to parse json");

            let expected_type = infer_analysed_type(&value);

            TypeAnnotatedValue::parse_with_type(&value, &expected_type).unwrap()
        }

        pub(crate) fn get_request_details(input: &str, header_map: &HeaderMap) -> RequestDetails {
            let request_body: Value = serde_json::from_str(input).expect("Failed to parse json");

            RequestDetails::from(
                &HashMap::new(),
                &HashMap::new(),
                &[],
                &request_body,
                header_map,
            )
            .unwrap()
        }

        pub(crate) fn request_details_from_request_path_variables(
            uri: Uri,
            path_pattern: AllPathPatterns,
        ) -> RequestDetails {
            let base_path: Vec<&str> = RouterPattern::split(uri.path()).collect();

            let path_params = path_pattern
                .path_patterns
                .into_iter()
                .enumerate()
                .filter_map(|(index, pattern)| match pattern {
                    PathPattern::Literal(_) => None,
                    PathPattern::Var(var) => Some((var, base_path[index])),
                })
                .collect::<HashMap<VarInfo, &str>>();

            RequestDetails::from(
                &path_params,
                &HashMap::new(),
                &path_pattern.query_params,
                &Value::Null,
                &HeaderMap::new(),
            )
            .unwrap()
        }

        #[tokio::test]
        async fn expr_to_string_round_trip_match_expr_err() {
            let noop_executor = DefaultEvaluator::noop();

            let worker_response = get_simple_worker_response_err();
            let component_metadata = get_function_response_analysed_type_result();

            let expr_str =
                r#"${let result = foo(1); match result { ok(x) => "foo", err(msg) => "error" }}"#;
            let expr1 = rib::from_string(expr_str).unwrap();
            let value1 = noop_executor
                .evaluate_with_worker_response(
                    &expr1,
                    Some(worker_response.clone()),
                    component_metadata.clone(),
                    None,
                )
                .await
                .unwrap();

            let expr2_string = expr1.to_string();
            let expr2 = rib::from_string(expr2_string.as_str()).unwrap();
            let value2 = noop_executor
                .evaluate_with_worker_response(
                    &expr2,
                    Some(worker_response),
                    component_metadata,
                    None,
                )
                .await
                .unwrap();

            let expected = TypeAnnotatedValue::Str("error".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }

        #[tokio::test]
        async fn expr_to_string_round_trip_match_expr_append() {
            let noop_executor = DefaultEvaluator::noop();

            // Output from worker
            let sequence_value = create_list(vec![
                TypeAnnotatedValue::Str("id1".to_string()),
                TypeAnnotatedValue::Str("id2".to_string()),
            ])
            .unwrap();

            let record_value = create_singleton_record("ids", &sequence_value).unwrap();

            let worker_response =
                create_error_result(record_value, Some(AnalysedType::Str(TypeStr))).unwrap();

            // Output from worker
            let return_type = AnalysedType::try_from(&worker_response).unwrap();

            let component_metadata =
                get_analysed_exports("foo", vec![AnalysedType::U64(TypeU64)], return_type);

            let expr_str = r#"${let result = foo(1); match result { ok(x) => "ok", err(x) => "append-${x.ids[0]}" }}"#;

            let expr1 = rib::from_string(expr_str).unwrap();
            let value1 = noop_executor
                .evaluate_with_worker_response(
                    &expr1,
                    Some(worker_response.clone()),
                    component_metadata.clone(),
                    None,
                )
                .await
                .unwrap();

            let expected = TypeAnnotatedValue::Str("append-id1".to_string());
            assert_eq!(&value1, &expected);
        }

        #[tokio::test]
        async fn expr_to_string_round_trip_match_expr_append_suffix() {
            let noop_executor = DefaultEvaluator::noop();

            // Output from worker
            let sequence_value = create_list(vec![
                TypeAnnotatedValue::Str("id1".to_string()),
                TypeAnnotatedValue::Str("id2".to_string()),
            ])
            .unwrap();

            let record_value = create_singleton_record("ids", &sequence_value).unwrap();

            let worker_response =
                create_ok_result(record_value, Some(AnalysedType::Str(TypeStr))).unwrap();

            // Output from worker
            let return_type = AnalysedType::try_from(&worker_response).unwrap();

            let component_metadata =
                get_analysed_exports("foo", vec![AnalysedType::U64(TypeU64)], return_type);

            let expr_str = r#"${let result = foo(1); match result { ok(x) => "prefix-${x.ids[0]}", err(msg) => "prefix-error-suffix" }}"#;

            let expr1 = rib::from_string(expr_str).unwrap();
            let value1 = noop_executor
                .evaluate_with_worker_response(
                    &expr1,
                    Some(worker_response.clone()),
                    component_metadata.clone(),
                    None,
                )
                .await
                .unwrap();

            let expected = TypeAnnotatedValue::Str("prefix-id1".to_string());
            assert_eq!(&value1, &expected);
        }
    }
}

use async_trait::async_trait;
pub use evaluator_context::*;
use std::sync::Arc;
mod evaluator_context;
pub(crate) mod getter;
mod math_op_evaluator;
pub(crate) mod path;
mod pattern_match_evaluator;

use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::get_json_from_typed_value;
use golem_wasm_rpc::TypeAnnotatedValue;

use crate::primitive::{GetPrimitive, Primitive};
use getter::GetError;
use getter::Getter;
use path::Path;
use rib::Expr;
use rib::Number;

use crate::worker_bridge_execution::{
    NoopWorkerRequestExecutor, RefinedWorkerResponse, WorkerRequestExecutor,
};

#[async_trait]
pub trait Evaluator {
    async fn evaluate(
        &self,
        expr: &Expr,
        evaluation_context: &EvaluationContext,
    ) -> Result<ExprEvaluationResult, EvaluationError>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprEvaluationResult {
    Value(TypeAnnotatedValue),
    Unit,
}

impl From<&RefinedWorkerResponse> for ExprEvaluationResult {
    fn from(value: &RefinedWorkerResponse) -> Self {
        match value {
            RefinedWorkerResponse::Unit => ExprEvaluationResult::Unit,
            RefinedWorkerResponse::SingleResult(typed_value) => {
                ExprEvaluationResult::Value(typed_value.clone())
            }
            RefinedWorkerResponse::MultipleResults(typed_value) => {
                ExprEvaluationResult::Value(typed_value.clone())
            }
        }
    }
}

impl ExprEvaluationResult {
    pub(crate) fn get_primitive(&self) -> Option<Primitive> {
        match self {
            ExprEvaluationResult::Value(value) => value.get_primitive(),
            ExprEvaluationResult::Unit => None,
        }
    }

    pub fn is_unit(&self) -> bool {
        matches!(self, ExprEvaluationResult::Unit)
    }

    pub fn get_value(&self) -> Option<TypeAnnotatedValue> {
        match self {
            ExprEvaluationResult::Value(value) => Some(value.clone()),
            ExprEvaluationResult::Unit => None,
        }
    }
}

impl From<TypeAnnotatedValue> for ExprEvaluationResult {
    fn from(value: TypeAnnotatedValue) -> Self {
        ExprEvaluationResult::Value(value)
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum EvaluationError {
    #[error(transparent)]
    InvalidReference(#[from] GetError),
    #[error("{0}")]
    Message(String),
}

impl<T: AsRef<str>> From<T> for EvaluationError {
    fn from(value: T) -> Self {
        EvaluationError::Message(value.as_ref().to_string())
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
impl Evaluator for DefaultEvaluator {
    async fn evaluate(
        &self,
        expr: &Expr,
        input: &EvaluationContext,
    ) -> Result<ExprEvaluationResult, EvaluationError> {
        let executor = self.worker_request_executor.clone();
        // An text evaluation needs to be careful with string values
        // and therefore returns ValueTyped
        async fn go(
            expr: &Expr,
            input: &mut EvaluationContext,
            executor: &Arc<dyn WorkerRequestExecutor + Sync + Send>,
        ) -> Result<ExprEvaluationResult, EvaluationError> {
            match expr {
                Expr::Identifier(variable) => input
                    .get_variable_value(variable.as_str())
                    .map(|v| v.into())
                    .map_err(|err| err.into()),

                Expr::Call(name, params) => {
                    let mut function_params = vec![];

                    for param in params {
                        let evaluated_param = Box::pin(go(param, input, executor)).await?;
                        let value = evaluated_param.get_value().ok_or(EvaluationError::Message(
                            "Function parameter is evaluated to unit".to_string(),
                        ))?;
                        function_params.push(value);
                    }

                    let result =
                        internal::call_worker_function(input, name, function_params, executor)
                            .await?;

                    let response_context = EvaluationContext::from_refined_worker_response(&result);

                    input.merge(&response_context);

                    Ok(ExprEvaluationResult::from(&result))
                }

                Expr::SelectIndex(expr, index) => {
                    let evaluation_result = Box::pin(go(expr, input, executor)).await?;
                    evaluation_result
                        .get_value()
                        .ok_or(EvaluationError::Message(format!(
                            "The text is evaluated to unit and doesn't have an index {}",
                            index
                        )))?
                        .get(&Path::from_index(*index))
                        .map(|r| r.into())
                        .map_err(|err| err.into())
                }

                Expr::SelectField(expr, field_name) => {
                    let evaluation_result = Box::pin(go(expr, input, executor))
                        .await?
                        .get_value()
                        .ok_or(EvaluationError::Message(format!(
                            "The text is evaluated to unit and doesn't have an field {}",
                            field_name
                        )))?;

                    evaluation_result
                        .get(&Path::from_key(field_name.as_str()))
                        .map(|r| r.into())
                        .map_err(|err| err.into())
                }

                Expr::EqualTo(left, right) => {
                    let left = Box::pin(go(left, input, executor)).await?;
                    let right = Box::pin(go(right, input, executor)).await?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| {
                        left == right
                    })
                }
                Expr::GreaterThan(left, right) => {
                    let left = Box::pin(go(left, input, executor)).await?;
                    let right = Box::pin(go(right, input, executor)).await?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| {
                        left > right
                    })
                }
                Expr::GreaterThanOrEqualTo(left, right) => {
                    let left = Box::pin(go(left, input, executor)).await?;
                    let right = Box::pin(go(right, input, executor)).await?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| {
                        left >= right
                    })
                }
                Expr::LessThan(left, right) => {
                    let left = Box::pin(go(left, input, executor)).await?;
                    let right = Box::pin(go(right, input, executor)).await?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| {
                        left < right
                    })
                }
                Expr::LessThanOrEqualTo(left, right) => {
                    let left = Box::pin(go(left, input, executor)).await?;
                    let right = Box::pin(go(right, input, executor)).await?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| {
                        left <= right
                    })
                }

                Expr::Not(expr) => {
                    let evaluated_expr = Box::pin(go(expr, input, executor)).await?;

                    match evaluated_expr {
                        ExprEvaluationResult::Value(TypeAnnotatedValue::Bool(value)) => Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Bool(!value))),
                        _ => Err(EvaluationError::Message(format!(
                            "The text is evaluated to {} but it is not a boolean value to apply not (!) operator on",
                           &evaluated_expr.get_value().map_or("unit".to_string(), |eval_result| get_json_from_typed_value(&eval_result).to_string())
                        ))),
                    }
                }

                Expr::Cond(pred0, left, right) => {
                    let pred = Box::pin(go(pred0, input, executor)).await?;
                    let left = Box::pin(go(left, input, executor)).await?;
                    let right = Box::pin(go(right, input, executor)).await?;

                    match pred {
                        ExprEvaluationResult::Value(TypeAnnotatedValue::Bool(value)) => {
                            if value {
                                Ok(left)
                            } else {
                                Ok(right)
                            }
                        }
                        _ => Err(EvaluationError::Message(format!(
                            "The predicate text is evaluated to {} but it is not a boolean value",
                            &pred.get_value().map_or("unit".to_string(), |eval_result| {
                                get_json_from_typed_value(&eval_result).to_string()
                            })
                        ))),
                    }
                }

                Expr::Let(str, expr) => {
                    let eval_result = Box::pin(go(expr, input, executor)).await?;

                    eval_result
                        .get_value()
                        .map_or(Ok(ExprEvaluationResult::Unit), |value| {
                            let typ = AnalysedType::from(&value);

                            let result = TypeAnnotatedValue::Record {
                                value: vec![(str.to_string(), value)],
                                typ: vec![(str.to_string(), typ)],
                            };

                            input.merge_variables(&result);

                            Ok(ExprEvaluationResult::Unit) // Result of a let binding is Unit
                        })
                }

                Expr::Multiple(multiple) => {
                    let mut result: Vec<ExprEvaluationResult> = vec![];

                    for expr in multiple {
                        match Box::pin(go(expr, input, executor)).await {
                            Ok(expr_result) => {
                                if let Some(value) = expr_result.get_value() {
                                    input.merge_variables(&value);
                                }
                                result.push(expr_result);
                            }
                            Err(result) => return Err(result),
                        }
                    }

                    Ok(result
                        .last()
                        .map_or(ExprEvaluationResult::Unit, |last| last.clone()))
                }

                Expr::Sequence(exprs) => {
                    let mut result: Vec<TypeAnnotatedValue> = vec![];

                    for expr in exprs {
                        match Box::pin(go(expr, input, executor)).await {
                            Ok(eval_result) => {
                                if let Some(value) = eval_result.get_value() {
                                    result.push(value);
                                } else {
                                    return Err(format!("The text {} is evaluated to unit and cannot be part of a record", rib::to_string(expr).unwrap()).into());
                                }
                            }
                            Err(result) => return Err(result),
                        }
                    }

                    let sequence = match result.first() {
                        Some(value) => TypeAnnotatedValue::List {
                            values: result.clone(),
                            typ: AnalysedType::from(value),
                        },
                        None => TypeAnnotatedValue::List {
                            values: result.clone(),
                            typ: AnalysedType::Tuple(vec![]),
                        }, // Support optional type in List
                    };

                    Ok(sequence.into())
                }

                Expr::Record(tuples) => {
                    let mut values: Vec<(String, TypeAnnotatedValue)> = vec![];

                    for (key, expr) in tuples {
                        match Box::pin(go(expr, input, executor)).await {
                            Ok(expr_result) => {
                                if let Some(value) = expr_result.get_value() {
                                    values.push((key.to_string(), value));
                                } else {
                                    return Err(format!("The text for key {} is evaluated to unit and cannot be part of a record", key).into());
                                }
                            }

                            Err(result) => return Err(result),
                        }
                    }

                    let types: Vec<(String, AnalysedType)> = values
                        .iter()
                        .map(|(key, value)| (key.clone(), AnalysedType::from(value)))
                        .collect();

                    Ok(TypeAnnotatedValue::Record {
                        value: values,
                        typ: types,
                    }
                    .into())
                }

                Expr::Concat(exprs) => {
                    let mut result = String::new();

                    for expr in exprs {
                        match Box::pin(go(expr, input, executor)).await {
                            Ok(value) => {
                                if let Some(primitive) = value.get_primitive() {
                                    result.push_str(primitive.to_string().as_str())
                                } else {
                                    return Err(EvaluationError::Message(format!(
                                        "Cannot append a complex text {} or unit to form text",
                                        &value.get_value().map_or("unit".to_string(), |v| {
                                            get_json_from_typed_value(&v).to_string()
                                        })
                                    )));
                                }
                            }

                            Err(result) => return Err(result),
                        }
                    }

                    Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Str(result)))
                }

                Expr::Literal(literal) => Ok(TypeAnnotatedValue::Str(literal.clone()).into()),

                Expr::Number(number) => match number {
                    Number::Unsigned(u64) => Ok(TypeAnnotatedValue::U64(*u64).into()),
                    Number::Signed(i64) => Ok(TypeAnnotatedValue::S64(*i64).into()),
                    Number::Float(f64) => Ok(TypeAnnotatedValue::F64(*f64).into()),
                },

                Expr::Boolean(bool) => Ok(TypeAnnotatedValue::Bool(*bool).into()),
                Expr::PatternMatch(match_text, arms) => {
                    pattern_match_evaluator::evaluate_pattern_match(
                        executor, match_text, arms, input,
                    )
                    .await
                }

                Expr::Option(option_expr) => {
                    match option_expr {
                        Some(expr) => {
                            let expr_result = Box::pin(go(expr, input, executor)).await?;

                            if let Some(value) = expr_result.get_value() {
                                let analysed_type = AnalysedType::from(&value);
                                Ok(TypeAnnotatedValue::Option {
                                    value: Some(Box::new(value)),
                                    typ: analysed_type,
                                }
                                .into())
                            } else {
                                Err(EvaluationError::Message(format!("The text {} is evaluated to unit and cannot be part of a option", rib::to_string(expr).unwrap())))
                            }
                        }
                        None => Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Option {
                            value: None,
                            typ: AnalysedType::Str,
                        })),
                    }
                }

                Expr::Result(result_expr) => {
                    match result_expr {
                        Ok(expr) => {
                            let expr_result = Box::pin(go(expr, input, executor)).await?;

                            if let Some(value) = expr_result.get_value() {
                                let analysed_type = AnalysedType::from(&value);

                                Ok(TypeAnnotatedValue::Result {
                                    value: Ok(Some(Box::new(value))),
                                    ok: Some(Box::new(analysed_type)),
                                    error: None,
                                }
                                .into())
                            } else {
                                Err(EvaluationError::Message(format!("The text {} is evaluated to unit and cannot be part of a result", rib::to_string(expr).unwrap())))
                            }
                        }
                        Err(expr) => {
                            let eval_result = Box::pin(go(expr, input, executor)).await?;

                            if let Some(value) = eval_result.get_value() {
                                let analysed_type = AnalysedType::from(&value);

                                Ok(TypeAnnotatedValue::Result {
                                    value: Err(Some(Box::new(value))),
                                    ok: None,
                                    error: Some(Box::new(analysed_type)),
                                }
                                .into())
                            } else {
                                Err(EvaluationError::Message(format!("The text {} is evaluated to unit and cannot be part of a result", rib::to_string(expr).unwrap())))
                            }
                        }
                    }
                }

                Expr::Tuple(tuple_exprs) => {
                    let mut result: Vec<TypeAnnotatedValue> = vec![];

                    for expr in tuple_exprs {
                        let eval_result = Box::pin(go(expr, input, executor)).await?;

                        if let Some(value) = eval_result.get_value() {
                            result.push(value);
                        } else {
                            return Err(EvaluationError::Message(format!(
                                "The text {} is evaluated to unit and cannot be part of a tuple",
                                rib::to_string(expr).unwrap()
                            )));
                        }
                    }

                    let typ: &Vec<AnalysedType> = &result.iter().map(AnalysedType::from).collect();

                    Ok(TypeAnnotatedValue::Tuple {
                        value: result,
                        typ: typ.clone(),
                    }
                    .into())
                }

                Expr::Flags(flags) => Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Flags {
                    values: flags.clone(),
                    typ: flags.clone(),
                })),
            }
        }

        let mut input = input.clone();
        go(expr, &mut input, &executor).await
    }
}

mod internal {
    use crate::evaluator::getter::Getter;
    use crate::evaluator::path::Path;
    use crate::evaluator::EvaluationContext;
    use crate::evaluator::EvaluationError;
    use crate::primitive::GetPrimitive;
    use crate::worker_bridge_execution::{
        RefinedWorkerResponse, WorkerRequest, WorkerRequestExecutor,
    };
    use golem_common::model::{ComponentId, IdempotencyKey};
    use golem_wasm_rpc::TypeAnnotatedValue;
    use rib::ParsedFunctionName;
    use std::str::FromStr;
    use std::sync::Arc;

    pub(crate) async fn call_worker_function(
        runtime: &EvaluationContext,
        function_name: &ParsedFunctionName,
        json_params: Vec<TypeAnnotatedValue>,
        executor: &Arc<dyn WorkerRequestExecutor + Sync + Send>,
    ) -> Result<RefinedWorkerResponse, EvaluationError> {
        let variables = runtime.clone().variables.ok_or(EvaluationError::Message(
            "No variables found in the context".to_string(),
        ))?;

        let analysed_function = runtime
            .find_function(function_name.clone())
            .map_err(|err| EvaluationError::Message(err.to_string()))?
            .ok_or(EvaluationError::Message(format!(
                "The function {} is not found at Runtime",
                function_name.site().interface_name().map_or(
                    function_name.function.function_name(),
                    |interface| format!(
                        "{{{}}}.{}",
                        interface,
                        function_name.function.function_name()
                    )
                )
            )))?;

        let worker_variables = variables.get(&Path::from_key("worker")).map_err(|_| {
            EvaluationError::Message("No worker variables found in the context".to_string())
        })?;

        let worker_name_typed = worker_variables.get(&Path::from_key("name")).map_err(|_| {
            EvaluationError::Message("No worker name found in the context".to_string())
        })?;

        let worker_name = worker_name_typed
            .get_primitive()
            .ok_or(EvaluationError::Message(
                "Worker name is not a string".to_string(),
            ))?
            .as_string();

        let idempotency_key = worker_variables
            .get(&Path::from_key("idempotency-key"))
            .ok()
            .and_then(|v| v.get_primitive())
            .map(|p| IdempotencyKey::new(p.as_string()));

        let component_id = worker_variables
            .get(&Path::from_key("component_id"))
            .map_err(|_| {
                EvaluationError::Message("No component_id found in the context".to_string())
            })?;

        let component_id_string = component_id
            .get_primitive()
            .ok_or(EvaluationError::Message(
                "Component_id is not a string".to_string(),
            ))?
            .as_string();

        let component_id = ComponentId::from_str(component_id_string.as_str())
            .map_err(|err| EvaluationError::Message(err.to_string()))?;

        let worker_request = WorkerRequest {
            component_id,
            worker_name,
            function_name: analysed_function.fqn,
            function_params: json_params,
            idempotency_key,
        };

        let worker_response = executor.execute(worker_request).await.map_err(|err| {
            EvaluationError::Message(format!("Failed to execute worker function: {}", err))
        })?;

        let refined_worker_response = worker_response.refined().map_err(|err| {
            EvaluationError::Message(format!("Failed to refine worker response: {}", err))
        })?;

        Ok(refined_worker_response)
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use std::str::FromStr;

    use golem_service_base::model::FunctionResult;
    use golem_service_base::type_inference::infer_analysed_type;
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::json::get_typed_value_from_json;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use http::{HeaderMap, Uri};
    use rib::Expr;
    use serde_json::{json, Value};

    use crate::api_definition::http::AllPathPatterns;
    use crate::evaluator::evaluator_context::EvaluationContext;
    use crate::evaluator::getter::GetError;
    use crate::evaluator::{DefaultEvaluator, EvaluationError, Evaluator, ExprEvaluationResult};
    use crate::worker_binding::RequestDetails;
    use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerResponse};
    use test_utils::*;

    #[async_trait]
    trait EvaluatorTestExt {
        async fn evaluate_with_request_details(
            &self,
            expr: &Expr,
            input: &RequestDetails,
        ) -> Result<TypeAnnotatedValue, EvaluationError>;
        async fn evaluate_with_worker_response(
            &self,
            expr: &Expr,
            worker_bridge_response: &RefinedWorkerResponse,
        ) -> Result<TypeAnnotatedValue, EvaluationError>;

        async fn evaluate_with(
            &self,
            expr: &Expr,
            input: &RequestDetails,
            worker_response: &RefinedWorkerResponse,
        ) -> Result<ExprEvaluationResult, EvaluationError>;
    }

    #[async_trait]
    impl<T: Evaluator + Send + Sync> EvaluatorTestExt for T {
        async fn evaluate_with_request_details(
            &self,
            expr: &Expr,
            input: &RequestDetails,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            let eval_result = self
                .evaluate(expr, &EvaluationContext::from_request_data(input))
                .await?;
            Ok(eval_result
                .get_value()
                .ok_or("The text is evaluated to unit and doesn't have a value")?)
        }

        async fn evaluate_with_worker_response(
            &self,
            expr: &Expr,
            worker_bridge_response: &RefinedWorkerResponse,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            let eval_result = self
                .evaluate(
                    expr,
                    &EvaluationContext::from_refined_worker_response(worker_bridge_response),
                )
                .await?;

            Ok(eval_result
                .get_value()
                .ok_or("The text is evaluated to unit and doesn't have a value")?)
        }

        async fn evaluate_with(
            &self,
            expr: &Expr,
            input: &RequestDetails,
            worker_response: &RefinedWorkerResponse,
        ) -> Result<ExprEvaluationResult, EvaluationError> {
            let mut evaluation_context = EvaluationContext::from_request_data(input);

            let response_context = EvaluationContext::from_refined_worker_response(worker_response);

            if let Some(variables) = response_context.variables {
                evaluation_context.merge_variables(&variables);
                evaluation_context.clone()
            } else {
                evaluation_context.clone()
            };

            let eval_result = self.evaluate(expr, &evaluation_context).await?;
            Ok(eval_result)
        }
    }

    trait WorkerBridgeExt {
        fn to_test_worker_bridge_response(&self) -> RefinedWorkerResponse;
    }

    impl WorkerBridgeExt for WorkerResponse {
        fn to_test_worker_bridge_response(&self) -> RefinedWorkerResponse {
            RefinedWorkerResponse::SingleResult(self.result.result.clone())
        }
    }

    #[tokio::test]
    async fn test_evaluation_with_request_path() {
        let noop_executor = DefaultEvaluator::noop();
        let uri = Uri::builder().path_and_query("/pId/items").build().unwrap();

        let path_pattern = AllPathPatterns::from_str("/{id}/items").unwrap();

        let resolved_variables = request_details_from_request_path_variables(uri, path_pattern);

        let expr = rib::from_string("${request.path.id}").unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::Str("pId".to_string());
        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_variables)
            .await;
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_id() {
        let noop_executor = DefaultEvaluator::noop();

        let resolved_variables = resolved_variables_from_request_body(
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

        let expr = rib::from_string("${request.body.id}").unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::Str("bId".to_string());
        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_variables)
            .await;
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_select_index() {
        let noop_executor = DefaultEvaluator::noop();

        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {

                           "id": "bId",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ]

                    }"#,
            &HeaderMap::new(),
        );

        let expr = rib::from_string("${request.body.titles[0]}").unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::Str("bTitle1".to_string());
        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_variables)
            .await;
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_select_from_object() {
        let noop_executor = DefaultEvaluator::noop();

        let resolved_request = resolved_variables_from_request_body(
            r#"
                    {

                     "address": {
                         "street": "bStreet",
                         "city": "bCity"
                      }

                    }"#,
            &HeaderMap::new(),
        );

        let expr = rib::from_string("${request.body.address.street} ${request.body.address.city}")
            .unwrap();

        dbg!(expr.clone());
        let expected_evaluated_result = TypeAnnotatedValue::Str("bStreet bCity".to_string());
        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_request)
            .await;
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_if_condition() {
        let noop_executor = DefaultEvaluator::noop();

        let mut header_map = HeaderMap::new();
        header_map.insert("authorisation", "admin".parse().unwrap());

        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "id": "bId"
                    }"#,
            &header_map,
        );

        let expr =
            rib::from_string(r#"${if request.headers.authorisation == "admin" then 200 else 401}"#)
                .unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::U64("200".parse().unwrap());
        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_variables)
            .await;
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_select_unknown_field() {
        let noop_executor = DefaultEvaluator::noop();

        let request_details = resolved_variables_from_request_body(
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

        let expr = rib::from_string("${request.body.address.street2}").unwrap();
        let expected_evaluated_result =
            EvaluationError::InvalidReference(GetError::KeyNotFound("street2".to_string()));

        let result = noop_executor
            .evaluate_with_request_details(&expr, &request_details)
            .await;
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_select_invalid_index() {
        let noop_executor = DefaultEvaluator::noop();

        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {

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

        let expr = rib::from_string("${request.body.titles[4]}").unwrap();
        let expected_evaluated_result =
            EvaluationError::InvalidReference(GetError::IndexNotFound(4));

        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_variables)
            .await;
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_index_of_object() {
        let noop_executor = DefaultEvaluator::noop();

        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                      "id": "bId",
                      "address": {
                        "street": "bStreet",
                        "city": "bCity"
                      }
                    }"#,
            &HeaderMap::new(),
        );

        let expr = rib::from_string("${request.body.address[4]}").unwrap();
        let expected_evaluated_result = EvaluationError::InvalidReference(GetError::NotArray {
            index: 4,
            found: json!(
                {
                    "street": "bStreet",
                    "city": "bCity"
                }
            )
            .to_string(),
        });

        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_variables)
            .await;
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_invalid_type_comparison() {
        let noop_executor = DefaultEvaluator::noop();

        let mut header_map = HeaderMap::new();
        header_map.insert("authorisation", "admin".parse().unwrap());

        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {

                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ]

                    }"#,
            &header_map,
        );

        let expr =
            rib::from_string("${if request.headers.authorisation then 200 else 401}").unwrap();

        let expected_evaluated_result = EvaluationError::Message(format!(
            "The predicate text is evaluated to {} but it is not a boolean value",
            json!("admin")
        ));
        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_variables)
            .await;
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_request_body_invalid_object_reference() {
        let noop_executor = DefaultEvaluator::noop();

        let resolved_variables = resolved_variables_from_request_body(
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

        let expr = rib::from_string("${request.body.address.street.name}").unwrap();
        let expected_evaluated_result = EvaluationError::InvalidReference(GetError::NotRecord {
            key_name: "name".to_string(),
            found: json!("bStreet").to_string(),
        });

        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_variables)
            .await;
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_zero_worker_response() {
        let noop_executor = DefaultEvaluator::noop();

        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "path": {
                           "id": "pId"
                        }
                    }"#,
            &HeaderMap::new(),
        );

        let expr = rib::from_string("${worker.response.address.street}").unwrap();
        let result = noop_executor
            .evaluate_with_request_details(&expr, &resolved_variables)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_optional() {
        let noop_executor = DefaultEvaluator::noop();

        let value: Value = serde_json::from_str(
            r#"
                        {

                           "id": "pId"
                        }
                   "#,
        )
        .expect("Failed to parse json");

        let expected_type = infer_analysed_type(&value);
        let result_as_typed_value =
            get_typed_value_from_json(&value, &AnalysedType::Option(Box::new(expected_type)))
                .unwrap();
        let worker_response = RefinedWorkerResponse::from_worker_response(&WorkerResponse::new(
            TypeAnnotatedValue::Tuple {
                typ: vec![AnalysedType::from(&result_as_typed_value)],
                value: vec![result_as_typed_value],
            },
            vec![FunctionResult {
                name: None,
                typ: AnalysedType::Record(vec![("id".to_string(), AnalysedType::Str)]).into(),
            }],
        ))
        .unwrap();

        let expr = rib::from_string(
            r#"${match worker.response { some(value) => "personal-id", none => "not found" }}"#,
        )
        .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response)
            .await;
        assert_eq!(
            result,
            Ok(TypeAnnotatedValue::Str("personal-id".to_string()))
        );
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_none() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response =
            get_worker_response(Value::Null.to_string().as_str()).to_test_worker_bridge_response();

        let expr = rib::from_string(
            r#"${match worker.response { some(value) => "personal-id", none => "not found" }}"#,
        )
        .unwrap();

        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response)
            .await;
        assert_eq!(result, Ok(TypeAnnotatedValue::Str("not found".to_string())));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_other_exprs() {
        let noop_executor = DefaultEvaluator::noop();

        let uri = Uri::builder()
            .path_and_query("/shopping-cart/foo")
            .build()
            .unwrap();

        let path_pattern = AllPathPatterns::from_str("/shopping-cart/{id}").unwrap();

        let resolved_variables_path =
            request_details_from_request_path_variables(uri.clone(), path_pattern.clone());

        let worker_bridge_response = &get_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "baz"
                        }
                    }"#,
        )
        .to_test_worker_bridge_response();

        let expr1 = rib::from_string(
            r#"${if request.path.id == "foo" then "bar" else match worker.response { ok(value) => value.id, err(msg) => "empty" }}"#,
        )
            .unwrap();

        let result1 = noop_executor
            .evaluate_with(&expr1, &resolved_variables_path, worker_bridge_response)
            .await;

        let expr2 = rib::from_string(
            r#"${if request.path.id == "bar" then "foo" else match worker.response { ok(foo) => foo.id, err(msg) => "empty" }}"#,

        ).unwrap();

        let result2 = noop_executor
            .evaluate_with(&expr2, &resolved_variables_path, worker_bridge_response)
            .await;

        let error_worker_response = get_worker_response(
            r#"
                    {
                        "err": {
                           "msg": "failed"
                        }
                    }"#,
        );

        let new_resolved_variables_from_request_path =
            request_details_from_request_path_variables(uri, path_pattern);

        let error_response_with_request_variables = new_resolved_variables_from_request_path;
        let error_worker_response = error_worker_response.to_test_worker_bridge_response();

        let expr3 = rib::from_string(
            r#"${if request.path.id == "bar" then "foo" else match worker.response { ok(foo) => foo.id, err(msg) => "empty" }}"#,

        ).unwrap();

        let result3 = noop_executor
            .evaluate_with(
                &expr3,
                &error_response_with_request_variables,
                &error_worker_response,
            )
            .await;

        assert_eq!(
            (result1, result2, result3),
            (
                Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Str(
                    "bar".to_string()
                ))),
                Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Str(
                    "baz".to_string()
                ))),
                Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Str(
                    "empty".to_string()
                )))
            )
        );
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "pId"
                        }
                    }"#,
        )
        .to_test_worker_bridge_response();

        let expr = rib::from_string(
            r#"${match worker.response { ok(value) => "personal-id", err(msg) => "not found" }}"#,
        )
        .unwrap();

        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response)
            .await;
        assert_eq!(
            result,
            Ok(TypeAnnotatedValue::Str("personal-id".to_string()))
        );
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_use_success_variable() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "pId"
                        }
                    }"#,
        )
        .to_test_worker_bridge_response();

        let expr = rib::from_string(
            r#"${match worker.response { ok(value) => value, err(msg) => "not found" }}"#,
        )
        .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response)
            .await;

        let expected_result = TypeAnnotatedValue::Record {
            value: vec![("id".to_string(), TypeAnnotatedValue::Str("pId".to_string()))],
            typ: vec![("id".to_string(), AnalysedType::Str)],
        };

        assert_eq!(result, Ok(expected_result));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_select_field() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "pId"
                        }
                    }"#,
        );

        let expr = rib::from_string(
            r#"${match worker.response { ok(value) => value.id, err(msg) => "not found" }}"#,
        )
        .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await;
        assert_eq!(result, Ok(TypeAnnotatedValue::Str("pId".to_string())));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_select_from_array() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = rib::from_string(
            r#"${match worker.response { ok(value) => value.ids[0], err(msg) => "not found" }}"#,
        )
        .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await;
        assert_eq!(result, Ok(TypeAnnotatedValue::Str("id1".to_string())));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_some_construction() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = rib::from_string(
            r#"${match worker.response { ok(value) => some(value.ids[0]), err(msg) => "not found" }}"#,
        )
        .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await;
        let expected = TypeAnnotatedValue::Option {
            value: Some(Box::new(TypeAnnotatedValue::Str("id1".to_string()))),
            typ: AnalysedType::Str,
        };
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_none_construction() {
        let noop_executor = DefaultEvaluator::noop();
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = rib::from_string(
            r#"${match worker.response { ok(value) => none, none => "not found" }}"#,
        )
        .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await;
        let expected = TypeAnnotatedValue::Option {
            value: None,
            typ: AnalysedType::Str,
        };
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_nested_construction() {
        let noop_executor = DefaultEvaluator::noop();
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr =
            rib::from_string("${match worker.response { ok(value) => some(none), none => none }}")
                .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await;
        let expected = TypeAnnotatedValue::Option {
            value: Some(Box::new(TypeAnnotatedValue::Option {
                typ: AnalysedType::Str,
                value: None,
            })),
            typ: AnalysedType::Option(Box::new(AnalysedType::Str)),
        };
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_ok_construction() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr =
            rib::from_string("${match worker.response { ok(value) => ok(1), none => err(2) }}")
                .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await;
        let expected = TypeAnnotatedValue::Result {
            value: Ok(Some(Box::new(TypeAnnotatedValue::U64(1)))),
            ok: Some(Box::new(AnalysedType::U64)),
            error: None,
        };
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_err_construction() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = get_worker_response(
            r#"
                    {
                        "err": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr =
            rib::from_string("${match worker.response { ok(value) => ok(1), err(msg) => err(2) }}")
                .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await;

        let expected = TypeAnnotatedValue::Result {
            value: Err(Some(Box::new(TypeAnnotatedValue::U64(2)))),
            error: Some(Box::new(AnalysedType::U64)),
            ok: None,
        };
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_wild_card() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = get_worker_response(
            r#"
                    {
                        "err": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr =
            rib::from_string("${match worker.response { ok(_) => ok(1), err(_) => err(2) }}")
                .unwrap();

        dbg!(expr.clone());
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await;

        let expected = TypeAnnotatedValue::Result {
            value: Err(Some(Box::new(TypeAnnotatedValue::U64(2)))),
            error: Some(Box::new(AnalysedType::U64)),
            ok: None,
        };
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_with_name_alias() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = get_worker_response(
            r#"
                    {
                        "err": {
                           "ok": {
                             "id": 1
                            }
                        }
                    }"#,
        );

        let expr = rib::from_string(
            "${match worker.response { a @ ok(b @ _) => ok(1), c @ err(d @ ok(e)) => {p : c, q: d, r: e.id} }}",
        )
            .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await
            .unwrap();

        let output_json = golem_wasm_rpc::json::get_json_from_typed_value(&result);

        let expected_json = json!({
            "p": {
                "err": {
                    "ok": {
                        "id": 1
                    }
                }
            },
            "q": {
                "ok": {
                    "id": 1
                }
            },
            "r": 1
        });
        assert_eq!(output_json, expected_json);
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_variant_positive() {
        let noop_executor = DefaultEvaluator::noop();

        let worker_response = WorkerResponse::new(
            TypeAnnotatedValue::Variant {
                case_name: "Foo".to_string(),
                case_value: Some(Box::new(TypeAnnotatedValue::Record {
                    typ: vec![("id".to_string(), AnalysedType::Str)],
                    value: vec![("id".to_string(), TypeAnnotatedValue::Str("pId".to_string()))],
                })),
                typ: vec![(
                    "Foo".to_string(),
                    Some(AnalysedType::Record(vec![(
                        "id".to_string(),
                        AnalysedType::Str,
                    )])),
                )],
            },
            vec![],
        );

        let expr =
            rib::from_string("${match worker.response { Foo(value) => ok(value.id) }}").unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response.to_test_worker_bridge_response())
            .await;

        let expected = TypeAnnotatedValue::Result {
            value: Ok(Some(Box::new(TypeAnnotatedValue::Str("pId".to_string())))),
            error: None,
            ok: Some(Box::new(AnalysedType::Str)),
        };
        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_variant_nested_with_some() {
        let noop_executor = DefaultEvaluator::noop();

        let output = TypeAnnotatedValue::Variant {
            case_name: "Foo".to_string(),
            case_value: Some(Box::new(TypeAnnotatedValue::Option {
                value: Some(Box::new(TypeAnnotatedValue::Record {
                    typ: vec![("id".to_string(), AnalysedType::Str)],
                    value: vec![("id".to_string(), TypeAnnotatedValue::Str("pId".to_string()))],
                })),
                typ: AnalysedType::Record(vec![("id".to_string(), AnalysedType::Str)]),
            })),
            typ: vec![
                (
                    "Foo".to_string(),
                    Some(AnalysedType::Option(Box::new(AnalysedType::Record(vec![
                        ("id".to_string(), AnalysedType::Str),
                    ])))),
                ),
                (
                    "Bar".to_string(),
                    Some(AnalysedType::Option(Box::new(AnalysedType::Record(vec![
                        ("id".to_string(), AnalysedType::Str),
                    ])))),
                ),
            ],
        };

        let worker_bridge_response =
            WorkerResponse::new(output, vec![]).to_test_worker_bridge_response();

        let expr = rib::from_string(
            r#"${match worker.response { Foo(some(value)) => value.id, err(msg) => "not found" }}"#,
        )
        .unwrap();

        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_bridge_response)
            .await;

        let expected = TypeAnnotatedValue::Str("pId".to_string());

        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_variant_nested_with_some_result() {
        let noop_executor = DefaultEvaluator::noop();

        let output = get_complex_variant_typed_value();

        let worker_bridge_response =
            WorkerResponse::new(output, vec![]).to_test_worker_bridge_response();

        let expr = rib::from_string(
            r#"${match worker.response { Foo(some(ok(value))) => value.id, err(msg) => "not found" }}"#,
        )
            .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_bridge_response)
            .await;

        let expected = TypeAnnotatedValue::Str("pId".to_string());

        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_variant_nested_type_mismatch() {
        let noop_executor = DefaultEvaluator::noop();

        let output = get_complex_variant_typed_value();

        let worker_bridge_response =
            WorkerResponse::new(output, vec![]).to_test_worker_bridge_response();

        let expr = rib::from_string(
            r#"${match worker.response { Foo(ok(some(value))) => value.id, err(msg) => "not found" }}"#,
        )
            .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_bridge_response)
            .await;

        assert!(result
            .err()
            .unwrap()
            .to_string()
            .starts_with("Type mismatch"))
    }

    #[tokio::test]
    async fn test_evaluation_with_pattern_match_variant_nested_with_none() {
        let noop_executor = DefaultEvaluator::noop();

        let output = TypeAnnotatedValue::Variant {
            case_name: "Foo".to_string(),
            case_value: Some(Box::new(TypeAnnotatedValue::Option {
                value: None,
                typ: AnalysedType::Record(vec![("id".to_string(), AnalysedType::Str)]),
            })),
            typ: vec![
                (
                    "Foo".to_string(),
                    Some(AnalysedType::Option(Box::new(AnalysedType::Record(vec![
                        ("id".to_string(), AnalysedType::Str),
                    ])))),
                ),
                (
                    "Bar".to_string(),
                    Some(AnalysedType::Option(Box::new(AnalysedType::Record(vec![
                        ("id".to_string(), AnalysedType::Str),
                    ])))),
                ),
            ],
        };

        let worker_response = WorkerResponse::new(output, vec![]).to_test_worker_bridge_response();

        let expr = rib::from_string(
            r#"${match worker.response { Foo(none) => "not found",  Foo(some(value)) => value.id }}"#,
        )
        .unwrap();
        let result = noop_executor
            .evaluate_with_worker_response(&expr, &worker_response)
            .await;

        let expected = TypeAnnotatedValue::Str("not found".to_string());

        assert_eq!(result, Ok(expected));
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_ok_record() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${{a : ok(1)}}").unwrap();

        let result = noop_executor
            .evaluate(&expr, &EvaluationContext::empty())
            .await;

        let expected = Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Record {
            typ: vec![(
                "a".to_string(),
                AnalysedType::Result {
                    ok: Some(Box::new(AnalysedType::U64)),
                    error: None,
                },
            )],
            value: vec![(
                "a".to_string(),
                TypeAnnotatedValue::Result {
                    ok: Some(Box::new(AnalysedType::U64)),
                    error: None,
                    value: Ok(Some(Box::new(TypeAnnotatedValue::U64(1)))),
                },
            )],
        }));

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_err_record() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${{a : err(1)}}").unwrap();

        let result = noop_executor
            .evaluate(&expr, &EvaluationContext::empty())
            .await;

        let expected = Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Record {
            typ: vec![(
                "a".to_string(),
                AnalysedType::Result {
                    error: Some(Box::new(AnalysedType::U64)),
                    ok: None,
                },
            )],
            value: vec![(
                "a".to_string(),
                TypeAnnotatedValue::Result {
                    ok: None,
                    error: Some(Box::new(AnalysedType::U64)),
                    value: Err(Some(Box::new(TypeAnnotatedValue::U64(1)))),
                },
            )],
        }));

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_simple_list() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${[1,2,3]}").unwrap();

        let result = noop_executor
            .evaluate(&expr, &EvaluationContext::empty())
            .await;

        let expected = Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::List {
            typ: AnalysedType::U64,
            values: vec![
                TypeAnnotatedValue::U64(1),
                TypeAnnotatedValue::U64(2),
                TypeAnnotatedValue::U64(3),
            ],
        }));

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_simple_tuple() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${(some(1),2,3)}").unwrap();

        let result = noop_executor
            .evaluate(&expr, &EvaluationContext::empty())
            .await;

        let expected = Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Tuple {
            typ: vec![
                AnalysedType::Option(Box::new(AnalysedType::U64)),
                AnalysedType::U64,
                AnalysedType::U64,
            ],
            value: vec![
                TypeAnnotatedValue::Option {
                    value: Some(Box::new(TypeAnnotatedValue::U64(1))),
                    typ: AnalysedType::U64,
                },
                TypeAnnotatedValue::U64(2),
                TypeAnnotatedValue::U64(3),
            ],
        }));

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_wave_like_syntax_flag() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${{A, B, C}}").unwrap();

        let result = noop_executor
            .evaluate(&expr, &EvaluationContext::empty())
            .await;

        let expected = Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Flags {
            typ: vec!["A".to_string(), "B".to_string(), "C".to_string()],
            values: vec!["A".to_string(), "B".to_string(), "C".to_string()],
        }));

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_wave_like_syntax_result_list() {
        let noop_executor = DefaultEvaluator::noop();

        let expr = rib::from_string("${[ok(1),ok(2)]}").unwrap();

        let result = noop_executor
            .evaluate(&expr, &EvaluationContext::empty())
            .await;

        let expected = Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::List {
            typ: AnalysedType::Result {
                ok: Some(Box::new(AnalysedType::U64)),
                error: None,
            },
            values: vec![
                TypeAnnotatedValue::Result {
                    ok: Some(Box::new(AnalysedType::U64)),
                    error: None,
                    value: Ok(Some(Box::new(TypeAnnotatedValue::U64(1)))),
                },
                TypeAnnotatedValue::Result {
                    ok: Some(Box::new(AnalysedType::U64)),
                    error: None,
                    value: Ok(Some(Box::new(TypeAnnotatedValue::U64(2)))),
                },
            ],
        }));

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_evaluation_with_multiple_lines() {
        let noop_executor = DefaultEvaluator::noop();

        let program = r"
            let x = { a : 1 };
            let y = { b : 2 };
            let z = x.a > y.b;
            z
          ";

        let expr = rib::from_string(format!("${{{}}}", program)).unwrap();
        dbg!(expr.clone());

        let result = noop_executor
            .evaluate(&expr, &EvaluationContext::empty())
            .await;

        let expected = Ok(ExprEvaluationResult::Value(TypeAnnotatedValue::Bool(false)));

        assert_eq!(result, expected);
    }

    mod test_utils {
        use crate::api_definition::http::{AllPathPatterns, PathPattern, VarInfo};
        use crate::evaluator::tests::{EvaluatorTestExt, WorkerBridgeExt};
        use crate::evaluator::DefaultEvaluator;
        use crate::http::router::RouterPattern;
        use crate::worker_binding::RequestDetails;
        use crate::worker_bridge_execution::WorkerResponse;
        use golem_service_base::type_inference::infer_analysed_type;
        use golem_wasm_ast::analysis::AnalysedType;
        use golem_wasm_rpc::json::get_typed_value_from_json;
        use golem_wasm_rpc::TypeAnnotatedValue;
        use http::{HeaderMap, Uri};
        use serde_json::{json, Value};
        use std::collections::HashMap;

        pub(crate) fn get_complex_variant_typed_value() -> TypeAnnotatedValue {
            TypeAnnotatedValue::Variant {
                case_name: "Foo".to_string(),
                case_value: Some(Box::new(TypeAnnotatedValue::Option {
                    value: Some(Box::new(TypeAnnotatedValue::Result {
                        value: Ok(Some(Box::new(TypeAnnotatedValue::Record {
                            typ: vec![("id".to_string(), AnalysedType::Str)],
                            value: vec![(
                                "id".to_string(),
                                TypeAnnotatedValue::Str("pId".to_string()),
                            )],
                        }))),
                        ok: Some(Box::new(AnalysedType::Record(vec![(
                            "id".to_string(),
                            AnalysedType::Str,
                        )]))),
                        error: None,
                    })),
                    typ: AnalysedType::Result {
                        ok: Some(Box::new(AnalysedType::Record(vec![(
                            "id".to_string(),
                            AnalysedType::Str,
                        )]))),
                        error: None,
                    },
                })),
                typ: vec![
                    (
                        "Foo".to_string(),
                        Some(AnalysedType::Option(Box::new(AnalysedType::Result {
                            ok: Some(Box::new(AnalysedType::Record(vec![(
                                "id".to_string(),
                                AnalysedType::Str,
                            )]))),
                            error: None,
                        }))),
                    ),
                    (
                        "Bar".to_string(),
                        Some(AnalysedType::Option(Box::new(AnalysedType::Result {
                            ok: Some(Box::new(AnalysedType::Record(vec![(
                                "id".to_string(),
                                AnalysedType::Str,
                            )]))),
                            error: None,
                        }))),
                    ),
                ],
            }
        }

        pub(crate) fn get_err_worker_response() -> WorkerResponse {
            let worker_response_value = get_typed_value_from_json(
                &json!({"err": { "id" : "afsal"} }),
                &AnalysedType::Result {
                    error: Some(Box::new(AnalysedType::Record(vec![(
                        "id".to_string(),
                        AnalysedType::Str,
                    )]))),
                    ok: None,
                },
            )
            .unwrap();

            WorkerResponse::new(worker_response_value, vec![])
        }

        pub(crate) fn get_worker_response(input: &str) -> WorkerResponse {
            let value: Value = serde_json::from_str(input).expect("Failed to parse json");

            let expected_type = infer_analysed_type(&value);
            let result_as_typed_value = get_typed_value_from_json(&value, &expected_type).unwrap();
            WorkerResponse::new(result_as_typed_value, vec![])
        }

        pub(crate) fn resolved_variables_from_request_body(
            input: &str,
            header_map: &HeaderMap,
        ) -> RequestDetails {
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

            let worker_response = get_err_worker_response();

            let expr1_string =
                r#"${match worker.response { ok(x) => "foo", err(msg) => "error" }}"#;
            let expr1 = rib::from_string(expr1_string).unwrap();
            let value1 = noop_executor
                .evaluate_with_worker_response(
                    &expr1,
                    &worker_response.to_test_worker_bridge_response(),
                )
                .await
                .unwrap();

            let expr2_string = expr1.to_string();
            let expr2 = rib::from_string(expr2_string.as_str()).unwrap();
            let value2 = noop_executor
                .evaluate_with_worker_response(
                    &expr2,
                    &worker_response.to_test_worker_bridge_response(),
                )
                .await
                .unwrap();

            let expected = TypeAnnotatedValue::Str("error".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }

        #[tokio::test]
        async fn expr_to_string_round_trip_match_expr_append() {
            let noop_executor = DefaultEvaluator::noop();

            let worker_response = get_err_worker_response().to_test_worker_bridge_response();

            let expr1_string =
                r#"append-${match worker.response { ok(x) => "foo", err(msg) => "error" }}"#;
            let expr1 = rib::from_string(expr1_string).unwrap();
            let value1 = noop_executor
                .evaluate_with_worker_response(&expr1, &worker_response)
                .await
                .unwrap();

            let expr2_string = expr1.to_string();
            let expr2 = rib::from_string(expr2_string.as_str()).unwrap();
            let value2 = noop_executor
                .evaluate_with_worker_response(&expr2, &worker_response)
                .await
                .unwrap();

            let expected = TypeAnnotatedValue::Str("append-error".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }

        #[tokio::test]
        async fn expr_to_string_round_trip_match_expr_append_suffix() {
            let noop_executor = DefaultEvaluator::noop();

            let worker_response = get_err_worker_response();

            let expr1_string =
                r#"prefix-${match worker.response { ok(x) => "foo", err(msg) => "error" }}-suffix"#;

            let expr1 = rib::from_string(expr1_string).unwrap();
            let value1 = noop_executor
                .evaluate_with_worker_response(
                    &expr1,
                    &worker_response.to_test_worker_bridge_response(),
                )
                .await
                .unwrap();

            let expr2_string = expr1.to_string();
            let expr2 = rib::from_string(expr2_string.as_str()).unwrap();
            let value2 = noop_executor
                .evaluate_with_worker_response(
                    &expr2,
                    &worker_response.to_test_worker_bridge_response(),
                )
                .await
                .unwrap();

            let expected = TypeAnnotatedValue::Str("prefix-error-suffix".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }
    }
}

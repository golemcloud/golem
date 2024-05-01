use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::get_json_from_typed_value;
use golem_wasm_rpc::TypeAnnotatedValue;

use crate::primitive::{GetPrimitive, Primitive};
use getter::GetError;
use getter::Getter;
use path::Path;
use crate::expression;
use crate::worker_bridge_execution::{WorkerBridgeResponse};

use crate::expression::{Expr, InnerNumber};
use crate::merge::Merge;

use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

mod getter;
mod math_op_evaluator;
mod path;
mod pattern_match_evaluator;


pub trait Evaluator {
    fn evaluate(&self, input_request: &TypeAnnotatedValue, worker_bridge_response: Option<&WorkerBridgeResponse>) -> Result<EvaluationResult, EvaluationError>;
}

#[derive(Debug, Clone)]
pub enum EvaluationResult {
    Value(TypeAnnotatedValue),
    Unit,
}

impl EvaluationResult {

    pub fn get_primitive(&self) -> Option<Primitive> {
        match self {
            EvaluationResult::Value(value) => value.get_primitive(),
            EvaluationResult::Unit => None,
        }
    }

    pub fn is_unit(&self)  -> bool {
        match self {
            EvaluationResult::Unit => true,
            _ => false,
        }
    }

    pub fn is_value(&self) -> bool {
        match self {
            EvaluationResult::Value(_) => true,
            _ => false,
        }
    }
    pub fn get_value(&self) -> Option<TypeAnnotatedValue> {
        match self {
            EvaluationResult::Value(value) => Some(value.clone()),
            EvaluationResult::Unit => None,
        }
    }
}

impl From<TypeAnnotatedValue> for EvaluationResult {
    fn from(value: TypeAnnotatedValue) -> Self {
        EvaluationResult::Value(value)
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum EvaluationError {
    #[error(transparent)]
    InvalidReference(#[from] GetError),
    #[error("{0}")]
    Message(String),
}

pub struct RawString<'t> {
    pub input: &'t str,
}

// When we expect only primitives within a string, and uses ${} not as an expr,
// but as a mere place holder. This type disallows complex structures to end up
// in values such as function-name.
impl<'t> RawString<'t> {
    pub fn new(str: &'t str) -> RawString<'t> {
        RawString { input: str }
    }
}

// Foo/{user-id}
impl<'t> Evaluator for RawString<'t> {
    fn evaluate(&self, input: &TypeAnnotatedValue, _worker_bridge_response: Option<&WorkerBridgeResponse>) -> Result<EvaluationResult, EvaluationError> {
        let mut combined_string = String::new();
        let mut tokenizer: Tokenizer = Tokenizer::new(self.input);

        while let Some(token) = tokenizer.next_token() {
            match token {
                Token::MultiChar(MultiCharTokens::InterpolationStart) => {
                    let place_holder_name = tokenizer.capture_string_until(&Token::RParen);

                    if let Some(place_holder_name) = place_holder_name {
                        let type_annotated_value =
                            input.get(&Path::from_key(place_holder_name.as_str()))?;

                        match type_annotated_value.get_primitive() {
                            Some(primitive) => {
                                combined_string.push_str(primitive.to_string().as_str())
                            }

                            None => {
                                return Err(EvaluationError::Message(format!(
                                    "Unsupported json type to be replaced in place holder. Make sure the values are primitive {}",
                                    place_holder_name,
                                )));
                            }
                        }
                    }
                }
                token => combined_string.push_str(token.to_string().as_str()),
            }
        }

        Ok(TypeAnnotatedValue::Str(combined_string).into())
    }
}

impl Evaluator for Expr {
    fn evaluate(&self, input: &TypeAnnotatedValue, worker_response: Option<&WorkerBridgeResponse>) -> Result<EvaluationResult, EvaluationError> {
        let expr: &Expr = self;

        // An expression evaluation needs to be careful with string values
        // and therefore returns ValueTyped
        fn go(
            expr: &Expr,
            input: &mut TypeAnnotatedValue,
            worker_response: Option<&WorkerBridgeResponse>
        ) -> Result<EvaluationResult, EvaluationError> {
            match expr {
                Expr::Request() => input
                    .get(&Path::from_key(Token::request().to_string().as_str()))
                    .map(|x|  x.into())
                    .map_err(|err| err.into()),

                Expr::WorkerResponse() => {
                   let result = match worker_response {
                        Some(WorkerBridgeResponse::MultipleResults(results)) => results.clone().into(),
                        Some(WorkerBridgeResponse::SingleResult(result)) => result.clone().into(),
                        Some(WorkerBridgeResponse::Unit) => EvaluationResult::Unit,
                        None => Err("Worker response is not available".to_string())?
                    };

                    Ok(result)
                },

                Expr::SelectIndex(expr, index) => {
                    let evaluation_result = go(expr, input, worker_response)?;
                    evaluation_result.get_value().ok_or(format!("The expression is evaluated to unit and doesn't have an index {}", index).into())?
                        .get(&Path::from_index(*index))
                        .map(|r| r.into())
                        .map_err(|err| err.into())
                }

                Expr::SelectField(expr, field_name) => {
                    let evaluation_result = go(expr, input, worker_response)?.get_value()
                        .ok_or(format!("The expression is evaluated to unit and doesn't have an field {}", field_name).into())?;;

                    evaluation_result
                        .get(&Path::from_key(field_name.as_str()))
                        .map(|r| r.into())
                        .map_err(|err| err.into())
                }

                Expr::EqualTo(left, right) => {
                    let left = go(left, input, worker_response)?;
                    let right = go(right, input, worker_response)?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| left == right)

                }
                Expr::GreaterThan(left, right) => {
                    let left = go(left, input, worker_response)?
                        .ok_or(format!("{} is evaluated to none", left))?;
                    let right = go(right, input, worker_response)?
                        .ok_or(format!("{} is evaluated to none", right))?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| left > right)
                }
                Expr::GreaterThanOrEqualTo(left, right) => {
                    let left = go(left, input, worker_response)?
                        .ok_or(format!("{} is evaluated to none", left))?;
                    let right = go(right, input, worker_response)?
                        .ok_or(format!("{} is evaluated to none", right))?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| left >= right)
                }
                Expr::LessThan(left, right) => {
                    let left = go(left, input, worker_response)?
                        .ok_or(format!("{} is evaluated to none", left))?;
                    let right = go(right, input, worker_response)?
                        .ok_or(format!("{} is evaluated to none", right))?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| left < right)
                }
                Expr::LessThanOrEqualTo(left, right) => {
                    let left = go(left, input, worker_response)?
                        .ok_or(format!("{} is evaluated to none", left))?;
                    let right = go(right, input, worker_response)?
                        .ok_or(format!("{} is evaluated to none", right))?;

                    math_op_evaluator::compare_eval_result(&left, &right, |left, right| left <= right)
                }

                Expr::Not(expr) => {
                    let evaluated_expr = expr.evaluate(input, worker_response)?;

                    match evaluated_expr {
                        EvaluationResult::Value(TypeAnnotatedValue::Bool(value)) => Ok(TypeAnnotatedValue::Bool(!value)),
                        _ => Err(EvaluationError::Message(format!(
                            "The expression is evaluated to {} but it is not a boolean value to apply not (!) operator on",
                           &evaluated_expr.get_value().map_or("unit", |eval_result| get_json_from_typed_value(&eval_result))
                        ))),
                    }
                }

                Expr::Cond(pred0, left, right) => {
                    let pred = go(pred0, input, worker_response)?;
                    let left = go(left, input, worker_response)?;
                    let right = go(right, input, worker_response)?;

                    match pred {
                        EvaluationResult::Value(TypeAnnotatedValue::Bool(value)) => {
                            if value {
                                Ok(left)
                            } else {
                                Ok(right)
                            }
                        }
                        _ => Err(EvaluationError::Message(format!(
                            "The predicate expression is evaluated to {} but it is not a boolean value",
                            &pred.get_value().map_or("unit", |eval_result| get_json_from_typed_value(&eval_result))
                        ))),
                    }
                }

                Expr::Let(str, expr) => {
                    let eval_result = go(expr, input, worker_response)?;

                    eval_result.get_value().map_or(Ok(EvaluationResult::Unit), |value| {
                        let typ = AnalysedType::from(&value);

                        let result = TypeAnnotatedValue::Record {
                            value: vec![(str.to_string(), value)],
                            typ: vec![(str.to_string(), typ)],
                        };

                        input.merge(&result);

                        Ok(EvaluationResult::Unit) // Result of a let binding is Unit
                    })
                }

                Expr::Multiple(multiple) => {
                    let mut result: Vec<EvaluationResult> = vec![];

                    for expr in multiple {
                        match go(expr, input, worker_response) {
                            Ok(expr_result) => {
                                if let Some(value) = expr_result.get_value() {
                                    input.merge(&value);
                                }
                                result.push(expr_result);
                            }
                            Err(result) => return Err(result),
                        }
                    }

                    Ok(result.last().map_or(EvaluationResult::Unit, |last| last.clone()))
                }

                Expr::Sequence(exprs) => {
                    let mut result: Vec<TypeAnnotatedValue> = vec![];

                    for expr in exprs {
                        match go(expr, input, worker_response) {
                            Ok(eval_result) => {
                                if let Some(value) = eval_result.get_value() {
                                    result.push(value);
                                } else {
                                    return Err(format!("The expression {} is evaluated to unit and cannot be part of a record", expression::to_string(expr).unwrap()));
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
                        match go(expr, input, worker_response) {
                            Ok(expr_result) => {
                                if let Some(value) = expr_result.get_value() {
                                    values.push((key.to_string(), value));
                                } else {
                                    return Err(format!("The expression for key {} is evaluated to unit and cannot be part of a record", key));
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
                    }.into())
                }

                Expr::Concat(exprs) => {
                    let mut result = String::new();

                    for expr in exprs {
                        match go(expr, input, worker_response) {
                            Ok(value) => {
                                if let Some(primitive) = value.get_primitive() {
                                    result.push_str(primitive.to_string().as_str())
                                } else {
                                    return Err(EvaluationError::Message(format!("Cannot append a complex expression {} or unit to form text", get_json_from_typed_value(&value))));
                                }
                            }

                            Err(result) => return Err(result),
                        }
                    }

                    Ok(TypeAnnotatedValue::Str(result))
                }

                Expr::Literal(literal) => Ok(TypeAnnotatedValue::Str(literal.clone()).into()),

                Expr::Number(number) => match number {
                    InnerNumber::UnsignedInteger(u64) => Ok(TypeAnnotatedValue::U64(*u64).into()),
                    InnerNumber::Integer(i64) => Ok(TypeAnnotatedValue::S64(*i64).into()),
                    InnerNumber::Float(f64) => Ok(TypeAnnotatedValue::F64(*f64).into()),
                },

                Expr::Variable(variable) => input
                    .get(&Path::from_key(variable.as_str()))
                    .map(|v| v.into())
                    .map_err(|err| err.into()),

                Expr::Boolean(bool) => Ok(TypeAnnotatedValue::Bool(*bool).into()),
                Expr::PatternMatch(match_expression, arms) => {
                    pattern_match_evaluator::evaluate_pattern_match(match_expression, arms, input, worker_response)
                }

                Expr::Option(option_expr) => match option_expr {
                    Some(expr) => {
                        let expr_result = go(expr, input, worker_response)?;

                        if let Some(value) = expr_result.get_value() {
                            let analysed_type = AnalysedType::from(&value);
                            Ok(TypeAnnotatedValue::Option {
                                value: Some(Box::new(value)),
                                typ: analysed_type,
                            }.into())
                        } else {
                            Err(EvaluationError::Message(format!("The expression {} is evaluated to unit and cannot be part of a option", expression::to_string(expr).unwrap())))
                        }

                    }
                    None => Ok(TypeAnnotatedValue::Option {
                        value: None,
                        typ: AnalysedType::Str,
                    }),
                },

                Expr::Result(result_expr) => match result_expr {
                    Ok(expr) => {
                        let expr_result = go(expr, input, worker_response)?;

                        if let Some(value) = expr_result.get_value() {
                            let analysed_type = AnalysedType::from(&value);

                            Ok(TypeAnnotatedValue::Result {
                                value: Ok(Some(Box::new(value))),
                                ok: Some(Box::new(analysed_type)),
                                error: None,
                            }.into())
                        } else {
                            Err(EvaluationError::Message(format!("The expression {} is evaluated to unit and cannot be part of a result", expression::to_string(expr).unwrap())))
                        }
                    }
                    Err(expr) => {
                        let eval_result = go(expr, input, worker_response)?;

                        if let Some(value) = eval_result.get_value() {
                            let analysed_type = AnalysedType::from(&value);

                            Ok(TypeAnnotatedValue::Result {
                                value: Err(Some(Box::new(value))),
                                ok: None,
                                error: Some(Box::new(analysed_type)),
                            }.into())
                        } else {
                            Err(EvaluationError::Message(format!("The expression {} is evaluated to unit and cannot be part of a result", expression::to_string(expr).unwrap())))
                        }
                    }
                },

                Expr::Tuple(tuple_exprs) => {
                    let mut result: Vec<TypeAnnotatedValue> = vec![];

                    for expr in tuple_exprs {
                        let eval_result = go(expr, input, worker_response)?;

                        if let Some(value) = eval_result.get_value() {
                            result.push(value);
                        } else {
                            return Err(EvaluationError::Message(format!("The expression {} is evaluated to unit and cannot be part of a tuple", expression::to_string(expr).unwrap())));
                        }
                    }

                    let typ: &Vec<AnalysedType> = &result.iter().map(AnalysedType::from).collect();

                    Ok(TypeAnnotatedValue::Tuple {
                        value: result,
                        typ: typ.clone(),
                    }.into())
                }

                Expr::Flags(flags) => Ok(TypeAnnotatedValue::Flags {
                    values: flags.clone(),
                    typ: flags.clone(),
                }),
            }
        }

        let mut input = input.clone();
        go(expr, &mut input, worker_response)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use golem_service_base::type_inference::infer_analysed_type;
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::json::get_typed_value_from_json;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use http::{HeaderMap, Uri};
    use serde_json::{json, Value};

    use crate::api_definition::http::AllPathPatterns;
    use crate::evaluator::getter::GetError;
    use crate::evaluator::{EvaluationError, Evaluator};
    use crate::expression;
    use crate::merge::Merge;
    use crate::worker_bridge_execution::{WorkerBridgeResponse, WorkerResponse};
    use test_utils::*;

    trait EvaluatorTestExt {
      fn evaluate(&self, input: &TypeAnnotatedValue) -> Result<TypeAnnotatedValue, EvaluationError>;
      fn evaluate_worker_bridge_response(&self, worker_bridge_response: &WorkerBridgeResponse) -> Result<TypeAnnotatedValue, EvaluationError>;
    }

    impl<T : Evaluator> EvaluatorTestExt for T {
        fn evaluate(&self, input: &TypeAnnotatedValue) -> Result<TypeAnnotatedValue, EvaluationError> {
            let eval_result = self.evaluate(input, None)?;
            Ok(eval_result.get_value().ok_or("The expression is evaluated to unit and doesn't have a value")?)
        }

        fn evaluate_worker_bridge_response(&self, worker_bridge_response: &WorkerBridgeResponse) -> Result<TypeAnnotatedValue, EvaluationError> {
            let empty_input = TypeAnnotatedValue::Record {
                value: vec![],
                typ: vec![],
            };
            let eval_result = self.evaluate(&empty_input, Some(worker_bridge_response))?;
            Ok(eval_result.get_value().ok_or("The expression is evaluated to unit and doesn't have a value")?)
        }
    }

    trait WorkerBridgeExt {
        fn to_test_worker_bridge_response(&self) -> WorkerBridgeResponse;
    }

    impl WorkerBridgeExt for WorkerResponse {
        fn to_test_worker_bridge_response(&self) -> WorkerBridgeResponse {
            WorkerBridgeResponse::SingleResult(self.result.result.clone())
        }
    }

    #[test]
    fn test_evaluation_with_request_path() {
        let uri = Uri::builder().path_and_query("/pId/items").build().unwrap();

        let path_pattern = AllPathPatterns::from_str("/{id}/items").unwrap();

        let resolved_variables = resolved_variables_from_request_path(uri, path_pattern);

        let expr = expression::from_string("${request.path.id}").unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::Str("pId".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_id() {
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

        let expr = expression::from_string("${request.body.id}").unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::Str("bId".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_select_index() {
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

        let expr = expression::from_string("${request.body.titles[0]}").unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::Str("bTitle1".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_select_from_object() {
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

        let expr =
            expression::from_string("${request.body.address.street} ${request.body.address.city}")
                .unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::Str("bStreet bCity".to_string());
        let result = expr.evaluate(&resolved_request);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_if_condition() {
        let mut header_map = HeaderMap::new();
        header_map.insert("authorisation", "admin".parse().unwrap());

        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "id": "bId"
                    }"#,
            &header_map,
        );

        let expr = expression::from_string(
            "${if request.header.authorisation == 'admin' then 200 else 401}",
        )
        .unwrap();
        let expected_evaluated_result = TypeAnnotatedValue::U64("200".parse().unwrap());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_select_unknown_field() {
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

        let expr = expression::from_string("${request.body.address.street2}").unwrap();
        let expected_evaluated_result =
            EvaluationError::InvalidReference(GetError::KeyNotFound("street2".to_string()));

        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_select_invalid_index() {
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

        let expr = expression::from_string("${request.body.titles[4]}").unwrap();
        let expected_evaluated_result =
            EvaluationError::InvalidReference(GetError::IndexNotFound(4));

        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_index_of_object() {
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

        let expr = expression::from_string("${request.body.address[4]}").unwrap();
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

        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_invalid_type_comparison() {
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

        let expr = expression::from_string("${if request.header.authorisation then 200 else 401}")
            .unwrap();

        let expected_evaluated_result = EvaluationError::Message(format!(
            "The predicate expression is evaluated to {} but it is not a boolean value",
            json!("admin")
        ));
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_invalid_object_reference() {
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

        let expr = expression::from_string("${request.body.address.street.name}").unwrap();
        let expected_evaluated_result = EvaluationError::InvalidReference(GetError::NotRecord {
            key_name: "name".to_string(),
            found: json!("bStreet").to_string(),
        });

        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_zero_worker_response() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "path": {
                           "id": "pId"
                        }
                    }"#,
            &HeaderMap::new(),
        );

        let expr = expression::from_string("${worker_response.address.street}").unwrap();
        let expected_evaluated_result =
            EvaluationError::InvalidReference(GetError::KeyNotFound("worker".to_string()));
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_pattern_match_optional() {
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
        let worker_response =
            WorkerBridgeResponse::from_worker_response(&WorkerResponse::new(result_as_typed_value, vec![])).unwrap();

        let expr = expression::from_string(
            "${match worker_response { some(value) => 'personal-id', none => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate_worker_bridge_response(&worker_response);
        assert_eq!(
            result,
            Ok(TypeAnnotatedValue::Str("personal-id".to_string()))
        );
    }

    #[test]
    fn test_evaluation_with_pattern_match_none() {
        let worker_response =
            get_worker_response(Value::Null.to_string().as_str()).result_with_worker_response_key();

        let expr = expression::from_string(
            "${match worker_response { some(value) => 'personal-id', none => 'not found' }}",
        )
        .unwrap();

        let result = expr.evaluate(&worker_response);
        assert_eq!(result, Ok(TypeAnnotatedValue::Str("not found".to_string())));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_other_exprs() {
        let uri = Uri::builder()
            .path_and_query("/shopping-cart/foo")
            .build()
            .unwrap();

        let path_pattern = AllPathPatterns::from_str("/shopping-cart/{id}").unwrap();

        let mut resolved_variables_path =
            resolved_variables_from_request_path(uri.clone(), path_pattern.clone());

        let success_response_with_input_variables = resolved_variables_path.merge(
            &get_worker_response(
                r#"
                    {
                        "ok": {
                           "id": "baz"
                        }
                    }"#,
            )
            .result_with_worker_response_key(),
        );

        let expr1 = expression::from_string(
            "${if request.path.id == 'foo' then 'bar' else match worker_response { ok(value) => value.id, err(msg) => 'empty' }}",
        )
            .unwrap();

        let result1 = expr1.evaluate(success_response_with_input_variables);

        // Intentionally bringing an curly brace
        let expr2 = expression::from_string(
            "${if request.path.id == 'bar' then 'foo' else match worker_response { ok(foo) => foo.id, err(msg) => 'empty' }}",

        ).unwrap();

        let result2 = expr2.evaluate(success_response_with_input_variables);

        let error_worker_response = get_worker_response(
            r#"
                    {
                        "err": {
                           "msg": "failed"
                        }
                    }"#,
        );

        let mut new_resolved_variables_from_request_path =
            resolved_variables_from_request_path(uri, path_pattern);

        let error_response_with_request_variables = new_resolved_variables_from_request_path
            .merge(&error_worker_response.result_with_worker_response_key());

        let expr3 = expression::from_string(
            "${if request.path.id == 'bar' then 'foo' else match worker_response { ok(foo) => foo.id, err(msg) => 'empty' }}",

        ).unwrap();

        let result3 = expr3.evaluate(error_response_with_request_variables);

        assert_eq!(
            (result1, result2, result3),
            (
                Ok(TypeAnnotatedValue::Str("bar".to_string())),
                Ok(TypeAnnotatedValue::Str("baz".to_string())),
                Ok(TypeAnnotatedValue::Str("empty".to_string()))
            )
        );
    }

    #[test]
    fn test_evaluation_with_pattern_match() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "pId"
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(value) => 'personal-id', err(msg) => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());
        assert_eq!(
            result,
            Ok(TypeAnnotatedValue::Str("personal-id".to_string()))
        );
    }

    #[test]
    fn test_evaluation_with_pattern_match_use_success_variable() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "pId"
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(value) => value, err(msg) => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());

        let expected_result = TypeAnnotatedValue::Record {
            value: vec![("id".to_string(), TypeAnnotatedValue::Str("pId".to_string()))],
            typ: vec![("id".to_string(), AnalysedType::Str)],
        };

        assert_eq!(result, Ok(expected_result));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_select_field() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "pId"
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(value) => value.id, err(msg) => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());
        assert_eq!(result, Ok(TypeAnnotatedValue::Str("pId".to_string())));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_select_from_array() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(value) => value.ids[0], err(msg) => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());
        assert_eq!(result, Ok(TypeAnnotatedValue::Str("id1".to_string())));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_some_construction() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(value) => some(value.ids[0]), err(msg) => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());
        let expected = TypeAnnotatedValue::Option {
            value: Some(Box::new(TypeAnnotatedValue::Str("id1".to_string()))),
            typ: AnalysedType::Str,
        };
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_none_construction() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(value) => none, none => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());
        let expected = TypeAnnotatedValue::Option {
            value: None,
            typ: AnalysedType::Str,
        };
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_nested_construction() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(value) => some(none), none => none }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());
        let expected = TypeAnnotatedValue::Option {
            value: Some(Box::new(TypeAnnotatedValue::Option {
                typ: AnalysedType::Str,
                value: None,
            })),
            typ: AnalysedType::Option(Box::new(AnalysedType::Str)),
        };
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_ok_construction() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(value) => ok(1), none => err(2) }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());
        let expected = TypeAnnotatedValue::Result {
            value: Ok(Some(Box::new(TypeAnnotatedValue::U64(1)))),
            ok: Some(Box::new(AnalysedType::U64)),
            error: None,
        };
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_err_construction() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "err": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(value) => ok(1), err(msg) => err(2) }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());

        let expected = TypeAnnotatedValue::Result {
            value: Err(Some(Box::new(TypeAnnotatedValue::U64(2)))),
            error: Some(Box::new(AnalysedType::U64)),
            ok: None,
        };
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_wild_card() {
        let worker_response = get_worker_response(
            r#"
                    {
                        "err": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = expression::from_string(
            "${match worker_response { ok(_) => ok(1), err(_) => err(2) }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());

        let expected = TypeAnnotatedValue::Result {
            value: Err(Some(Box::new(TypeAnnotatedValue::U64(2)))),
            error: Some(Box::new(AnalysedType::U64)),
            ok: None,
        };
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_name_alias() {
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

        let expr = expression::from_string(
            "${match worker_response { a @ ok(b @ _) => ok(1), c @ err(d @ ok(e)) => {p : c, q: d, r: e.id} }}",
        )
            .unwrap();
        let result = expr
            .evaluate(&worker_response.result_with_worker_response_key())
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

    #[test]
    fn test_evaluation_with_pattern_match_variant_positive() {
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
            }, vec![]);

        let expr =
            expression::from_string("${match worker_response { Foo(value) => ok(value.id) }}")
                .unwrap();
        let result = expr.evaluate_worker_bridge_response(&worker_response.result_with_worker_response_key());

        let expected = TypeAnnotatedValue::Result {
            value: Ok(Some(Box::new(TypeAnnotatedValue::Str("pId".to_string())))),
            error: None,
            ok: Some(Box::new(AnalysedType::Str)),
        };
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_pattern_match_variant_nested_with_some() {
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

        let worker_bridge_response = WorkerResponse::new(output, vec![]).to_test_worker_bridge_response();

        let expr = expression::from_string(
            "${match worker_response { Foo(some(value)) => value.id, err(msg) => 'not found' }}",
        )
        .unwrap();

        let result = expr.evaluate_worker_bridge_response(&worker_bridge_response);

        let expected = TypeAnnotatedValue::Str("pId".to_string());

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_pattern_match_variant_nested_with_some_result() {
        let output = get_complex_variant_typed_value();

        let worker_bridge_response = WorkerResponse::new(output, vec![]).to_test_worker_bridge_response();

        let expr = expression::from_string(
            "${match worker_response { Foo(some(ok(value))) => value.id, err(msg) => 'not found' }}",
        )
            .unwrap();
        let result = expr.evaluate_worker_bridge_response(&worker_bridge_response);

        let expected = TypeAnnotatedValue::Str("pId".to_string());

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_pattern_match_variant_nested_type_mismatch() {
        let output = get_complex_variant_typed_value();

        let worker_bridge_response = WorkerResponse::new(output, vec![]).to_test_worker_bridge_response();

        let expr = expression::from_string(
            "${match worker_response { Foo(ok(some(value))) => value.id, err(msg) => 'not found' }}",
        )
            .unwrap();
        let result = expr.evaluate(&worker_bridge_response.result_with_worker_response_key());

        assert!(result
            .err()
            .unwrap()
            .to_string()
            .starts_with("Type mismatch"))
    }

    #[test]
    fn test_evaluation_with_pattern_match_variant_nested_with_none() {
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

        let expr = expression::from_string(
            "${match worker_response { Foo(none) => 'not found',  Foo(some(value)) => value.id }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());

        let expected = TypeAnnotatedValue::Str("not found".to_string());

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_evaluation_with_wave_like_syntax_ok_record() {
        let expr = expression::from_string("${{a : ok(1)}}").unwrap();

        let result = expr.evaluate(&TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        });

        let expected = Ok(TypeAnnotatedValue::Record {
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
        });

        assert_eq!(result, expected);
    }

    #[test]
    fn test_evaluation_with_wave_like_syntax_err_record() {
        let expr = expression::from_string("${{a : err(1)}}").unwrap();

        let result = expr.evaluate(&TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        });

        let expected = Ok(TypeAnnotatedValue::Record {
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
        });

        assert_eq!(result, expected);
    }

    #[test]
    fn test_evaluation_with_wave_like_syntax_simple_list() {
        let expr = expression::from_string("${[1,2,3]}").unwrap();

        let result = expr.evaluate(&TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        });

        let expected = Ok(TypeAnnotatedValue::List {
            typ: AnalysedType::U64,
            values: vec![
                TypeAnnotatedValue::U64(1),
                TypeAnnotatedValue::U64(2),
                TypeAnnotatedValue::U64(3),
            ],
        });

        assert_eq!(result, expected);
    }

    #[test]
    fn test_evaluation_with_wave_like_syntax_simple_tuple() {
        let expr = expression::from_string("${(some(1),2,3)}").unwrap();

        let result = expr.evaluate(&TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        });

        let expected = Ok(TypeAnnotatedValue::Tuple {
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
        });

        assert_eq!(result, expected);
    }

    #[test]
    fn test_evaluation_wave_like_syntax_flag() {
        let expr = expression::from_string("${{A, B, C}}").unwrap();

        let result = expr.evaluate(&TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        });

        let expected = Ok(TypeAnnotatedValue::Flags {
            typ: vec!["A".to_string(), "B".to_string(), "C".to_string()],
            values: vec!["A".to_string(), "B".to_string(), "C".to_string()],
        });

        assert_eq!(result, expected);
    }

    #[test]
    fn test_evaluation_with_wave_like_syntax_result_list() {
        let expr = expression::from_string("${[ok(1),ok(2)]}").unwrap();

        let result = expr.evaluate(&TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        });

        let expected = Ok(TypeAnnotatedValue::List {
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
        });

        assert_eq!(result, expected);
    }

    #[test]
    fn test_evaluation_with_multiple_lines() {
        let program = r"
            let x = { a : 1 };
            let y = { b : 2 };
            let z = x.a > y.b;
            z
          ";

        let expr = expression::from_string(format!("${{{}}}", program)).unwrap();

        // We don't need any information any request to evaluate the above program
        let empty_input = &TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        };

        let result = expr.evaluate(empty_input);

        let expected = Ok(TypeAnnotatedValue::Bool(false));

        assert_eq!(result, expected);
    }

    mod test_utils {
        use crate::api_definition::http::{AllPathPatterns, PathPattern};
        use crate::evaluator::Evaluator;
        use crate::expression;
        use crate::http::router::RouterPattern;
        use crate::http::{ApiInputPath, InputHttpRequest};
        use crate::worker_bridge_execution::WorkerResponse;
        use golem_service_base::type_inference::infer_analysed_type;
        use golem_wasm_ast::analysis::AnalysedType;
        use golem_wasm_rpc::json::get_typed_value_from_json;
        use golem_wasm_rpc::TypeAnnotatedValue;
        use http::{HeaderMap, Method, Uri};
        use serde_json::{json, Value};
        use std::collections::HashMap;
        use crate::evaluator::tests::{EvaluatorTestExt, WorkerBridgeExt};

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
        ) -> TypeAnnotatedValue {
            let request_body: Value = serde_json::from_str(input).expect("Failed to parse json");

            let input_http_request = InputHttpRequest {
                req_body: request_body.clone(),
                headers: header_map.clone(),
                req_method: Method::GET,
                input_path: ApiInputPath {
                    base_path: "/api".to_string(),
                    query_path: None,
                },
            };

            input_http_request
                .get_type_annotated_value(HashMap::new(), &[])
                .unwrap()
        }

        pub(crate) fn resolved_variables_from_request_path(
            uri: Uri,
            path_pattern: AllPathPatterns,
        ) -> TypeAnnotatedValue {
            let input_http_request = InputHttpRequest {
                req_body: serde_json::Value::Null,
                headers: HeaderMap::new(),
                req_method: Method::GET,
                input_path: ApiInputPath {
                    base_path: uri.path().to_string(),
                    query_path: uri.query().map(|x| x.to_string()),
                },
            };

            let base_path: Vec<&str> = RouterPattern::split(uri.path()).collect();

            let path_params = path_pattern
                .path_patterns
                .into_iter()
                .enumerate()
                .filter_map(|(index, pattern)| match pattern {
                    PathPattern::Literal(_) => None,
                    PathPattern::Var(var) => Some((var, base_path[index])),
                })
                .collect();

            input_http_request
                .get_type_annotated_value(path_params, &path_pattern.query_params)
                .unwrap()
        }

        #[test]
        fn expr_to_string_round_trip_match_expr_err() {
            let worker_response = get_err_worker_response();

            let expr1_string = "${match worker_response { ok(x) => 'foo', err(msg) => 'error' }}";
            let expr1 = expression::from_string(expr1_string).unwrap();
            let value1 = expr1
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expr2_string = expr1.to_string();
            let expr2 = expression::from_string(expr2_string.as_str()).unwrap();
            let value2 = expr2
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expected = TypeAnnotatedValue::Str("error".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }

        #[test]
        fn expr_to_string_round_trip_match_expr_append() {
            let worker_response = get_err_worker_response().to_test_worker_bridge_response();

            let expr1_string =
                "append-${match worker_response { ok(x) => 'foo', err(msg) => 'error' }}";
            let expr1 = expression::from_string(expr1_string).unwrap();
            let value1 = expr1
                .evaluate_worker_bridge_response(&worker_response)
                .unwrap();

            let expr2_string = expr1.to_string();
            let expr2 = expression::from_string(expr2_string.as_str()).unwrap();
            let value2 = expr2
                .evaluate_worker_bridge_response(&worker_response)
                .unwrap();

            let expected = TypeAnnotatedValue::Str("append-error".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }

        #[test]
        fn expr_to_string_round_trip_match_expr_append_suffix() {
            let worker_response = get_err_worker_response();

            let expr1_string =
                "prefix-${match worker_response { ok(x) => 'foo', err(msg) => 'error' }}-suffix";
            let expr1 = expression::from_string(expr1_string).unwrap();
            let value1 = expr1
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expr2_string = expr1.to_string();
            let expr2 = expression::from_string(expr2_string.as_str()).unwrap();
            let value2 = expr2
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expected = TypeAnnotatedValue::Str("prefix-error-suffix".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }
    }
}

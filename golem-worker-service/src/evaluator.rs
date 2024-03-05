use std::fmt::Display;

use serde_json::Value;

use super::tokeniser::tokenizer::{Token, Tokenizer};
use crate::expr::{ConstructorPattern, Expr};
use crate::resolved_variables::{Path, ResolvedVariables};
use crate::value_typed::ValueTyped;

pub trait Evaluator<T> {
    fn evaluate(&self, resolved_variables: &ResolvedVariables) -> Result<T, EvaluationError>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum EvaluationError {
    Message(String),
}

impl Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvaluationError::Message(string) => write!(f, "{}", string),
        }
    }
}

pub struct Primitive<'t> {
    pub input: &'t str,
}

// When we expect only primitives within a string, and uses ${} not as an expr,
// but as a mere place holder. This type disallows complex structures to end up
// in values such as function-name.
impl<'t> Primitive<'t> {
    pub fn new(str: &'t str) -> Primitive<'t> {
        Primitive { input: str }
    }
}

// Foo/{user-id}
impl<'t> Evaluator<String> for Primitive<'t> {
    fn evaluate(&self, place_holder_values: &ResolvedVariables) -> Result<String, EvaluationError> {
        let mut combined_string = String::new();
        let result: crate::tokeniser::tokenizer::TokeniserResult = Tokenizer::new(self.input).run();

        let mut cursor = result.to_cursor();

        while let Some(token) = cursor.next_token() {
            match token {
                Token::InterpolationStart => {
                    let place_holder_name = cursor
                        .capture_string_until(vec![&Token::InterpolationStart], &Token::CloseParen);

                    if let Some(place_holder_name) = place_holder_name {
                        match place_holder_values.get_key(place_holder_name.as_str()) {
                            Some(place_holder_value) => match place_holder_value {
                                Value::Bool(bool) => {
                                    combined_string.push_str(bool.to_string().as_str())
                                }
                                Value::Number(number) => {
                                    combined_string.push_str(number.to_string().as_str())
                                }
                                Value::String(string) => {
                                    combined_string.push_str(string.to_string().as_str())
                                }

                                _ => {
                                    return Result::Err(EvaluationError::Message(format!(
                                        "Unsupported json type to be replaced in place holder. Make sure the values are primitive {}",
                                        place_holder_name,
                                    )));
                                }
                            },

                            None => {
                                return Result::Err(EvaluationError::Message(format!(
                                    "No value for the place holder {}",
                                    place_holder_name,
                                )));
                            }
                        }
                    }
                }
                token => combined_string.push_str(token.to_string().as_str()),
            }
        }

        Ok(combined_string)
    }
}

impl Evaluator<Value> for Expr {
    fn evaluate(&self, resolved_variables: &ResolvedVariables) -> Result<Value, EvaluationError> {
        let expr: &Expr = self;

        // An expression evaluation needs to be careful with string values
        // and therefore returns ValueTyped
        fn go(
            expr: &Expr,
            resolved_variables: &ResolvedVariables,
        ) -> Result<ValueTyped, EvaluationError> {
            match expr.clone() {
                Expr::Request() => {
                    match resolved_variables.get_path(&Path::from_string_unsafe(
                        Token::Request.to_string().as_str(),
                    )) {
                        Some(v) => Ok(ValueTyped::from_json(&v)),
                        None => Err(EvaluationError::Message(
                            "Details of request is missing".to_string(),
                        )),
                    }
                }
                Expr::WorkerResponse() => {
                    match resolved_variables.get_path(&Path::from_string_unsafe(
                        Token::WorkerResponse.to_string().as_str(),
                    )) {
                        Some(v) => Ok(ValueTyped::from_json(&v)),
                        None => Err(EvaluationError::Message(
                            "Details of worker response is missing".to_string(),
                        )),
                    }
                }

                Expr::SelectIndex(expr, index) => {
                    let evaluation_result = go(&expr, resolved_variables)?;

                    evaluation_result
                        .as_array()
                        .ok_or(EvaluationError::Message(format!(
                            "Result is not an array to get the index {}",
                            index
                        )))?
                        .get(index)
                        .map(ValueTyped::from_json)
                        .ok_or(EvaluationError::Message(format!(
                            "The array doesn't contain {} elements",
                            index
                        )))
                }

                Expr::SelectField(expr, field_name) => {
                    let evaluation_result = go(&expr, resolved_variables)?;

                    evaluation_result
                        .as_object()
                        .ok_or(EvaluationError::Message(format!(
                            "Result is not an object to get the field {}",
                            field_name
                        )))?
                        .get(&field_name)
                        .map(ValueTyped::from_json)
                        .ok_or(EvaluationError::Message(format!(
                            "The result doesn't contain the field {}",
                            field_name
                        )))
                }

                Expr::EqualTo(left, right) => {
                    let left = go(&left, resolved_variables)?;
                    let right = go(&right, resolved_variables)?;

                    let result = left
                        .equal_to(right)
                        .map_err(|err| EvaluationError::Message(err.to_string()))?;

                    Ok(ValueTyped::Boolean(result))
                }
                Expr::GreaterThan(left, right) => {
                    let left = go(&left, resolved_variables)?;
                    let right = go(&right, resolved_variables)?;

                    let result = left
                        .greater_than(right)
                        .map_err(|err| EvaluationError::Message(err.to_string()))?;

                    Ok(ValueTyped::Boolean(result))
                }
                Expr::GreaterThanOrEqualTo(left, right) => {
                    let left = go(&left, resolved_variables)?;
                    let right = go(&right, resolved_variables)?;

                    let result = left
                        .greater_than_or_equal_to(right)
                        .map_err(|err| EvaluationError::Message(err.to_string()))?;

                    Ok(ValueTyped::Boolean(result))
                }
                Expr::LessThan(left, right) => {
                    let left = go(&left, resolved_variables)?;
                    let right = go(&right, resolved_variables)?;
                    let result = left
                        .less_than(right)
                        .map_err(|err| EvaluationError::Message(err.to_string()))?;

                    Ok(ValueTyped::Boolean(result))
                }
                Expr::LessThanOrEqualTo(left, right) => {
                    let left = go(&left, resolved_variables)?;
                    let right = go(&right, resolved_variables)?;
                    let result = left
                        .less_than_or_equal_to(right)
                        .map_err(|err| EvaluationError::Message(err.to_string()))?;

                    Ok(ValueTyped::Boolean(result))
                }
                Expr::Not(expr) => {
                    let evaluated_expr = expr.evaluate(resolved_variables)?;

                    let bool = evaluated_expr.as_bool().ok_or(EvaluationError::Message(format!(
                        "The expression is evaluated to {} but it is not a boolean expression to apply not (!) operator on",
                        evaluated_expr
                    )))?;

                    Ok(ValueTyped::Boolean(!bool))
                }

                Expr::Cond(pred0, left, right) => {
                    let pred = go(&pred0, resolved_variables)?;
                    let left = go(&left, resolved_variables)?;
                    let right = go(&right, resolved_variables)?;

                    let bool: bool = pred.as_bool().ok_or(EvaluationError::Message(format!(
                        "The predicate expression is evaluated to {}, but it is not a boolean expression",
                        pred
                    )))?;

                    if bool {
                        Ok(left)
                    } else {
                        Ok(right)
                    }
                }

                Expr::Sequence(exprs) => {
                    let mut result: Vec<Value> = vec![];

                    for expr in exprs {
                        match go(&expr, resolved_variables) {
                            Ok(value) => result.push(value.to_json()),
                            Err(result) => return Err(result),
                        }
                    }

                    Ok(ValueTyped::ComplexJson(Value::Array(result)))
                }

                Expr::Record(tuples) => {
                    let mut map: serde_json::Map<String, Value> = serde_json::Map::new();

                    for (key, expr) in tuples {
                        match go(&expr, resolved_variables) {
                            Ok(value) => {
                                map.insert(key, value.to_json());
                            }

                            Err(result) => return Err(result),
                        }
                    }

                    Ok(ValueTyped::ComplexJson(Value::Object(map)))
                }

                Expr::Concat(exprs) => {
                    let mut result = String::new();

                    for expr in exprs {
                        match go(&expr, resolved_variables) {
                            Ok(value) => {
                                if let Some(primitive) = value.get_primitive_string() {
                                    result.push_str(primitive.as_str())
                                } else {
                                    return Err(EvaluationError::Message(format!("Cannot append a complex expression {} to form strings. Please check the expression", value)));
                                }
                            }

                            Err(result) => return Err(result),
                        }
                    }

                    Ok(ValueTyped::String(result))
                }

                Expr::Literal(literal) => Ok(ValueTyped::from_string(literal.as_str())),

                Expr::Number(number) => Ok(ValueTyped::from_number_expr(number)),

                Expr::PathVar(path_var) => resolved_variables
                    .get_key(path_var.as_str())
                    .map(ValueTyped::from_json)
                    .ok_or(EvaluationError::Message(format!(
                        "The result doesn't contain the field {}",
                        path_var
                    ))),
                Expr::Variable(variable) => resolved_variables
                    .get_key(variable.as_str())
                    .map(ValueTyped::from_json)
                    .ok_or(EvaluationError::Message(format!(
                        "The result doesn't contain the field {}",
                        variable
                    ))),
                Expr::Boolean(bool) => Ok(ValueTyped::Boolean(bool)),
                Expr::PatternMatch(input_expr, constructors) => {
                    // if `match worker.response`, then match_evaluated is the value of worker.response
                    let match_evaluated = go(&input_expr, resolved_variables)?.to_json();
                    let mut resolved_result: Option<Value> = None;

                    for constructor in constructors {
                        // if `match worker.response { some(value) =>...}`, then condition pattern is some(value)
                        let (condition_pattern, possible_resolution) = constructor.0;

                        match condition_pattern {
                            //  if `match worker.response { some(value) => value}`, then condition key is some
                            // and patterns is vec![ConstructorPattern::Literal(Expr::Variable(value))] (as an example)
                            ConstructorPattern::Constructor(condition_key, patterns) => {
                                if patterns.len() > 1 {
                                    return Err(EvaluationError::Message(
                                        "Pattern matching is currently supported only for single pattern in constructor. i.e, {}(person), {}, {}(person_info) etc and not {}(age, birth_date)".to_string(),
                                    ));
                                } else if patterns.is_empty() {
                                    if match_evaluated == Value::String(condition_key.clone()) {
                                        let result =
                                            go(&possible_resolution, resolved_variables)?.to_json();
                                        resolved_result = Some(result);
                                        break;
                                    } else if condition_key == Token::None.to_string() {
                                        if match_evaluated == Value::Null {
                                            let result =
                                                go(&possible_resolution, resolved_variables)?
                                                    .to_json();
                                            resolved_result = Some(result);
                                            break;
                                        }
                                    }
                                } else {
                                    let result = if condition_key == Token::Some.to_string() {
                                        if match_evaluated != Value::Null {
                                            Some(&match_evaluated)
                                        } else {
                                            None
                                        }
                                    } else {
                                        match_evaluated.get(condition_key)
                                    };

                                    match result {
                                        Some(value) => {
                                            let first_pattern = &patterns[0];

                                            match first_pattern {
                                                ConstructorPattern::Literal(expr) => {
                                                    match *expr.clone() {
                                                        Expr::Variable(variable) => {
                                                            let mut resolved_variables =
                                                                resolved_variables.clone();
                                                            resolved_variables.insert(
                                                                Path::from_raw_string(
                                                                    variable.as_str(),
                                                                ),
                                                                value.clone(),
                                                            );
                                                            let value = go(
                                                                &possible_resolution,
                                                                &resolved_variables,
                                                            )?
                                                            .to_json();
                                                            resolved_result = Some(value);
                                                            break;
                                                        }
                                                        _ => {
                                                            return Err(EvaluationError::Message(
                                                                "Currently only variable pattern is supported. i.e, some(value), err(message) etc".to_string(),
                                                            ));
                                                        }
                                                    }
                                                }
                                                _ => {
                                                    return Err(EvaluationError::Message(
                                                        "Currently only variable pattern is supported. i.e, some(value), err(message) etc".to_string(),
                                                    ));
                                                }
                                            }
                                        }
                                        None => {}
                                    }
                                }
                            }
                            _ => {
                                return Err(EvaluationError::Message(
                                    "Currently only constructor pattern is supported".to_string(),
                                ));
                            }
                        }
                    }

                    resolved_result
                        .map(|value| ValueTyped::from_json(&value))
                        .ok_or(EvaluationError::Message(
                            "Pattern matching failed".to_string(),
                        ))
                }
                Expr::Constructor0(_) => Err(EvaluationError::Message(
                    "Constructor0 is not supported yet".to_string(),
                )),
            }
        }

        go(expr, resolved_variables).map(|v| v.to_json())
    }
}

#[cfg(test)]
mod tests {
    use crate::evaluator::{EvaluationError, Evaluator};
    use crate::expr::Expr;
    use crate::resolved_variables::ResolvedVariables;
    use http::HeaderMap;
    use serde_json::Value;
    use std::collections::HashMap;

    fn resolved_variables_from_worker_response(input: &str) -> ResolvedVariables {
        let worker_response: Value = serde_json::from_str(input).expect("Failed to parse json");

        ResolvedVariables::from_worker_response(&worker_response)
    }

    fn resolved_variables_from_request_body(input: &str) -> ResolvedVariables {
        let request_body: Value = serde_json::from_str(input).expect("Failed to parse json");

        ResolvedVariables::from_http_request(
            &request_body,
            &HeaderMap::new(),
            HashMap::new(),
            vec![],
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap()
    }

    fn resolved_variables_from_request_path(
        json_str: &str,
        path_values: &HashMap<usize, String>,
        spec_variables: &HashMap<usize, String>,
    ) -> ResolvedVariables {
        let request_body: Value = serde_json::from_str(json_str).expect("Failed to parse json");

        ResolvedVariables::from_http_request(
            &request_body,
            &HeaderMap::new(),
            HashMap::new(),
            vec![],
            path_values,
            spec_variables,
        )
        .unwrap()
    }

    #[test]
    fn test_evaluation_with_request_path() {
        let mut request_path_values = HashMap::new();
        request_path_values.insert(0, "pId".to_string());

        let mut spec_path_variables = HashMap::new();
        spec_path_variables.insert(0, "id".to_string());

        let resolved_variables = resolved_variables_from_request_path(
            r#"
                    {
                        "body": {
                           "id": "bId",
                        },
                    }"#,
            &request_path_values,
            &spec_path_variables,
        );

        let expr = Expr::from_primitive_string("${request.path.id}").unwrap();
        let expected_evaluated_result = Value::String("pID".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_id() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "body": {
                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string("${request.body.id}").unwrap();
        let expected_evaluated_result = Value::String("bId".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_select_index() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "path": {
                           "id": "pId"
                        },
                        "body": {
                           "id": "bId",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string("${request.body.titles[0]}").unwrap();
        let expected_evaluated_result = Value::String("bTitle1".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_select_from_object() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "path": {
                           "id": "pId"
                        },
                        "body": {
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }
                        },
                        "headers": {
                           "authorisation": "admin",
                           "content-type": "application/json"
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string(
            "${request.body.address.street} ${request.body.address.city}",
        )
        .unwrap();
        let expected_evaluated_result = Value::String("bStreet bCity".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_if_condition() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "path": {
                           "id": "pId"
                        },
                        "body": {
                           "id": "bId"
                        },
                        "headers": {
                           "authorisation": "admin",
                           "content-type": "application/json"
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string(
            "${if (request.headers.authorisation == 'admin') then 200 else 401}",
        )
        .unwrap();
        let expected_evaluated_result = Value::Number("200".parse().unwrap());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_select_unknown_field() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "body": {
                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string("${request.body.address.street2}").unwrap();
        let expected_evaluated_result =
            EvaluationError::Message("The result doesn't contain the field street2".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_select_invalid_index() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "path": {
                           "id": "pId"
                        },
                        "body": {
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string("${request.body.titles[4]}").unwrap();
        let expected_evaluated_result =
            EvaluationError::Message("The array doesn't contain 4 elements".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_8() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "path": {
                           "id": "pId"
                        },
                        "body": {
                           "id": "bId",
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string("${request.body.address[4]}").unwrap();
        let expected_evaluated_result =
            EvaluationError::Message("Result is not an array to get the index 4".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_invalid_type_comparison() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {
                        "body": {
                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ]
                        },
                        "headers": {
                           "authorisation": "admin",
                           "content-type": "application/json"
                        }
                    }"#,
        );

        let expr =
            Expr::from_primitive_string("${if (request.headers.authorisation) then 200 else 401}")
                .unwrap();
        let expected_evaluated_result = EvaluationError::Message(
            "The predicate expression is evaluated to admin, but it is not a boolean expression"
                .to_string(),
        );
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_request_body_invalid_object_reference() {
        let resolved_variables = resolved_variables_from_request_body(
            r#"
                    {

                        "body": {
                           "id": "bId",
                           "name": "bName",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ],
                           "address": {
                             "street": "bStreet",
                             "city": "bCity"
                           }
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string("${request.body.address.street.name}").unwrap();
        let expected_evaluated_result =
            EvaluationError::Message("Result is not an object to get the field name".to_string());
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
        );

        let expr = Expr::from_primitive_string("${worker.response.address.street}").unwrap();
        let expected_evaluated_result =
            EvaluationError::Message("Details of worker response is missing".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_pattern_match() {
        let resolved_variables = resolved_variables_from_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "pId"
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string(
            "${match worker.response { ok(value) => 'personal-id', err(msg) => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(Value::String("personal-id".to_string())));
    }

    #[test]
    fn test_evaluation_with_pattern_match_use_success_variable() {
        let resolved_variables = resolved_variables_from_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "pId"
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string(
            "${match worker.response { ok(value) => value, err => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&resolved_variables);
        let expected_result =
            serde_json::Map::from_iter(vec![("id".to_string(), Value::String("pId".to_string()))]);
        assert_eq!(result, Ok(Value::Object(expected_result)));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_select_field() {
        let resolved_variables = resolved_variables_from_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "pId"
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string(
            "${match worker.response { ok(value) => value.id, err => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(Value::String("pId".to_string())));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_select_from_array() {
        let resolved_variables = resolved_variables_from_worker_response(
            r#"
                    {
                        "ok": {
                           "ids": ["id1", "id2"]
                        }
                    }"#,
        );

        let expr = Expr::from_primitive_string(
            "${match worker.response { ok(value) => value.ids[0], none => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(Value::String("id1".to_string())));
    }
}

use golem_wasm_rpc::TypeAnnotatedValue;
use std::fmt::Display;
use std::ops::Deref;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::JsonFunctionResult;
use super::tokeniser::tokenizer::{Token, Tokenizer};
use crate::expr::{ConstructorPattern, ConstructorTypeName, Expr, InBuiltConstructorInner, InnerNumber};
use crate::path::{Path};
use crate::getter::Getter;
use crate::primitive::GetPrimitive;
use crate::merge::Merge;


pub trait Evaluator {
    fn evaluate(
        &self,
        input: &TypeAnnotatedValue,
    ) -> Result<TypeAnnotatedValue, EvaluationError>;
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
    fn evaluate(&self, input: &TypeAnnotatedValue) -> Result<TypeAnnotatedValue, EvaluationError> {
        let mut combined_string = String::new();
        let result: crate::tokeniser::tokenizer::TokeniserResult = Tokenizer::new(self.input).run();

        let mut cursor = result.to_cursor();

        while let Some(token) = cursor.next_token() {
            match token {
                Token::InterpolationStart => {
                    let place_holder_name = cursor
                        .capture_string_until(vec![&Token::InterpolationStart], &Token::CloseParen);

                    if let Some(place_holder_name) = place_holder_name {
                        match input.get(&Path::from_key(place_holder_name.as_str())) {
                            Some(type_annotated_value) => {
                                match type_annotated_value.get_primitive() {
                                    Some(primitive) => {
                                        combined_string.push_str(primitive.to_string().as_str())
                                    }

                                    None => {
                                        return Result::Err(EvaluationError::Message(format!(
                                            "Unsupported json type to be replaced in place holder. Make sure the values are primitive {}",
                                            place_holder_name,
                                        )));
                                    }
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

        Ok(TypeAnnotatedValue::Str(combined_string))
    }
}

impl Evaluator for Expr {
    fn evaluate(&self, input: &TypeAnnotatedValue) -> Result<TypeAnnotatedValue, EvaluationError> {
        let expr: &Expr = self;

        // An expression evaluation needs to be careful with string values
        // and therefore returns ValueTyped
        fn go(
            expr: &Expr,
            input: &TypeAnnotatedValue,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            match expr.clone() {
                Expr::Request() => {
                    match input.get(&Path::from_raw_string(
                        Token::Request.to_string().as_str(),
                    )) {
                        Some(v) => Ok(v),
                        None => Err(EvaluationError::Message(
                            "Details of request is missing".to_string(),
                        )),
                    }
                }
                Expr::WorkerResponse() => {
                    match input.get(&Path::from_raw_string(
                        Token::WorkerResponse.to_string().as_str(),
                    )) {
                        Some(v) => Ok(v),
                        None => Err(EvaluationError::Message(
                            "Details of worker response is missing".to_string(),
                        )),
                    }
                }

                Expr::SelectIndex(expr, index) => {
                    let evaluation_result = go(&expr, input)?;

                    evaluation_result.get(&Path::from_index(index))
                        .ok_or(EvaluationError::Message(format!(
                            "Unable to fetch the element at index {}",
                            index
                        )))
                }

                Expr::SelectField(expr, field_name) => {
                    let evaluation_result = go(&expr, input)?;

                    evaluation_result
                        .get(&Path::from_key(field_name.as_str()))
                        .ok_or(EvaluationError::Message(format!(
                            "Unable to obtaint the field {}",
                            field_name
                        )))
                }

                Expr::EqualTo(left, right) => {
                    let left = go(&left, input)?;
                    let right = go(&right, input)?;

                    match (left.get_primitive(), right.get_primitive()) {
                        (Some(left), Some(right)) => {
                            let result = left == right;
                            Ok(TypeAnnotatedValue::Bool(result))
                        }
                        _ => Err(EvaluationError::Message(
                            "Unsupported json type to compare".to_string(),
                        )),
                    }
                }
                Expr::GreaterThan(left, right) => {
                    let left = go(&left, input)?;
                    let right = go(&right, input)?;

                    match (left.get_primitive(), right.get_primitive()) {
                        (Some(left), Some(right)) => {
                            let result = left > right;
                            Ok(TypeAnnotatedValue::Bool(result))
                        }
                        _ => Err(EvaluationError::Message(
                            "Unsupported json type to compare".to_string(),
                        )),
                    }
                }
                Expr::GreaterThanOrEqualTo(left, right) => {
                    let left = go(&left, input)?;
                    let right = go(&right, input)?;

                    match (left.get_primitive(), right.get_primitive()) {
                        (Some(left), Some(right)) => {
                            let result = left >= right;
                            Ok(TypeAnnotatedValue::Bool(result))
                        }
                        _ => Err(EvaluationError::Message(
                            "Unsupported json type to compare".to_string(),
                        )),
                    }
                }
                Expr::LessThan(left, right) => {
                    let left = go(&left, input)?;
                    let right = go(&right, input)?;

                    match (left.get_primitive(), right.get_primitive()) {
                        (Some(left), Some(right)) => {
                            let result = left < right;
                            Ok(TypeAnnotatedValue::Bool(result))
                        }
                        _ => Err(EvaluationError::Message(
                            "Unsupported json type to compare".to_string(),
                        )),
                    }
                }
                Expr::LessThanOrEqualTo(left, right) => {
                    let left = go(&left, input)?;
                    let right = go(&right, input)?;

                    match (left.get_primitive(), right.get_primitive()) {
                        (Some(left), Some(right)) => {
                            let result = left <= right;
                            Ok(TypeAnnotatedValue::Bool(result))
                        }
                        _ => Err(EvaluationError::Message(
                            "Unsupported json type to compare".to_string(),
                        )),
                    }
                }

                Expr::Not(expr) => {
                    let evaluated_expr = expr.evaluate(input)?;

                    match evaluated_expr {
                        TypeAnnotatedValue::Bool(value) => Ok(TypeAnnotatedValue::Bool(!value)),
                        _ => Err(EvaluationError::Message(format!(
                            "The expression is evaluated to {} but it is not a boolean expression to apply not (!) operator on",
                            JsonFunctionResult::from(evaluated_expr).0
                        ))),
                    }
                }

                Expr::Cond(pred0, left, right) => {
                    let pred = go(&pred0, input)?;
                    let left = go(&left, input)?;
                    let right = go(&right, input)?;

                    match pred {
                        TypeAnnotatedValue::Bool(value) => {
                            if value {
                                Ok(left)
                            } else {
                                Ok(right)
                            }
                        }
                        _ => Err(EvaluationError::Message(format!(
                            "The predicate expression is evaluated to {} but it is not a boolean expression",
                            JsonFunctionResult::from(pred).0
                        ))),
                    }
                }

                Expr::Sequence(exprs) => {
                    let mut result: Vec<TypeAnnotatedValue> = vec![];

                    for expr in exprs {
                        match go(&expr, input) {
                            Ok(value) => {
                                result.push(value)
                            },
                            Err(result) => return Err(result),
                        }
                    }
                    match result.get(0) {
                        Some(value) =>
                            Ok(TypeAnnotatedValue::List { values: result.clone(), typ: AnalysedType::from(value.clone()) }),
                        None =>
                            Ok(TypeAnnotatedValue::List { values: result.clone(), typ: AnalysedType::Tuple(vec![]) }), // Support optional type in List
                    }
                }

                Expr::Record(tuples) => {
                    let mut values: Vec<(String, TypeAnnotatedValue)> = vec![];

                    for (key, expr) in tuples {
                        match go(&expr, input) {
                            Ok(value) => {
                                values.push((key, value));
                            }

                            Err(result) => return Err(result),
                        }
                    }

                    let types: Vec<(String, AnalysedType)> =
                        values.iter().map(|(key, value)| (key.clone(), AnalysedType::from(value.clone()))).collect();

                    Ok(TypeAnnotatedValue::Record { value: values, typ: types })
                }

                Expr::Concat(exprs) => {
                    let mut result = String::new();

                    for expr in exprs {
                        match go(&expr, input) {
                            Ok(value) => {
                                if let Some(primitive) = value.get_primitive() {
                                    result.push_str(primitive.to_string().as_str())
                                } else {
                                    return Err(EvaluationError::Message(format!("Cannot append a complex expression {} to form strings. Please check the expression", JsonFunctionResult::from(value).0)));
                                }
                            }

                            Err(result) => return Err(result),
                        }
                    }

                    Ok(TypeAnnotatedValue::Str(result))
                }

                Expr::Literal(literal) => Ok(TypeAnnotatedValue::Str(literal)),

                Expr::Number(number) => match number {
                    InnerNumber::UnsignedInteger(u64) => Ok(TypeAnnotatedValue::U64(u64)),
                    InnerNumber::Integer(i64) => Ok(TypeAnnotatedValue::S64(i64)),
                    InnerNumber::Float(f64) => Ok(TypeAnnotatedValue::F64(f64)),
                }

                Expr::PathVar(path_var) => input
                    .get(&Path::from_key(path_var.as_str()))
                    .ok_or(EvaluationError::Message(format!(
                        "The result doesn't contain the field {}",
                        path_var
                    ))),

                Expr::Variable(variable) => {
                    input.get(&Path::from_raw_string(variable.as_str()))
                        .ok_or(EvaluationError::Message(format!(
                            "The result doesn't contain the field {}",
                            variable
                        )))
                },

                Expr::Boolean(bool) => Ok(TypeAnnotatedValue::Bool(bool)),
                Expr::PatternMatch(input_expr, constructors) => {

                    let constructors: &Vec<(ConstructorPattern, Expr)> =
                        &constructors.iter().map(|(constructor)| (constructor.0.0.clone(), *constructor.0.1.clone())).collect();

                    handle_pattern_match(&input_expr, constructors, input)
                }
                // TODO; Constructing Expr is not done yet
                Expr::Constructor0(_) => Err(EvaluationError::Message(
                    "Constructor0 is not supported yet".to_string(),
                )),
            }
        }

        go(expr, input)
    }
}

fn handle_pattern_match(
    input_expr: &Expr,
    constructors: &Vec<(ConstructorPattern, Expr)>,
    input: &TypeAnnotatedValue,
) -> Result<TypeAnnotatedValue, EvaluationError> {
    let match_evaluated = input_expr.evaluate(input)?;

    let mut resolved_result: Option<TypeAnnotatedValue> = None;

    for constructor in constructors {
        let (condition_pattern, possible_resolution) = constructor;

        match condition_pattern {
            ConstructorPattern::Constructor(condition_key, patterns) => {
                if patterns.clone().len() > 1 {
                    return Err(EvaluationError::Message(
                        "Pattern matching is currently supported only for single pattern in constructor. i.e, {}(person), {}, {}(person_info) etc and not {}(age, birth_date)".to_string(),
                    ));
                } else {
                    let pattern_expr_variable = match &patterns[0] {
                        ConstructorPattern::Literal(expr) => {
                            match *expr.clone() {
                                Expr::Variable(variable) => variable,
                                _ => {
                                    return Err(EvaluationError::Message(
                                        "Currently only variable pattern is supported. i.e, some(value), ok(value), err(message) etc".to_string(),
                                    ));
                                }
                            }
                        },
                        _ => {
                            return Err(EvaluationError::Message(
                                "Currently only variable pattern is supported. i.e, some(value), ok(value), err(message) etc".to_string(),
                            ));
                        }
                    };
                    match condition_key {
                        ConstructorTypeName::InBuiltConstructor(constructor_type) => match constructor_type {
                            InBuiltConstructorInner::Some => {
                                match &match_evaluated {
                                    TypeAnnotatedValue::Option { value, .. } => {
                                        match value {
                                            Some(v) => {
                                                let result = possible_resolution.evaluate(&input.merge(
                                                    &TypeAnnotatedValue::Record {
                                                        value: vec![(pattern_expr_variable.to_string(), *v.clone())],
                                                        typ: vec![(pattern_expr_variable.to_string(), AnalysedType::from(*v.clone()))]
                                                    }
                                                ))?;

                                                resolved_result = Some(result);
                                            }

                                            None => {}
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            InBuiltConstructorInner::None => {
                                match &match_evaluated {
                                    TypeAnnotatedValue::Option { value, .. } => {
                                        match value {
                                            Some(_) => {}
                                            None => {
                                                let result = possible_resolution.evaluate(input)?;

                                                resolved_result = Some(result);
                                                break;
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            InBuiltConstructorInner::Ok => {
                                match &match_evaluated {
                                    TypeAnnotatedValue::Result { value, .. } => {
                                        match value {
                                            Ok(v) => {
                                                let result = possible_resolution.evaluate(&input.merge(
                                                    &TypeAnnotatedValue::Record {
                                                        value: vec![(pattern_expr_variable.to_string(), *v.clone().unwrap())],
                                                        typ: vec![(pattern_expr_variable.to_string(), AnalysedType::from(*v.clone().unwrap()))]
                                                    }
                                                ))?;

                                                resolved_result = Some(result);
                                                break;
                                            }

                                            Err(_) => {}
                                        }
                                    }
                                    _ => {}
                                }
                            },
                            InBuiltConstructorInner::Err => {
                                match &match_evaluated {
                                    TypeAnnotatedValue::Result {value, ..} => {
                                        match value {
                                            Ok(_) => {}
                                            Err(v) => {
                                                let result =
                                                    &possible_resolution.evaluate(
                                                        &input.merge( &TypeAnnotatedValue::Record {
                                                            value: vec![(pattern_expr_variable.to_string(), *v.clone().unwrap())],
                                                            typ: vec![(pattern_expr_variable.to_string(), AnalysedType::from(*v.clone().unwrap()))]
                                                        })
                                                    )?;

                                                resolved_result = Some(result.clone());
                                                break;
                                            }

                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        ConstructorTypeName::CustomConstructor(_) => {
                            return Err(EvaluationError::Message(
                                "Pattern matching is currently supported only for inbuilt constructors. ok, err, some, none".to_string(),
                            ));
                        }
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
        .ok_or(EvaluationError::Message(
            "Pattern matching failed".to_string(),
        ))
}

#[cfg(test)]
mod tests {
    use crate::evaluator::{EvaluationError, Evaluator};
    use crate::expr::Expr;
    use http::HeaderMap;
    use serde_json::Value;
    use std::collections::HashMap;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use golem_service_base::model::Type;
    use crate::http_request::InputHttpRequest;
    use crate::worker_response::WorkerResponse;
    use crate::merge::Merge;
    use crate::type_inference::infer_analysed_type;

    fn resolved_variables_from_worker_response(input: &str) -> TypeAnnotatedValue {
        let value: Value = serde_json::from_str(input).expect("Failed to parse json");

        let expected_type = infer_analysed_type(&value).unwrap();
        TypeAnnotatedValue::from_json_value(&value, &expected_type).unwrap()
    }

    fn resolved_variables_from_request_body(
        input: &str,
        header_map: &HeaderMap,
    ) -> TypeAnnotatedValue {
        let request_body: Value = serde_json::from_str(input).expect("Failed to parse json");

        InputHttpRequest::get_request_body(&request_body)
            .unwrap()
            .merge(&InputHttpRequest::get_headers(header_map).unwrap())
    }

    fn resolved_variables_from_request_path(
        path_values: &HashMap<usize, String>,
        spec_variables: &HashMap<usize, String>,
    ) -> TypeAnnotatedValue {
        InputHttpRequest::get_request_path_values(
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

        let resolved_variables =
            resolved_variables_from_request_path(&request_path_values, &spec_path_variables);

        let expr = Expr::from_primitive_string("${request.path.id}").unwrap();
        let expected_evaluated_result = Value::String("pId".to_string());
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

                           "id": "bId",
                           "titles": [
                             "bTitle1", "bTitle2"
                           ]

                    }"#,
            &HeaderMap::new(),
        );

        let expr = Expr::from_primitive_string("${request.body.titles[0]}").unwrap();
        let expected_evaluated_result = Value::String("bTitle1".to_string());
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

        let expr = Expr::from_primitive_string(
            "${request.body.address.street} ${request.body.address.city}",
        )
        .unwrap();
        let expected_evaluated_result = Value::String("bStreet bCity".to_string());
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

        let expr = Expr::from_primitive_string(
            "${if (request.header.authorisation == 'admin') then 200 else 401}",
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

        let expr = Expr::from_primitive_string("${request.body.titles[4]}").unwrap();
        let expected_evaluated_result =
            EvaluationError::Message("The array doesn't contain 4 elements".to_string());
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

        let expr = Expr::from_primitive_string("${request.body.address[4]}").unwrap();
        let expected_evaluated_result =
            EvaluationError::Message("Result is not an array to get the index 4".to_string());
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

        let expr =
            Expr::from_primitive_string("${if (request.header.authorisation) then 200 else 401}")
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
            &HeaderMap::new(),
        );

        let expr = Expr::from_primitive_string("${worker.response.address.street}").unwrap();
        let expected_evaluated_result =
            EvaluationError::Message("Details of worker response is missing".to_string());
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_pattern_match_optional() {
        let resolved_variables = resolved_variables_from_worker_response(
            r#"
                        {

                           "id": "pId"
                        }
                   "#,
        );

        let expr = Expr::from_primitive_string(
            "${match worker.response { some(value) => 'personal-id', none => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(Value::String("personal-id".to_string())));
    }

    #[test]
    fn test_evaluation_with_pattern_match_none() {
        let resolved_variables =
            resolved_variables_from_worker_response(Value::Null.to_string().as_str());

        let expr = Expr::from_primitive_string(
            "${match worker.response { some(value) => 'personal-id', none => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Ok(Value::String("not found".to_string())));
    }

    #[test]
    fn test_evaluation_with_pattern_match_with_other_exprs() {
        let mut request_path_values = HashMap::new();
        request_path_values.insert(0, "foo".to_string());

        let mut spec_path_variables = HashMap::new();
        spec_path_variables.insert(0, "id".to_string());

        let mut resolved_variables =
            resolved_variables_from_request_path(&request_path_values, &spec_path_variables);

        resolved_variables.extend(&resolved_variables_from_worker_response(
            r#"
                    {
                        "ok": {
                           "id": "baz"
                        }
                    }"#,
        ));

        let expr1 = Expr::from_primitive_string(
            "${if request.path.id == 'foo' then 'bar' else match worker.response { ok(value) => value.id, err(msg) => 'empty' }}",
        )
            .unwrap();

        let result1 = expr1.evaluate(&resolved_variables);

        // Intentionally bringing an curly brace
        let expr2 = Expr::from_primitive_string(
            "${if request.path.id == 'bar' then 'foo' else { match worker.response { ok(foo) => foo.id, err(msg) => 'empty' }} }",

        ).unwrap();

        let result2 = expr2.evaluate(&resolved_variables);

        let new_worker_response = resolved_variables_from_worker_response(
            r#"
                    {
                        "err": {
                           "msg": "failed"
                        }
                    }"#,
        );

        resolved_variables.extend(&new_worker_response);

        let expr3 = Expr::from_primitive_string(
            "${if request.path.id == 'bar' then 'foo' else { match worker.response { ok(foo) => foo.id, err(msg) => 'empty' }} }",

        ).unwrap();

        let result3 = expr3.evaluate(&resolved_variables);

        assert_eq!(
            (result1, result2, result3),
            (
                Ok(Value::String("bar".to_string())),
                Ok(Value::String("baz".to_string())),
                Ok(Value::String("empty".to_string()))
            )
        );
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
            "${match worker.response { ok(value) => value, err(msg) => 'not found' }}",
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
            "${match worker.response { ok(value) => value.id, err(msg) => 'not found' }}",
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

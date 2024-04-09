use std::ops::Deref;

use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::get_json_from_typed_value;
use golem_wasm_rpc::TypeAnnotatedValue;

use crate::primitive::GetPrimitive;
use getter::GetError;
use getter::Getter;
use path::Path;

use crate::expression::{
    ConstructorPattern, ConstructorTypeName, Expr, InBuiltConstructorInner, InnerNumber,
};
use crate::merge::Merge;

use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

mod getter;
mod path;

pub trait Evaluator {
    fn evaluate(&self, input: &TypeAnnotatedValue) -> Result<TypeAnnotatedValue, EvaluationError>;
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
    fn evaluate(&self, input: &TypeAnnotatedValue) -> Result<TypeAnnotatedValue, EvaluationError> {
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
            input: &mut TypeAnnotatedValue,
        ) -> Result<TypeAnnotatedValue, EvaluationError> {
            match expr {
                Expr::Request() => input
                    .get(&Path::from_key(Token::request().to_string().as_str()))
                    .map_err(|err| err.into()),
                Expr::Worker() => input
                    .get(&Path::from_key(Token::worker().to_string().as_str()))
                    .map_err(|err| err.into()),

                Expr::SelectIndex(expr, index) => {
                    let evaluation_result = go(expr, input)?;
                    evaluation_result
                        .get(&Path::from_index(*index))
                        .map_err(|err| err.into())
                }

                Expr::SelectField(expr, field_name) => {
                    let evaluation_result = go(expr, input)?;

                    evaluation_result
                        .get(&Path::from_key(field_name.as_str()))
                        .map_err(|err| err.into())
                }

                Expr::EqualTo(left, right) => {
                    let left = go(left, input)?;
                    let right = go(right, input)?;

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
                    let left = go(left, input)?;
                    let right = go(right, input)?;

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
                    let left = go(left, input)?;
                    let right = go(right, input)?;

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
                    let left = go(left, input)?;
                    let right = go(right, input)?;

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
                    let left = go(left, input)?;
                    let right = go(right, input)?;

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
                            "The expression is evaluated to {} but it is not a boolean value to apply not (!) operator on",
                            get_json_from_typed_value(&evaluated_expr)
                        ))),
                    }
                }

                Expr::Cond(pred0, left, right) => {
                    let pred = go(pred0, input)?;
                    let left = go(left, input)?;
                    let right = go(right, input)?;

                    match pred {
                        TypeAnnotatedValue::Bool(value) => {
                            if value {
                                Ok(left)
                            } else {
                                Ok(right)
                            }
                        }
                        _ => Err(EvaluationError::Message(format!(
                            "The predicate expression is evaluated to {} but it is not a boolean value",
                            get_json_from_typed_value(&pred)
                        ))),
                    }
                }

                Expr::Let(_, _) | Expr::Multiple(_) => todo!(),

                Expr::Sequence(exprs) => {
                    let mut result: Vec<TypeAnnotatedValue> = vec![];

                    for expr in exprs {
                        match go(expr, input) {
                            Ok(value) => result.push(value),
                            Err(result) => return Err(result),
                        }
                    }
                    match result.first() {
                        Some(value) => Ok(TypeAnnotatedValue::List {
                            values: result.clone(),
                            typ: AnalysedType::from(value),
                        }),
                        None => Ok(TypeAnnotatedValue::List {
                            values: result.clone(),
                            typ: AnalysedType::Tuple(vec![]),
                        }), // Support optional type in List
                    }
                }

                Expr::Record(tuples) => {
                    let mut values: Vec<(String, TypeAnnotatedValue)> = vec![];

                    for (key, expr) in tuples {
                        match go(expr, input) {
                            Ok(value) => {
                                values.push((key.to_string(), value));
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
                    })
                }

                Expr::Concat(exprs) => {
                    let mut result = String::new();

                    for expr in exprs {
                        match go(expr, input) {
                            Ok(value) => {
                                if let Some(primitive) = value.get_primitive() {
                                    result.push_str(primitive.to_string().as_str())
                                } else {
                                    return Err(EvaluationError::Message(format!("Cannot append a complex expression {} to form strings. Please check the expression", get_json_from_typed_value(&value))));
                                }
                            }

                            Err(result) => return Err(result),
                        }
                    }

                    Ok(TypeAnnotatedValue::Str(result))
                }

                Expr::Literal(literal) => Ok(TypeAnnotatedValue::Str(literal.clone())),

                Expr::Number(number) => match number {
                    InnerNumber::UnsignedInteger(u64) => Ok(TypeAnnotatedValue::U64(*u64)),
                    InnerNumber::Integer(i64) => Ok(TypeAnnotatedValue::S64(*i64)),
                    InnerNumber::Float(f64) => Ok(TypeAnnotatedValue::F64(*f64)),
                },

                Expr::Variable(variable) => input
                    .get(&Path::from_key(variable.as_str()))
                    .map_err(|err| err.into()),

                Expr::Boolean(bool) => Ok(TypeAnnotatedValue::Bool(*bool)),
                Expr::PatternMatch(input_expr, constructors) => {
                    let constructors: &Vec<(ConstructorPattern, Expr)> = &constructors
                        .iter()
                        .map(|constructor| (constructor.0 .0.clone(), *constructor.0 .1.clone()))
                        .collect();

                    handle_pattern_match(input_expr, constructors, input)
                }
                Expr::Constructor0(constructor) => handle_expr_construction(constructor, input),
                Expr::Tuple(tuple_exprs) => {
                    let mut result: Vec<TypeAnnotatedValue> = vec![];

                    for expr in tuple_exprs {
                        let type_annotated_value = go(expr, input)?;
                        result.push(type_annotated_value);
                    }

                    let typ: &Vec<AnalysedType> = &result.iter().map(AnalysedType::from).collect();

                    Ok(TypeAnnotatedValue::Tuple {
                        value: result,
                        typ: typ.clone(),
                    })
                }

                Expr::Flags(flags) => Ok(TypeAnnotatedValue::Flags {
                    values: flags.clone(),
                    typ: flags.clone(),
                }),
            }
        }

        let mut input = input.clone();
        go(expr, &mut input)
    }
}

fn handle_expr_construction(
    constructor: &ConstructorPattern,
    input: &TypeAnnotatedValue,
) -> Result<TypeAnnotatedValue, EvaluationError> {
    match constructor {
        ConstructorPattern::WildCard => Err(EvaluationError::Message(
            "Found a wild card which is an invalid expression".to_string(),
        )),
        ConstructorPattern::As(_, _) => Err(EvaluationError::Message(
            "Found an as pattern which is an invalid expression".to_string(),
        )),
        ConstructorPattern::Constructor(constructor_name, constructors) => match constructor_name {
            ConstructorTypeName::InBuiltConstructor(in_built) => match in_built {
                InBuiltConstructorInner::Ok => {
                    let one_constructor = constructors.first().ok_or(EvaluationError::Message(
                        "Ok constructor should have one constructor".to_string(),
                    ))?;

                    let result = handle_expr_construction(one_constructor, input)?;
                    let analysed_type = AnalysedType::from(&result);
                    Ok(TypeAnnotatedValue::Result {
                        value: Ok(Some(Box::new(result))),
                        ok: Some(Box::new(analysed_type)),
                        error: None,
                    })
                }
                InBuiltConstructorInner::Err => {
                    let one_constructor = constructors.first().ok_or(EvaluationError::Message(
                        "Err constructor should have one constructor".to_string(),
                    ))?;
                    let result = handle_expr_construction(one_constructor, input)?;
                    let analysed_type = AnalysedType::from(&result);
                    Ok(TypeAnnotatedValue::Result {
                        value: Err(Some(Box::new(result))),
                        error: Some(Box::new(analysed_type)),
                        ok: None,
                    })
                }
                InBuiltConstructorInner::None => Ok(TypeAnnotatedValue::Option {
                    typ: AnalysedType::Str,
                    value: None,
                }),
                InBuiltConstructorInner::Some => {
                    let one_constructor = constructors.first().ok_or(EvaluationError::Message(
                        "Some constructor should have one constructor".to_string(),
                    ))?;
                    let result = handle_expr_construction(one_constructor, input)?;
                    let analysed_type = AnalysedType::from(&result);
                    Ok(TypeAnnotatedValue::Option {
                        value: Some(Box::new(result)),
                        typ: analysed_type,
                    })
                }
            },
            // Considering any custom construction to be variant
            ConstructorTypeName::CustomConstructor(name) => {
                let one_constructor = constructors.first().ok_or(EvaluationError::Message(
                    "Some constructor should have one constructor".to_string(),
                ))?;
                let result = handle_expr_construction(one_constructor, input)?;
                let analysed_type = AnalysedType::from(&result);
                Ok(TypeAnnotatedValue::Variant {
                    typ: vec![(name.clone(), Some(analysed_type))],
                    case_name: name.clone(),
                    case_value: Some(Box::new(result)),
                })
            }
        },
        ConstructorPattern::Literal(possible_expr) => possible_expr.evaluate(input),
    }
}
fn handle_pattern_match(
    input_expr: &Expr,
    constructors: &Vec<(ConstructorPattern, Expr)>,
    input: &mut TypeAnnotatedValue,
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
                    // Lazily evaluated. We need to look at the patterns only when it is required
                    let pattern_expr_variable = || {
                        match &patterns.first() {
                            Some(ConstructorPattern::Literal(expr)) => match *expr.clone() {
                                Expr::Variable(variable) => Ok(variable),
                                _ => {
                                    Err(EvaluationError::Message(
                                        "Currently only variable pattern is supported. i.e, some(value), ok(value), err(message) etc".to_string(),
                                    ))
                                }
                            },
                            None => Err(EvaluationError::Message(
                                "Zero patterns found".to_string(),
                            )),
                            _ => {
                                Err(EvaluationError::Message(
                                    "Currently only variable pattern is supported. i.e, some(value), ok(value), err(message) etc".to_string(),
                                ))
                            }
                        }
                    };
                    match condition_key {
                        ConstructorTypeName::InBuiltConstructor(constructor_type) => {
                            match constructor_type {
                                InBuiltConstructorInner::Some => match &match_evaluated {
                                    TypeAnnotatedValue::Option { value, .. } => {
                                        if let Some(v) = value {
                                            let pattern_expr_variable = pattern_expr_variable()?;
                                            let result = possible_resolution.evaluate(
                                                input.merge(&TypeAnnotatedValue::Record {
                                                    value: vec![(
                                                        pattern_expr_variable.clone(),
                                                        *v.clone(),
                                                    )],
                                                    typ: vec![(
                                                        pattern_expr_variable.clone(),
                                                        AnalysedType::from(v.as_ref()),
                                                    )],
                                                }),
                                            )?;

                                            resolved_result = Some(result);
                                        }
                                    }
                                    // We allow all other type annotated value to be a success, even if it is not an Option.
                                    // This is for user-friendliness. Example: Say we have a request body `{user-id : 10}`
                                    // and we allow users to perform `match request.body.user-id { some(value) => value, none => 'not found'}`
                                    // even if request.body.user-id type is not Option
                                    other_type_annotated_value => {
                                        let pattern_expr_variable = pattern_expr_variable()?;
                                        let result = possible_resolution.evaluate(input.merge(
                                            &TypeAnnotatedValue::Record {
                                                value: vec![(
                                                    pattern_expr_variable.clone(),
                                                    other_type_annotated_value.clone(),
                                                )],
                                                typ: vec![(
                                                    pattern_expr_variable.clone(),
                                                    AnalysedType::from(other_type_annotated_value),
                                                )],
                                            },
                                        ))?;

                                        resolved_result = Some(result);
                                    }
                                },
                                InBuiltConstructorInner::None => {
                                    if let TypeAnnotatedValue::Option { value: None, .. } =
                                        &match_evaluated
                                    {
                                        let result = possible_resolution.evaluate(input)?;

                                        resolved_result = Some(result);
                                        break;
                                    }
                                }

                                InBuiltConstructorInner::Ok => {
                                    if let TypeAnnotatedValue::Result { value: Ok(v), .. } =
                                        &match_evaluated
                                    {
                                        let result = possible_resolution.evaluate(input.merge(
                                            &TypeAnnotatedValue::Record {
                                                value: vec![(
                                                    pattern_expr_variable()?.to_string(),
                                                    *v.clone().unwrap(),
                                                )],
                                                typ: vec![(
                                                    pattern_expr_variable()?.to_string(),
                                                    AnalysedType::from(v.as_ref().unwrap().deref()),
                                                )],
                                            },
                                        ))?;

                                        resolved_result = Some(result);
                                        break;
                                    }
                                }
                                InBuiltConstructorInner::Err => {
                                    if let TypeAnnotatedValue::Result { value: Err(v), .. } =
                                        &match_evaluated
                                    {
                                        let result = &possible_resolution.evaluate(input.merge(
                                            &TypeAnnotatedValue::Record {
                                                value: vec![(
                                                    pattern_expr_variable()?.to_string(),
                                                    *v.clone().unwrap(),
                                                )],
                                                typ: vec![(
                                                    pattern_expr_variable()?.to_string(),
                                                    AnalysedType::from(v.as_ref().unwrap().deref()),
                                                )],
                                            },
                                        ))?;

                                        resolved_result = Some(result.clone());
                                        break;
                                    }
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

    resolved_result.ok_or(EvaluationError::Message(
        "Pattern matching failed".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use http::{HeaderMap, Uri};
    use serde_json::{json, Value};

    use crate::api_definition::http::AllPathPatterns;
    use crate::evaluator::getter::GetError;
    use crate::evaluator::{EvaluationError, Evaluator};
    use crate::expression;
    use crate::merge::Merge;
    use test_utils::*;

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

        let expr = expression::from_string("${worker.response.address.street}").unwrap();
        let expected_evaluated_result =
            EvaluationError::InvalidReference(GetError::KeyNotFound("worker".to_string()));
        let result = expr.evaluate(&resolved_variables);
        assert_eq!(result, Err(expected_evaluated_result));
    }

    #[test]
    fn test_evaluation_with_pattern_match_optional() {
        let worker_response = get_worker_response(
            r#"
                        {

                           "id": "pId"
                        }
                   "#,
        );

        let expr = expression::from_string(
            "${match worker.response { some(value) => 'personal-id', none => 'not found' }}",
        )
        .unwrap();
        let result = expr.evaluate(&worker_response.result_with_worker_response_key());
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
            "${match worker.response { some(value) => 'personal-id', none => 'not found' }}",
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
            "${if request.path.id == 'foo' then 'bar' else match worker.response { ok(value) => value.id, err(msg) => 'empty' }}",
        )
            .unwrap();

        let result1 = expr1.evaluate(success_response_with_input_variables);

        // Intentionally bringing an curly brace
        let expr2 = expression::from_string(
            "${if request.path.id == 'bar' then 'foo' else match worker.response { ok(foo) => foo.id, err(msg) => 'empty' }}",

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
            "${if request.path.id == 'bar' then 'foo' else match worker.response { ok(foo) => foo.id, err(msg) => 'empty' }}",

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
            "${match worker.response { ok(value) => 'personal-id', err(msg) => 'not found' }}",
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
            "${match worker.response { ok(value) => value, err(msg) => 'not found' }}",
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
            "${match worker.response { ok(value) => value.id, err(msg) => 'not found' }}",
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
            "${match worker.response { ok(value) => value.ids[0], none => 'not found' }}",
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
            "${match worker.response { ok(value) => some(value.ids[0]), none => 'not found' }}",
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
            "${match worker.response { ok(value) => none, none => 'not found' }}",
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
            "${match worker.response { ok(value) => some(none), none => none }}",
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
            "${match worker.response { ok(value) => ok(1), none => err(2) }}",
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
            "${match worker.response { ok(value) => ok(1), err(msg) => err(2) }}",
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
    fn test_evaluation_with_wave_like_syntax_variant() {
        let expr = expression::from_string("${Foo(some(2))}").unwrap();

        let result = expr.evaluate(&TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        });

        let expected = Ok(TypeAnnotatedValue::Variant {
            typ: vec![(
                "Foo".to_string(),
                Some(AnalysedType::Option(Box::new(AnalysedType::U64))),
            )],
            case_name: "Foo".to_string(),
            case_value: Some(Box::new(TypeAnnotatedValue::Option {
                value: Some(Box::new(TypeAnnotatedValue::U64(2))),
                typ: AnalysedType::U64,
            })),
        });

        assert_eq!(result, expected);
    }

    #[test]
    fn test_evaluation_with_wave_like_syntax_variant_with_if_condition() {
        let expr =
            expression::from_string("${if 1 == 2 then Foo(some(2)) else Bar(some(3)) }").unwrap();

        let result = expr.evaluate(&TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        });

        let expected = Ok(TypeAnnotatedValue::Variant {
            typ: vec![(
                "Bar".to_string(),
                Some(AnalysedType::Option(Box::new(AnalysedType::U64))),
            )],
            case_name: "Bar".to_string(),
            case_value: Some(Box::new(TypeAnnotatedValue::Option {
                value: Some(Box::new(TypeAnnotatedValue::U64(3))),
                typ: AnalysedType::U64,
            })),
        });

        assert_eq!(result, expected);
    }

    #[test]
    fn test_evaluation_with_wave_like_syntax_variant_with_match_expr() {
        let expr =
            expression::from_string("${match some(1) {some(x) => Foo(some(x)), none => Bar(1) }}")
                .unwrap();

        let result = expr.evaluate(&TypeAnnotatedValue::Record {
            value: vec![],
            typ: vec![],
        });

        let expected = Ok(TypeAnnotatedValue::Variant {
            typ: vec![(
                "Foo".to_string(),
                Some(AnalysedType::Option(Box::new(AnalysedType::U64))),
            )],
            case_name: "Foo".to_string(),
            case_value: Some(Box::new(TypeAnnotatedValue::Option {
                value: Some(Box::new(TypeAnnotatedValue::U64(1))),
                typ: AnalysedType::U64,
            })),
        });

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

            WorkerResponse {
                result: worker_response_value,
            }
        }

        pub(crate) fn get_worker_response(input: &str) -> WorkerResponse {
            let value: Value = serde_json::from_str(input).expect("Failed to parse json");

            let expected_type = infer_analysed_type(&value);
            let result_as_typed_value = get_typed_value_from_json(&value, &expected_type).unwrap();
            WorkerResponse {
                result: result_as_typed_value,
            }
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

            let expr1_string = "${match worker.response { ok(x) => 'foo', err(msg) => 'error' }}";
            let expr1 = expression::from_string(expr1_string).unwrap();
            let value1 = expr1
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expr2_string = expr1.to_string().unwrap();
            let expr2 = expression::from_string(expr2_string.as_str()).unwrap();
            let value2 = expr2
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expected = TypeAnnotatedValue::Str("error".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }

        #[test]
        fn expr_to_string_round_trip_match_expr_append() {
            let worker_response = get_err_worker_response();

            let expr1_string =
                "append-${match worker.response { ok(x) => 'foo', err(msg) => 'error' }}";
            let expr1 = expression::from_string(expr1_string).unwrap();
            let value1 = expr1
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expr2_string = expr1.to_string().unwrap();
            let expr2 = expression::from_string(expr2_string.as_str()).unwrap();
            let value2 = expr2
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expected = TypeAnnotatedValue::Str("append-error".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }

        #[test]
        fn expr_to_string_round_trip_match_expr_append_suffix() {
            let worker_response = get_err_worker_response();

            let expr1_string =
                "prefix-${match worker.response { ok(x) => 'foo', err(msg) => 'error' }}-suffix";
            let expr1 = expression::from_string(expr1_string).unwrap();
            let value1 = expr1
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expr2_string = expr1.to_string().unwrap();
            let expr2 = expression::from_string(expr2_string.as_str()).unwrap();
            let value2 = expr2
                .evaluate(&worker_response.result_with_worker_response_key())
                .unwrap();

            let expected = TypeAnnotatedValue::Str("prefix-error-suffix".to_string());
            assert_eq!((&value1, &value2), (&expected, &expected));
        }
    }
}

use std::fmt::Display;
use std::str::FromStr;

use crate::evaluator::primitive::Primitive;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::parser::expr_parser::ExprParser;
use crate::parser::{GolemParser, ParseError};

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Expr {
    Request(),
    Worker(),
    SelectField(Box<Expr>, String),
    SelectIndex(Box<Expr>, usize),
    Sequence(Vec<Expr>),
    Record(Vec<(String, Box<Expr>)>),
    Literal(String),
    Number(InnerNumber),
    Variable(String),
    Boolean(bool),
    PathVar(String),
    Concat(Vec<Expr>),
    Not(Box<Expr>),
    GreaterThan(Box<Expr>, Box<Expr>),
    GreaterThanOrEqualTo(Box<Expr>, Box<Expr>),
    LessThanOrEqualTo(Box<Expr>, Box<Expr>),
    EqualTo(Box<Expr>, Box<Expr>),
    LessThan(Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    PatternMatch(Box<Expr>, Vec<ConstructorPatternExpr>),
    Constructor0(ConstructorPattern), // Can exist standalone from pattern match
}

impl Expr {
    pub fn unsigned_integer(value: u64) -> Expr {
        Expr::Number(InnerNumber::UnsignedInteger(value))
    }

    pub fn integer(value: i64) -> Expr {
        Expr::Number(InnerNumber::Integer(value))
    }

    pub fn float(value: f64) -> Expr {
        Expr::Number(InnerNumber::Float(value))
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum InnerNumber {
    UnsignedInteger(u64),
    Integer(i64),
    Float(f64),
}

impl Display for InnerNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InnerNumber::UnsignedInteger(value) => write!(f, "{}", value),
            InnerNumber::Integer(value) => write!(f, "{}", value),
            InnerNumber::Float(value) => write!(f, "{}", value),
        }
    }
}

// This standalone is not a valid expression
// and can only be part of PatternMatch

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct ConstructorPatternExpr(pub (ConstructorPattern, Box<Expr>));

// A constructor pattern by itself is an expr,
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ConstructorPattern {
    WildCard,
    As(String, Box<ConstructorPattern>),
    Constructor(ConstructorTypeName, Vec<ConstructorPattern>),
    Literal(Box<Expr>),
}

impl ConstructorPattern {
    pub fn constructor(
        constructor_name: &str,
        variables: Vec<ConstructorPattern>,
    ) -> Result<ConstructorPattern, ParseError> {
        if constructor_name == "ok" {
            validate_single_variable_constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::Ok),
                variables,
            )
        } else if constructor_name == "err" {
            validate_single_variable_constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::Err),
                variables,
            )
        } else if constructor_name == "none" {
            validate_empty_constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::None),
                variables,
            )
        } else if constructor_name == "some" {
            validate_single_variable_constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::Some),
                variables,
            )
        } else {
            let constructor_type =
                ConstructorTypeName::CustomConstructor(constructor_name.to_string());
            Ok(ConstructorPattern::Constructor(constructor_type, variables))
        }
    }
}

fn validate_empty_constructor(
    constructor_type: ConstructorTypeName,
    variables: Vec<ConstructorPattern>,
) -> Result<ConstructorPattern, ParseError> {
    if !variables.is_empty() {
        Err(ParseError::Message(
            "constructor should have zero variables".to_string(),
        ))
    } else {
        Ok(ConstructorPattern::Constructor(constructor_type, variables))
    }
}

fn validate_single_variable_constructor(
    constructor_type: ConstructorTypeName,
    variables: Vec<ConstructorPattern>,
) -> Result<ConstructorPattern, ParseError> {
    if variables.len() != 1 {
        Err(ParseError::Message(
            "constructor should have exactly one variable".to_string(),
        ))
    } else {
        match variables.first().unwrap() {
            ConstructorPattern::Literal(_) => {
                Ok(ConstructorPattern::Constructor(constructor_type, variables))
            }

            ConstructorPattern::Constructor(_, _) => {
                Ok(ConstructorPattern::Constructor(constructor_type, variables))
            }
            _ => Err(ParseError::Message(
                "Ok constructor should have exactly one variable".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ConstructorTypeName {
    InBuiltConstructor(InBuiltConstructorInner),
    CustomConstructor(String),
}

impl Display for ConstructorTypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstructorTypeName::InBuiltConstructor(inner) => write!(f, "{}", inner),
            ConstructorTypeName::CustomConstructor(name) => write!(f, "{}", name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum InBuiltConstructorInner {
    Ok,
    Err,
    None,
    Some,
}

impl Display for InBuiltConstructorInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InBuiltConstructorInner::Ok => write!(f, "ok"),
            InBuiltConstructorInner::Err => write!(f, "err"),
            InBuiltConstructorInner::None => write!(f, "none"),
            InBuiltConstructorInner::Some => write!(f, "some"),
        }
    }
}

impl Expr {
    pub fn is_literal(&self) -> bool {
        match self {
            Expr::Literal(_) => true,
            Expr::Concat(vec) => vec.iter().all(|x| x.is_literal()),
            _ => false,
        }
    }

    // A primitive string can be converted to Expr. Not that if the string is already as complex as yaml or json, use functions such as Expr::from_yaml_value, Expr::from_json_value
    pub fn from_primitive_string(input: &str) -> Result<Expr, ParseError> {
        let expr_parser = ExprParser {};
        expr_parser.parse(input)
    }

    // A json can be mapped to Expr. Example: Json::Record can be mmpped to Expr::Record
    pub fn from_json_value(input: &Value) -> Result<Expr, ParseError> {
        match input {
            Value::Bool(bool) => Ok(Expr::Literal(bool.to_string())),
            Value::Number(number) => Ok(Expr::Literal(number.to_string())),
            Value::String(string) => Expr::from_primitive_string(string),
            Value::Array(sequence) => {
                let mut exprs: Vec<Expr> = vec![];

                for s in sequence {
                    let expr: Expr = Expr::from_json_value(s)?;
                    exprs.push(expr);
                }

                Ok(Expr::Sequence(exprs))
            }
            Value::Object(mapping) => {
                let mut tuple_vec: Vec<(String, Box<Expr>)> = vec![];

                for (k, v) in mapping {
                    let value_expr: Expr = Expr::from_json_value(v)?;

                    tuple_vec.push((k.clone(), Box::new(value_expr)))
                }
                Ok(Expr::Record(tuple_vec))
            }

            Value::Null => Err(ParseError::Message("Json Null not implemented".to_string())),
        }
    }

    //  TODO; GOL-248: Ideally, Expr::from(expr_string).to_string() should be similar to expr_string.
    //  They don't need to be exact but there are confusing expressions being emitted
    pub fn to_json_value(&self) -> Result<Value, String> {
        fn go(expr: &Expr, is_code: bool) -> Result<InternalValue, String> {
            match expr {
                Expr::SelectField(expr0, field_name) => {
                    let expr0: &Expr = expr0;
                    let expr_internal = go(expr0, true)?;

                    // Doesn't matter if inner expression was interpolated, we will interpolate it outside
                    match expr_internal {
                        InternalValue::Interpolated(expr_string) =>
                            Ok(InternalValue::Interpolated(format!(
                                "{}.{}",
                                expr_string,
                                field_name
                            ))),
                        InternalValue::NonInterpolated(expr_string) =>
                            Ok(InternalValue::Interpolated(format!(
                                "{}.{}",
                                expr_string,
                                field_name
                            ))),
                        InternalValue::Quoted(expr_string) =>
                            Ok(InternalValue::Interpolated(format!(
                                "'{}'.{}",
                                expr_string,
                                field_name
                            ))),
                        InternalValue::RawJson(_) =>
                            Err("Invalid selection of field. Example of a valid selection: request.body.users[1]".into())
                    }
                }

                Expr::SelectIndex(expr0, index) => {
                    let expr0: &Expr = expr0;
                    let expr_internal = go(expr0, true)?;

                    // Doesn't matter if inner expression was interpolated, we will interpolate it outside
                    match expr_internal {
                        InternalValue::Interpolated(expr_string) =>
                            Ok(InternalValue::Interpolated(format!(
                                "{}[{}]",
                                expr_string,
                                index
                            ))),
                        InternalValue::NonInterpolated(expr_string) =>
                            Ok(InternalValue::Interpolated(format!(
                                "{}[{}]",
                                expr_string,
                                index
                            ))),
                        InternalValue::Quoted(expr_string) =>
                            Ok(InternalValue::Interpolated(format!(
                                "'{}'[{}]",
                                expr_string,
                                index
                            ))),
                        InternalValue::RawJson(_) =>
                            Err("Invalid selection of field. Example of a valid selection: request.body.users[1]".into())
                    }
                }

                Expr::Request() => Ok(InternalValue::Interpolated("request".to_string())),

                Expr::Worker() => Ok(InternalValue::Interpolated("worker".to_string())),

                Expr::Record(values) => {
                    let mut mapping = serde_json::Map::new();

                    for (key, expr) in values {
                        let key_yaml = key.clone();
                        match *expr.clone() {
                            Expr::Literal(value) => {
                                mapping.insert(key_yaml, Value::String(value.clone()));
                            }
                            Expr::Variable(variable) => {
                                mapping
                                    .insert(key_yaml, Value::String(format!("${{{}}}", variable)));
                            }
                            _ => {
                                let value_json = expr.to_json_value()?;
                                mapping.insert(key_yaml, value_json);
                            }
                        }
                    }

                    Ok(InternalValue::RawJson(Value::Object(mapping)))
                }

                Expr::Sequence(values) => {
                    let mut vs: Vec<Value> = vec![];
                    for expr in values {
                        match expr {
                            Expr::Literal(value) => {
                                vs.push(Value::String(value.clone()));
                            }
                            Expr::Variable(variable) => {
                                vs.push(Value::String(format!("${{{}}}", variable)));
                            }
                            _ => {
                                let value_json = expr.to_json_value()?;
                                vs.push(value_json);
                            }
                        }
                    }

                    Ok(InternalValue::RawJson(Value::Array(vs)))
                }

                Expr::Literal(value) => match Primitive::from(value.clone()) {
                    Primitive::String(string) => {
                        if is_code {
                            Ok(InternalValue::Quoted(string.clone()))
                        } else {
                            Ok(InternalValue::NonInterpolated(string.clone()))
                        }
                    }

                    Primitive::Num(num) => Ok(InternalValue::NonInterpolated(num.to_string())),
                    Primitive::Bool(bool) => Ok(InternalValue::NonInterpolated(bool.to_string())),
                },

                Expr::PathVar(value) => Ok(InternalValue::Interpolated(value.clone())),
                Expr::Concat(values) => {
                    let mut vs: Vec<String> = vec![];
                    for value in values {
                        let v = go(value, is_code)?;
                        match v {
                            InternalValue::Interpolated(v) => {
                                vs.push(format!("${{{}}}", v));
                            }
                            InternalValue::NonInterpolated(v) => {
                                vs.push(v);
                            }
                            InternalValue::Quoted(v) => {
                                vs.push(v.to_string());
                            }
                            _ => return Err("Not supported type".into()),
                        }
                    }
                    let value_string = vs.join("");

                    if is_code {
                        Ok(InternalValue::Quoted(value_string))
                    } else {
                        Ok(InternalValue::NonInterpolated(value_string))
                    }
                }

                Expr::Not(value) => {
                    let v = go(value, true)?;

                    match v {
                        // Bringing interpolation outside
                        InternalValue::Interpolated(v) => {
                            Ok(InternalValue::Interpolated(format!("!{}", v)))
                        }
                        // Bringing interpolation
                        InternalValue::NonInterpolated(v) => {
                            Ok(InternalValue::Interpolated(format!("!{}", v)))
                        }
                        // Bringing quotes outside, with interpolation inside
                        InternalValue::Quoted(v) => {
                            Ok(InternalValue::Quoted(format!("${{!{}}}", v)))
                        }

                        InternalValue::RawJson(_) => {
                            Err("Applying ! to a complex json is not supported".into())
                        }
                    }
                }
                Expr::GreaterThan(value1, value2) => {
                    let v1 = go(value1, true)?;
                    let v2 = go(value2, true)?;

                    Ok(InternalValue::Interpolated(format!(
                        "{}>{}",
                        v1.unwrap_string(),
                        v2.unwrap_string()
                    )))
                }
                Expr::GreaterThanOrEqualTo(value1, value2) => {
                    let v1 = go(value1, true)?;
                    let v2 = go(value2, true)?;

                    Ok(InternalValue::Interpolated(format!(
                        "{}>={}",
                        v1.unwrap_string(),
                        v2.unwrap_string()
                    )))
                }
                Expr::EqualTo(value1, value2) => {
                    let v1 = go(value1, true)?;
                    let v2 = go(value2, true)?;

                    Ok(InternalValue::Interpolated(format!(
                        "{}=={}",
                        v1.unwrap_string(),
                        v2.unwrap_string()
                    )))
                }
                Expr::LessThan(value1, value2) => {
                    let v1 = go(value1, true)?;
                    let v2 = go(value2, true)?;

                    Ok(InternalValue::Interpolated(format!(
                        "{}<{}",
                        v1.unwrap_string(),
                        v2.unwrap_string()
                    )))
                }

                Expr::LessThanOrEqualTo(value1, value2) => {
                    let v1 = go(value1, true)?;
                    let v2 = go(value2, true)?;

                    Ok(InternalValue::Interpolated(format!(
                        "{}<={}",
                        v1.unwrap_string(),
                        v2.unwrap_string()
                    )))
                }
                Expr::Cond(pred, value1, value2) => {
                    let p = go(pred, true)?;
                    let v1 = go(value1, true)?;
                    let v2 = go(value2, true)?;

                    Ok(InternalValue::Interpolated(format!(
                        "if ({}) then {} else {}",
                        p.unwrap_string(),
                        v1.unwrap_string(),
                        v2.unwrap_string()
                    )))
                }

                Expr::Number(number) => Ok(InternalValue::Interpolated(number.to_string())),

                Expr::Variable(variable) => Ok(InternalValue::Interpolated(variable.clone())),
                Expr::Boolean(boolean) => Ok(InternalValue::Interpolated(boolean.to_string())),

                Expr::PatternMatch(condition, match_cases) => {
                    let c = go(condition, true)?;
                    let mut match_cases_str = vec![];

                    for match_case in match_cases {
                        let constructor_pattern = format!(
                            "{} => {}",
                            convert_constructor_to_string(&match_case.0 .0, &|x| go(x, true))?,
                            go(&match_case.0 .1, true)?.unwrap_string()
                        );

                        match_cases_str.push(constructor_pattern);
                    }

                    Ok(InternalValue::Interpolated(format!(
                        "match {} {{ {} }}",
                        c.unwrap_string(),
                        match_cases_str.join(", ")
                    )))
                }

                Expr::Constructor0(constructor) => Ok(InternalValue::NonInterpolated(
                    convert_constructor_to_string(constructor, &|x| go(x, false))?,
                )),
            }
        }

        let internal_result = go(self, false)?;

        match internal_result {
            InternalValue::Interpolated(string) => Ok(Value::String(format!("${{{}}}", string))),
            InternalValue::NonInterpolated(value) => Ok(Value::String(value)),
            InternalValue::Quoted(value) => Ok(Value::String(format!("'{}'", value))),
            InternalValue::RawJson(value) => Ok(value),
        }
    }

    pub fn to_string(&self) -> Result<String, String> {
        let v = self.to_json_value();
        match v {
            Ok(serde_json::Value::String(v)) => Ok(v),
            _ => Err("Not supported type".to_string()),
        }
    }
}

fn convert_constructor_to_string<F>(
    match_case: &ConstructorPattern,
    get_internal_value: &F,
) -> Result<String, String>
where
    F: Fn(&Expr) -> Result<InternalValue, String>,
{
    match match_case {
        ConstructorPattern::WildCard => Ok("_".to_string()),
        ConstructorPattern::As(name, pattern) => Ok(format!(
            "{} as {}",
            convert_constructor_to_string(pattern, get_internal_value)?,
            name
        )),
        ConstructorPattern::Constructor(constructor_type, variables) => {
            let mut variables_str = vec![];
            for pattern in variables {
                let string = convert_constructor_to_string(pattern, get_internal_value)?;
                variables_str.push(string);
            }

            Ok(format!(
                "{}({})",
                constructor_type,
                variables_str.join(", ")
            ))
        }
        ConstructorPattern::Literal(expr) => Ok(match *expr.clone() {
            Expr::Variable(s) => s,
            any_expr => get_internal_value(&any_expr)?.unwrap_string(),
        }),
    }
}

impl FromStr for Expr {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let expr_parser = ExprParser {};
        expr_parser.parse(s)
    }
}

impl<'de> Deserialize<'de> for Expr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match Expr::from_json_value(&value) {
            Ok(expr) => Ok(expr),
            Err(message) => Err(serde::de::Error::custom(message.to_string())),
        }
    }
}

impl Serialize for Expr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.to_json_value() {
            Ok(value) => serde_json::Value::serialize(value, serializer),
            Err(error) => Err(serde::ser::Error::custom(error.to_string())),
        }
    }
}

enum InternalValue {
    Interpolated(String),
    NonInterpolated(String),
    RawJson(Value),
    Quoted(String),
}

impl InternalValue {
    fn unwrap_string(self) -> String {
        match self {
            InternalValue::Interpolated(s) => s,
            InternalValue::NonInterpolated(s) => s,
            InternalValue::Quoted(s) => format!("'{}'", s),
            InternalValue::RawJson(v) => match v {
                // Unwrap quotes
                Value::String(s) => s,
                v => v.to_string(),
            },
        }
    }
}

impl Display for InternalValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InternalValue::Interpolated(s) => write!(f, "{}", s),
            InternalValue::NonInterpolated(s) => write!(f, "{}", s),
            InternalValue::RawJson(v) => match v {
                // Unwrap quotes
                Value::String(s) => write!(f, "{}", s),
                v => write!(f, "{}", v),
            },

            InternalValue::Quoted(s) => write!(f, "'{}'", s),
        }
    }
}

//TODO: GOL-249 Add more round trip tests
#[cfg(test)]
mod tests {
    use crate::evaluator::Evaluator;
    use crate::expression::expr::Expr;
    use crate::worker_request::worker_response::WorkerResponse;
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::json::get_typed_value_from_json;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use serde_json::{json, Value};

    #[test]
    fn test_expr_from_json_value() {
        let json = json!({
            "name": "John",
            "age": 30,
            "cars": ["Ford", "BMW", "Fiat"],
            "user": "${worker.response}-user"
        });

        let expr = Expr::from_json_value(&json).unwrap();
        let result = expr.to_json_value().unwrap();

        let expected = json!({
            "name": "John",
            "age": "30",
            "cars": ["Ford", "BMW", "Fiat"],
            "user": "${worker.response}-user"
        });

        assert_eq!(result, expected);
    }

    #[test]
    fn test_expr_to_json_value() {
        let expr = Expr::Record(vec![
            (
                "name".to_string(),
                Box::new(Expr::Literal("John".to_string())),
            ),
            ("age".to_string(), Box::new(Expr::Literal("30".to_string()))),
            (
                "cars".to_string(),
                Box::new(Expr::Sequence(vec![
                    Expr::Literal("Ford".to_string()),
                    Expr::Literal("BMW".to_string()),
                    Expr::Literal("Fiat".to_string()),
                ])),
            ),
        ]);

        let json = expr.to_json_value().unwrap();

        assert_eq!(
            json,
            json!({
                "name": "John",
                "age": "30",
                "cars": ["Ford", "BMW", "Fiat"]
            })
        );
    }

    #[test]
    fn test_round_trip_simple_string() {
        let worker_response = "foo";
        let expr = Expr::from_primitive_string(worker_response).unwrap();
        let result = expr.to_json_value().unwrap();

        assert_eq!(result, Value::String("foo".to_string()));
    }

    #[test]
    fn expr_to_string_round_trip_match_expr_ok() {
        let worker_response = get_ok_worker_response();

        let expr1_string = "${match worker.response { ok(x) => '${x.id}-foo', err(msg) => msg }}";
        let expr1 = Expr::from_primitive_string(expr1_string).unwrap();
        let value1 = expr1
            .evaluate(&worker_response.result_with_worker_response_key())
            .unwrap();

        let expr2_string = expr1.to_string().unwrap();
        let expr2 = Expr::from_primitive_string(expr2_string.as_str()).unwrap();
        let value2 = expr2
            .evaluate(&worker_response.result_with_worker_response_key())
            .unwrap();

        let expected = TypeAnnotatedValue::Str("afsal-foo".to_string());
        assert_eq!((&value1, &value2), (&expected, &expected));
    }

    #[test]
    fn expr_to_string_round_trip_match_expr_err() {
        let worker_response = get_err_worker_response();

        let expr1_string = "${match worker.response { ok(x) => 'foo', err(msg) => 'error' }}";
        let expr1 = Expr::from_primitive_string(expr1_string).unwrap();
        let value1 = expr1
            .evaluate(&worker_response.result_with_worker_response_key())
            .unwrap();

        let expr2_string = expr1.to_string().unwrap();
        let expr2 = Expr::from_primitive_string(expr2_string.as_str()).unwrap();
        let value2 = expr2
            .evaluate(&worker_response.result_with_worker_response_key())
            .unwrap();

        let expected = TypeAnnotatedValue::Str("error".to_string());
        assert_eq!((&value1, &value2), (&expected, &expected));
    }

    fn get_err_worker_response() -> WorkerResponse {
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

    fn get_ok_worker_response() -> WorkerResponse {
        let worker_response_value = get_typed_value_from_json(
            &json!({"ok": { "id" : "afsal"} }),
            &AnalysedType::Result {
                ok: Some(Box::new(AnalysedType::Record(vec![(
                    "id".to_string(),
                    AnalysedType::Str,
                )]))),
                error: None,
            },
        )
        .unwrap();

        WorkerResponse {
            result: worker_response_value,
        }
    }

    #[test]
    fn expr_to_string_round_trip_match_expr_append() {
        let worker_response = get_err_worker_response();

        let expr1_string =
            "append-${match worker.response { ok(x) => 'foo', err(msg) => 'error' }}";
        let expr1 = Expr::from_primitive_string(expr1_string).unwrap();
        let value1 = expr1
            .evaluate(&worker_response.result_with_worker_response_key())
            .unwrap();

        let expr2_string = expr1.to_string().unwrap();
        let expr2 = Expr::from_primitive_string(expr2_string.as_str()).unwrap();
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
        let expr1 = Expr::from_primitive_string(expr1_string).unwrap();
        let value1 = expr1
            .evaluate(&worker_response.result_with_worker_response_key())
            .unwrap();

        let expr2_string = expr1.to_string().unwrap();
        let expr2 = Expr::from_primitive_string(expr2_string.as_str()).unwrap();
        let value2 = expr2
            .evaluate(&worker_response.result_with_worker_response_key())
            .unwrap();

        let expected = TypeAnnotatedValue::Str("prefix-error-suffix".to_string());
        assert_eq!((&value1, &value2), (&expected, &expected));
    }
}

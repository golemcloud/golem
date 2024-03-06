use std::fmt::Display;
use std::str::FromStr;

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize, Serializer};

use crate::parser::expr_parser::ExprParser;
use crate::parser::{GolemParser, ParseError};

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Expr {
    Request(),
    WorkerResponse(),
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

impl Display for ConstructorPatternExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} => {}", self.0.0, Expr::to_json_value(&self.0.1).unwrap())
    }
}

// A constructor pattern by itself is an expr,
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ConstructorPattern {
    WildCard,
    As(String, Box<ConstructorPattern>),
    Constructor(ConstructorTypeName, Vec<ConstructorPattern>),
    Literal(Box<Expr>),
}

impl Display for ConstructorPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstructorPattern::WildCard => write!(f, "_"),
            ConstructorPattern::As(name, pattern) => write!(f, "{} as {}", pattern, name),
            ConstructorPattern::Constructor(constructor_type, variables) => {
                write!(f, "{}({})", constructor_type, variables.iter().map(|x| x.to_string()).collect::<Vec<String>>().join(", "))
            }
            ConstructorPattern::Literal(expr) => write!(f, "{}", expr.to_string().unwrap()),
        }
    }
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
    pub fn from_json_value(input: &serde_json::Value) -> Result<Expr, ParseError> {
        match input {
            serde_json::Value::Bool(bool) => Ok(Expr::Literal(bool.to_string())),
            serde_json::Value::Number(number) => Ok(Expr::Literal(number.to_string())),
            serde_json::Value::String(string) => Expr::from_primitive_string(string),
            serde_json::Value::Array(sequence) => {
                let mut exprs: Vec<Expr> = vec![];

                for s in sequence {
                    let expr: Expr = Expr::from_json_value(s)?;
                    exprs.push(expr);
                }

                Ok(Expr::Sequence(exprs))
            }
            serde_json::Value::Object(mapping) => {
                let mut tuple_vec: Vec<(String, Box<Expr>)> = vec![];

                for (k, v) in mapping {
                    let value_expr: Expr = Expr::from_json_value(v)?;

                    tuple_vec.push((k.clone(), Box::new(value_expr)))
                }
                Ok(Expr::Record(tuple_vec))
            }

            serde_json::Value::Null => {
                Err(ParseError::Message("Json Null not implemented".to_string()))
            }
        }
    }

    pub fn to_json_value(&self) -> Result<serde_json::Value, String> {
        fn go(expr: &Expr) -> Result<InternalValue<serde_json::Value>, String> {
            match expr {
                Expr::SelectField(expr0, field_name) => {
                    let expr0: &Expr = expr0;
                    let expr = go(expr0)?;

                    match expr.unwrap() {
                        serde_json::Value::String(expr_string) =>
                            Ok(InternalValue::Interpolated(serde_json::Value::String(format!(
                                "{}.{}",
                                expr_string,
                                field_name
                            )))),
                        _ => Err("Invalid selection of field. {}. Example of a valid selection: request.body.users".into())
                    }
                }

                Expr::SelectIndex(expr0, index) => {
                    let expr0: &Expr = expr0;
                    let expr = go(expr0)?;

                    match expr.unwrap() {
                        serde_json::Value::String(expr_string) =>
                            Ok(InternalValue::Interpolated(serde_json::Value::String(format!(
                                "{}[{}]",
                                expr_string,
                                index
                            )))),
                        _ => Err("Invalid selection of index. Example of a valid selection: request.body.users[1]".into())
                    }
                }

                Expr::Request() => Ok(InternalValue::Interpolated(serde_json::Value::String(
                    "request".to_string(),
                ))),

                // TODO; only worker as response
                Expr::WorkerResponse() => Ok(InternalValue::Interpolated(
                    serde_json::Value::String("worker.response".to_string()),
                )),

                Expr::Record(values) => {
                    let mut mapping = serde_json::Map::new();

                    for (key, value) in values {
                        let key_yaml = key.clone();
                        let value_json = Expr::to_json_value(value)?;
                        mapping.insert(key_yaml, value_json);
                    }

                    Ok(InternalValue::NonInterpolated(serde_json::Value::Object(
                        mapping,
                    )))
                }
                Expr::Sequence(values) => {
                    let mut vs: Vec<serde_json::Value> = vec![];
                    for value in values {
                        let v = value.to_json_value()?;
                        vs.push(v);
                    }
                    Ok(InternalValue::NonInterpolated(serde_json::Value::Array(vs)))
                }
                Expr::Literal(value) => Ok(InternalValue::NonInterpolated(
                    serde_json::Value::String(value.clone()),
                )),
                Expr::PathVar(value) => Ok(InternalValue::Interpolated(serde_json::Value::String(
                    value.clone(),
                ))),
                Expr::Concat(values) => {
                    let mut vs: Vec<String> = vec![];
                    for value in values {
                        let v = go(value)?;
                        match v {
                            InternalValue::Interpolated(serde_json::Value::String(v)) => {
                                vs.push(format!("${{{}}}", v));
                            }
                            InternalValue::NonInterpolated(serde_json::Value::String(v)) => {
                                vs.push(v);
                            }
                            _ => return Err("Not supported type".into()),
                        }
                    }
                    Ok(InternalValue::NonInterpolated(serde_json::Value::String(
                        vs.join(""),
                    )))
                }
                Expr::Not(value) => {
                    let v = value.to_json_value()?;
                    match v {
                        serde_json::Value::String(v) => Ok(InternalValue::Interpolated(
                            serde_json::Value::String(format!("!{}", v)),
                        )),
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::GreaterThan(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}>{}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::GreaterThanOrEqualTo(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}>={}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::EqualTo(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}=={}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::LessThan(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}<{}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::LessThanOrEqualTo(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}<={}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::Cond(pred, value1, value2) => {
                    let p = go(pred)?;
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    // FIXME: How did we handle encoding of elseif ?
                    // It's good to remove this encoding as it doesn't give much value add.
                    // once parsed, we can pipe the input directly to the datastore
                    match (p.unwrap(), v1.unwrap(), v2.unwrap()) {
                        (
                            serde_json::Value::String(p),
                            serde_json::Value::String(v1),
                            serde_json::Value::String(v2),
                        ) => Ok(InternalValue::Interpolated(serde_json::Value::String(
                            format!("if ({}) then {} else {}", p, v1, v2),
                        ))),
                        _ => Err("Not supported type".into()),
                    }
                }

                Expr::Number(number) => Ok(InternalValue::Interpolated(serde_json::Value::String(
                    number.to_string(),
                ))),
                Expr::Variable(variable) => Ok(InternalValue::Interpolated(
                    serde_json::Value::String(variable.clone()),
                )),
                Expr::Boolean(boolean) => Ok(InternalValue::Interpolated(
                    serde_json::Value::String(boolean.clone().to_string()),
                )),
                Expr::PatternMatch(condition, match_cases) => {
                    let c = go(condition)?;
                    let mut match_cases_str = vec![];
                    for match_case in match_cases {
                        match_cases_str.push(match_case.to_string());
                    }
                    match c.unwrap() {
                        serde_json::Value::String(c) => Ok(InternalValue::Interpolated(
                            serde_json::Value::String(format!("match {} {{ {} }}", c, match_cases_str.join(", ")),
                        )),
                    ),
                    _ => Err("Not supported type".into()),
                }},
                Expr::Constructor0(constructor) => Ok(InternalValue::NonInterpolated(
                    serde_json::Value::String(constructor.to_string()),
                )),
            }
        }

        let internal_result = go(self)?;

        match internal_result {
            InternalValue::Interpolated(serde_json::Value::String(string)) => {
                Ok(serde_json::Value::String(format!("${{{}}}", string)))
            }
            InternalValue::NonInterpolated(value) => Ok(value),
            _ => Err("Cannot write back the expr".into()),
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

enum InternalValue<T> {
    Interpolated(T),
    NonInterpolated(T),
}

impl<T> InternalValue<T> {
    fn unwrap(&self) -> &T {
        match self {
            InternalValue::Interpolated(value) => value,
            InternalValue::NonInterpolated(value) => value,
        }
    }
}

mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_round_trip_json() {
        let json = json!({
            "name": "John",
            "age": 30,
            "cars": ["Ford", "BMW", "Fiat"],
            "user": "${worker.response}-user"
        });

        let expr = Expr::from_json_value(&json).unwrap();
        let result = expr.to_json_value().unwrap();

        assert_eq!(
            result,
            json
        );
    }

    #[test]
    fn test_expr_to_json_value() {
        let expr = Expr::Record(vec![
            ("name".to_string(), Box::new(Expr::Literal("John".to_string()))),
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
    fn test_expr_to_string() {
        let expr = "${match worker.response { ok(x) => x, err(msg) => y}}";
        let string = Expr::from_primitive_string(expr).unwrap().to_string().unwrap();
        assert_eq!(string, expr);
    }
}
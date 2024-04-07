use std::fmt::Display;
use std::str::FromStr;

use crate::expression;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::parser::expr_parser::ExprParser;
use crate::parser::{GolemParser, ParseError};

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Expr {
    Request(),
    Let(String, Box<Expr>),
    Worker(),
    SelectField(Box<Expr>, String),
    SelectIndex(Box<Expr>, usize),
    Sequence(Vec<Expr>),
    Record(Vec<(String, Box<Expr>)>),
    Tuple(Vec<Expr>),
    Literal(String),
    Number(InnerNumber),
    Flags(Vec<String>),
    Variable(String),
    Boolean(bool),
    Concat(Vec<Expr>),
    Multiple(Vec<Expr>),
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
    pub fn from_str(input: &str) -> Result<Expr, ParseError> {
        let expr_parser = ExprParser {};
        expr_parser.parse(input)
    }

    pub fn to_string(&self) -> Result<String, String> {
        expression::to_string(self).map_err(|x| x.to_string())
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
        match value {
            Value::String(expr_string) => match expression::from_string(expr_string) {
                Ok(expr) => Ok(expr),
                Err(message) => Err(serde::de::Error::custom(message.to_string())),
            },

            e => Err(serde::de::Error::custom(format!(
                "Failed to deserialise expression {}",
                e
            ))),
        }
    }
}

impl Serialize for Expr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match expression::to_string(self) {
            Ok(value) => serde_json::Value::serialize(&Value::String(value), serializer),
            Err(error) => Err(serde::ser::Error::custom(error.to_string())),
        }
    }
}

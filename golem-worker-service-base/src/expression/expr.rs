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
    Let(String, Box<Expr>),
    SelectField(Box<Expr>, String),
    SelectIndex(Box<Expr>, usize),
    Sequence(Vec<Expr>),
    Record(Vec<(String, Box<Expr>)>),
    Tuple(Vec<Expr>),
    Literal(String),
    Number(InnerNumber),
    Flags(Vec<String>),
    Identifier(String), // Upto the evaluator to find from the context what String represents
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
    PatternMatch(Box<Expr>, Vec<MatchArm>),
    Option(Option<Box<Expr>>),
    Result(Result<Box<Expr>, Box<Expr>>),
    Call(String, Vec<Expr>), // Upto the evaluator to find from the context what String represents
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

// Ex: Some(x) => foo
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct MatchArm(pub (ArmPattern, Box<Expr>));

// Ex: Some(x)
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ArmPattern {
    WildCard,
    As(String, Box<ArmPattern>),
    Constructor(ConstructorTypeName, Vec<ArmPattern>),
    Literal(Box<Expr>),
}

impl ArmPattern {
    pub fn from_expr(expr: Expr) -> ArmPattern {
        match expr {
            Expr::Option(Some(expr)) => ArmPattern::Constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::Some),
                vec![ArmPattern::Literal(expr)],
            ),
            Expr::Option(None) => ArmPattern::Constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::None),
                vec![],
            ),
            Expr::Result(Ok(expr)) => ArmPattern::Constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::Ok),
                vec![ArmPattern::Literal(expr)],
            ),
            Expr::Result(Err(expr)) => ArmPattern::Constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::Err),
                vec![ArmPattern::Literal(expr)],
            ),
            // Need to revisit, that we represented wild card with Expr::Empty
            Expr::Multiple(exprs) if exprs.is_empty() => ArmPattern::WildCard,
            _ => ArmPattern::Literal(Box::new(expr)),
        }
    }

    pub fn from(pattern_name: &str, variables: Vec<ArmPattern>) -> Result<ArmPattern, ParseError> {
        if pattern_name == "ok" {
            validate_single_variable_constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::Ok),
                variables,
            )
        } else if pattern_name == "err" {
            validate_single_variable_constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::Err),
                variables,
            )
        } else if pattern_name == "none" {
            validate_empty_constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::None),
                variables,
            )
        } else if pattern_name == "some" {
            validate_single_variable_constructor(
                ConstructorTypeName::InBuiltConstructor(InBuiltConstructorInner::Some),
                variables,
            )
        } else {
            let constructor_type = ConstructorTypeName::CustomConstructor(pattern_name.to_string());
            Ok(ArmPattern::Constructor(constructor_type, variables))
        }
    }
}

fn validate_empty_constructor(
    constructor_type: ConstructorTypeName,
    variables: Vec<ArmPattern>,
) -> Result<ArmPattern, ParseError> {
    if !variables.is_empty() {
        Err(ParseError::Message(
            "constructor should have zero variables".to_string(),
        ))
    } else {
        Ok(ArmPattern::Constructor(constructor_type, variables))
    }
}

fn validate_single_variable_constructor(
    constructor_type: ConstructorTypeName,
    variables: Vec<ArmPattern>,
) -> Result<ArmPattern, ParseError> {
    if variables.len() != 1 {
        Err(ParseError::Message(format!(
            "constructor should have exactly one variable for {}",
            constructor_type
        )))
    } else {
        Ok(ArmPattern::Constructor(constructor_type, variables))
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
}

impl FromStr for Expr {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let expr_parser = ExprParser {};
        expr_parser.parse(s)
    }
}

impl Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", expression::to_string(self).unwrap())
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

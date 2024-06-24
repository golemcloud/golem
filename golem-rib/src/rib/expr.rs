use crate::rib::function_name::ParsedFunctionName;
use bincode::{Decode, Encode};
use std::fmt::Display;

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
    Identifier(String),
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
    Call(ParsedFunctionName, Vec<Expr>),
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
pub struct MatchArm(pub (ArmPattern, Box<Expr>));

// Ex: Some(x)
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ArmPattern {
    WildCard,
    As(String, Box<ArmPattern>),
    Constructor(ConstructorTypeName, Vec<ArmPattern>),
    Literal(Box<Expr>),
}

impl ArmPattern {}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ConstructorTypeName {
    InBuiltConstructor(InBuiltConstructorInner),
    Identifier(String),
}

impl Display for ConstructorTypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstructorTypeName::InBuiltConstructor(inner) => write!(f, "{}", inner),
            ConstructorTypeName::Identifier(name) => write!(f, "{}", name),
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

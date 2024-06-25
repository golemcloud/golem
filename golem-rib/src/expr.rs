use crate::function_name::ParsedFunctionName;
use crate::parser::literal::literal;
use crate::parser::rib_expr::rib_expr;
use bincode::{Decode, Encode};
use combine::easy;
use combine::EasyParser;
use std::fmt::Display;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use crate::text;

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Expr {
    Let(String, Box<Expr>),
    SelectField(Box<Expr>, String),
    SelectIndex(Box<Expr>, usize),
    Sequence(Vec<Expr>),
    Record(Vec<(String, Box<Expr>)>),
    Tuple(Vec<Expr>),
    Literal(String),
    Number(Number),
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

impl Expr {
    /// Parse a text directly as Rib expression
    /// Example of a Rib expression:
    ///
    /// ```rib
    ///   let result = worker.response;
    ///   let error_message = "invalid response from worker";
    ///
    ///   match result {
    ///     some(record) => record,
    ///     none => "Error: ${error_message}"
    ///   }
    /// ```
    ///
    /// Rib supports conditional calls, function calls, pattern-matching,
    /// string interpolation (see error message above) etc.
    ///
    /// You can see an example of string interpolation for the `none` case above.
    pub fn from_str(input: &str) -> Result<Expr, easy::ParseError<&str>> {
        rib_expr().easy_parse(input.as_ref()).map(|(expr, _)| expr)
    }

    /// Parse an interpolated text as Rib expression.
    /// Usually `from_str` is all that you need.
    /// `from_interpolated_str` can be used when you want to be really strict that only if text is wrapped in `${..}`, it should
    /// be considered as a Rib expression.
    ///
    /// Example 1:
    ///
    /// ```rib
    ///   ${
    ///     let result = worker.response;
    ///     let error_message = "invalid response from worker";
    ///
    ///     match result {
    ///       some(record) => record,
    ///       none => "Error: ${error_message}"
    ///     }
    ///   }
    /// ```
    /// You can see the entire text is wrapped in an interpolation to specify that it's containing
    /// a Rib expression and anything outside is considered as a literal string.
    ///
    /// Example 2:
    ///
    /// ```rib
    ///  worker-id-${request.user_id}
    /// ```
    ///
    /// This will be parsed as `Expr::Concat(Expr::Literal("worker-id-"), Expr::SelectField(Expr::Identifier("request"), "user_id"))`
    ///
    /// The following will work as well:
    ///
    /// ```rib
    ///   ${if foo > 1 then bar else "baz-${user.id}"}
    /// ```
    ///
    ///
    pub fn from_interpolated_str(input: &str) -> Result<Expr, easy::ParseError<&str>> {
        literal().easy_parse(input.as_ref()).map(|(expr, _)| expr)
    }
    pub fn unsigned_integer(u64: u64) -> Expr {
        Expr::Number(Number::Unsigned(u64))
    }

    pub fn signed_integer(i64: i64) -> Expr {
        Expr::Number(Number::Signed(i64))
    }

    pub fn float(float: f64) -> Expr {
        Expr::Number(Number::Float(float))
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Number {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
}

impl Display for Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Number::Unsigned(value) => write!(f, "{}", value),
            Number::Signed(value) => write!(f, "{}", value),
            Number::Float(value) => write!(f, "{}", value),
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
    Constructor(String, Vec<ArmPattern>),
    Literal(Box<Expr>),
}


impl Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", text::to_string(self).unwrap())
    }
}

impl<'de> Deserialize<'de> for Expr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            Value::String(expr_string) => match text::from_string(expr_string.as_str()) {
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
        match text::to_string(self) {
            Ok(value) => serde_json::Value::serialize(&Value::String(value), serializer),
            Err(error) => Err(serde::ser::Error::custom(error.to_string())),
        }
    }
}

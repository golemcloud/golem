// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::function_name::ParsedFunctionName;
use crate::parser::rib_expr::rib_program;
use crate::text;
use bincode::{Decode, Encode};
use combine::EasyParser;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::fmt::Display;
use std::str::FromStr;

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
    /// string interpolation (see error_message above) etc.
    ///
    pub fn from_text(input: &str) -> Result<Expr, String> {
        rib_program()
            .easy_parse(input.as_ref())
            .map(|(expr, _)| expr)
            .map_err(|err| err.to_string())
    }

    /// Parse an interpolated text as Rib expression. The input is always expected to be wrapped with `${..}`
    /// This is mainly to keep the backward compatibility where Golem Cloud console passes a Rib Expression always wrapped in `${..}`
    ///
    /// Explanation:
    /// Usually `Expr::from_text` is all that you need which takes a plain text and try to parse it as an Expr.
    /// `from_interpolated_str` can be used when you want to be strict - only if text is wrapped in `${..}`, it should
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
    /// You can see the entire text is wrapped in `${..}` to specify that it's containing
    /// a Rib expression and anything outside is considered as a literal string.
    ///
    /// The advantage of using `from_interpolated_str` is Rib the behaviour is consistent that only those texts
    //  within `${..}` are considered as Rib expressions all the time.
    ///
    /// Example 2:
    ///
    /// ```rib
    ///  worker-id-${request.user_id}
    /// ```
    /// ```rib
    ///   ${"worker-id-${request.user_id}"}
    /// ```
    /// ```rib
    ///   ${request.user_id}
    /// ```
    /// ```rib
    ///   foo-${"worker-id-${request.user_id}"}
    /// ```
    /// etc.
    ///
    /// The first one will be parsed as `Expr::Concat(Expr::Literal("worker-id-"), Expr::SelectField(Expr::Identifier("request"), "user_id"))`.
    ///
    /// The following will work too.
    /// In the below example, the entire if condition is a Rib expression  (because it is wrapped in ${..}) and
    /// the else condition is resolved to  a literal where part of it is a Rib expression itself (user.id).
    ///
    /// ```rib
    ///   ${if foo > 1 then bar else "baz-${user.id}"}
    /// ```
    /// If you need the following to be considered as Rib program (without interpolation), use `Expr::from_text` instead.
    ///
    /// ```rib
    ///   if foo > 1 then bar else "baz-${user.id}"
    /// ```
    ///
    pub fn from_interpolated_str(input: &str) -> Result<Expr, String> {
        let input = format!("\"{}\"", input);
        Self::from_text(input.as_str())
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

impl ArmPattern {
    // Helper to construct ok(v). Cannot be used if there is nested constructors such as ok(some(v)))
    pub fn ok(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Result(Ok(Box::new(Expr::Identifier(
            binding_variable.to_string(),
        ))))))
    }

    // Helper to construct err(v). Cannot be used if there is nested constructors such as err(some(v)))
    pub fn err(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Result(Err(Box::new(Expr::Identifier(
            binding_variable.to_string(),
        ))))))
    }

    // Helper to construct some(v). Cannot be used if there is nested constructors such as some(ok(v)))
    pub fn some(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Option(Some(Box::new(Expr::Identifier(
            binding_variable.to_string(),
        ))))))
    }

    pub fn none() -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Option(None)))
    }

    pub fn identifier(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Identifier(binding_variable.to_string())))
    }
    pub fn custom_constructor(name: &str, args: Vec<ArmPattern>) -> ArmPattern {
        ArmPattern::Constructor(name.to_string(), args)
    }
}

impl Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", text::to_string(self).unwrap())
    }
}

impl FromStr for Expr {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Expr::from_interpolated_str(s).map_err(|err| err.to_string())
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

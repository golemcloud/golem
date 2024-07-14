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

impl TryFrom<golem_api_grpc::proto::golem::rib::Expr> for Expr {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::Expr) -> Result<Self, Self::Error> {
        let expr = value.expr.ok_or("Missing expr")?;

        let expr = match expr {
            golem_api_grpc::proto::golem::rib::expr::Expr::Let(expr) => {
                let name = expr.name;
                let expr = *expr.expr.ok_or("Missing expr")?;
                Expr::Let(name, Box::new(expr.try_into()?))
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Not(expr) => {
                let expr = expr.expr.ok_or("Missing expr")?;
                Expr::Not(Box::new((*expr).try_into()?))
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThan(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::GreaterThan(
                    Box::new((*left).try_into()?),
                    Box::new((*right).try_into()?),
                )
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThanOrEqual(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::GreaterThanOrEqualTo(
                    Box::new((*left).try_into()?),
                    Box::new((*right).try_into()?),
                )
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::LessThan(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::LessThan(
                    Box::new((*left).try_into()?),
                    Box::new((*right).try_into()?),
                )
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::LessThanOrEqual(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::LessThanOrEqualTo(
                    Box::new((*left).try_into()?),
                    Box::new((*right).try_into()?),
                )
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::EqualTo(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::EqualTo(
                    Box::new((*left).try_into()?),
                    Box::new((*right).try_into()?),
                )
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Cond(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let cond = expr.cond.ok_or("Missing cond expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::Cond(
                    Box::new((*left).try_into()?),
                    Box::new((*cond).try_into()?),
                    Box::new((*right).try_into()?),
                )
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Concat(
                golem_api_grpc::proto::golem::rib::ConcatExpr { exprs },
            ) => {
                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::Concat(exprs)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Multiple(
                golem_api_grpc::proto::golem::rib::MultipleExpr { exprs },
            ) => {
                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::Multiple(exprs)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Sequence(
                golem_api_grpc::proto::golem::rib::SequenceExpr { exprs },
            ) => {
                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::Sequence(exprs)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Tuple(
                golem_api_grpc::proto::golem::rib::TupleExpr { exprs },
            ) => {
                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::Tuple(exprs)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Record(
                golem_api_grpc::proto::golem::rib::RecordExpr { fields },
            ) => {
                let mut values: Vec<(String, Box<Expr>)> = vec![];
                for record in fields.into_iter() {
                    let name = record.name;
                    let expr = record.expr.ok_or("Missing expr")?;
                    values.push((name, Box::new(expr.try_into()?)));
                }
                Expr::Record(values)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Flags(
                golem_api_grpc::proto::golem::rib::FlagsExpr { values },
            ) => Expr::Flags(values),
            golem_api_grpc::proto::golem::rib::expr::Expr::Literal(
                golem_api_grpc::proto::golem::rib::LiteralExpr { value },
            ) => Expr::Literal(value),
            golem_api_grpc::proto::golem::rib::expr::Expr::Identifier(
                golem_api_grpc::proto::golem::rib::IdentifierExpr { name },
            ) => Expr::Identifier(name),
            golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
                golem_api_grpc::proto::golem::rib::BooleanExpr { value },
            ) => Expr::Boolean(value),
            golem_api_grpc::proto::golem::rib::expr::Expr::Number(expr) => {
                Expr::Number(expr.try_into()?)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::SelectField(expr) => {
                let expr = *expr;
                let field = expr.field;
                let expr = *expr.expr.ok_or(
                    "Mi\
                ssing expr",
                )?;
                Expr::SelectField(Box::new(expr.try_into()?), field)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndex(expr) => {
                let expr = *expr;
                let index = expr.index as usize;
                let expr = *expr.expr.ok_or("Missing expr")?;
                Expr::SelectIndex(Box::new(expr.try_into()?), index)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Option(expr) => match expr.expr {
                Some(expr) => Expr::Option(Some(Box::new((*expr).try_into()?))),
                None => Expr::Option(None),
            },
            golem_api_grpc::proto::golem::rib::expr::Expr::Result(expr) => {
                let result = expr.result.ok_or("Missing result")?;
                match result {
                    golem_api_grpc::proto::golem::rib::result_expr::Result::Ok(expr) => {
                        Expr::Result(Ok(Box::new((*expr).try_into()?)))
                    }
                    golem_api_grpc::proto::golem::rib::result_expr::Result::Err(expr) => {
                        Expr::Result(Err(Box::new((*expr).try_into()?)))
                    }
                }
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::PatternMatch(expr) => {
                let patterns: Vec<MatchArm> = expr
                    .patterns
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                let expr = expr.expr.ok_or("Missing expr")?;
                Expr::PatternMatch(Box::new((*expr).try_into()?), patterns)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Call(expr) => {
                let params: Vec<Expr> = expr
                    .params
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                let name = expr.name.ok_or("Missing name")?;
                Expr::Call(name.try_into()?, params)
            }
        };
        Ok(expr)
    }
}

impl From<Expr> for golem_api_grpc::proto::golem::rib::Expr {
    fn from(value: Expr) -> Self {
        let expr = match value {
            Expr::Let(name, expr) => golem_api_grpc::proto::golem::rib::expr::Expr::Let(Box::new(
                golem_api_grpc::proto::golem::rib::LetExpr {
                    name,
                    expr: Some(Box::new((*expr).into())),
                },
            )),
            Expr::SelectField(expr, field) => {
                golem_api_grpc::proto::golem::rib::expr::Expr::SelectField(Box::new(
                    golem_api_grpc::proto::golem::rib::SelectFieldExpr {
                        expr: Some(Box::new((*expr).into())),
                        field,
                    },
                ))
            }
            Expr::SelectIndex(expr, index) => {
                golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndex(Box::new(
                    golem_api_grpc::proto::golem::rib::SelectIndexExpr {
                        expr: Some(Box::new((*expr).into())),
                        index: index as u64,
                    },
                ))
            }
            Expr::Sequence(exprs) => golem_api_grpc::proto::golem::rib::expr::Expr::Sequence(
                golem_api_grpc::proto::golem::rib::SequenceExpr {
                    exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                },
            ),
            Expr::Record(fields) => golem_api_grpc::proto::golem::rib::expr::Expr::Record(
                golem_api_grpc::proto::golem::rib::RecordExpr {
                    fields: fields
                        .into_iter()
                        .map(
                            |(name, expr)| golem_api_grpc::proto::golem::rib::RecordFieldExpr {
                                name,
                                expr: Some((*expr).into()),
                            },
                        )
                        .collect(),
                },
            ),
            Expr::Tuple(exprs) => golem_api_grpc::proto::golem::rib::expr::Expr::Tuple(
                golem_api_grpc::proto::golem::rib::TupleExpr {
                    exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                },
            ),
            Expr::Literal(value) => golem_api_grpc::proto::golem::rib::expr::Expr::Literal(
                golem_api_grpc::proto::golem::rib::LiteralExpr { value },
            ),
            Expr::Number(number) => {
                golem_api_grpc::proto::golem::rib::expr::Expr::Number(number.into())
            }
            Expr::Flags(values) => golem_api_grpc::proto::golem::rib::expr::Expr::Flags(
                golem_api_grpc::proto::golem::rib::FlagsExpr { values },
            ),
            Expr::Identifier(name) => golem_api_grpc::proto::golem::rib::expr::Expr::Identifier(
                golem_api_grpc::proto::golem::rib::IdentifierExpr { name },
            ),
            Expr::Boolean(value) => golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
                golem_api_grpc::proto::golem::rib::BooleanExpr { value },
            ),
            Expr::Concat(exprs) => golem_api_grpc::proto::golem::rib::expr::Expr::Concat(
                golem_api_grpc::proto::golem::rib::ConcatExpr {
                    exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                },
            ),
            Expr::Multiple(exprs) => golem_api_grpc::proto::golem::rib::expr::Expr::Multiple(
                golem_api_grpc::proto::golem::rib::MultipleExpr {
                    exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                },
            ),
            Expr::Not(expr) => golem_api_grpc::proto::golem::rib::expr::Expr::Not(Box::new(
                golem_api_grpc::proto::golem::rib::NotExpr {
                    expr: Some(Box::new((*expr).into())),
                },
            )),
            Expr::GreaterThan(left, right) => {
                golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThan(Box::new(
                    golem_api_grpc::proto::golem::rib::GreaterThanExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                ))
            }
            Expr::GreaterThanOrEqualTo(left, right) => {
                golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThanOrEqual(Box::new(
                    golem_api_grpc::proto::golem::rib::GreaterThanOrEqualToExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                ))
            }
            Expr::LessThan(left, right) => golem_api_grpc::proto::golem::rib::expr::Expr::LessThan(
                Box::new(golem_api_grpc::proto::golem::rib::LessThanExpr {
                    left: Some(Box::new((*left).into())),
                    right: Some(Box::new((*right).into())),
                }),
            ),
            Expr::LessThanOrEqualTo(left, right) => {
                golem_api_grpc::proto::golem::rib::expr::Expr::LessThanOrEqual(Box::new(
                    golem_api_grpc::proto::golem::rib::LessThanOrEqualToExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                ))
            }
            Expr::EqualTo(left, right) => golem_api_grpc::proto::golem::rib::expr::Expr::EqualTo(
                Box::new(golem_api_grpc::proto::golem::rib::EqualToExpr {
                    left: Some(Box::new((*left).into())),
                    right: Some(Box::new((*right).into())),
                }),
            ),
            Expr::Cond(left, cond, right) => golem_api_grpc::proto::golem::rib::expr::Expr::Cond(
                Box::new(golem_api_grpc::proto::golem::rib::CondExpr {
                    left: Some(Box::new((*left).into())),
                    cond: Some(Box::new((*cond).into())),
                    right: Some(Box::new((*right).into())),
                }),
            ),
            Expr::PatternMatch(expr, arms) => {
                golem_api_grpc::proto::golem::rib::expr::Expr::PatternMatch(Box::new(
                    golem_api_grpc::proto::golem::rib::PatternMatchExpr {
                        expr: Some(Box::new((*expr).into())),
                        patterns: arms.into_iter().map(|a| a.into()).collect(),
                    },
                ))
            }
            Expr::Option(expr) => golem_api_grpc::proto::golem::rib::expr::Expr::Option(Box::new(
                golem_api_grpc::proto::golem::rib::OptionExpr {
                    expr: expr.map(|expr| Box::new((*expr).into())),
                },
            )),
            Expr::Result(expr) => {
                let result = match expr {
                    Ok(expr) => golem_api_grpc::proto::golem::rib::result_expr::Result::Ok(
                        Box::new((*expr).into()),
                    ),
                    Err(expr) => golem_api_grpc::proto::golem::rib::result_expr::Result::Err(
                        Box::new((*expr).into()),
                    ),
                };

                golem_api_grpc::proto::golem::rib::expr::Expr::Result(Box::new(
                    golem_api_grpc::proto::golem::rib::ResultExpr {
                        result: Some(result),
                    },
                ))
            }
            Expr::Call(function_name, args) => golem_api_grpc::proto::golem::rib::expr::Expr::Call(
                golem_api_grpc::proto::golem::rib::CallExpr {
                    name: Some(function_name.into()),
                    params: args.into_iter().map(|expr| expr.into()).collect(),
                },
            ),
        };

        golem_api_grpc::proto::golem::rib::Expr { expr: Some(expr) }
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

impl TryFrom<golem_api_grpc::proto::golem::rib::NumberExpr> for Number {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::NumberExpr) -> Result<Self, Self::Error> {
        let number = value.number.ok_or("Missing number")?;
        match number {
            golem_api_grpc::proto::golem::rib::number_expr::Number::Unsigned(value) => {
                Ok(Number::Unsigned(value))
            }
            golem_api_grpc::proto::golem::rib::number_expr::Number::Signed(value) => {
                Ok(Number::Signed(value))
            }
            golem_api_grpc::proto::golem::rib::number_expr::Number::Float(value) => {
                Ok(Number::Float(value))
            }
        }
    }
}

impl From<Number> for golem_api_grpc::proto::golem::rib::NumberExpr {
    fn from(value: Number) -> Self {
        golem_api_grpc::proto::golem::rib::NumberExpr {
            number: Some(match value {
                Number::Unsigned(value) => {
                    golem_api_grpc::proto::golem::rib::number_expr::Number::Unsigned(value)
                }
                Number::Signed(value) => {
                    golem_api_grpc::proto::golem::rib::number_expr::Number::Signed(value)
                }
                Number::Float(value) => {
                    golem_api_grpc::proto::golem::rib::number_expr::Number::Float(value)
                }
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct MatchArm(pub (ArmPattern, Box<Expr>));

impl TryFrom<golem_api_grpc::proto::golem::rib::MatchArm> for MatchArm {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::MatchArm) -> Result<Self, Self::Error> {
        let pattern = value.pattern.ok_or("Missing pattern")?;
        let expr = value.expr.ok_or("Missing expr")?;
        Ok(MatchArm((pattern.try_into()?, Box::new(expr.try_into()?))))
    }
}

impl From<MatchArm> for golem_api_grpc::proto::golem::rib::MatchArm {
    fn from(value: MatchArm) -> Self {
        let (pattern, expr) = value.0;
        golem_api_grpc::proto::golem::rib::MatchArm {
            pattern: Some(pattern.into()),
            expr: Some((*expr).into()),
        }
    }
}

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

impl TryFrom<golem_api_grpc::proto::golem::rib::ArmPattern> for ArmPattern {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::ArmPattern) -> Result<Self, Self::Error> {
        let pattern = value.pattern.ok_or("Missing pattern")?;
        match pattern {
            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::WildCard(_) => {
                Ok(ArmPattern::WildCard)
            }
            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::As(asp) => {
                let name = asp.name;
                let pattern = asp.pattern.ok_or("Missing pattern")?;
                Ok(ArmPattern::As(name, Box::new((*pattern).try_into()?)))
            }
            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::Constructor(
                golem_api_grpc::proto::golem::rib::ConstructorArmPattern { name, patterns },
            ) => {
                let patterns = patterns
                    .into_iter()
                    .map(ArmPattern::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ArmPattern::Constructor(name, patterns))
            }
            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::Literal(
                golem_api_grpc::proto::golem::rib::LiteralArmPattern { expr },
            ) => {
                let inner = expr.ok_or("Missing expr")?;
                Ok(ArmPattern::Literal(Box::new(inner.try_into()?)))
            }
        }
    }
}

impl From<ArmPattern> for golem_api_grpc::proto::golem::rib::ArmPattern {
    fn from(value: ArmPattern) -> Self {
        match value {
            ArmPattern::WildCard => golem_api_grpc::proto::golem::rib::ArmPattern {
                pattern: Some(
                    golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::WildCard(
                        golem_api_grpc::proto::golem::rib::WildCardArmPattern {},
                    ),
                ),
            },
            ArmPattern::As(name, pattern) => golem_api_grpc::proto::golem::rib::ArmPattern {
                pattern: Some(golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::As(
                    Box::new(golem_api_grpc::proto::golem::rib::AsArmPattern {
                        name,
                        pattern: Some(Box::new((*pattern).into())),
                    }),
                )),
            },
            ArmPattern::Constructor(name, patterns) => {
                golem_api_grpc::proto::golem::rib::ArmPattern {
                    pattern: Some(
                        golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::Constructor(
                            golem_api_grpc::proto::golem::rib::ConstructorArmPattern {
                                name,
                                patterns: patterns
                                    .into_iter()
                                    .map(golem_api_grpc::proto::golem::rib::ArmPattern::from)
                                    .collect(),
                            },
                        ),
                    ),
                }
            }
            ArmPattern::Literal(expr) => golem_api_grpc::proto::golem::rib::ArmPattern {
                pattern: Some(
                    golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::Literal(
                        golem_api_grpc::proto::golem::rib::LiteralArmPattern {
                            expr: Some((*expr).into()),
                        },
                    ),
                ),
            },
        }
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

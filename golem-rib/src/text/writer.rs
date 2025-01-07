// Copyright 2024-2025 Golem Cloud
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

use crate::expr::Expr;
use crate::{ArmPattern, MatchArm};
use std::fmt::Display;
use std::io::Write;

pub fn write_expr(expr: &Expr) -> Result<String, WriterError> {
    let mut buf = vec![];
    let mut writer = Writer::new(&mut buf);

    writer.write_expr(expr)?;

    String::from_utf8(buf)
        .map_err(|err| WriterError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err)))
}

pub fn write_arm_pattern(arm_pattern: &ArmPattern) -> Result<String, WriterError> {
    let mut buf = vec![];
    let mut writer = Writer::new(&mut buf);

    internal::write_arm_pattern(arm_pattern, &mut writer)?;

    String::from_utf8(buf)
        .map_err(|err| WriterError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err)))
}

struct Writer<W> {
    inner: W,
}

#[derive(Debug)]
pub enum WriterError {
    Io(std::io::Error),
}

impl From<std::io::Error> for WriterError {
    fn from(err: std::io::Error) -> Self {
        WriterError::Io(err)
    }
}

impl Display for WriterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriterError::Io(err) => write!(f, "IO error: {err}"),
        }
    }
}

impl<W: Write> Writer<W> {
    fn new(w: W) -> Self {
        Self { inner: w }
    }

    fn write_code_start(&mut self) -> Result<(), WriterError> {
        self.write_display("${")
    }

    fn write_code_end(&mut self) -> Result<(), WriterError> {
        self.write_display("}")
    }

    fn write_expr(&mut self, expr: &Expr) -> Result<(), WriterError> {
        match expr {
            Expr::Literal(string, _) => {
                self.write_display("\"")?;
                self.write_str(string)?;
                self.write_display("\"")
            }
            Expr::Identifier(identifier, _) => self.write_str(identifier.name()),

            Expr::Let(variable_id, type_name, expr, _) => {
                self.write_str("let ")?;
                self.write_str(variable_id.name())?;
                if let Some(type_name) = type_name {
                    self.write_str(": ")?;
                    self.write_display(type_name)?;
                };
                self.write_str(" = ")?;
                self.write_expr(expr)
            }
            Expr::SelectField(expr, field_name, _) => {
                self.write_expr(expr)?;
                self.write_str(".")?;
                self.write_str(field_name)
            }
            Expr::SelectIndex(expr, index, _) => {
                self.write_expr(expr)?;
                self.write_display("[")?;
                self.write_display(index)?;
                self.write_display("]")
            }
            Expr::Sequence(sequence, _) => {
                self.write_display("[")?;
                for (idx, expr) in sequence.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(",")?;
                        self.write_display(" ")?;
                    }
                    self.write_expr(expr)?;
                }
                self.write_display("]")
            }
            Expr::Record(record, _) => {
                self.write_display("{")?;
                for (idx, (key, value)) in record.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(",")?;
                        self.write_display(" ")?;
                    }
                    self.write_str(key)?;
                    self.write_display(":")?;
                    self.write_display(" ")?;
                    self.write_expr(value)?;
                }
                self.write_display("}")
            }
            Expr::Tuple(tuple, _) => {
                self.write_display("(")?;
                for (idx, expr) in tuple.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(",")?;
                        self.write_display(" ")?;
                    }
                    self.write_expr(expr)?;
                }
                self.write_display(")")
            }
            Expr::Number(number, type_name, _) => {
                self.write_display(number.value.to_string())?;
                if let Some(type_name) = type_name {
                    self.write_display(type_name)?;
                }
                Ok(())
            }
            Expr::Flags(flags, _) => {
                self.write_display("{")?;
                for (idx, flag) in flags.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(",")?;
                        self.write_display(" ")?;
                    }
                    self.write_str(flag)?;
                }
                self.write_display("}")
            }
            Expr::Boolean(bool, _) => self.write_display(bool),
            Expr::Concat(concatenated, _) => {
                self.write_display("\"")?;
                internal::write_concatenated_exprs(self, concatenated)?;
                self.write_display("\"")
            }
            Expr::ExprBlock(expr, _) => {
                for (idx, expr) in expr.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(";")?;
                        self.write_display("\n")?;
                    }
                    self.write_expr(expr)?;
                }
                Ok(())
            }
            Expr::Not(expr, _) => {
                self.write_str("!")?;
                self.write_expr(expr)
            }
            Expr::GreaterThan(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" > ")?;
                self.write_expr(right)
            }
            Expr::Plus(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" + ")?;
                self.write_expr(right)
            }
            Expr::Minus(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" - ")?;
                self.write_expr(right)
            }
            Expr::Divide(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" / ")?;
                self.write_expr(right)
            }
            Expr::Multiply(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" * ")?;
                self.write_expr(right)
            }
            Expr::GreaterThanOrEqualTo(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" >= ")?;
                self.write_expr(right)
            }
            Expr::LessThanOrEqualTo(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" <= ")?;
                self.write_expr(right)
            }
            Expr::EqualTo(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" == ")?;
                self.write_expr(right)
            }
            Expr::LessThan(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" < ")?;
                self.write_expr(right)
            }
            Expr::Cond(if_expr, left, right, _) => {
                self.write_str("if ")?;
                self.write_expr(if_expr)?;
                self.write_str(" then ")?;
                self.write_expr(left)?;
                self.write_str(" else ")?;
                self.write_expr(right)
            }
            Expr::PatternMatch(match_expr, match_terms, _) => {
                self.write_str("match ")?;
                self.write_expr(match_expr)?;
                self.write_str(" { ")?;
                self.write_display(" ")?;
                for (idx, match_term) in match_terms.iter().enumerate() {
                    if idx != 0 {
                        self.write_str(", ")?;
                    }
                    let MatchArm {
                        arm_pattern,
                        arm_resolution_expr,
                    } = &match_term;
                    internal::write_arm_pattern(arm_pattern, self)?;
                    self.write_str(" => ")?;
                    self.write_expr(arm_resolution_expr)?;
                }
                self.write_str(" } ")
            }
            Expr::Option(constructor, _) => match constructor {
                Some(expr) => {
                    self.write_str("some(")?;
                    self.write_expr(expr)?;
                    self.write_str(")")
                }
                None => self.write_str("none"),
            },
            Expr::Result(constructor, _) => match constructor {
                Ok(expr) => {
                    self.write_str("ok(")?;
                    self.write_expr(expr)?;
                    self.write_str(")")
                }
                Err(expr) => {
                    self.write_str("err(")?;
                    self.write_expr(expr)?;
                    self.write_str(")")
                }
            },

            Expr::Call(invocation_name, params, _) => {
                let function_name = invocation_name.to_string();

                self.write_str(function_name)?;
                self.write_display("(")?;
                for (idx, param) in params.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(",")?;
                        self.write_display(" ")?;
                    }
                    self.write_expr(param)?;
                }
                self.write_display(")")
            }

            Expr::Unwrap(expr, _) => {
                self.write_str("unwrap(")?;
                self.write_expr(expr)?;
                self.write_str(")")
            }

            Expr::Throw(msg, _) => {
                self.write_str("throw(")?;
                self.write_str(msg)?;
                self.write_str(")")
            }
            Expr::GetTag(expr, _) => {
                self.write_str("get_tag(")?;
                self.write_expr(expr)?;
                self.write_str(")")
            }
            Expr::And(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" && ")?;
                self.write_expr(right)
            }
            Expr::Or(left, right, _) => {
                self.write_expr(left)?;
                self.write_str(" || ")?;
                self.write_expr(right)
            }
            Expr::ListComprehension {
                iterated_variable,
                iterable_expr,
                yield_expr,
                ..
            } => {
                self.write_display("for")?;
                self.write_display(iterated_variable.to_string())?;
                self.write_display(" in ")?;
                self.write_expr(iterable_expr)?;
                self.write_display(" { ")?;
                self.write_display("\n")?;
                internal::write_yield_block(self, yield_expr)?;
                self.write_display(";")?;
                self.write_display(" } ")
            }

            Expr::ListReduce {
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
                ..
            } => {
                self.write_display("reduce ")?;
                self.write_display(reduce_variable.to_string())?;
                self.write_display(", ")?;
                self.write_display(iterated_variable.to_string())?;
                self.write_display(" in ")?;
                self.write_expr(iterable_expr)?;
                self.write_display(" from ")?;
                self.write_expr(init_value_expr)?;
                self.write_display(" { ")?;
                self.write_display("\n")?;
                internal::write_yield_block(self, yield_expr)?;
                self.write_display(" } ")
            }
        }
    }

    fn write_str(&mut self, s: impl AsRef<str>) -> Result<(), WriterError> {
        self.inner.write_all(s.as_ref().as_bytes())?;
        Ok(())
    }

    fn write_display(&mut self, d: impl std::fmt::Display) -> Result<(), WriterError> {
        write!(self.inner, "{d}")?;
        Ok(())
    }
}

mod internal {
    use crate::expr::{ArmPattern, Expr};
    use crate::text::writer::{Writer, WriterError};

    pub(crate) enum ExprType<'a> {
        Code(&'a Expr),
        Text(&'a str),
        StringInterpolated,
    }

    pub(crate) fn write_yield_block<W>(
        writer: &mut Writer<W>,
        expr: &Expr,
    ) -> Result<(), WriterError>
    where
        W: std::io::Write,
    {
        if let Expr::ExprBlock(yield_lines, _) = expr {
            let last_line_index = yield_lines.len() - 1;

            for (index, line) in yield_lines.iter().enumerate() {
                if index == last_line_index {
                    writer.write_display("yield ")?;
                    writer.write_expr(line)?;
                } else {
                    writer.write_expr(line)?;
                }

                writer.write_display("\n")?;
            }

            Ok(())
        } else {
            writer.write_display("yield ")?;
            writer.write_expr(expr)
        }
    }

    pub(crate) fn get_expr_type(expr: &Expr) -> ExprType {
        match expr {
            Expr::Literal(str, _) => ExprType::Text(str),
            Expr::Concat(_, _) => ExprType::StringInterpolated,
            expr => ExprType::Code(expr),
        }
    }

    // Only to make sure that we are not wrapping literals with quotes - intercepting
    // the logic within the writer for ExprType::Code
    pub(crate) fn write_concatenated_exprs<W>(
        writer: &mut Writer<W>,
        exprs: &[Expr],
    ) -> Result<(), WriterError>
    where
        W: std::io::Write,
    {
        for expr in exprs.iter() {
            match get_expr_type(expr) {
                ExprType::Text(text) => {
                    writer.write_str(text)?;
                }
                ExprType::Code(expr) => {
                    writer.write_code_start()?;
                    writer.write_expr(expr)?;
                    writer.write_code_end()?;
                }
                ExprType::StringInterpolated => {
                    writer.write_code_start()?;
                    writer.write_expr(expr)?;
                    writer.write_code_end()?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn write_arm_pattern<W>(
        match_case: &ArmPattern,
        writer: &mut Writer<W>,
    ) -> Result<(), WriterError>
    where
        W: std::io::Write,
    {
        match match_case {
            ArmPattern::WildCard => writer.write_str("_"),
            ArmPattern::As(name, pattern) => {
                writer.write_str(name)?;
                writer.write_str(" @ ")?;
                write_arm_pattern(pattern, writer)
            }
            ArmPattern::Constructor(constructor_type, variables) => {
                if !variables.is_empty() {
                    writer.write_display(constructor_type)?;

                    writer.write_str("(")?;

                    for (idx, pattern) in variables.iter().enumerate() {
                        if idx != 0 {
                            writer.write_str(",")?;
                        }
                        write_arm_pattern(pattern, writer)?;
                    }

                    writer.write_str(")")
                } else {
                    writer.write_display(constructor_type)
                }
            }

            ArmPattern::TupleConstructor(variables) => {
                writer.write_str("(")?;

                for (idx, pattern) in variables.iter().enumerate() {
                    if idx != 0 {
                        writer.write_str(",")?;
                    }
                    write_arm_pattern(pattern, writer)?;
                }

                writer.write_str(")")
            }

            ArmPattern::ListConstructor(patterns) => {
                writer.write_str("[")?;

                for (idx, pattern) in patterns.iter().enumerate() {
                    if idx != 0 {
                        writer.write_str(",")?;
                    }
                    write_arm_pattern(pattern, writer)?;
                }

                writer.write_str("]")
            }

            ArmPattern::RecordConstructor(fields) => {
                writer.write_str("{")?;

                for (idx, (key, value)) in fields.iter().enumerate() {
                    if idx != 0 {
                        writer.write_str(",")?;
                    }
                    writer.write_str(key)?;
                    writer.write_str(":")?;
                    write_arm_pattern(value, writer)?;
                }

                writer.write_str("}")
            }

            ArmPattern::Literal(expr) => match *expr.clone() {
                Expr::Identifier(s, _) => writer.write_str(s.name()),
                any_expr => writer.write_expr(&any_expr),
            },
        }
    }
}

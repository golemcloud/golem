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

use bigdecimal::BigDecimal;
use combine::parser::char;
use combine::parser::char::{char, digit, spaces};
use combine::{attempt, choice, many, many1, optional, parser, position, Stream};
use combine::{ParseError, Parser};
use std::str::FromStr;

use super::binary_op::{binary_op, BinaryOp};
use crate::expr::Expr;
use crate::parser::boolean::boolean_literal;
use crate::parser::call::call;
use crate::parser::cond::conditional;
use crate::parser::errors::RibParseError;
use crate::parser::flag::flag;
use crate::parser::identifier::identifier;
use crate::parser::integer::integer;
use crate::parser::let_binding::let_binding;
use crate::parser::literal::literal;
use crate::parser::multi_line_code_block::multi_line_block;
use crate::parser::not::not;
use crate::parser::optional::option;
use crate::parser::pattern_match::pattern_match;
use crate::parser::range_type::{range_type, RangeType};
use crate::parser::sequence::sequence;
use crate::parser::tuple::tuple;
use crate::rib_source_span::{GetSourcePosition, SourceSpan};

use crate::parser::list_aggregation::list_aggregation;
use crate::parser::list_comprehension::list_comprehension;
use crate::parser::record::record;
use crate::parser::result::result;
use crate::parser::type_name::type_name;
use crate::TypeName;

// A rib expression := (simple_expr, rib_expr_rest*)
// A simple_expr never has any expression that starts with rib_expression
// (ex: select_field, select_index, +, -, *,/, etc)
parser! {
    pub fn rib_expr[Input]()(Input) -> Expr
    where [Input: combine::Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>, Input::Position: GetSourcePosition]
    {
       with_position(rib_expr_())
    }
}

pub fn rib_expr_<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    spaces()
        .with(
            (simple_expr(), rib_expr_rest()).and_then(|(expr, rest): (Expr, RibRest)| {
                let with_index = index_expr(expr, rest.indices);

                let with_field =
                    rest.field_selection
                        .into_iter()
                        .fold(with_index, |acc, field_expr| match field_expr.base {
                            FractionOrExpr::Expr(Expr::Call {
                                call_type,
                                generic_type_parameter,
                                args,
                                ..
                            }) => {
                                let base = Expr::invoke_worker_function(
                                    acc,
                                    call_type.function_name().unwrap().to_string(),
                                    generic_type_parameter,
                                    args,
                                );
                                index_expr(base, field_expr.index_expr)
                                    .with_type_annotation_opt(field_expr.type_name)
                            }

                            FractionOrExpr::Expr(expr) => {
                                let selection = build_selection(acc, expr).unwrap();
                                index_expr(selection, field_expr.index_expr)
                                    .with_type_annotation_opt(field_expr.type_name)
                            }

                            FractionOrExpr::Fraction(fraction) => match acc {
                                Expr::Number { number, .. } => {
                                    let combined = fraction
                                        .combine_with_integer(number.value)
                                        .map_err(RibParseError::Message)
                                        .unwrap();

                                    match field_expr.type_name {
                                        Some(type_name) => {
                                            Expr::untyped_number_with_type_name(combined, type_name)
                                        }
                                        None => Expr::untyped_number(combined),
                                    }
                                }

                                _ => {
                                    panic!("Fraction can only be applied to numbers")
                                }
                            },
                        });

                let with_range = match rest.range_info {
                    Some(range_info) => match range_info.base.expr {
                        Some(rhs) => match range_info.base.range_type {
                            RangeType::Inclusive => index_expr(
                                Expr::range_inclusive(with_field, rhs),
                                range_info.index_expr,
                            ),
                            RangeType::Exclusive => {
                                index_expr(Expr::range(with_field, rhs), range_info.index_expr)
                            }
                        },
                        None => match range_info.base.range_type {
                            RangeType::Inclusive => {
                                return Err(RibParseError::Message(
                                    "Exclusive range should have a right hand side".to_string(),
                                ))
                            }
                            RangeType::Exclusive => {
                                index_expr(Expr::range_from(with_field), range_info.index_expr)
                            }
                        },
                    },
                    None => with_field,
                };

                let with_binary =
                    rest.binary_ops
                        .into_iter()
                        .fold(with_range, |acc, (op, next)| {
                            let next = index_expr(next.base, next.index_expr);

                            match op {
                                BinaryOp::GreaterThan => Expr::greater_than(acc, next),
                                BinaryOp::LessThan => Expr::less_than(acc, next),
                                BinaryOp::LessThanOrEqualTo => {
                                    Expr::less_than_or_equal_to(acc, next)
                                }
                                BinaryOp::GreaterThanOrEqualTo => {
                                    Expr::greater_than_or_equal_to(acc, next)
                                }
                                BinaryOp::EqualTo => Expr::equal_to(acc, next),
                                BinaryOp::And => Expr::and(acc, next),
                                BinaryOp::Or => Expr::or(acc, next),
                                BinaryOp::Add => Expr::plus(acc, next),
                                BinaryOp::Subtract => Expr::minus(acc, next),
                                BinaryOp::Multiply => Expr::multiply(acc, next),
                                BinaryOp::Divide => Expr::divide(acc, next),
                            }
                        });

                Ok(with_binary)
            }),
        )
        .skip(spaces())
}

parser! {
    fn simple_expr[Input]()(Input) -> Expr
    where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>, Input::Position: GetSourcePosition]
    {
        with_position(simple_expr_())
    }
}

fn simple_expr_<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    (
        spaces()
            .with(choice((
                list_comprehension(),
                list_aggregation(),
                pattern_match(),
                let_binding(),
                conditional(),
                attempt(flag_or_record()),
                multi_line_block(),
                tuple(),
                boolean_literal(),
                literal(),
                not(),
                option(),
                result(),
                attempt(call()),
                sequence(),
                identifier(),
                integer(),
            )))
            .skip(spaces()),
        optional(optional(char(':').skip(spaces())).with(type_name())).skip(spaces()),
    )
        .map(|(expr, type_name)| match type_name {
            Some(type_name) => expr.with_type_annotation(type_name),
            None => expr,
        })
}

fn with_position<Input>(
    parser: impl Parser<Input, Output = Expr>,
) -> impl Parser<Input, Output = Expr>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    (position(), parser, position()).map(|(start, expr, end)| {
        let start_pos: Input::Position = start;
        let start = start_pos.get_source_position();
        let end_pos: Input::Position = end;
        let end = end_pos.get_source_position();
        let span = SourceSpan::new(start, end);
        expr.with_source_span(span)
    })
}

fn flag_or_record<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    choice((attempt(flag()), attempt(record()))).message("Unable to parse flag or record")
}

struct RibRest {
    indices: IndexExpr,
    field_selection: Vec<WithIndex<FractionOrExpr>>,
    range_info: Option<WithIndex<RangeInfo>>,
    binary_ops: Vec<(BinaryOp, WithIndex<Expr>)>,
}

#[derive(Debug, Clone)]
struct IndexExpr {
    exprs: Vec<Expr>,
}

struct WithIndex<T> {
    index_expr: IndexExpr,
    base: T,
    type_name: Option<TypeName>,
}

#[derive(Clone, Debug)]
struct RangeInfo {
    range_type: RangeType,
    expr: Option<Expr>,
}

impl RangeInfo {
    pub fn new(range_type: RangeType, expr: Option<Expr>) -> Self {
        Self { range_type, expr }
    }
}

enum FractionOrExpr {
    Fraction(Fraction),
    Expr(Expr),
}

fn rib_expr_rest_<Input>() -> impl Parser<Input, Output = RibRest>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    attempt(
        (
            select_index_expression().skip(spaces()),
            many(attempt(
                char('.').skip(spaces()).with(
                    (
                        fraction()
                            .map(FractionOrExpr::Fraction)
                            .or(simple_expr().skip(spaces()).map(FractionOrExpr::Expr)),
                        select_index_expression().skip(spaces()),
                        optional(type_annotation()).skip(spaces()),
                    )
                        .map(|(field_expr, indices, type_name)| WithIndex {
                            index_expr: indices,
                            base: field_expr,
                            type_name,
                        }),
                ),
            )),
            optional(
                (
                    range_rest(),
                    select_index_expression().skip(spaces()),
                    optional(type_annotation()),
                )
                    .map(|(range_info, indices, type_name)| WithIndex {
                        index_expr: indices,
                        base: range_info,
                        type_name,
                    })
                    .skip(spaces()),
            ),
            many((
                binary_op(),
                (
                    rib_expr().skip(spaces()),
                    select_index_expression().skip(spaces()),
                ),
            ))
            .map(|binary_math: Vec<(BinaryOp, (Expr, IndexExpr))>| {
                binary_math
                    .into_iter()
                    .map(|(op, (expr, index_expr))| {
                        (
                            op,
                            WithIndex {
                                index_expr,
                                base: expr,
                                type_name: None,
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
            .map(
                |(indices, field_selection, range_info, binary_ops)| RibRest {
                    indices,
                    field_selection,
                    range_info,
                    binary_ops,
                },
            ),
    )
}

parser! {
    fn rib_expr_rest[Input]()(Input) -> RibRest
    where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>, Input::Position: GetSourcePosition]
    {
        rib_expr_rest_()
    }
}

// Recursively build the selection accumulating to the LHS
fn build_selection(base: Expr, next: Expr) -> Result<Expr, RibParseError> {
    match next {
        Expr::Identifier {
            variable_id,
            type_annotation,
            ..
        } => Ok(Expr::select_field(
            base,
            variable_id.name().as_str(),
            type_annotation,
        )),
        Expr::SelectField {
            expr: second,
            field: last,
            type_annotation: type_name,
            inferred_type,
            source_span,
        } => {
            let inner_select = build_selection(base, *second)?;
            Ok(Expr::SelectField {
                expr: Box::new(inner_select),
                field: last,
                type_annotation: type_name,
                inferred_type,
                source_span,
            })
        }
        Expr::SelectIndex {
            expr: second,
            index: last_index,
            type_annotation: type_name,
            inferred_type,
            source_span,
        } => {
            let inner_select = build_selection(base, *second)?;
            Ok(Expr::SelectIndex {
                expr: Box::new(inner_select),
                index: last_index,
                type_annotation: type_name,
                inferred_type,
                source_span,
            })
        }
        Expr::SelectDynamic {
            expr: second,
            index: last_index,
            type_annotation: type_name,
            inferred_type,
            source_span,
        } => {
            let inner_select = build_selection(base, *second)?;
            Ok(Expr::SelectDynamic {
                expr: Box::new(inner_select),
                index: last_index,
                type_annotation: type_name,
                inferred_type,
                source_span,
            })
        }
        expr => Err(RibParseError::Message(format!(
            "unable to select field from expression: {:?}",
            expr
        ))),
    }
}

// This is anything that comes after a simple avoiding left recursion
fn range_rest<Input>() -> impl Parser<Input, Output = RangeInfo>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    (range_type().skip(spaces()), optional(simple_expr_())).map(|(range_type, expr)| match expr {
        Some(expr) => RangeInfo::new(range_type, Some(expr)),
        None => RangeInfo::new(range_type, None),
    })
}

fn select_index_expression<Input>() -> impl Parser<Input, Output = IndexExpr>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    many((char('['), rib_expr(), char(']').skip(spaces()))).map(
        |collections: Vec<(char, Expr, char)>| IndexExpr {
            exprs: collections
                .into_iter()
                .map(|(_, index_or_range, _)| index_or_range)
                .collect::<Vec<_>>(),
        },
    )
}

fn type_annotation<Input>() -> impl Parser<Input, Output = TypeName>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    optional(char(':').skip(spaces())).with(type_name())
}

pub(crate) struct Fraction(String);

impl Fraction {
    pub fn combine_with_integer(&self, big: BigDecimal) -> Result<BigDecimal, String> {
        let left = big.to_string();
        let right = self.0.to_string();
        let result = format!("{}.{}", left, right);
        BigDecimal::from_str(&result).map_err(|e| e.to_string())
    }
}

/// Represents an optional exponent part of a number.
/// - `char` → The exponent marker (`e` or `E`).
/// - `Option<char>` → An optional sign (`+` or `-`).
/// - `Vec<char>` → The sequence of digits forming the exponent value.
type Exponent = (char, Option<char>, Vec<char>);

fn fraction<Input>() -> impl Parser<Input, Output = Fraction>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    (
        many1(digit()),
        optional((
            char('e').or(char('E')),
            optional(char('+').or(char('-'))),
            many1(digit()),
        )),
    )
        .map(
            |(fraction_part, exponent_opt): (Vec<char>, Option<Exponent>)| {
                let fraction_str = fraction_part.into_iter().collect::<String>();
                match exponent_opt {
                    Some((exp_marker, sign_opt, exponent_digits)) => {
                        let exponent_str = exponent_digits.into_iter().collect::<String>();
                        Fraction(format!(
                            "{}{}{}{}",
                            fraction_str,
                            exp_marker,
                            sign_opt.unwrap_or_default(),
                            exponent_str
                        ))
                    }
                    None => Fraction(fraction_str),
                }
            },
        )
}

fn index_expr(base_expr: Expr, index_expr: IndexExpr) -> Expr {
    index_expr
        .exprs
        .into_iter()
        .fold(base_expr, |acc, index_expr| {
            Expr::select_dynamic(acc, index_expr, None)
        })
}

#[cfg(test)]
mod tests {
    use crate::generic_type_parameter::GenericTypeParameter;
    use crate::{ArmPattern, DynamicParsedFunctionName, Expr, MatchArm, TypeName};
    use bigdecimal::BigDecimal;
    use std::str::FromStr;
    use test_r::test;

    #[test]
    fn test_float_1() {
        let input = "1.234";
        let result = Expr::from_text(input);
        let expected = Expr::untyped_number(BigDecimal::from_str("1.234").unwrap());
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_float_2() {
        let input = "-1.234";
        let result = Expr::from_text(input);
        let expected = Expr::untyped_number(BigDecimal::from_str("-1.234").unwrap());
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_float_3() {
        let input = "6.022e+23";
        let result = Expr::from_text(input);
        let expected = Expr::untyped_number(BigDecimal::from_str("6.022e+23").unwrap());
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_float_4() {
        let input = "6.022e-23";
        let result = Expr::from_text(input);
        let expected = Expr::untyped_number(BigDecimal::from_str("6.022e-23").unwrap());
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_float_5() {
        let input = "6.022e-23:f32";
        let result = Expr::from_text(input);
        let expected = Expr::untyped_number_with_type_name(
            BigDecimal::from_str("6.022e-23").unwrap(),
            TypeName::F32,
        );
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_select_index_1() {
        let input = "foo[0]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::identifier_global("foo", None),
                Expr::untyped_number(BigDecimal::from(0)),
                None
            ))
        );
    }

    #[test]
    fn test_select_index_2() {
        let input = "foo[0][1]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::select_dynamic(
                    Expr::identifier_global("foo", None),
                    Expr::untyped_number(BigDecimal::from(0)),
                    None
                ),
                Expr::untyped_number(BigDecimal::from(1)),
                None
            ))
        );
    }

    #[test]
    fn test_select_index_3() {
        let input = "foo[bar]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None),
                None
            ))
        );
    }

    #[test]
    fn test_select_index_4() {
        let input = "foo[1..2]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::identifier_global("foo", None),
                Expr::range(
                    Expr::untyped_number(BigDecimal::from(1)),
                    Expr::untyped_number(BigDecimal::from(2))
                ),
                None
            ))
        );
    }

    #[test]
    fn test_select_field_1() {
        let input = "foo.bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::identifier_global("foo", None),
                "bar",
                None
            ))
        );
    }

    #[test]
    fn test_select_field_2() {
        let input = "foo.bar: u32";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::identifier_global("foo", None),
                "bar",
                Some(TypeName::U32)
            ))
        );
    }

    #[test]
    fn test_select_field_3() {
        let input = "{foo: bar}.foo";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::record(vec![(
                    "foo".to_string(),
                    Expr::identifier_global("bar", None)
                )]),
                "foo",
                None
            ))
        );
    }

    #[test]
    fn test_select_field_4() {
        let input = "{foo: bar}.foo: u32";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::record(vec![(
                    "foo".to_string(),
                    Expr::identifier_global("bar", None)
                )]),
                "foo",
                Some(TypeName::U32)
            ))
        );
    }

    #[test]
    fn test_select_field_5() {
        let input = "foo.bar.baz";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                "baz",
                None
            ))
        );
    }

    #[test]
    fn test_select_field_6() {
        let input = "foo.bar.baz: u32";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                "baz",
                Some(TypeName::U32)
            ))
        );
    }

    #[test]
    fn test_select_field_7() {
        let input = "foo[0].bar[1]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::select_field(
                    Expr::select_dynamic(
                        Expr::identifier_global("foo", None),
                        Expr::untyped_number(BigDecimal::from(0)),
                        None
                    ),
                    "bar",
                    None
                ),
                Expr::untyped_number(BigDecimal::from(1)),
                None
            ))
        );
    }

    #[test]
    fn test_select_field_8() {
        let input = "foo[0].bar[1]: u32";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::select_field(
                    Expr::select_dynamic(
                        Expr::identifier_global("foo", None),
                        Expr::untyped_number(BigDecimal::from(0)),
                        None
                    ),
                    "bar",
                    None
                ),
                Expr::untyped_number(BigDecimal::from(1)),
                Some(TypeName::U32)
            ))
        );
    }

    #[test]
    fn test_select_field_9() {
        let input = "foo.bar[0].baz";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::select_dynamic(
                    Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                    Expr::untyped_number(BigDecimal::from(0)),
                    None
                ),
                "baz",
                None
            ))
        );
    }

    #[test]
    fn test_select_field_10() {
        let result = Expr::from_text("foo.bar > \"bar\"");
        assert_eq!(
            result,
            Ok(Expr::greater_than(
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                Expr::literal("bar")
            ))
        );
    }

    #[test]
    fn test_select_field_11() {
        let result = Expr::from_text("foo.bar > 1");
        assert_eq!(
            result,
            Ok(Expr::greater_than(
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                Expr::untyped_number(BigDecimal::from(1))
            ))
        );
    }

    #[test]
    fn test_select_field_12() {
        let input = "if foo.bar > 1 then foo.bar else foo.baz";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::cond(
                Expr::greater_than(
                    Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                    Expr::untyped_number(BigDecimal::from(1))
                ),
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                Expr::select_field(Expr::identifier_global("foo", None), "baz", None)
            ))
        );
    }

    #[test]
    fn test_select_field_13() {
        let input = "match foo { _ => bar, ok(x) => x, err(x) => x, none => foo, some(x) => x, foo => foo.bar }";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::pattern_match(
                Expr::identifier_global("foo", None),
                vec![
                    MatchArm::new(ArmPattern::WildCard, Expr::identifier_global("bar", None)),
                    MatchArm::new(
                        ArmPattern::constructor(
                            "ok",
                            vec![ArmPattern::Literal(Box::new(Expr::identifier_global(
                                "x", None
                            )))]
                        ),
                        Expr::identifier_global("x", None),
                    ),
                    MatchArm::new(
                        ArmPattern::constructor(
                            "err",
                            vec![ArmPattern::Literal(Box::new(Expr::identifier_global(
                                "x", None
                            )))]
                        ),
                        Expr::identifier_global("x", None),
                    ),
                    MatchArm::new(
                        ArmPattern::constructor("none", vec![]),
                        Expr::identifier_global("foo", None),
                    ),
                    MatchArm::new(
                        ArmPattern::constructor(
                            "some",
                            vec![ArmPattern::Literal(Box::new(Expr::identifier_global(
                                "x", None
                            )))]
                        ),
                        Expr::identifier_global("x", None),
                    ),
                    MatchArm::new(
                        ArmPattern::Literal(Box::new(Expr::identifier_global("foo", None))),
                        Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                    ),
                ]
            ))
        );
    }

    #[test]
    fn test_worker_function_invoke_1() {
        let expr = Expr::from_text("worker.function-name()").unwrap();
        let worker_variable = Expr::identifier_global("worker", None);
        let function_name = "function-name".to_string();

        assert_eq!(
            expr,
            Expr::invoke_worker_function(worker_variable, function_name, None, vec![])
        );
    }

    #[test]
    fn test_worker_function_invoke_2() {
        let expr = Expr::from_text("worker.function-name[foo]()").unwrap();
        let worker_variable = Expr::identifier_global("worker", None);
        let function_name = "function-name".to_string();
        let type_parameter = GenericTypeParameter {
            value: "foo".to_string(),
        };

        assert_eq!(
            expr,
            Expr::invoke_worker_function(
                worker_variable,
                function_name,
                Some(type_parameter),
                vec![]
            )
        );
    }

    #[test]
    fn test_worker_function_invoke_3() {
        let expr = Expr::from_text(r#"worker.function-name[foo](foo, bar)"#).unwrap();
        let worker_variable = Expr::identifier_global("worker", None);
        let type_parameter = GenericTypeParameter {
            value: "foo".to_string(),
        };
        let function_name = "function-name".to_string();

        assert_eq!(
            expr,
            Expr::invoke_worker_function(
                worker_variable,
                function_name,
                Some(type_parameter),
                vec![
                    Expr::identifier_global("foo", None),
                    Expr::identifier_global("bar", None)
                ]
            )
        );
    }

    #[test]
    fn test_worker_function_invoke_4() {
        let expr = Expr::from_text(r#"worker.function-name(foo, bar)"#).unwrap();
        let worker_variable = Expr::identifier_global("worker", None);
        let function_name = "function-name".to_string();

        assert_eq!(
            expr,
            Expr::invoke_worker_function(
                worker_variable,
                function_name,
                None,
                vec![
                    Expr::identifier_global("foo", None),
                    Expr::identifier_global("bar", None)
                ]
            )
        );
    }

    #[test]
    fn test_worker_function_invoke_5() {
        let rib_expr = r#"
          let worker = instance("my-worker");
          worker.function-name(foo, bar, baz)
        "#;
        let expr = Expr::from_text(rib_expr).unwrap();
        let worker_variable = Expr::identifier_global("worker", None);
        let function_name = "function-name".to_string();

        let expected = Expr::expr_block(vec![
            Expr::let_binding(
                "worker",
                Expr::call_worker_function(
                    DynamicParsedFunctionName::parse("instance").unwrap(),
                    None,
                    None,
                    vec![Expr::literal("my-worker")],
                ),
                None,
            ),
            Expr::invoke_worker_function(
                worker_variable,
                function_name,
                None,
                vec![
                    Expr::identifier_global("foo", None),
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("baz", None),
                ],
            ),
        ]);
        assert_eq!(expr, expected);
    }

    #[test]
    fn test_worker_function_invoke_6() {
        let rib_expr = r#"
          let worker = instance("my-worker");
          worker.function-name[foo](foo, bar, baz)
        "#;
        let expr = Expr::from_text(rib_expr).unwrap();
        let worker_variable = Expr::identifier_global("worker", None);
        let function_name = "function-name".to_string();
        let type_parameter = GenericTypeParameter {
            value: "foo".to_string(),
        };

        let expected = Expr::expr_block(vec![
            Expr::let_binding(
                "worker",
                Expr::call_worker_function(
                    DynamicParsedFunctionName::parse("instance").unwrap(),
                    None,
                    None,
                    vec![Expr::literal("my-worker")],
                ),
                None,
            ),
            Expr::invoke_worker_function(
                worker_variable,
                function_name,
                Some(type_parameter),
                vec![
                    Expr::identifier_global("foo", None),
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("baz", None),
                ],
            ),
        ]);
        assert_eq!(expr, expected);
    }

    #[test]
    fn test_worker_function_invoke_7() {
        let rib_expr = r#"
          let worker = instance[foo]("my-worker");
          worker.function-name[bar](foo, bar, baz)
        "#;
        let expr = Expr::from_text(rib_expr).unwrap();
        let worker_variable = Expr::identifier_global("worker", None);
        let function_name = "function-name".to_string();
        let type_parameter1 = GenericTypeParameter {
            value: "foo".to_string(),
        };

        let type_parameter2 = GenericTypeParameter {
            value: "bar".to_string(),
        };

        let expected = Expr::expr_block(vec![
            Expr::let_binding(
                "worker",
                Expr::call_worker_function(
                    DynamicParsedFunctionName::parse("instance").unwrap(),
                    Some(type_parameter1),
                    None,
                    vec![Expr::literal("my-worker")],
                ),
                None,
            ),
            Expr::invoke_worker_function(
                worker_variable,
                function_name,
                Some(type_parameter2),
                vec![
                    Expr::identifier_global("foo", None),
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("baz", None),
                ],
            ),
        ]);
        assert_eq!(expr, expected);
    }
}

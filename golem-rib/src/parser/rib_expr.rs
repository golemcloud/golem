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

use combine::parser::char;
use combine::parser::char::{char, spaces};
use combine::{attempt, choice, optional, parser, position, Stream};
use combine::{ParseError, Parser};

use super::binary_op::BinaryOp;
use crate::expr::Expr;
use crate::parser::boolean::boolean_literal;
use crate::parser::call::call;
use crate::parser::cond::conditional;
use crate::parser::errors::RibParseError;
use crate::parser::flag::flag;
use crate::parser::identifier::identifier;
use crate::parser::let_binding::let_binding;
use crate::parser::literal::literal;
use crate::parser::multi_line_code_block::multi_line_block;
use crate::parser::not::not;
use crate::parser::number::number;
use crate::parser::optional::option;
use crate::parser::pattern_match::pattern_match;
use crate::parser::range_type::RangeType;
use crate::parser::rib_expr::internal::RibRest;
use crate::parser::select_field::select_field;
use crate::parser::select_index::{select_index, IndexOrRange};
use crate::parser::sequence::sequence;
use crate::parser::tuple::tuple;
use crate::rib_source_span::GetSourcePosition;

use crate::parser::list_aggregation::list_aggregation;
use crate::parser::list_comprehension::list_comprehension;
use crate::parser::record::record;
use crate::parser::result::result;
use crate::parser::type_name::type_name;
use crate::parser::worker_function_invoke::worker_function_invoke;

// A rib expression := (simple_expr, rib_expr_rest*)
// A simple recursion never goes in recursion on LHS
parser! {
    pub fn rib_expr[Input]()(Input) -> Expr
    where [Input: combine::Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>, Input::Position: GetSourcePosition]
    {
       rib_expr_()
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
    (
        position(),
        spaces()
            .with(
                (simple_expr(), internal::rib_expr_rest()).map(|(expr, rest)| match rest {
                    RibRest::All(index_expressions, binary_expressions) => {
                        let new_expr =
                            index_expressions
                                .into_iter()
                                .fold(expr, |acc, index_or_range| match index_or_range {
                                    IndexOrRange::Index(index) => Expr::select_index(acc, index),
                                    IndexOrRange::Dynamic(expr) => {
                                        Expr::select_dynamic(acc, expr, None)
                                    }
                                });

                        binary_expressions
                            .into_iter()
                            .fold(new_expr, |acc, (op, next)| match op {
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
                            })
                    }

                    RibRest::Range((range_type, opt)) => match range_type {
                        RangeType::Inclusive => match opt {
                            Some(rhs) => Expr::range_inclusive(expr, rhs),
                            None => panic!("Inclusive range should have a right hand side"),
                        },
                        RangeType::Exclusive => match opt {
                            Some(rhs) => Expr::range(expr, rhs),
                            None => Expr::range_from(expr),
                        },
                    },
                }),
            )
            .skip(spaces()),
        position(),
    )
        .map(|(start, expr, end)| {
            let start_pos: Input::Position = start;
            let start = start_pos.get_source_position();
            let end_pos: Input::Position = end;
            let end = end_pos.get_source_position();
            let span = crate::rib_source_span::SourceSpan::new(start, end);
            expr.with_source_span(span)
        })
}

pub fn simple_expr_<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
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
                attempt(worker_function_invoke()), // has to backtrack if there is fails at arguments parsing
                attempt(select_field()),           // succeeds at select_field
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
                number(),
            )))
            .skip(spaces()),
        optional(optional(char(':').skip(spaces())).with(type_name())),
    )
        .map(|(expr, type_name)| match type_name {
            Some(type_name) => expr.with_type_annotation(type_name),
            None => expr,
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

parser! {
    pub(crate) fn simple_expr[Input]()(Input) -> Expr
    where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>, Input::Position: GetSourcePosition]
    {
        simple_expr_()
    }
}

mod internal {
    use crate::parser::binary_op::{binary_op, BinaryOp};
    use crate::parser::errors::RibParseError;
    use crate::parser::range_type::{range_type, RangeType};
    use crate::parser::rib_expr::{rib_expr, simple_expr, simple_expr_};
    use crate::rib_source_span::GetSourcePosition;
    use crate::Expr;

    use crate::parser::select_index::{select_index2, IndexOrRange};
    use combine::parser::char::char;
    use combine::parser::char::spaces;
    use combine::{attempt, many, optional, parser, ParseError, Parser, Stream};
    // A simple expression is a composition of all parsers that doesn't involve left recursion

    pub(crate) enum RibRest {
        All(Vec<IndexOrRange>, Vec<(BinaryOp, Expr)>),
        Range((RangeType, Option<Expr>)),
    }

    pub fn rib_expr_rest_<Input>() -> impl Parser<Input, Output = RibRest>
    where
        Input: Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        attempt(
            (range_type(), optional(simple_expr_())).map(|(range_type, expr)| match expr {
                Some(expr) => RibRest::Range((range_type, Some(expr))),
                None => RibRest::Range((range_type, None)),
            }),
        )
        .or(attempt(
            (
                many((char('['), select_index2(), char(']').skip(spaces()))).map(
                    |collections: Vec<(char, IndexOrRange, char)>| {
                        collections
                            .into_iter()
                            .map(|(_, index_or_range, _)| index_or_range)
                            .collect::<Vec<_>>()
                    },
                ),
                many((binary_op(), rib_expr())),
            )
                .map(|(indices, binary_math)| RibRest::All(indices, binary_math)),
        ))
    }

    parser! {
        pub(crate) fn rib_expr_rest[Input]()(Input) -> RibRest
        where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>, Input::Position: GetSourcePosition]
        {
            rib_expr_rest_()
        }
    }
}

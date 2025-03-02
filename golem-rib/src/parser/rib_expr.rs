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
use crate::parser::rib_expr::internal::{index_expr, RibRest};
use crate::parser::sequence::sequence;
use crate::parser::tuple::tuple;
use crate::rib_source_span::GetSourcePosition;

use crate::parser::list_aggregation::list_aggregation;
use crate::parser::list_comprehension::list_comprehension;
use crate::parser::record::record;
use crate::parser::result::result;
use crate::parser::type_name::type_name;

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
                (simple_expr(), internal::rib_expr_rest()).and_then(|(expr, rest)| match rest {
                    RibRest {
                        indices: index_expressions,
                        field_selection: field_expr,
                        range_info: range_info_opt,
                        binary_ops: binary_expressions,
                    } => {
                        let with_index = internal::index_expr(expr, index_expressions);

                        let with_field =
                            field_expr.into_iter().fold(with_index, |acc, field_expr| {
                                match field_expr.base {
                                    // if the base is a call, we consider it as a method call
                                    Expr::Call {
                                        call_type,
                                        generic_type_parameter,
                                        args,
                                        ..
                                    } => {
                                        let base = Expr::invoke_worker_function(
                                            acc,
                                            call_type.function_name().unwrap().to_string(),
                                            generic_type_parameter,
                                            args,
                                        );
                                        // allowing `worker.foo("bar")[1]`
                                        // further allowing `worker.foo("bar")[1].baz`

                                        index_expr(base, field_expr.index_expr)
                                            .with_type_annotation_opt(field_expr.type_name)
                                    }

                                    // If it's any other expresion
                                    expr => {
                                        dbg!(expr.clone());
                                        let selection = internal::build_selection(acc, expr);
                                        index_expr(selection, field_expr.index_expr)
                                            .with_type_annotation_opt(field_expr.type_name)
                                    }
                                }
                            });

                        let with_range = match range_info_opt {
                            Some(range_info) => match range_info.base.expr {
                                Some(rhs) => match range_info.base.range_type {
                                    RangeType::Inclusive => index_expr(
                                        Expr::range_inclusive(with_field, rhs),
                                        range_info.index_expr,
                                    ),
                                    RangeType::Exclusive => index_expr(
                                        Expr::range(with_field, rhs),
                                        range_info.index_expr,
                                    ),
                                },
                                None => match range_info.base.range_type {
                                    RangeType::Inclusive => {
                                        return Err(RibParseError::Message(
                                            "Exclusive range should have a right hand side"
                                                .to_string(),
                                        ))
                                    }
                                    RangeType::Exclusive => index_expr(
                                        Expr::range_from(with_field),
                                        range_info.index_expr,
                                    ),
                                },
                            },
                            None => with_field,
                        };

                        let with_binary =
                            binary_expressions
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
                    }
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
    use crate::{Expr, TypeName};
    use std::ops::Index;
    use crate::parser::type_name::type_name;
    use combine::parser::char::char;
    use combine::parser::char::spaces;
    use combine::{attempt, many, optional, parser, ParseError, Parser, Stream};
    // A simple expression is a composition of all parsers that doesn't involve left recursion

    pub(crate) struct RibRest {
        pub(crate) indices: IndexExpr, // The suffix to simple expressions on the left side with indices
        pub(crate) field_selection: Vec<WithIndex<Expr>>, // Each field selection may have possible suffix of indices
        pub(crate) range_info: Option<WithIndex<RangeInfo>>, // The range info but with possible suffix of indices
        pub(crate) binary_ops: Vec<(BinaryOp, WithIndex<Expr>)>, // A binary op stricly don't have any suffix until the next recursion, and it has to be simple expression
    }

    #[derive(Debug, Clone)]
    pub(crate) struct IndexExpr {
        pub(crate) exprs: Vec<Expr>,
    }

    pub(crate) struct WithIndex<T> {
        pub(crate) index_expr: IndexExpr,
        pub(crate) base: T,
        pub(crate) type_name: Option<TypeName>,
    }

    #[derive(Clone, Debug)]
    pub(crate) struct RangeInfo {
        pub(crate) range_type: RangeType,
        pub(crate) expr: Option<Expr>,
    }

    impl RangeInfo {
        pub fn new(range_type: RangeType, expr: Option<Expr>) -> Self {
            Self { range_type, expr }
        }
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
            (
                select_index_expression().skip(spaces()),
                many(attempt(
                    char('.').skip(spaces()).with(
                        (
                            simple_expr().skip(spaces()),
                            select_index_expression().skip(spaces()),
                            optional(type_annotation()),
                        )
                            .map(|(field_expr, indices, type_name)| {
                                WithIndex {
                                    index_expr: indices,
                                    base: field_expr,
                                    type_name,
                                }
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
                many((binary_op(), (simple_expr(), select_index_expression()))).map(
                    |binary_math: Vec<(BinaryOp, (Expr, IndexExpr))>| {
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
                    },
                ),
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
        pub(crate) fn rib_expr_rest[Input]()(Input) -> RibRest
        where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>, Input::Position: GetSourcePosition]
        {
            rib_expr_rest_()
        }
    }

    // Recursively build the selection accumulating to the LHS
    pub(crate) fn build_selection(base: Expr, next: Expr) -> Expr {
        match next {
            Expr::Identifier {
                variable_id,
                type_annotation,
                ..
            } => Expr::select_field(base, variable_id.name().as_str(), type_annotation),
            Expr::SelectField {
                expr: second,
                field: last,
                type_annotation: type_name,
                inferred_type,
                source_span,
            } => {
                let inner_select = build_selection(base, *second);
                Expr::SelectField {
                    expr: Box::new(inner_select),
                    field: last,
                    type_annotation: type_name,
                    inferred_type,
                    source_span,
                }
            }
            Expr::SelectIndex {
                expr: second,
                index: last_index,
                type_annotation: type_name,
                inferred_type,
                source_span,
            } => {
                let inner_select = build_selection(base, *second);
                Expr::SelectIndex {
                    expr: Box::new(inner_select),
                    index: last_index,
                    type_annotation: type_name,
                    inferred_type,
                    source_span,
                }
            }
            Expr::SelectDynamic {
                expr: second,
                index: last_index,
                type_annotation: type_name,
                inferred_type,
                source_span,
            } => {
                let inner_select = build_selection(base, *second);
                Expr::SelectDynamic {
                    expr: Box::new(inner_select),
                    index: last_index,
                    type_annotation: type_name,
                    inferred_type,
                    source_span,
                }
            }
            _ => base,
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
        (range_type().skip(spaces()), optional(simple_expr_())).map(|(range_type, expr)| match expr
        {
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

    // Fold over a base expression to build index expressions
    pub(crate) fn index_expr(base_expr: Expr, index_expr: IndexExpr) -> Expr {
        index_expr
            .exprs
            .into_iter()
            .fold(base_expr, |acc, index_expr| {
                Expr::select_dynamic(acc, index_expr, None)
            })
    }
}

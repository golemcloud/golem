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

use combine::parser;
use combine::parser::char;
use combine::parser::char::spaces;
use combine::{ParseError, Parser};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;

use super::binary_op::BinaryOp;

// A rib expression := (simple_expr, rib_expr_rest*)
parser! {
    pub fn rib_expr[Input]()(Input) -> Expr
    where [Input: combine::Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,]
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
{
    spaces()
        .with(
            (internal::simple_expr(), internal::rib_expr_rest()).map(|(expr, rest)| {
                // FIXME: Respect operator precedence
                rest.into_iter().fold(expr, |acc, (op, next)| match op {
                    BinaryOp::GreaterThan => Expr::greater_than(acc, next),
                    BinaryOp::LessThan => Expr::less_than(acc, next),
                    BinaryOp::LessThanOrEqualTo => Expr::less_than_or_equal_to(acc, next),
                    BinaryOp::GreaterThanOrEqualTo => Expr::greater_than_or_equal_to(acc, next),
                    BinaryOp::EqualTo => Expr::equal_to(acc, next),
                    BinaryOp::And => Expr::and(acc, next),
                    BinaryOp::Or => Expr::or(acc, next),
                    BinaryOp::Add => Expr::plus(acc, next),
                    BinaryOp::Subtract => Expr::minus(acc, next),
                    BinaryOp::Multiply => Expr::multiply(acc, next),
                    BinaryOp::Divide => Expr::divide(acc, next),
                })
            }),
        )
        .skip(spaces())
}

mod internal {
    use crate::parser::binary_op::{binary_op, BinaryOp};
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
    use crate::parser::record::record;
    use crate::parser::result::result;

    use crate::parser::list_aggregation::list_aggregation;
    use crate::parser::list_comprehension::list_comprehension;
    use crate::parser::select_field::select_field;
    use crate::parser::select_index::select_index;
    use crate::parser::sequence::sequence;
    use crate::parser::tuple::tuple;
    use crate::Expr;
    use combine::parser::char::spaces;
    use combine::{attempt, choice, many, parser, ParseError, Parser, Stream};

    // A simple expression is a composition of all parsers that doesn't involve left recursion
    pub fn simple_expr_<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        spaces()
            .with(choice((
                list_comprehension(),
                list_aggregation(),
                pattern_match(),
                let_binding(),
                conditional(),
                selection_expr(),
                flag_or_record(),
                multi_line_block(),
                tuple(),
                sequence(),
                boolean_literal(),
                literal(),
                not(),
                option(),
                result(),
                attempt(call()),
                identifier(),
                number(),
            )))
            .skip(spaces())
    }

    parser! {
        pub(crate) fn simple_expr[Input]()(Input) -> Expr
        where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,]
        {
            simple_expr_()
        }
    }

    pub fn rib_expr_rest_<Input>() -> impl Parser<Input, Output = Vec<(BinaryOp, Expr)>>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        many((binary_op(), simple_expr()))
    }

    parser! {
        pub(crate) fn rib_expr_rest[Input]()(Input) -> Vec<(BinaryOp, Expr)>
        where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,]
        {
            rib_expr_rest_()
        }
    }

    fn flag_or_record<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        choice((attempt(flag()), attempt(record()))).message("Unable to parse flag or record")
    }

    fn selection_expr<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        choice((attempt(select_field()), attempt(select_index())))
            .message("Unable to parse selection expression")
    }
}

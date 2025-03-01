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

use bigdecimal::ToPrimitive;
use combine::parser::char::{char as char_, spaces};
use combine::{attempt, choice, many1, optional, ParseError, Parser};

use internal::*;

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::identifier::identifier;
use crate::parser::rib_expr::rib_expr;
use crate::rib_source_span::GetSourcePosition;

// Index can be handled as an expression itself
// but this can be replaced once we are sure dynamic
// selection works without any issues.
pub enum IndexOrRange {
    Index(usize),
    Dynamic(Expr),
}

// TODO: Index or dynamic doesn't need to exist, but introduced temporarily to reduce
// test failures
pub struct IndexExpression(pub IndexOrRange);

pub fn select_index2<Input>() -> impl Parser<Input, Output = IndexOrRange>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    spaces()
        .with(
            attempt(pos_num().skip(spaces()).map(IndexOrRange::Index))
                .or(attempt(rib_expr().map(IndexOrRange::Dynamic)))
                .map(|index_or_range| index_or_range),
        )
        .message("Invalid index selection")
}

pub fn select_index<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    spaces()
        .with(
            (
                sequence_base_expr().skip(spaces()),
                char_('[').skip(spaces()),
                attempt(pos_num().skip(spaces()).map(IndexOrRange::Index))
                    .or(attempt(rib_expr().map(IndexOrRange::Dynamic))),
                char_(']').skip(spaces()),
                optional(nested_indices()),
            )
                .and_then(|(expr, _, index_or_range, _, possible_indices)| {
                    match index_or_range {
                        IndexOrRange::Index(index) => match possible_indices {
                            Some(indices) => Ok(build_select_index_from(
                                Expr::select_index(expr, index),
                                indices,
                            )),
                            None => Ok(Expr::select_index(expr, index)),
                        },
                        IndexOrRange::Dynamic(index_dynamic) => match possible_indices {
                            Some(_) => Err(RibParseError::Message(
                                "nested indexing is currently only supported for literal numbers"
                                    .to_string(),
                            )),
                            None => Ok(Expr::select_dynamic(expr, index_dynamic, None)),
                        },
                    }
                }),
        )
        .message("Invalid index selection")
}

mod internal {
    use bigdecimal::BigDecimal;
    use combine::parser::char::char as char_;

    use crate::parser::number::number;
    use crate::parser::sequence::sequence;

    use super::*;

    pub(crate) fn build_select_index_from(base_expr: Expr, indices: Vec<usize>) -> Expr {
        let mut result = base_expr;
        for index in indices {
            result = Expr::select_index(result, index);
        }
        result
    }

    pub(crate) fn nested_indices<Input>() -> impl Parser<Input, Output = Vec<usize>>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        many1(
            (
                char_('[').skip(spaces()),
                pos_num().skip(spaces()),
                char_(']').skip(spaces()),
            )
                .map(|(_, number, _)| number),
        )
        .map(|result: Vec<usize>| result)
    }

    pub(crate) fn pos_num<Input>() -> impl Parser<Input, Output = usize>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        number().map(|s: Expr| match s {
            Expr::Number { number, .. } => {
                if number.value < BigDecimal::from(0) {
                    panic!("Cannot use a negative number to index",)
                } else {
                    number.value.to_usize().unwrap()
                }
            }
            _ => panic!("Cannot use a float number to index",),
        })
    }

    pub(crate) fn sequence_base_expr<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        choice((attempt(sequence()), attempt(identifier())))
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::expr::*;

    #[test]
    fn test_select_index() {
        let input = "foo[0]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_index(Expr::identifier_global("foo", None), 0))
        );
    }

    #[test]
    fn test_recursive_select_index() {
        let input = "foo[0][1]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_index(
                Expr::select_index(Expr::identifier_global("foo", None), 0),
                1
            ))
        );
    }

    #[test]
    fn test_select_dynamic_index_1() {
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
    fn test_select_dynamic_index_2() {
        let input = "foo[1 .. 2]";
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
}

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

pub fn select_index<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    spaces().with(
        (
            base_expr().skip(spaces()),
            char_('[').skip(spaces()),
            pos_num().skip(spaces()),
            char_(']').skip(spaces()),
            optional(nested_indices()),
        )
            .map(
                |(expr, _, number, _, possible_indices)| match possible_indices {
                    Some(indices) => {
                        build_select_index_from(Expr::select_index(expr, number), indices)
                    }
                    None => Expr::select_index(expr, number),
                },
            ),
    )
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
    {
        number().map(|s: Expr| match s {
            Expr::Number(number, _, _) => {
                if number.value < BigDecimal::from(0) {
                    panic!("Cannot use a negative number to index",)
                } else {
                    number.value.to_usize().unwrap()
                }
            }
            _ => panic!("Cannot use a float number to index",),
        })
    }

    pub(crate) fn base_expr<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        choice((attempt(sequence()), attempt(identifier())))
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::EasyParser;

    use crate::expr::*;
    use crate::parser::rib_expr::rib_expr;

    #[test]
    fn test_select_index() {
        let input = "foo[0]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::select_index(Expr::identifier("foo"), 0), ""))
        );
    }

    #[test]
    fn test_recursive_select_index() {
        let input = "foo[0][1]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::select_index(Expr::select_index(Expr::identifier("foo"), 0), 1),
                ""
            ))
        );
    }
}

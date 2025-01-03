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

use combine::parser::char::{alpha_num, char, spaces, string};
use combine::{attempt, not_followed_by, ParseError, Parser};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::rib_expr::rib_expr;

pub fn conditional<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    // Use attempt only for the initial "if" to resolve ambiguity with identifiers
    attempt(
        string("if")
            .skip(not_followed_by(alpha_num().or(char('-')).or(char('_'))))
            .skip(spaces()),
    )
    .with(
        (
            rib_expr().skip(spaces()),
            string("then").skip(spaces()),
            rib_expr().skip(spaces()),
            string("else").skip(spaces()),
            rib_expr().skip(spaces()),
        )
            .map(|(cond, _, then_expr, _, else_expr)| Expr::cond(cond, then_expr, else_expr)),
    )
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::EasyParser;

    use super::*;

    #[test]
    fn test_conditional() {
        let input = "if foo then bar else baz";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::cond(
                    Expr::identifier("foo"),
                    Expr::identifier("bar"),
                    Expr::identifier("baz")
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_conditional_of_sequences() {
        let input = "if foo then [bar] else [baz]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::cond(
                    Expr::identifier("foo"),
                    Expr::sequence(vec![Expr::identifier("bar")]),
                    Expr::sequence(vec![Expr::identifier("baz")])
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_if_condition_inside_else() {
        let input = "if foo then bar else if baz then qux else quux";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::cond(
                    Expr::identifier("foo"),
                    Expr::identifier("bar"),
                    Expr::cond(
                        Expr::identifier("baz"),
                        Expr::identifier("qux"),
                        Expr::identifier("quux")
                    )
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_if_condition_inside_then() {
        let input = "if foo then if bar then baz else qux else quux";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::cond(
                    Expr::identifier("foo"),
                    Expr::cond(
                        Expr::identifier("bar"),
                        Expr::identifier("baz"),
                        Expr::identifier("qux")
                    ),
                    Expr::identifier("quux")
                ),
                ""
            ))
        );
    }
}

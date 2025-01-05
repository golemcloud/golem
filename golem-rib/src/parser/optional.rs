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

use combine::parser::char::alpha_num;
use combine::parser::char::spaces;
use combine::{
    attempt, choice, not_followed_by,
    parser::char::{char, string},
    ParseError, Parser,
};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;

use super::rib_expr::rib_expr;

pub fn option<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    choice((
        attempt(string("some").skip(char('('))).with(
            rib_expr()
                .skip(spaces())
                .skip(char(')'))
                .map(|expr| Expr::option(Some(expr))),
        ),
        (attempt(string("none").skip(not_followed_by(alpha_num().or(char('-')).or(char('_')))))
            .map(|_| Expr::option(None))),
    ))
    .message("Invalid syntax for Option type")
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::EasyParser;

    use super::*;

    #[test]
    fn test_some() {
        let input = "some(foo)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::option(Some(Expr::identifier("foo"))), ""))
        );
    }

    #[test]
    fn test_none() {
        let input = "none";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::option(None), "")));
    }

    #[test]
    fn test_nested_some() {
        let input = "some(some(foo))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::option(Some(Expr::option(Some(Expr::identifier("foo"))))),
                ""
            ))
        );
    }

    #[test]
    fn test_some_of_sequence() {
        let input = "some([foo, bar])";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::option(Some(Expr::sequence(vec![
                    Expr::identifier("foo"),
                    Expr::identifier("bar")
                ]))),
                ""
            ))
        );
    }

    #[test]
    fn test_some_of_literal() {
        let input = "some(\"foo\")";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::option(Some(Expr::literal("foo"))), "")));
    }
}

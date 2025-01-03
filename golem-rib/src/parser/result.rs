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

use combine::parser::char::spaces;
use combine::{
    attempt, choice,
    parser::char::{char, string},
    ParseError, Parser,
};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;

use super::rib_expr::rib_expr;

pub fn result<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    choice((
        attempt(string("ok").skip(char('(')))
            .with((rib_expr().skip(spaces()), char(')')).map(|(expr, _)| Expr::ok(expr))),
        attempt(string("err").skip(char('(')))
            .with((rib_expr().skip(spaces()), char(')')).map(|(expr, _)| Expr::err(expr))),
    ))
    .message("Invalid syntax for Result type")
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::EasyParser;

    use super::*;

    #[test]
    fn test_result() {
        let input = "ok(foo)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::ok(Expr::identifier("foo")), "")));
    }

    #[test]
    fn test_result_err() {
        let input = "err(foo)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::err(Expr::identifier("foo")), "")));
    }

    #[test]
    fn test_ok_of_sequence() {
        let input = "ok([foo, bar])";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::ok(Expr::sequence(vec![
                    Expr::identifier("foo"),
                    Expr::identifier("bar")
                ])),
                ""
            ))
        );
    }

    #[test]
    fn test_err_of_sequence() {
        let input = "err([foo, bar])";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::err(Expr::sequence(vec![
                    Expr::identifier("foo"),
                    Expr::identifier("bar")
                ])),
                ""
            ))
        );
    }

    #[test]
    fn test_ok_of_err() {
        let input = "ok(err(foo))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::ok(Expr::err(Expr::identifier("foo"))), ""))
        );
    }

    #[test]
    fn test_err_of_ok() {
        let input = "err(ok(foo))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::err(Expr::ok(Expr::identifier("foo"))), ""))
        );
    }

    #[test]
    fn test_ok_of_ok() {
        let input = "ok(ok(foo))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::ok(Expr::ok(Expr::identifier("foo"))), ""))
        );
    }

    #[test]
    fn test_err_of_err() {
        let input = "err(err(foo))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::err(Expr::err(Expr::identifier("foo"))), ""))
        );
    }
}

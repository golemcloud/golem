// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use combine::{
    attempt, choice,
    parser::char::{char, string},
    ParseError, Parser,
};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;

use super::rib_expr::rib_expr;
use crate::rib_source_span::GetSourcePosition;

pub fn result<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    (choice((
        attempt(string("ok").skip(char('(')))
            .with((rib_expr(), char(')')).map(|(expr, _)| Expr::ok(expr, None))),
        attempt(string("err").skip(char('(')))
            .with((rib_expr(), char(')')).map(|(expr, _)| Expr::err(expr, None))),
    )))
    .and_then(|expr| match expr {
        Expr::Result { expr: Ok(expr), .. } => Ok(Expr::ok(*expr, None)),
        Expr::Result {
            expr: Err(expr), ..
        } => Ok(Expr::err(*expr, None)),
        _ => Err(RibParseError::Message(
            "Invalid syntax for Result type".to_string(),
        )),
    })
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    #[test]
    fn test_result() {
        let input = "ok(foo)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::ok(Expr::identifier_global("foo", None), None))
        );
    }

    #[test]
    fn test_result_err() {
        let input = "err(foo)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::err(Expr::identifier_global("foo", None), None))
        );
    }

    #[test]
    fn test_ok_of_sequence() {
        let input = "ok([foo, bar])";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::ok(
                Expr::sequence(
                    vec![
                        Expr::identifier_global("foo", None),
                        Expr::identifier_global("bar", None)
                    ],
                    None
                ),
                None
            ))
        );
    }

    #[test]
    fn test_err_of_sequence() {
        let input = "err([foo, bar])";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::err(
                Expr::sequence(
                    vec![
                        Expr::identifier_global("foo", None),
                        Expr::identifier_global("bar", None)
                    ],
                    None
                ),
                None
            ))
        );
    }

    #[test]
    fn test_ok_of_err() {
        let input = "ok(err(foo))";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::ok(
                Expr::err(Expr::identifier_global("foo", None), None),
                None
            ))
        );
    }

    #[test]
    fn test_err_of_ok() {
        let input = "err(ok(foo))";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::err(
                Expr::ok(Expr::identifier_global("foo", None), None),
                None
            ))
        );
    }

    #[test]
    fn test_ok_of_ok() {
        let input = "ok(ok(foo))";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::ok(
                Expr::ok(Expr::identifier_global("foo", None), None),
                None
            ))
        );
    }

    #[test]
    fn test_err_of_err() {
        let input = "err(err(foo))";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::err(
                Expr::err(Expr::identifier_global("foo", None), None),
                None
            ))
        );
    }
}

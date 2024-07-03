// Copyright 2024 Golem Cloud
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

use combine::{
    between, choice,
    parser::char::{char, string},
    Parser,
};

use combine::parser::char::spaces;

use crate::expr::Expr;

use super::rib_expr::rib_expr;

use combine::stream::easy;

pub fn result<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((
        spaces().with(
            between(string("ok("), char(')'), rib_expr())
                .map(|expr| Expr::Result(Ok(Box::new(expr)))),
        ),
        spaces().with(
            between(string("err("), char(')'), rib_expr())
                .map(|expr| Expr::Result(Err(Box::new(expr)))),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_result() {
        let input = "ok(foo)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Result(Ok(Box::new(Expr::Identifier("foo".to_string())))),
                ""
            ))
        );
    }

    #[test]
    fn test_result_err() {
        let input = "err(foo)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Result(Err(Box::new(Expr::Identifier("foo".to_string())))),
                ""
            ))
        );
    }

    #[test]
    fn test_ok_of_sequence() {
        let input = "ok([foo, bar])";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Result(Ok(Box::new(Expr::Sequence(vec![
                    Expr::Identifier("foo".to_string()),
                    Expr::Identifier("bar".to_string())
                ])))),
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
                Expr::Result(Err(Box::new(Expr::Sequence(vec![
                    Expr::Identifier("foo".to_string()),
                    Expr::Identifier("bar".to_string())
                ])))),
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
            Ok((
                Expr::Result(Ok(Box::new(Expr::Result(Err(Box::new(Expr::Identifier(
                    "foo".to_string()
                ))))))),
                ""
            ))
        );
    }

    #[test]
    fn test_err_of_ok() {
        let input = "err(ok(foo))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Result(Err(Box::new(Expr::Result(Ok(Box::new(Expr::Identifier(
                    "foo".to_string()
                ))))))),
                ""
            ))
        );
    }

    #[test]
    fn test_ok_of_ok() {
        let input = "ok(ok(foo))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Result(Ok(Box::new(Expr::Result(Ok(Box::new(Expr::Identifier(
                    "foo".to_string()
                ))))))),
                ""
            ))
        );
    }

    #[test]
    fn test_err_of_err() {
        let input = "err(err(foo))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Result(Err(Box::new(Expr::Result(Err(Box::new(
                    Expr::Identifier("foo".to_string())
                )))))),
                ""
            ))
        );
    }
}

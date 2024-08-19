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
    parser::char::{char, spaces, string},
    Parser,
};

use crate::expr::Expr;

use super::rib_expr::rib_expr;
use combine::stream::easy;

pub fn option<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((
        spaces().with(
            between(string("some("), char(')'), rib_expr()).map(|expr| Expr::option(Some(expr))),
        ),
        spaces().with(string("none").map(|_| Expr::option(None))),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

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

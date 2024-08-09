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
    between,
    parser::char::{char, spaces},
    sep_by, Parser,
};

use crate::expr::Expr;

use super::rib_expr::rib_expr;

use combine::stream::easy;

pub fn tuple<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        between(
            char('('),
            char(')'),
            sep_by(rib_expr(), char(',').skip(spaces())),
        )
        .map(Expr::tuple),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_empty_tuple() {
        let input = "()";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::tuple(vec![]), "")));
    }

    #[test]
    fn test_singleton_tuple() {
        let input = "(foo)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::tuple(vec![Expr::identifier("foo")]), "")));
    }

    #[test]
    fn test_tuple() {
        let input = "(foo, bar)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::tuple(vec![Expr::identifier("foo"), Expr::identifier("bar")]),
                ""
            ))
        );
    }

    #[test]
    fn test_tuple_of_sequence() {
        let input = "([foo, bar], [baz, qux])";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::tuple(vec![
                    Expr::sequence(vec![Expr::identifier("foo"), Expr::identifier("bar")]),
                    Expr::sequence(vec![Expr::identifier("baz"), Expr::identifier("qux")])
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_tuple_of_record() {
        let input = "({foo: bar}, {baz: qux})";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::tuple(vec![
                    Expr::record(vec![("foo".to_string(), Expr::identifier("bar"))]),
                    Expr::record(vec![("baz".to_string(), Expr::identifier("qux"))])
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_tuple_of_literal() {
        let input = "(\"foo\", \"bar\")";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::tuple(vec![Expr::literal("foo"), Expr::literal("bar")]),
                ""
            ))
        );
    }

    #[test]
    fn test_tuple_of_tuple() {
        let input = "((foo, bar), (baz, qux))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::tuple(vec![
                    Expr::tuple(vec![Expr::identifier("foo"), Expr::identifier("bar")]),
                    Expr::tuple(vec![Expr::identifier("baz"), Expr::identifier("qux")])
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_tuple_of_result() {
        let input = "(ok(foo), err(bar))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::tuple(vec![
                    Expr::ok(Expr::identifier("foo")),
                    Expr::err(Expr::identifier("bar"))
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_tuple_option() {
        let input = "(some(foo), none)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::tuple(vec![
                    Expr::option(Some(Expr::identifier("foo"))),
                    Expr::option(None)
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_tuple_of_cond() {
        let input = "(if foo then bar else baz, if qux then quux else quuz)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::tuple(vec![
                    Expr::cond(
                        Expr::identifier("foo"),
                        Expr::identifier("bar"),
                        Expr::identifier("baz")
                    ),
                    Expr::cond(
                        Expr::identifier("qux"),
                        Expr::identifier("quux"),
                        Expr::identifier("quuz")
                    )
                ]),
                ""
            ))
        );
    }
}

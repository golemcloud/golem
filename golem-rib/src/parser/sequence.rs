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

use crate::expr::Expr;
use combine::parser::char::{char, spaces};
use combine::sep_by;
use combine::{between, Parser};

use crate::parser::rib_expr::rib_expr;
use combine::stream::easy;

pub fn sequence<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        between(
            char('['),
            char(']'),
            sep_by(rib_expr(), char(',').skip(spaces())),
        )
        .map(Expr::sequence)
        .message("Unable to parse sequece"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_empty_sequence() {
        let input = "[]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::sequence(vec![]), "")));
    }

    #[test]
    fn test_singleton_sequence() {
        let input = "[foo]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::sequence(vec![Expr::identifier("foo")]), ""))
        );
    }

    #[test]
    fn test_sequence() {
        let input = "[foo, bar]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::sequence(vec![Expr::identifier("foo"), Expr::identifier("bar")]),
                ""
            ))
        );
    }

    #[test]
    fn test_sequence_of_not() {
        let input = "[!foo, !bar]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::sequence(vec![
                    Expr::not(Expr::identifier("foo")),
                    Expr::not(Expr::identifier("bar"))
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_sequence_of_literal() {
        let input = "[\"foo\", \"bar\"]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::sequence(vec![Expr::literal("foo"), Expr::literal("bar")]),
                ""
            ))
        );
    }

    #[test]
    fn test_sequence_of_sequence() {
        let input = "[[foo, bar], [bar, bar]]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::sequence(vec![
                    Expr::sequence(vec![Expr::identifier("foo"), Expr::identifier("bar")]),
                    Expr::sequence(vec![Expr::identifier("bar"), Expr::identifier("bar")])
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_sequence_of_option() {
        let input = "[some(x), some(y), some(z)]";
        let result = rib_expr().easy_parse(input);

        assert_eq!(
            result,
            Ok((
                Expr::sequence(vec![
                    Expr::option(Some(Expr::identifier("x"))),
                    Expr::option(Some(Expr::identifier("y"))),
                    Expr::option(Some(Expr::identifier("z")))
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_sequence_of_result() {
        let input = "[ok(x), ok(y), ok(z)]";
        let result = rib_expr().easy_parse(input);

        assert_eq!(
            result,
            Ok((
                Expr::sequence(vec![
                    Expr::ok(Expr::identifier("x")),
                    Expr::ok(Expr::identifier("y")),
                    Expr::ok(Expr::identifier("z"))
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_sequence_of_cond() {
        let input = "[if foo then bar else baz, if qux then quux else quuz]";
        let result = rib_expr().easy_parse(input);

        assert_eq!(
            result,
            Ok((
                Expr::sequence(vec![
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

    #[test]
    fn test_sequence_of_tuple() {
        let input = "[(foo, bar), (baz, qux)]";
        let result = rib_expr().easy_parse(input);

        assert_eq!(
            result,
            Ok((
                Expr::sequence(vec![
                    Expr::tuple(vec![Expr::identifier("foo"), Expr::identifier("bar")]),
                    Expr::tuple(vec![Expr::identifier("baz"), Expr::identifier("qux")])
                ]),
                ""
            ))
        );
    }
}

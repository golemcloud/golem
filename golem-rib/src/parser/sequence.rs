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
        .map(Expr::Sequence)
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
        assert_eq!(result, Ok((Expr::Sequence(vec![]), "")));
    }

    #[test]
    fn test_singleton_sequence() {
        let input = "[foo]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Sequence(vec![Expr::Identifier("foo".to_string())]),
                ""
            ))
        );
    }

    #[test]
    fn test_sequence() {
        let input = "[foo, bar]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Sequence(vec![
                    Expr::Identifier("foo".to_string()),
                    Expr::Identifier("bar".to_string())
                ]),
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
                Expr::Sequence(vec![
                    Expr::Not(Box::new(Expr::Identifier("foo".to_string()))),
                    Expr::Not(Box::new(Expr::Identifier("bar".to_string())))
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
                Expr::Sequence(vec![
                    Expr::Literal("foo".to_string()),
                    Expr::Literal("bar".to_string())
                ]),
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
                Expr::Sequence(vec![
                    Expr::Sequence(vec![
                        Expr::Identifier("foo".to_string()),
                        Expr::Identifier("bar".to_string())
                    ]),
                    Expr::Sequence(vec![
                        Expr::Identifier("bar".to_string()),
                        Expr::Identifier("bar".to_string())
                    ])
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
                Expr::Sequence(vec![
                    Expr::Option(Some(Box::new(Expr::Identifier("x".to_string())))),
                    Expr::Option(Some(Box::new(Expr::Identifier("y".to_string())))),
                    Expr::Option(Some(Box::new(Expr::Identifier("z".to_string()))))
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
                Expr::Sequence(vec![
                    Expr::Result(Ok(Box::new(Expr::Identifier("x".to_string())))),
                    Expr::Result(Ok(Box::new(Expr::Identifier("y".to_string())))),
                    Expr::Result(Ok(Box::new(Expr::Identifier("z".to_string()))))
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
                Expr::Sequence(vec![
                    Expr::Cond(
                        Box::new(Expr::Identifier("foo".to_string())),
                        Box::new(Expr::Identifier("bar".to_string())),
                        Box::new(Expr::Identifier("baz".to_string()))
                    ),
                    Expr::Cond(
                        Box::new(Expr::Identifier("qux".to_string())),
                        Box::new(Expr::Identifier("quux".to_string())),
                        Box::new(Expr::Identifier("quuz".to_string()))
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
                Expr::Sequence(vec![
                    Expr::Tuple(vec![
                        Expr::Identifier("foo".to_string()),
                        Expr::Identifier("bar".to_string())
                    ]),
                    Expr::Tuple(vec![
                        Expr::Identifier("baz".to_string()),
                        Expr::Identifier("qux".to_string())
                    ])
                ]),
                ""
            ))
        );
    }
}

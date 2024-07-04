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
        .map(Expr::Tuple),
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
        assert_eq!(result, Ok((Expr::Tuple(vec![]), "")));
    }

    #[test]
    fn test_singleton_tuple() {
        let input = "(foo)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::Tuple(vec![Expr::Identifier("foo".to_string())]), ""))
        );
    }

    #[test]
    fn test_tuple() {
        let input = "(foo, bar)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Tuple(vec![
                    Expr::Identifier("foo".to_string()),
                    Expr::Identifier("bar".to_string())
                ]),
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
                Expr::Tuple(vec![
                    Expr::Sequence(vec![
                        Expr::Identifier("foo".to_string()),
                        Expr::Identifier("bar".to_string())
                    ]),
                    Expr::Sequence(vec![
                        Expr::Identifier("baz".to_string()),
                        Expr::Identifier("qux".to_string())
                    ])
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
                Expr::Tuple(vec![
                    Expr::Record(vec![(
                        "foo".to_string(),
                        Box::new(Expr::Identifier("bar".to_string()))
                    )]),
                    Expr::Record(vec![(
                        "baz".to_string(),
                        Box::new(Expr::Identifier("qux".to_string()))
                    )])
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
                Expr::Tuple(vec![
                    Expr::Literal("foo".to_string()),
                    Expr::Literal("bar".to_string())
                ]),
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
                Expr::Tuple(vec![
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

    #[test]
    fn test_tuple_of_result() {
        let input = "(ok(foo), err(bar))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Tuple(vec![
                    Expr::Result(Ok(Box::new(Expr::Identifier("foo".to_string())))),
                    Expr::Result(Err(Box::new(Expr::Identifier("bar".to_string()))))
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
                Expr::Tuple(vec![
                    Expr::Option(Some(Box::new(Expr::Identifier("foo".to_string())))),
                    Expr::Option(None)
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
                Expr::Tuple(vec![
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
}

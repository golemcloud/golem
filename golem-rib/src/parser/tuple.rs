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
    between,
    parser::char::{char, spaces},
    sep_by, ParseError, Parser,
};

use super::rib_expr::rib_expr;
use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::rib_source_span::GetSourcePosition;

pub fn tuple<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    spaces().with(
        between(
            char('('),
            char(')'),
            sep_by(rib_expr(), char(',').skip(spaces().silent())),
        )
        .map(Expr::tuple),
    )
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::stream::position;
    use combine::EasyParser;

    use super::*;

    #[test]
    fn test_empty_tuple() {
        let input = "()";
        let result = Expr::from_text(input);
        assert_eq!(result, Ok(Expr::tuple(vec![])));
    }

    #[test]
    fn test_singleton_tuple() {
        let input = "(foo)";
        let result = rib_expr()
            .easy_parse(position::Stream::new(input))
            .map(|x| x.0);
        assert_eq!(
            result,
            Ok(Expr::tuple(vec![Expr::identifier_global("foo", None)]))
        );
    }

    #[test]
    fn test_tuple() {
        let input = "(foo, bar)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::tuple(vec![
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None)
            ]))
        );
    }

    #[test]
    fn test_tuple_of_sequence() {
        let input = "([foo, bar], [baz, qux])";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::tuple(vec![
                Expr::sequence(
                    vec![
                        Expr::identifier_global("foo", None),
                        Expr::identifier_global("bar", None)
                    ],
                    None
                ),
                Expr::sequence(
                    vec![
                        Expr::identifier_global("baz", None),
                        Expr::identifier_global("qux", None)
                    ],
                    None
                )
            ]))
        );
    }

    #[test]
    fn test_tuple_of_record() {
        let input = "({foo: bar}, {baz: qux})";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::tuple(vec![
                Expr::record(vec![(
                    "foo".to_string(),
                    Expr::identifier_global("bar", None)
                )]),
                Expr::record(vec![(
                    "baz".to_string(),
                    Expr::identifier_global("qux", None)
                )])
            ]))
        );
    }

    #[test]
    fn test_tuple_of_literal() {
        let input = "(\"foo\", \"bar\")";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::tuple(vec![
                Expr::literal("foo"),
                Expr::literal("bar")
            ]))
        );
    }

    #[test]
    fn test_tuple_of_tuple() {
        let input = "((foo, bar), (baz, qux))";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::tuple(vec![
                Expr::tuple(vec![
                    Expr::identifier_global("foo", None),
                    Expr::identifier_global("bar", None)
                ]),
                Expr::tuple(vec![
                    Expr::identifier_global("baz", None),
                    Expr::identifier_global("qux", None)
                ])
            ]))
        );
    }

    #[test]
    fn test_tuple_of_result() {
        let input = "(ok(foo), err(bar))";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::tuple(vec![
                Expr::ok(Expr::identifier_global("foo", None), None),
                Expr::err(Expr::identifier_global("bar", None), None)
            ]))
        );
    }

    #[test]
    fn test_tuple_option() {
        let input = "(some(foo), none)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::tuple(vec![
                Expr::option(Some(Expr::identifier_global("foo", None))),
                Expr::option(None)
            ]))
        );
    }

    #[test]
    fn test_tuple_of_cond() {
        let input = "(if foo then bar else baz, if qux then quux else quuz)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::tuple(vec![
                Expr::cond(
                    Expr::identifier_global("foo", None),
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("baz", None)
                ),
                Expr::cond(
                    Expr::identifier_global("qux", None),
                    Expr::identifier_global("quux", None),
                    Expr::identifier_global("quuz", None)
                )
            ]))
        );
    }
}

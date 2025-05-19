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

use combine::parser::char::{char, spaces};
use combine::{between, Parser};
use combine::{sep_by, ParseError};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::rib_expr::rib_expr;
use crate::rib_source_span::GetSourcePosition;

pub fn sequence<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    spaces().with(
        (between(
            char('['),
            char(']'),
            sep_by(rib_expr(), char(',').skip(spaces().silent())),
        ))
        .map(|exprs| Expr::sequence(exprs, None)),
    )
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    #[test]
    fn test_empty_sequence() {
        let input = "[]";
        let result = Expr::from_text(input);
        assert_eq!(result, Ok(Expr::sequence(vec![], None)));
    }

    #[test]
    fn test_singleton_sequence() {
        let input = "[foo]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![Expr::identifier_global("foo", None)],
                None
            ))
        );
    }

    #[test]
    fn test_sequence() {
        let input = "[foo, bar]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![
                    Expr::identifier_global("foo", None),
                    Expr::identifier_global("bar", None)
                ],
                None
            ))
        );
    }

    #[test]
    fn test_sequence_of_not() {
        let input = "[!foo, !bar]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![
                    Expr::not(Expr::identifier_global("foo", None)),
                    Expr::not(Expr::identifier_global("bar", None))
                ],
                None
            ))
        );
    }

    #[test]
    fn test_sequence_of_literal() {
        let input = "[\"foo\", \"bar\"]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![Expr::literal("foo"), Expr::literal("bar")],
                None
            ))
        );
    }

    #[test]
    fn test_sequence_of_sequence() {
        let input = "[[foo, bar], [bar, bar]]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![
                    Expr::sequence(
                        vec![
                            Expr::identifier_global("foo", None),
                            Expr::identifier_global("bar", None)
                        ],
                        None
                    ),
                    Expr::sequence(
                        vec![
                            Expr::identifier_global("bar", None),
                            Expr::identifier_global("bar", None)
                        ],
                        None
                    )
                ],
                None
            ))
        );
    }

    #[test]
    fn test_sequence_of_option() {
        let input = "[some(x), some(y), some(z)]";
        let result = Expr::from_text(input);

        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![
                    Expr::option(Some(Expr::identifier_global("x", None))),
                    Expr::option(Some(Expr::identifier_global("y", None))),
                    Expr::option(Some(Expr::identifier_global("z", None)))
                ],
                None
            ))
        );
    }

    #[test]
    fn test_sequence_of_result() {
        let input = "[ok(x), ok(y), ok(z)]";
        let result = Expr::from_text(input);

        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![
                    Expr::ok(Expr::identifier_global("x", None), None),
                    Expr::ok(Expr::identifier_global("y", None), None),
                    Expr::ok(Expr::identifier_global("z", None), None)
                ],
                None
            ))
        );
    }

    #[test]
    fn test_sequence_of_cond() {
        let input = "[if foo then bar else baz, if qux then quux else quuz]";
        let result = Expr::from_text(input);

        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![
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
                ],
                None
            ))
        );
    }

    #[test]
    fn test_sequence_of_tuple() {
        let input = "[(foo, bar), (baz, qux)]";
        let result = Expr::from_text(input);

        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![
                    Expr::tuple(vec![
                        Expr::identifier_global("foo", None),
                        Expr::identifier_global("bar", None)
                    ]),
                    Expr::tuple(vec![
                        Expr::identifier_global("baz", None),
                        Expr::identifier_global("qux", None)
                    ])
                ],
                None
            ))
        );
    }
}

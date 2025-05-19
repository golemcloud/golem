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

use combine::{parser, ParseError, Stream};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::literal::internal::literal_;
use crate::rib_source_span::GetSourcePosition;

parser! {
    pub fn literal[Input]()(Input) -> Expr
    where [
        Input: Stream<Token = char>,
        RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,
        Input::Position: GetSourcePosition
    ]
    {
        literal_()
    }
}

mod internal {
    use crate::expr::Expr;
    use crate::parser::block::block;
    use crate::parser::errors::RibParseError;
    use crate::rib_source_span::GetSourcePosition;
    use combine::parser::char::char as char_;
    use combine::parser::char::spaces;
    use combine::parser::repeat::many;
    use combine::{between, choice, many1, none_of, ParseError, Parser};

    pub fn literal_<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        spaces().with(
            between(
                char_('\"'),
                char_('\"'),
                many(choice((dynamic_term(), static_term()))),
            )
            .map(|parts: Vec<LiteralTerm>| {
                if parts.is_empty() {
                    Expr::literal("")
                } else if parts.len() == 1 {
                    let first = parts.first().unwrap();
                    match first {
                        LiteralTerm::Static(s) => Expr::literal(s),
                        LiteralTerm::Dynamic(expr) => match expr {
                            Expr::Literal {
                                value, source_span, ..
                            } => Expr::literal(value).with_source_span(source_span.clone()),
                            _ => Expr::concat(vec![expr.clone()]),
                        },
                    }
                } else {
                    Expr::concat(parts.into_iter().map(Expr::from).collect())
                }
            }),
        )
    }

    fn static_term<Input>() -> impl Parser<Input, Output = LiteralTerm>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        many1(none_of("\"${}".chars()))
            .map(LiteralTerm::Static)
            .message("Unable to parse static part of literal")
    }

    fn dynamic_term<Input>() -> impl Parser<Input, Output = LiteralTerm>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        between(
            char_('$').with(char_('{')).skip(spaces()),
            char_('}'),
            block(),
        )
        .map(LiteralTerm::Dynamic)
    }

    enum LiteralTerm {
        Static(String),
        Dynamic(Expr),
    }

    impl From<LiteralTerm> for Expr {
        fn from(term: LiteralTerm) -> Self {
            match term {
                LiteralTerm::Static(s) => Expr::literal(&s),
                LiteralTerm::Dynamic(expr) => expr,
            }
        }
    }
}

#[cfg(test)]
mod literal_parse_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use super::*;

    #[test]
    fn test_empty_literal() {
        let input = "\"\"";
        let result = Expr::from_text(input);
        assert_eq!(result, Ok(Expr::literal("")));
    }

    #[test]
    fn test_literal() {
        let input = "\"foo\"";
        let result = Expr::from_text(input);
        assert_eq!(result, Ok(Expr::literal("foo")));
    }

    #[test]
    fn test_literal_with_interpolation11() {
        let input = "\"foo-${bar}-baz\"";
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::concat(vec![
                Expr::literal("foo-"),
                Expr::identifier_global("bar", None),
                Expr::literal("-baz"),
            ])
        );
    }

    #[test]
    fn test_interpolated_strings_in_if_condition() {
        let input = "if foo == \"bar-${worker_id}\" then 1 else \"baz\"";
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::cond(
                Expr::equal_to(
                    Expr::identifier_global("foo", None),
                    Expr::concat(vec![
                        Expr::literal("bar-"),
                        Expr::identifier_global("worker_id", None)
                    ])
                ),
                Expr::number(BigDecimal::from(1)),
                Expr::literal("baz"),
            )
        );
    }

    #[test]
    fn test_interpolated_strings_with_special_chars() {
        let input = "\"\n\t<>/!@#%&^&*()_+[]; ',.${bar}-ba!z-${qux}\"";
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::concat(vec![
                Expr::literal("\n\t<>/!@#%&^&*()_+[]; ',."),
                Expr::identifier_global("bar", None),
                Expr::literal("-ba!z-"),
                Expr::identifier_global("qux", None),
            ])
        );
    }
}

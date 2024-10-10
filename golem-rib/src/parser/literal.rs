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

use combine::{parser, ParseError, Stream};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::literal::internal::literal_;

parser! {
    pub fn literal[Input]()(Input) -> Expr
    where [
        Input: Stream<Token = char>,
        RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,
    ]
    {
        literal_()
    }
}

mod internal {
    use combine::parser::char::{char as char_, char, letter, space};
    use combine::parser::char::{digit, spaces};
    use combine::parser::repeat::many;
    use combine::{between, choice, many1, sep_by, ParseError, Parser};

    use crate::expr::Expr;
    use crate::parser::errors::RibParseError;
    use crate::parser::rib_expr::rib_expr;

    // Literal can handle string interpolation
    pub fn literal_<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        spaces()
            .with(
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
                                Expr::Literal(s, _) => Expr::literal(s),
                                _ => Expr::concat(vec![expr.clone()]),
                            },
                        }
                    } else {
                        Expr::concat(parts.into_iter().map(Expr::from).collect())
                    }
                }),
            )
            .message("Invalid literal")
    }

    fn static_term<Input>() -> impl Parser<Input, Output = LiteralTerm>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        many1(
            letter().or(space()).or(digit()).or(char_('_').or(char_('-')
                .or(char_('.'))
                .or(char_('/'))
                .or(char_(':').or(char_('@'))))),
        )
        .map(LiteralTerm::Static)
        .message("Unable to parse static part of literal")
    }

    fn dynamic_term<Input>() -> impl Parser<Input, Output = LiteralTerm>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        between(
            char_('$').with(char_('{')).skip(spaces()),
            char_('}'),
            block(),
        )
        .map(LiteralTerm::Dynamic)
    }

    pub fn block<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        spaces().with(
            sep_by(rib_expr().skip(spaces()), char(';').skip(spaces())).map(
                |expressions: Vec<Expr>| {
                    if expressions.len() == 1 {
                        expressions.first().unwrap().clone()
                    } else {
                        Expr::multiple(expressions)
                    }
                },
            ),
        )
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
mod tests {
    use test_r::test;

    use combine::stream::position;
    use combine::EasyParser;

    use crate::parser::rib_expr::{rib_expr, rib_program};

    use super::*;

    #[test]
    fn test_empty_literal() {
        let input = "\"\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::literal(""), "")));
    }

    #[test]
    fn test_literal() {
        let input = "\"foo\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::literal("foo"), "")));
    }

    #[test]
    fn test_literal_with_interpolation11() {
        let input = "\"foo-${bar}-baz\"";
        let result = rib_program()
            .easy_parse(position::Stream::new(input))
            .map(|x| x.0);
        assert_eq!(
            result,
            Ok(Expr::concat(vec![
                Expr::literal("foo-"),
                Expr::identifier("bar"),
                Expr::literal("-baz"),
            ]))
        );
    }

    #[test]
    fn test_interpolated_strings_in_if_condition() {
        let input = "if foo == \"bar-${worker_id}\" then 1 else \"baz\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::cond(
                    Expr::equal_to(
                        Expr::identifier("foo"),
                        Expr::concat(vec![Expr::literal("bar-"), Expr::identifier("worker_id")])
                    ),
                    Expr::number(1f64),
                    Expr::literal("baz"),
                ),
                ""
            ))
        );
    }
}

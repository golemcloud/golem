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

use crate::parser::literal::internal::literal_;
use combine::{easy, parser, Stream};

parser! {
    pub fn literal['t]()(easy::Stream<&'t str>) -> Expr
    where [
        easy::Stream<&'t str>: Stream<Token = char>,
    ]
    {
        literal_()
    }
}

mod internal {
    use crate::expr::Expr;
    use crate::parser::rib_expr::rib_program;
    use combine::parser::char::{char as char_, letter, space};
    use combine::parser::char::{digit, spaces};
    use combine::parser::repeat::many;
    use combine::stream::easy;
    use combine::{attempt, between, choice, many1, Parser};

    // Literal can handle string interpolation
    pub fn literal_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        spaces().with(
            between(
                char_('\"'),
                char_('\"'),
                many(choice((attempt(interpolation()), static_part()))),
            )
            .map(|parts: Vec<Expr>| {
                if parts.is_empty() {
                    Expr::Literal("".to_string())
                } else if parts.len() == 1 {
                    parts.first().unwrap().clone()
                } else {
                    Expr::Concat(parts)
                }
            })
            .message("Unable to parse literal"),
        )
    }

    fn static_part<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        many1(
            letter()
                .or(space())
                .or(digit())
                .or(char_('_').or(char_('-').or(char_('.')).or(char_('/')).or(char_(':')))),
        )
        .map(|s: String| Expr::Literal(s))
        .message("Unable to parse static part of literal")
    }

    fn interpolation<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        between(
            char_('$').with(char_('{')).skip(spaces()),
            char_('}'),
            rib_program(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::rib_expr::rib_expr;
    use combine::EasyParser;

    #[test]
    fn test_empty_literal() {
        let input = "\"\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Literal("".to_string()), "")));
    }

    #[test]
    fn test_literal() {
        let input = "\"foo\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Literal("foo".to_string()), "")));
    }

    #[test]
    fn test_literal_with_interpolation() {
        let input = "\"foo-${bar}-baz\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Concat(vec![
                    Expr::Literal("foo-".to_string()),
                    Expr::Identifier("bar".to_string()),
                    Expr::Literal("-baz".to_string()),
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_interpolated_strings_in_if_condition() {
        let input = "if foo == \"bar-${worker_id}\" then 1 else \"baz\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Cond(
                    Box::new(Expr::EqualTo(
                        Box::new(Expr::Identifier("foo".to_string())),
                        Box::new(Expr::Concat(vec![
                            Expr::Literal("bar-".to_string()),
                            Expr::Identifier("worker_id".to_string())
                        ]))
                    )),
                    Box::new(Expr::unsigned_integer(1)),
                    Box::new(Expr::Literal("baz".to_string())),
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_direct_interpolation() {
        let input = "\"${foo}\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Identifier("foo".to_string()), "")));
    }

    #[test]
    fn test_direct_interpolation_flag() {
        let input = "\"${{foo}}\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Flags(vec!["foo".to_string()]), "")));
    }
}

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

use combine::parser::char::{char as char_, letter, spaces};
use combine::stream::easy;
use combine::{attempt, choice, many1, Parser, Stream};

use crate::expr::Expr;
use crate::parser::record::record;

use crate::parser::identifier::identifier;

use combine::parser;
use internal::*;

parser! {
    pub fn select_field['t]()(easy::Stream<&'t str>) -> Expr
    where [
        easy::Stream<&'t str>: Stream<Token = char>,
    ]
    {
        select_field_()
    }
}

mod internal {
    use crate::parser::select_index::select_index;

    use super::*;
    use combine::error::StreamError;
    use combine::parser::char::char;
    use combine::parser::char::digit;

    // We make base_expr and the children strict enough carefully, to avoid
    // stack overflow without affecting the grammer.
    pub(crate) fn select_field_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        spaces().with(
            (
                base_expr(),
                char('.').skip(spaces()),
                choice((
                    attempt(select_field()),
                    attempt(select_index()),
                    attempt(identifier()),
                )),
            )
                .and_then(|(base, _, opt)| {
                    build_selector(base, opt).ok_or(easy::Error::message_static_message(
                        "Invalid field/index selection",
                    ))
                }),
        )
    }

    // To avoid stack overflow, we reverse the field selection to avoid direct recursion to be the first step
    // but we offload this recursion in `build-selector`.
    // This implies the last expression after a dot could be an index selection or a field selection
    // and with `inner select` we accumulate the selection towards the left side
    // This will not affect the grammer, however, refactoring this logic should fail for some tests
    fn build_selector(base: Expr, nest: Expr) -> Option<Expr> {
        // a.b
        match nest {
            Expr::Identifier(str) => Some(Expr::SelectField(Box::new(base), str)),
            Expr::SelectField(second, last) => {
                let inner_select = build_selector(base, *second)?;
                Some(Expr::SelectField(Box::new(inner_select), last))
            }
            Expr::SelectIndex(second, last_index) => {
                let inner_select = build_selector(base, *second)?;
                Some(Expr::SelectIndex(Box::new(inner_select), last_index))
            }
            _ => None,
        }
    }

    pub(crate) fn base_expr<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        choice((
            attempt(select_index()),
            attempt(record()),
            attempt(field_name().map(Expr::Identifier)),
        ))
    }

    pub(crate) fn field_name<'t>() -> impl Parser<easy::Stream<&'t str>, Output = String> {
        many1(letter().or(digit()).or(char_('-')).or(char_('_')))
            .map(|s: Vec<char>| s.into_iter().collect())
            .message("Unable to parse field name")
    }
}

#[cfg(test)]
mod tests {
    use crate::expr::*;
    use crate::parser::rib_expr::rib_expr;
    use combine::EasyParser;

    #[test]
    fn test_select_field() {
        let input = "foo.bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectField(
                    Box::new(Expr::Identifier("foo".to_string())),
                    "bar".to_string()
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_select_field_from_record() {
        let input = "{foo: bar}.foo";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectField(
                    Box::new(Expr::Record(vec![(
                        "foo".to_string(),
                        Box::new(Expr::Identifier("bar".to_string()))
                    )])),
                    "foo".to_string()
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_nested_field_selection() {
        let input = "foo.bar.baz";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectField(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("foo".to_string())),
                        "bar".to_string()
                    )),
                    "baz".to_string()
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_recursive_select_index_in_select_field() {
        let input = "foo[0].bar[1]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectIndex(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::SelectIndex(
                            Box::new(Expr::Identifier("foo".to_string())),
                            0
                        )),
                        "bar".to_string()
                    )),
                    1
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_recursive_select_field_in_select_index() {
        let input = "foo.bar[0].baz";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectField(
                    Box::new(Expr::SelectIndex(
                        Box::new(Expr::SelectField(
                            Box::new(Expr::Identifier("foo".to_string())),
                            "bar".to_string()
                        )),
                        0
                    )),
                    "baz".to_string()
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_selection_field_with_binary_comparison_1() {
        let result = rib_expr().easy_parse("foo.bar > \"bar\"");
        assert_eq!(
            result,
            Ok((
                Expr::GreaterThan(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("foo".to_string())),
                        "bar".to_string()
                    )),
                    Box::new(Expr::Literal("bar".to_string()))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_selection_field_with_binary_comparison_2() {
        let result = rib_expr().easy_parse("foo.bar > 1");
        assert_eq!(
            result,
            Ok((
                Expr::GreaterThan(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("foo".to_string())),
                        "bar".to_string()
                    )),
                    Box::new(Expr::unsigned_integer(1))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_select_field_in_if_condition() {
        let input = "if foo.bar > 1 then foo.bar else foo.baz";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Cond(
                    Box::new(Expr::GreaterThan(
                        Box::new(Expr::SelectField(
                            Box::new(Expr::Identifier("foo".to_string())),
                            "bar".to_string()
                        )),
                        Box::new(Expr::unsigned_integer(1))
                    )),
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("foo".to_string())),
                        "bar".to_string()
                    )),
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("foo".to_string())),
                        "baz".to_string()
                    ))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_selection_field_in_match_expr() {
        let input = "match foo { _ => bar, ok(x) => x, err(x) => x, none => foo, some(x) => x, foo => foo.bar }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::PatternMatch(
                    Box::new(Expr::Identifier("foo".to_string())),
                    vec![
                        MatchArm((
                            ArmPattern::WildCard,
                            Box::new(Expr::Identifier("bar".to_string()))
                        )),
                        MatchArm((
                            ArmPattern::Literal(Box::new(Expr::Result(Ok(Box::new(
                                Expr::Identifier("x".to_string())
                            ))))),
                            Box::new(Expr::Identifier("x".to_string()))
                        )),
                        MatchArm((
                            ArmPattern::Literal(Box::new(Expr::Result(Err(Box::new(
                                Expr::Identifier("x".to_string())
                            ))))),
                            Box::new(Expr::Identifier("x".to_string()))
                        )),
                        MatchArm((
                            ArmPattern::Literal(Box::new(Expr::Option(None))),
                            Box::new(Expr::Identifier("foo".to_string()))
                        )),
                        MatchArm((
                            ArmPattern::Literal(Box::new(Expr::Option(Some(Box::new(
                                Expr::Identifier("x".to_string())
                            ))))),
                            Box::new(Expr::Identifier("x".to_string()))
                        )),
                        MatchArm((
                            ArmPattern::Literal(Box::new(Expr::Identifier("foo".to_string()))),
                            Box::new(Expr::SelectField(
                                Box::new(Expr::Identifier("foo".to_string())),
                                "bar".to_string()
                            ))
                        )),
                    ]
                ),
                ""
            ))
        );
    }
}

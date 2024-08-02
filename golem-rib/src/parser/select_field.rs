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
            Expr::Identifier(variable_id, _) => {
                Some(Expr::select_field(base, variable_id.name().as_str()))
            }
            Expr::SelectField(second, last, _) => {
                let inner_select = build_selector(base, *second)?;
                Some(Expr::select_field(inner_select, last.as_str()))
            }
            Expr::SelectIndex(second, last_index, _) => {
                let inner_select = build_selector(base, *second)?;
                Some(Expr::select_index(inner_select, last_index))
            }
            _ => None,
        }
    }

    pub(crate) fn base_expr<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        choice((
            attempt(select_index()),
            attempt(record()),
            attempt(field_name().map(|s| Expr::identifier(s.as_str()))),
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
                Expr::select_field(Expr::identifier("foo".to_string()), "bar".to_string()),
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
                Expr::select_field(
                    Expr::record(vec![(
                        "foo".to_string(),
                        Expr::identifier("bar".to_string())
                    )]),
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
                Expr::select_field(
                    Expr::select_field(Expr::identifier("foo".to_string()), "bar".to_string()),
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
                Expr::select_index(
                    Expr::select_field(
                        Expr::select_index(Expr::identifier("foo".to_string()), 0),
                        "bar".to_string()
                    ),
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
                Expr::select_field(
                    Expr::select_index(
                        Expr::select_field(Expr::identifier("foo".to_string()), "bar".to_string()),
                        0
                    ),
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
                Expr::greater_than(
                    Expr::select_field(Expr::identifier("foo".to_string()), "bar".to_string()),
                    Expr::literal("bar".to_string())
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
                Expr::greater_than(
                    Expr::select_field(Expr::identifier("foo".to_string()), "bar".to_string()),
                    Expr::number(1f64)
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
                Expr::cond(
                    Expr::greater_than(
                        Expr::select_field(Expr::identifier("foo".to_string()), "bar".to_string()),
                        Expr::number(1f64)
                    ),
                    Expr::select_field(Expr::identifier("foo".to_string()), "bar".to_string()),
                    Expr::select_field(Expr::identifier("foo".to_string()), "baz".to_string())
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
                Expr::pattern_match(
                    Expr::identifier("foo".to_string()),
                    vec![
                        MatchArm::match_arm(
                            ArmPattern::WildCard,
                            Expr::identifier("bar".to_string())
                        ),
                        MatchArm::match_arm(
                            ArmPattern::Literal(Box::new(Expr::ok(Expr::identifier(
                                "x".to_string()
                            )))),
                            Expr::identifier("x".to_string())
                        ),
                        MatchArm::match_arm(
                            ArmPattern::Literal(Box::new(Expr::err(Expr::identifier(
                                "x".to_string()
                            )))),
                            Expr::identifier("x".to_string())
                        ),
                        MatchArm::match_arm(
                            ArmPattern::Literal(Box::new(Expr::option(None))),
                            Expr::identifier("foo".to_string())
                        ),
                        MatchArm::match_arm(
                            ArmPattern::Literal(Box::new(Expr::option(Some(Expr::identifier(
                                "x".to_string()
                            ))))),
                            Expr::identifier("x".to_string())
                        ),
                        MatchArm::match_arm(
                            ArmPattern::Literal(Box::new(Expr::identifier("foo".to_string()))),
                            Expr::select_field(
                                Expr::identifier("foo".to_string()),
                                "bar".to_string()
                            )
                        ),
                    ]
                ),
                ""
            ))
        );
    }
}

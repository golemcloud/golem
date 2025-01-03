// Copyright 2024-2025 Golem Cloud
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

use combine::parser;
use combine::parser::char::spaces;
use combine::{attempt, choice, ParseError, Parser, Stream};

use internal::*;

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::identifier::identifier;
use crate::parser::record::record;

parser! {
    pub fn select_field[Input]()(Input) -> Expr
    where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,]
    {
        select_field_()
    }
}

mod internal {
    use combine::parser::char::{char, digit, letter};
    use combine::{many1, ParseError};

    use crate::parser::errors::RibParseError;
    use crate::parser::select_index::select_index;

    use super::*;

    // We make base_expr and the children strict enough carefully, to avoid
    // stack overflow without affecting the grammer.
    pub(crate) fn select_field_<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
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
                .map(|(base, _, opt)| {
                    build_selector(base, opt).expect("Invalid field/index selection")
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

    fn base_expr<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        choice((
            attempt(select_index()),
            attempt(record()),
            attempt(field_name().map(|s| Expr::identifier(s.as_str()))),
        ))
    }

    fn field_name<Input>() -> impl Parser<Input, Output = String>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        text().message("Unable to parse field name")
    }

    fn text<Input>() -> impl Parser<Input, Output = String>
    where
        Input: Stream<Token = char>,
    {
        many1(letter().or(digit()).or(char('_').or(char('-'))))
            .map(|s: Vec<char>| s.into_iter().collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use combine::EasyParser;

    use crate::expr::*;
    use crate::parser::rib_expr::rib_expr;

    #[test]
    fn test_select_field() {
        let input = "foo.bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::select_field(Expr::identifier("foo"), "bar"), ""))
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
                    Expr::record(vec![("foo".to_string(), Expr::identifier("bar"))]),
                    "foo"
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
                Expr::select_field(Expr::select_field(Expr::identifier("foo"), "bar"), "baz"),
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
                    Expr::select_field(Expr::select_index(Expr::identifier("foo"), 0), "bar"),
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
                    Expr::select_index(Expr::select_field(Expr::identifier("foo"), "bar"), 0),
                    "baz"
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
                    Expr::select_field(Expr::identifier("foo"), "bar"),
                    Expr::literal("bar")
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
                    Expr::select_field(Expr::identifier("foo"), "bar"),
                    Expr::untyped_number(BigDecimal::from(1))
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
                        Expr::select_field(Expr::identifier("foo"), "bar"),
                        Expr::untyped_number(BigDecimal::from(1))
                    ),
                    Expr::select_field(Expr::identifier("foo"), "bar"),
                    Expr::select_field(Expr::identifier("foo"), "baz")
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
                    Expr::identifier("foo"),
                    vec![
                        MatchArm::new(ArmPattern::WildCard, Expr::identifier("bar")),
                        MatchArm::new(
                            ArmPattern::constructor(
                                "ok",
                                vec![ArmPattern::Literal(Box::new(Expr::identifier("x")))]
                            ),
                            Expr::identifier("x"),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor(
                                "err",
                                vec![ArmPattern::Literal(Box::new(Expr::identifier("x")))]
                            ),
                            Expr::identifier("x"),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor("none", vec![]),
                            Expr::identifier("foo"),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor(
                                "some",
                                vec![ArmPattern::Literal(Box::new(Expr::identifier("x")))]
                            ),
                            Expr::identifier("x"),
                        ),
                        MatchArm::new(
                            ArmPattern::Literal(Box::new(Expr::identifier("foo"))),
                            Expr::select_field(Expr::identifier("foo"), "bar"),
                        ),
                    ]
                ),
                ""
            ))
        );
    }
}

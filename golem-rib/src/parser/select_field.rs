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
use combine::{choice, ParseError, Stream};

use internal::*;

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::record::record;
use crate::rib_source_span::GetSourcePosition;

parser! {
    pub fn select_field[Input]()(Input) -> Expr
    where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>, Input::Position: GetSourcePosition]
    {
        select_field_()
    }
}

mod internal {
    use combine::parser::char::{char, digit, letter};
    use combine::{many1, optional, ParseError};
    use std::ops::Deref;

    use super::*;
    use crate::parser::errors::RibParseError;
    use crate::parser::identifier::identifier_text;
    use crate::parser::select_index::select_index;
    use crate::parser::type_name::parse_type_name;
    use crate::rib_source_span::GetSourcePosition;
    use combine::{
        attempt,
        parser::char::{char as char_, spaces},
        Parser,
    };

    // We make base_expr and the children strict enough carefully, to avoid
    // stack overflow without affecting the grammer.
    pub(crate) fn select_field_<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        spaces().with(
            (
                base_expr(),
                char('.').skip(spaces()),
                choice((
                    attempt(select_field()),
                    attempt(select_index()),
                    attempt(identifier_text().map(|x| Expr::identifier_global(x, None))),
                )),
                optional(
                    char_(':')
                        .skip(spaces())
                        .with(parse_type_name())
                        .skip(spaces()),
                ),
            )
                .and_then(|(base, _, opt, optional)| {
                    let expr = build_selector(base, opt);

                    match expr {
                        Some(Expr::SelectField {
                            field,
                            expr,
                            type_annotation: inner_typ,
                            ..
                        }) => {
                            if let Some(typ) = optional {
                                Ok(Expr::select_field(expr.deref().clone(), field, Some(typ)))
                            } else {
                                Ok(Expr::select_field(expr.deref().clone(), field, inner_typ))
                            }
                        }

                        Some(Expr::SelectIndex {
                            expr,
                            index,
                            type_annotation: inner_typ,
                            inferred_type,
                            source_span,
                        }) => {
                            if let Some(typ) = optional {
                                Ok(Expr::select_index_with_type_annotation(
                                    expr.deref().clone(),
                                    index,
                                    typ,
                                ))
                            } else {
                                Ok(Expr::SelectIndex {
                                    expr,
                                    index,
                                    type_annotation: inner_typ,
                                    inferred_type,
                                    source_span,
                                })
                            }
                        }

                        _ => Err(RibParseError::Message("Invalid Select Index".to_string())),
                    }
                }),
        )
    }

    // To avoid stack overflow, we reverse the field selection to avoid direct recursion to be the first step
    // but we offload this recursion in `build-selector`.
    // This implies the last expression after a dot could be an index selection or a field selection
    // and with `inner select` we accumulate the selection towards the left side.
    // We also propagate any type name in between towards the outer.
    fn build_selector(base: Expr, nest: Expr) -> Option<Expr> {
        match nest {
            Expr::Identifier { variable_id, .. } => {
                Some(Expr::select_field(base, variable_id.name().as_str(), None))
            }
            Expr::SelectField {
                expr: second,
                field: last,
                type_annotation: type_name,
                inferred_type,
                source_span,
            } => {
                let inner_select = build_selector(base, *second)?;
                Some(Expr::SelectField {
                    expr: Box::new(inner_select),
                    field: last,
                    type_annotation: type_name,
                    inferred_type,
                    source_span,
                })
            }
            Expr::SelectIndex {
                expr: second,
                index: last_index,
                type_annotation: type_name,
                inferred_type,
                source_span,
            } => {
                let inner_select = build_selector(base, *second)?;
                Some(Expr::SelectIndex {
                    expr: Box::new(inner_select),
                    index: last_index,
                    type_annotation: type_name,
                    inferred_type,
                    source_span,
                })
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
        Input::Position: GetSourcePosition,
    {
        choice((
            attempt(select_index()),
            attempt(record()),
            attempt(field_name().map(|s| Expr::identifier_global(s.as_str(), None))),
        ))
    }

    fn field_name<Input>() -> impl Parser<Input, Output = String>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
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

    use crate::expr::*;
    use crate::TypeName;

    #[test]
    fn test_select_field() {
        let input = "foo.bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::identifier_global("foo", None),
                "bar",
                None
            ))
        );
    }

    #[test]
    fn test_select_field_with_type_annotation() {
        let input = "foo.bar: u32";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::identifier_global("foo", None),
                "bar",
                Some(TypeName::U32)
            ))
        );
    }

    #[test]
    fn test_select_field_from_record() {
        let input = "{foo: bar}.foo";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::record(vec![(
                    "foo".to_string(),
                    Expr::identifier_global("bar", None)
                )]),
                "foo",
                None
            ))
        );
    }

    #[test]
    fn test_select_field_from_record_with_type_annotation() {
        let input = "{foo: bar}.foo: u32";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::record(vec![(
                    "foo".to_string(),
                    Expr::identifier_global("bar", None)
                )]),
                "foo",
                Some(TypeName::U32)
            ))
        );
    }

    #[test]
    fn test_nested_field_selection() {
        let input = "foo.bar.baz";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                "baz",
                None
            ))
        );
    }

    #[test]
    fn test_nested_field_selection_with_type_annotation() {
        let input = "foo.bar.baz: u32";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                "baz",
                Some(TypeName::U32)
            ))
        );
    }

    #[test]
    fn test_nested_field_selection_with_double_type_annotation() {
        let input = "foo.bar: u32.baz: u32";
        let result = Expr::from_text(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_recursive_select_index_in_select_field() {
        let input = "foo[0].bar[1]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_index(
                Expr::select_field(
                    Expr::select_index(Expr::identifier_global("foo", None), 0),
                    "bar",
                    None
                ),
                1
            ))
        );
    }

    #[test]
    fn test_recursive_select_index_in_select_field_with_type_annotation() {
        let input = "foo[0].bar[1]: u32";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_index_with_type_annotation(
                Expr::select_field(
                    Expr::select_index(Expr::identifier_global("foo", None), 0),
                    "bar",
                    None
                ),
                1,
                TypeName::U32
            ))
        );
    }

    #[test]
    fn test_recursive_select_field_in_select_index() {
        let input = "foo.bar[0].baz";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_field(
                Expr::select_index(
                    Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                    0
                ),
                "baz",
                None
            ))
        );
    }

    #[test]
    fn test_selection_field_with_binary_comparison_1() {
        let result = Expr::from_text("foo.bar > \"bar\"");
        assert_eq!(
            result,
            Ok(Expr::greater_than(
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                Expr::literal("bar")
            ))
        );
    }

    #[test]
    fn test_selection_field_with_binary_comparison_2() {
        let result = Expr::from_text("foo.bar > 1");
        assert_eq!(
            result,
            Ok(Expr::greater_than(
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                Expr::untyped_number(BigDecimal::from(1))
            ))
        );
    }

    #[test]
    fn test_select_field_in_if_condition() {
        let input = "if foo.bar > 1 then foo.bar else foo.baz";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::cond(
                Expr::greater_than(
                    Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                    Expr::untyped_number(BigDecimal::from(1))
                ),
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                Expr::select_field(Expr::identifier_global("foo", None), "baz", None)
            ))
        );
    }

    #[test]
    fn test_selection_field_in_match_expr() {
        let input = "match foo { _ => bar, ok(x) => x, err(x) => x, none => foo, some(x) => x, foo => foo.bar }";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::pattern_match(
                Expr::identifier_global("foo", None),
                vec![
                    MatchArm::new(ArmPattern::WildCard, Expr::identifier_global("bar", None)),
                    MatchArm::new(
                        ArmPattern::constructor(
                            "ok",
                            vec![ArmPattern::Literal(Box::new(Expr::identifier_global(
                                "x", None
                            )))]
                        ),
                        Expr::identifier_global("x", None),
                    ),
                    MatchArm::new(
                        ArmPattern::constructor(
                            "err",
                            vec![ArmPattern::Literal(Box::new(Expr::identifier_global(
                                "x", None
                            )))]
                        ),
                        Expr::identifier_global("x", None),
                    ),
                    MatchArm::new(
                        ArmPattern::constructor("none", vec![]),
                        Expr::identifier_global("foo", None),
                    ),
                    MatchArm::new(
                        ArmPattern::constructor(
                            "some",
                            vec![ArmPattern::Literal(Box::new(Expr::identifier_global(
                                "x", None
                            )))]
                        ),
                        Expr::identifier_global("x", None),
                    ),
                    MatchArm::new(
                        ArmPattern::Literal(Box::new(Expr::identifier_global("foo", None))),
                        Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                    ),
                ]
            ))
        );
    }
}

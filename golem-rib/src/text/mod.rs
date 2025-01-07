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

use crate::expr::Expr;
use crate::ArmPattern;

mod writer;

use crate::text::writer::WriterError;

pub fn from_string(input: impl AsRef<str>) -> Result<Expr, String> {
    let trimmed = input.as_ref().trim();

    // This check is kept for backward compatibility to support rib programs that were wrapped in `${..}`
    // Rib's grammar doesn't support wrapping the expressions in`${}` anymore, and therefore
    // we unwrap before calling Expr::from_text
    if trimmed.starts_with("${") && trimmed.ends_with("}") {
        let trimmed_open = trimmed.strip_prefix("${").unwrap();
        let trimmed_closing = trimmed_open.strip_suffix('}').unwrap();
        Expr::from_text(trimmed_closing)
    } else {
        Expr::from_text(input.as_ref())
    }
}

pub fn to_string(expr: &Expr) -> Result<String, WriterError> {
    writer::write_expr(expr)
}

pub fn to_string_arm_pattern(arm_pattern: &ArmPattern) -> Result<String, WriterError> {
    writer::write_arm_pattern(arm_pattern)
}

#[cfg(test)]
mod interpolation_tests {
    use test_r::test;

    use crate::{text, Expr};

    #[test]
    fn test_expr_wrapped_in_interpolation() {
        let input = r#"${foo}"#;
        let result = text::from_string(input);
        assert_eq!(result, Ok(Expr::identifier("foo")));

        let input = r#"${{foo}}"#;
        let result = text::from_string(input);
        assert_eq!(result, Ok(Expr::flags(vec!["foo".to_string()])));

        let input = r#"${{foo: "bar"}}"#;
        let result = text::from_string(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![(
                "foo".to_string(),
                Expr::literal("bar")
            )]))
        );
    }
}

#[cfg(test)]
mod record_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::expr::*;
    use crate::text::{from_string, to_string, Expr};
    use crate::MatchArm;

    #[test]
    fn test_round_trip_simple_record_single() {
        let input_expr = Expr::record(vec![("field".to_string(), Expr::identifier("request"))]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "{field: request}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_multiple() {
        let input_expr = Expr::record(vec![
            ("field".to_string(), Expr::identifier("request")),
            ("field".to_string(), Expr::identifier("request")),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "{field: request, field: request}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_literal() {
        let input_expr = Expr::record(vec![
            ("field".to_string(), Expr::literal("hello")),
            ("field".to_string(), Expr::literal("world")),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"{field: "hello", field: "world"}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_number() {
        let input_expr = Expr::record(vec![
            (
                "field".to_string(),
                Expr::untyped_number(BigDecimal::from(1)),
            ),
            (
                "field".to_string(),
                Expr::untyped_number(BigDecimal::from(2)),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "{field: 1, field: 2}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_select_field() {
        let input_expr = Expr::record(vec![
            (
                "field".to_string(),
                Expr::select_field(Expr::identifier("request"), "foo"),
            ),
            (
                "field".to_string(),
                Expr::select_field(Expr::identifier("request"), "bar"),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "{field: request.foo, field: request.bar}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_select_index() {
        let input_expr = Expr::record(vec![
            (
                "field".to_string(),
                Expr::select_index(Expr::identifier("request"), 1),
            ),
            (
                "field".to_string(),
                Expr::select_index(Expr::identifier("request"), 2),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "{field: request[1], field: request[2]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_sequence() {
        let input_expr = Expr::record(vec![
            (
                "field".to_string(),
                Expr::sequence(vec![
                    Expr::identifier("request"),
                    Expr::identifier("request"),
                ]),
            ),
            (
                "field".to_string(),
                Expr::sequence(vec![
                    Expr::identifier("request"),
                    Expr::identifier("request"),
                ]),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "{field: [request, request], field: [request, request]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_record() {
        let input_expr = Expr::record(vec![
            (
                "a".to_string(),
                Expr::record(vec![
                    ("ab".to_string(), Expr::identifier("request")),
                    ("ac".to_string(), Expr::identifier("request")),
                ]),
            ),
            (
                "b".to_string(),
                Expr::sequence(vec![Expr::record(vec![
                    ("bc".to_string(), Expr::identifier("request")),
                    ("bd".to_string(), Expr::identifier("request")),
                ])]),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str =
            "{a: {ab: request, ac: request}, b: [{bc: request, bd: request}]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_tuple() {
        let input_expr = Expr::record(vec![
            (
                "a".to_string(),
                Expr::tuple(vec![
                    Expr::identifier("request"),
                    Expr::identifier("worker"),
                ]),
            ),
            (
                "b".to_string(),
                Expr::tuple(vec![
                    Expr::identifier("request"),
                    Expr::identifier("worker"),
                ]),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = "{a: (request, worker), b: (request, worker)}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_flags() {
        let input_expr = Expr::record(vec![
            (
                "a".to_string(),
                Expr::flags(vec!["flag1".to_string(), "flag2".to_string()]),
            ),
            (
                "b".to_string(),
                Expr::flags(vec!["flag3".to_string(), "flag4".to_string()]),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = "{a: {flag1, flag2}, b: {flag3, flag4}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_boolean() {
        let input_expr = Expr::record(vec![
            ("a".to_string(), Expr::boolean(true)),
            ("b".to_string(), Expr::boolean(false)),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = "{a: true, b: false}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_concatenation() {
        let input_expr = Expr::record(vec![
            (
                "a".to_string(),
                Expr::concat(vec![
                    Expr::literal("user-id-1-"),
                    Expr::select_field(Expr::identifier("request"), "user-id-1"),
                ]),
            ),
            (
                "b".to_string(),
                Expr::concat(vec![
                    Expr::literal("user-id-2-"),
                    Expr::select_field(Expr::identifier("request"), "user-id-2"),
                ]),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str =
            r#"{a: "user-id-1-${request.user-id-1}", b: "user-id-2-${request.user-id-2}"}"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_math_op() {
        let input_expr = Expr::record(vec![
            (
                "a".to_string(),
                Expr::greater_than(
                    Expr::untyped_number(BigDecimal::from(1)),
                    Expr::untyped_number(BigDecimal::from(2)),
                ),
            ),
            (
                "b".to_string(),
                Expr::less_than(
                    Expr::untyped_number(BigDecimal::from(1)),
                    Expr::untyped_number(BigDecimal::from(2)),
                ),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = "{a: 1 > 2, b: 1 < 2}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_if_condition() {
        let input_expr = Expr::record(vec![
            (
                "a".to_string(),
                Expr::cond(
                    Expr::equal_to(
                        Expr::select_field(Expr::identifier("request"), "foo"),
                        Expr::literal("bar"),
                    ),
                    Expr::literal("success"),
                    Expr::literal("failed"),
                ),
            ),
            (
                "b".to_string(),
                Expr::cond(
                    Expr::equal_to(
                        Expr::select_field(Expr::identifier("request"), "foo"),
                        Expr::literal("bar"),
                    ),
                    Expr::literal("success"),
                    Expr::literal("failed"),
                ),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = r#"{a: if request.foo == "bar" then "success" else "failed", b: if request.foo == "bar" then "success" else "failed"}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_pattern_match() {
        let input_expr = Expr::record(vec![
            (
                "a".to_string(),
                Expr::pattern_match(
                    Expr::identifier("request"),
                    vec![
                        MatchArm::new(
                            ArmPattern::constructor(
                                "ok",
                                vec![ArmPattern::literal(Expr::identifier("foo"))],
                            ),
                            Expr::literal("success"),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor(
                                "err",
                                vec![ArmPattern::literal(Expr::identifier("msg"))],
                            ),
                            Expr::literal("failure"),
                        ),
                    ],
                ),
            ),
            (
                "b".to_string(),
                Expr::pattern_match(
                    Expr::identifier("request"),
                    vec![
                        MatchArm::new(
                            ArmPattern::constructor(
                                "ok",
                                vec![ArmPattern::literal(Expr::identifier("foo"))],
                            ), // Use Constructor for ok
                            Expr::literal("success"),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor(
                                "err",
                                vec![ArmPattern::literal(Expr::identifier("msg"))],
                            ),
                            Expr::pattern_match(
                                Expr::identifier("request"),
                                vec![
                                    MatchArm::new(
                                        ArmPattern::constructor(
                                            "ok",
                                            vec![ArmPattern::literal(Expr::identifier("foo"))],
                                        ),
                                        Expr::literal("success"),
                                    ),
                                    MatchArm::new(
                                        ArmPattern::constructor(
                                            "err",
                                            vec![ArmPattern::literal(Expr::identifier("msg"))],
                                        ),
                                        Expr::literal("failure"),
                                    ),
                                ],
                            ),
                        ),
                    ],
                ),
            ),
        ]);

        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = r#"{a: match request {  ok(foo) => "success", err(msg) => "failure" } , b: match request {  ok(foo) => "success", err(msg) => match request {  ok(foo) => "success", err(msg) => "failure" }  } }"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_record_of_constructor() {
        let input_expr = Expr::record(vec![
            ("a".to_string(), Expr::ok(Expr::literal("foo"))),
            ("b".to_string(), Expr::err(Expr::literal("msg"))),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = r#"{a: ok("foo"), b: err("msg")}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_record_literal_invalid() {
        let expr_str = r#"
                 {body: golem:component/api.{get-character}(), headers: { x-test: 'foobar' } }
            "#;

        let result = from_string(expr_str);

        assert!(result.is_err());

        let expr_str = r#"
                {body: golem:component/api.{get-character}(), headers: { x-test: "foobar" } }
            "#;

        let result = from_string(expr_str);

        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod sequence_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::expr::Expr;
    use crate::text::{from_string, to_string};
    use crate::{ArmPattern, MatchArm};

    #[test]
    fn test_round_trip_read_write_sequence_empty() {
        let input_expr = Expr::sequence(vec![]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    // A few non-round-trip text based tests
    #[test]
    fn test_sequence_of_records_singleton() {
        let expr_string = "[{bc: request}]";
        let output_expr = from_string(expr_string).unwrap();
        let expected_expr = Expr::sequence(vec![Expr::record(vec![(
            "bc".to_string(),
            Expr::identifier("request"),
        )])]);
        assert_eq!(output_expr, expected_expr);
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_request() {
        let input_expr = Expr::sequence(vec![
            Expr::identifier("request"),
            Expr::identifier("request"),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[request, request]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_literal() {
        let input_expr = Expr::sequence(vec![Expr::literal("hello"), Expr::literal("world")]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"["hello", "world"]"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_select_field() {
        let input_expr = Expr::sequence(vec![
            Expr::select_field(Expr::identifier("request"), "field"),
            Expr::select_field(Expr::identifier("request"), "field"),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[request.field, request.field]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_select_index() {
        let input_expr = Expr::sequence(vec![
            Expr::select_index(Expr::identifier("request"), 1),
            Expr::select_index(Expr::identifier("request"), 2),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[request[1], request[2]]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_sequence() {
        let input_expr = Expr::sequence(vec![
            Expr::sequence(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
            Expr::sequence(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[[request, request], [request, request]]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_tuple() {
        let input_expr = Expr::sequence(vec![
            Expr::tuple(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
            Expr::tuple(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[(request, request), (request, request)]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_record() {
        let input_expr = Expr::sequence(vec![
            Expr::record(vec![("field".to_string(), Expr::identifier("request"))]),
            Expr::record(vec![("field".to_string(), Expr::identifier("request"))]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[{field: request}, {field: request}]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_flags() {
        let input_expr = Expr::sequence(vec![
            Expr::flags(vec!["flag1".to_string(), "flag2".to_string()]),
            Expr::flags(vec!["flag3".to_string(), "flag4".to_string()]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[{flag1, flag2}, {flag3, flag4}]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_concat() {
        let input_expr = Expr::sequence(vec![
            Expr::concat(vec![
                Expr::literal("user-id-1-"),
                Expr::select_field(Expr::identifier("request"), "user-id-1"),
            ]),
            Expr::concat(vec![
                Expr::literal("user-id-2-"),
                Expr::select_field(Expr::identifier("request"), "user-id-2"),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"["user-id-1-${request.user-id-1}", "user-id-2-${request.user-id-2}"]"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_math_op() {
        let input_expr = Expr::sequence(vec![
            Expr::greater_than(
                Expr::untyped_number(BigDecimal::from(1)),
                Expr::untyped_number(BigDecimal::from(2)),
            ),
            Expr::less_than(
                Expr::untyped_number(BigDecimal::from(1)),
                Expr::untyped_number(BigDecimal::from(2)),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[1 > 2, 1 < 2]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_if_condition() {
        let input_expr = Expr::sequence(vec![
            Expr::cond(
                Expr::equal_to(
                    Expr::select_field(Expr::identifier("request"), "foo"),
                    Expr::literal("bar"),
                ),
                Expr::literal("success"),
                Expr::literal("failed"),
            ),
            Expr::cond(
                Expr::equal_to(
                    Expr::select_field(Expr::identifier("request"), "foo"),
                    Expr::literal("bar"),
                ),
                Expr::literal("success"),
                Expr::literal("failed"),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"[if request.foo == "bar" then "success" else "failed", if request.foo == "bar" then "success" else "failed"]"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_pattern_match() {
        let input_expr = Expr::sequence(vec![
            Expr::pattern_match(
                Expr::identifier("request"),
                vec![
                    MatchArm::new(
                        ArmPattern::Constructor(
                            "ok".to_string(),
                            vec![ArmPattern::literal(Expr::identifier("foo"))],
                        ),
                        Expr::literal("success"),
                    ),
                    MatchArm::new(
                        ArmPattern::Constructor(
                            "err".to_string(),
                            vec![ArmPattern::literal(Expr::identifier("msg"))],
                        ),
                        Expr::literal("failure"),
                    ),
                ],
            ),
            Expr::pattern_match(
                Expr::identifier("request"),
                vec![
                    MatchArm::new(
                        ArmPattern::Constructor(
                            "ok".to_string(),
                            vec![ArmPattern::literal(Expr::identifier("foo"))],
                        ),
                        Expr::literal("success"),
                    ),
                    MatchArm::new(
                        ArmPattern::Constructor(
                            "err".to_string(),
                            vec![ArmPattern::literal(Expr::identifier("msg"))],
                        ),
                        Expr::pattern_match(
                            Expr::identifier("request"),
                            vec![
                                MatchArm::new(
                                    ArmPattern::Constructor(
                                        "ok".to_string(),
                                        vec![ArmPattern::literal(Expr::identifier("foo"))],
                                    ), // Use Constructor for ok
                                    Expr::literal("success"),
                                ),
                                MatchArm::new(
                                    ArmPattern::Constructor(
                                        "err".to_string(),
                                        vec![ArmPattern::literal(Expr::identifier("msg"))],
                                    ),
                                    Expr::literal("failure"),
                                ),
                            ],
                        ),
                    ),
                ],
            ),
        ]);

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"[match request {  ok(foo) => "success", err(msg) => "failure" } , match request {  ok(foo) => "success", err(msg) => match request {  ok(foo) => "success", err(msg) => "failure" }  } ]"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_constructor() {
        let input_expr = Expr::sequence(vec![
            Expr::ok(Expr::literal("foo")),
            Expr::err(Expr::literal("msg")),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[ok(\"foo\"), err(\"msg\")]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod tuple_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_read_write_tuple_empty() {
        let input_expr = Expr::tuple(vec![]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "()".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_request() {
        let input_expr = Expr::tuple(vec![
            Expr::identifier("request"),
            Expr::identifier("request"),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "(request, request)".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_literal() {
        let input_expr = Expr::tuple(vec![Expr::literal("hello"), Expr::literal("world")]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"("hello", "world")"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_select_field() {
        let input_expr = Expr::tuple(vec![
            Expr::select_field(Expr::identifier("request"), "field"),
            Expr::select_field(Expr::identifier("request"), "field"),
        ]);
        let _expr_str = to_string(&input_expr).unwrap();
        let _expected_str = "(request.field, request.field)".to_string();
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_select_index() {
        let input_expr = Expr::tuple(vec![
            Expr::select_index(Expr::identifier("request"), 1),
            Expr::select_index(Expr::identifier("request"), 2),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "(request[1], request[2])".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_tuple() {
        let input_expr = Expr::tuple(vec![
            Expr::tuple(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
            Expr::tuple(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "((request, request), (request, request))".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_sequence() {
        let input_expr = Expr::tuple(vec![
            Expr::sequence(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
            Expr::sequence(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "([request, request], [request, request])".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_record() {
        let input_expr = Expr::tuple(vec![
            Expr::record(vec![("field".to_string(), Expr::identifier("request"))]),
            Expr::record(vec![("field".to_string(), Expr::identifier("request"))]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "({field: request}, {field: request})".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_flags() {
        let input_expr = Expr::tuple(vec![
            Expr::flags(vec!["flag1".to_string(), "flag2".to_string()]),
            Expr::flags(vec!["flag3".to_string(), "flag4".to_string()]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "({flag1, flag2}, {flag3, flag4})".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_concat() {
        let input_expr = Expr::tuple(vec![
            Expr::concat(vec![
                Expr::literal("user-id-1-"),
                Expr::select_field(Expr::identifier("request"), "user-id-1"),
            ]),
            Expr::concat(vec![
                Expr::literal("user-id-2-"),
                Expr::select_field(Expr::identifier("request"), "user-id-2"),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"("user-id-1-${request.user-id-1}", "user-id-2-${request.user-id-2}")"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_math_op() {
        let input_expr = Expr::tuple(vec![
            Expr::greater_than(
                Expr::untyped_number(BigDecimal::from(1)),
                Expr::untyped_number(BigDecimal::from(2)),
            ),
            Expr::less_than(
                Expr::untyped_number(BigDecimal::from(1)),
                Expr::untyped_number(BigDecimal::from(2)),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "(1 > 2, 1 < 2)".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_constructor() {
        let input_expr = Expr::tuple(vec![
            Expr::ok(Expr::literal("foo")),
            Expr::err(Expr::literal("msg")),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"(ok("foo"), err("msg"))"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod simple_values_test {
    use bigdecimal::BigDecimal;
    use std::str::FromStr;
    use test_r::test;

    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_read_write_literal() {
        let input_expr = Expr::literal("hello");
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "\"hello\"".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_request() {
        let input_expr = Expr::identifier("request");
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "request".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_number_float() {
        let input_expr = Expr::untyped_number(BigDecimal::from_str("1.1").unwrap());
        let expr_str = to_string(&input_expr).unwrap();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!(input_expr, output_expr);
    }

    #[test]
    fn test_round_trip_read_write_number_u64() {
        let input_expr = Expr::untyped_number(BigDecimal::from(1));
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "1".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_number_i64() {
        let input_expr = Expr::untyped_number(BigDecimal::from(-1));
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "-1".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_worker() {
        let input_expr = Expr::identifier("worker");
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "worker".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_variable() {
        let input_expr = Expr::identifier("variable");
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "variable".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_boolean() {
        let input_expr = Expr::boolean(true);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "true".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod let_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::expr::Expr;
    use crate::parser::type_name::TypeName;
    use crate::text::{from_string, to_string};
    use crate::{InferredType, VariableId};

    #[test]
    fn test_round_trip_read_write_let() {
        let input_expr = Expr::expr_block(vec![
            Expr::let_binding("x", Expr::literal("hello")),
            Expr::let_binding("y", Expr::literal("bar")),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "let x = \"hello\";\nlet y = \"bar\"".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_let_with_type_binding_str() {
        let input_expr = Expr::expr_block(vec![
            Expr::Let(
                VariableId::global("x".to_string()),
                Some(TypeName::Str),
                Box::new(Expr::literal("hello")),
                InferredType::Unknown,
            ),
            Expr::Let(
                VariableId::global("y".to_string()),
                Some(TypeName::Str),
                Box::new(Expr::literal("bar")),
                InferredType::Unknown,
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "let x: string = \"hello\";\nlet y: string = \"bar\"".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_let_with_type_binding_u8() {
        let input_expr = Expr::expr_block(vec![
            Expr::Let(
                VariableId::global("x".to_string()),
                Some(TypeName::U8),
                Box::new(Expr::untyped_number(BigDecimal::from(1))),
                InferredType::Unknown,
            ),
            Expr::Let(
                VariableId::global("y".to_string()),
                Some(TypeName::U8),
                Box::new(Expr::untyped_number(BigDecimal::from(2))),
                InferredType::Unknown,
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "let x: u8 = 1;\nlet y: u8 = 2".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_let_with_type_binding_u16() {
        let input_expr = Expr::expr_block(vec![
            Expr::Let(
                VariableId::global("x".to_string()),
                Some(TypeName::U16),
                Box::new(Expr::untyped_number(BigDecimal::from(1))),
                InferredType::Unknown,
            ),
            Expr::Let(
                VariableId::global("y".to_string()),
                Some(TypeName::U16),
                Box::new(Expr::untyped_number(BigDecimal::from(2))),
                InferredType::Unknown,
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "let x: u16 = 1;\nlet y: u16 = 2".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_let_with_type_binding_u32() {
        let input_expr = Expr::expr_block(vec![
            Expr::Let(
                VariableId::global("x".to_string()),
                Some(TypeName::U32),
                Box::new(Expr::untyped_number(BigDecimal::from(1))),
                InferredType::Unknown,
            ),
            Expr::Let(
                VariableId::global("y".to_string()),
                Some(TypeName::U32),
                Box::new(Expr::untyped_number(BigDecimal::from(2))),
                InferredType::Unknown,
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "let x: u32 = 1;\nlet y: u32 = 2".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_let_with_type_binding_option() {
        let input_expr = Expr::expr_block(vec![
            Expr::Let(
                VariableId::global("x".to_string()),
                Some(TypeName::Option(Box::new(TypeName::Str))),
                Box::new(Expr::Option(
                    Some(Box::new(Expr::literal("foo"))),
                    InferredType::Option(Box::new(InferredType::Str)),
                )),
                InferredType::Unknown,
            ),
            Expr::Let(
                VariableId::global("y".to_string()),
                Some(TypeName::Option(Box::new(TypeName::Str))),
                Box::new(Expr::Option(
                    Some(Box::new(Expr::literal("bar"))),
                    InferredType::Option(Box::new(InferredType::Str)),
                )),
                InferredType::Unknown,
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            "let x: option<string> = some(\"foo\");\nlet y: option<string> = some(\"bar\")"
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_let_with_type_binding_list() {
        let input_expr = Expr::expr_block(vec![
            Expr::Let(
                VariableId::global("x".to_string()),
                Some(TypeName::List(Box::new(TypeName::Str))),
                Box::new(Expr::Sequence(
                    vec![Expr::literal("foo")],
                    InferredType::List(Box::new(InferredType::Str)),
                )),
                InferredType::Unknown,
            ),
            Expr::Let(
                VariableId::global("y".to_string()),
                Some(TypeName::List(Box::new(TypeName::Str))),
                Box::new(Expr::Sequence(
                    vec![Expr::literal("bar")],
                    InferredType::List(Box::new(InferredType::Str)),
                )),
                InferredType::Unknown,
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            "let x: list<string> = [\"foo\"];\nlet y: list<string> = [\"bar\"]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_let_with_type_binding_tuple() {
        let input_expr = Expr::expr_block(vec![
            Expr::Let(
                VariableId::global("x".to_string()),
                Some(TypeName::Tuple(vec![TypeName::Str])),
                Box::new(Expr::Tuple(
                    vec![Expr::literal("foo")],
                    InferredType::Tuple(vec![InferredType::Str]),
                )),
                InferredType::Unknown,
            ),
            Expr::Let(
                VariableId::global("y".to_string()),
                Some(TypeName::Tuple(vec![TypeName::Str])),
                Box::new(Expr::Tuple(
                    vec![Expr::literal("bar")],
                    InferredType::Tuple(vec![InferredType::Str]),
                )),
                InferredType::Unknown,
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            "let x: tuple<string> = (\"foo\");\nlet y: tuple<string> = (\"bar\")".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod selection_tests {
    use test_r::test;

    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_read_write_select_field_from_request() {
        let input_expr = Expr::select_field(Expr::identifier("request"), "field");
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "request.field".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_select_index_from_request() {
        let input_expr = Expr::select_index(Expr::identifier("request"), 1);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "request[1]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_select_field_from_record() {
        let input_expr = Expr::select_field(
            Expr::record(vec![("field".to_string(), Expr::identifier("request"))]),
            "field",
        );
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "{field: request}.field".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_select_index_from_sequence() {
        let input_expr = Expr::select_index(
            Expr::sequence(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
            1,
        );
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "[request, request][1]".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod flag_tests {
    use test_r::test;

    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_read_write_flags_single() {
        let input_expr = Expr::flags(vec!["flag1".to_string()]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "{flag1}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_flags() {
        let input_expr = Expr::flags(vec![
            "flag1".to_string(),
            "flag2".to_string(),
            "flag3".to_string(),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "{flag1, flag2, flag3}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod match_tests {
    use bigdecimal::BigDecimal;
    use std::str::FromStr;
    use test_r::test;

    use crate::expr::ArmPattern;
    use crate::expr::Expr;
    use crate::expr::MatchArm;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_match_expr() {
        let mut input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::constructor(
                        "ok",
                        vec![ArmPattern::literal(Expr::identifier("foo"))],
                    ),
                    Expr::literal("success"),
                ),
                MatchArm::new(
                    ArmPattern::constructor(
                        "err",
                        vec![ArmPattern::literal(Expr::identifier("msg"))],
                    ),
                    Expr::literal("failure"),
                ),
            ],
        );

        input_expr.reset_type();

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  ok(foo) => "success", err(msg) => "failure" } "#.to_string();
        let mut output_expr = from_string(expr_str.as_str()).unwrap();
        output_expr.reset_type();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_flags() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::constructor(
                        "ok",
                        vec![ArmPattern::literal(Expr::identifier("foo"))],
                    ),
                    Expr::flags(vec!["flag1".to_string(), "flag2".to_string()]),
                ),
                MatchArm::new(
                    ArmPattern::constructor(
                        "err",
                        vec![ArmPattern::literal(Expr::identifier("msg"))],
                    ),
                    Expr::literal("failure"),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  ok(foo) => {flag1, flag2}, err(msg) => "failure" } "#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_tuple() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::constructor(
                        "ok",
                        vec![ArmPattern::literal(Expr::identifier("foo"))],
                    ),
                    Expr::tuple(vec![
                        Expr::identifier("request"),
                        Expr::identifier("request"),
                    ]),
                ),
                MatchArm::new(
                    ArmPattern::constructor(
                        "err",
                        vec![ArmPattern::literal(Expr::identifier("msg"))],
                    ),
                    Expr::literal("failure"),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  ok(foo) => (request, request), err(msg) => "failure" } "#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_sequence() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::constructor(
                        "ok",
                        vec![ArmPattern::literal(Expr::identifier("foo"))],
                    ),
                    Expr::sequence(vec![
                        Expr::identifier("request"),
                        Expr::identifier("request"),
                    ]),
                ),
                MatchArm::new(
                    ArmPattern::constructor(
                        "err",
                        vec![ArmPattern::literal(Expr::identifier("msg"))],
                    ),
                    Expr::literal("failure"),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  ok(foo) => [request, request], err(msg) => "failure" } "#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_record() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::constructor(
                        "ok",
                        vec![ArmPattern::literal(Expr::identifier("foo"))],
                    ),
                    Expr::record(vec![("field".to_string(), Expr::identifier("request"))]),
                ),
                MatchArm::new(
                    ArmPattern::constructor(
                        "err",
                        vec![ArmPattern::literal(Expr::identifier("msg"))],
                    ),
                    Expr::literal("failure"),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  ok(foo) => {field: request}, err(msg) => "failure" } "#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_math_op() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::constructor(
                        "ok",
                        vec![ArmPattern::literal(Expr::identifier("foo"))],
                    ),
                    Expr::greater_than(
                        Expr::untyped_number(BigDecimal::from_str("1.1").unwrap()),
                        Expr::untyped_number(BigDecimal::from(2)),
                    ),
                ),
                MatchArm::new(
                    ArmPattern::constructor(
                        "err",
                        vec![ArmPattern::literal(Expr::identifier("msg"))],
                    ),
                    Expr::less_than(
                        Expr::untyped_number(BigDecimal::from(1)),
                        Expr::untyped_number(BigDecimal::from(2)),
                    ),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "match request {  ok(foo) => 1.1 > 2, err(msg) => 1 < 2 } ".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_if_condition() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::constructor(
                        "ok",
                        vec![ArmPattern::literal(Expr::identifier("foo"))],
                    ),
                    Expr::cond(
                        Expr::equal_to(
                            Expr::select_field(Expr::identifier("request"), "foo"),
                            Expr::literal("bar"),
                        ),
                        Expr::literal("success"),
                        Expr::literal("failed"),
                    ),
                ),
                MatchArm::new(
                    ArmPattern::constructor(
                        "err",
                        vec![ArmPattern::literal(Expr::identifier("msg"))],
                    ),
                    Expr::literal("failure"),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  ok(foo) => if request.foo == "bar" then "success" else "failed", err(msg) => "failure" } "#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_multiple_constructor_variables() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::custom_constructor(
                        "foo",
                        vec![ArmPattern::identifier("a"), ArmPattern::identifier("b")],
                    ),
                    Expr::literal("success"),
                ),
                MatchArm::new(
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Expr::literal("failure"),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  foo(a,b) => "success", bar(c) => "failure" } "#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_empty_constructor_variables() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(ArmPattern::identifier("foo"), Expr::literal("success")),
                MatchArm::new(
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Expr::literal("failure"),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  foo => "success", bar(c) => "failure" } "#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_empty_with_nested_constructor_patterns() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::custom_constructor(
                        "foo",
                        vec![ArmPattern::custom_constructor(
                            "bar",
                            vec![ArmPattern::identifier("v1")],
                        )],
                    ),
                    Expr::literal("success"),
                ),
                MatchArm::new(
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Expr::literal("failure"),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  foo(bar(v1)) => "success", bar(c) => "failure" } "#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_variants_in_arm_rhs() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::identifier("foo1"),
                    Expr::ok(Expr::literal("foo")),
                ),
                MatchArm::new(
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Expr::err(Expr::literal("bar")),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  foo1 => ok("foo"), bar(c) => err("bar") } "#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_variants_in_wild_pattern() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::custom_constructor("foo1", vec![ArmPattern::WildCard]),
                    Expr::ok(Expr::literal("foo")),
                ),
                MatchArm::new(
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Expr::err(Expr::literal("bar")),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  foo1(_) => ok("foo"), bar(c) => err("bar") } "#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_variants_with_alias() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::As(
                        "name".to_string(),
                        Box::new(ArmPattern::custom_constructor(
                            "foo1",
                            vec![ArmPattern::WildCard],
                        )),
                    ),
                    Expr::ok(Expr::literal("foo")),
                ),
                MatchArm::new(
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Expr::err(Expr::literal("bar")),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  name @ foo1(_) => ok("foo"), bar(c) => err("bar") } "#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_variants_with_nested_alias() {
        let input_expr = Expr::pattern_match(
            Expr::identifier("request"),
            vec![
                MatchArm::new(
                    ArmPattern::As(
                        "a".to_string(),
                        Box::new(ArmPattern::custom_constructor(
                            "foo",
                            vec![ArmPattern::As(
                                "b".to_string(),
                                Box::new(ArmPattern::WildCard),
                            )],
                        )),
                    ),
                    Expr::ok(Expr::literal("foo")),
                ),
                MatchArm::new(
                    ArmPattern::As(
                        "c".to_string(),
                        Box::new(ArmPattern::custom_constructor(
                            "bar",
                            vec![ArmPattern::As(
                                "d".to_string(),
                                Box::new(ArmPattern::custom_constructor(
                                    "baz",
                                    vec![ArmPattern::identifier("x")],
                                )),
                            )],
                        )),
                    ),
                    Expr::err(Expr::literal("bar")),
                ),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"match request {  a @ foo(b @ _) => ok("foo"), c @ bar(d @ baz(x)) => err("bar") } "#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod if_cond_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_if_condition_literals() {
        let input_expr = Expr::cond(
            Expr::equal_to(Expr::literal("foo"), Expr::literal("bar")),
            Expr::literal("success"),
            Expr::literal("failed"),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"if "foo" == "bar" then "success" else "failed""#.to_string();
        let output_expr = from_string(expected_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_select_field() {
        let input_expr = Expr::cond(
            Expr::equal_to(
                Expr::select_field(Expr::identifier("request"), "foo"),
                Expr::literal("bar"),
            ),
            Expr::literal("success"),
            Expr::literal("failed"),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"if request.foo == "bar" then "success" else "failed""#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_nested_if_condition() {
        let input_expr = Expr::cond(
            Expr::equal_to(
                Expr::select_field(Expr::identifier("request"), "foo"),
                Expr::literal("bar"),
            ),
            Expr::literal("success"),
            Expr::cond(
                Expr::equal_to(
                    Expr::select_field(Expr::identifier("request"), "foo"),
                    Expr::literal("baz"),
                ),
                Expr::literal("success"),
                Expr::literal("failed"),
            ),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"if request.foo == "bar" then "success" else if request.foo == "baz" then "success" else "failed""#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_tuple() {
        let input_expr = Expr::cond(
            Expr::equal_to(Expr::identifier("foo"), Expr::identifier("bar")),
            Expr::tuple(vec![Expr::identifier("foo"), Expr::identifier("bar")]),
            Expr::tuple(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"if foo == bar then (foo, bar) else (request, request)"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_sequence() {
        let input_expr = Expr::cond(
            Expr::equal_to(Expr::identifier("foo"), Expr::identifier("bar")),
            Expr::sequence(vec![
                Expr::identifier("request"),
                Expr::identifier("request"),
            ]),
            Expr::sequence(vec![Expr::identifier("foo"), Expr::identifier("bar")]),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"if foo == bar then [request, request] else [foo, bar]"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_record() {
        let input_expr = Expr::cond(
            Expr::equal_to(Expr::identifier("field1"), Expr::identifier("field2")),
            Expr::record(vec![("field".to_string(), Expr::identifier("request"))]),
            Expr::literal("failed"),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"if field1 == field2 then {field: request} else "failed""#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_flags() {
        let input_expr = Expr::cond(
            Expr::equal_to(
                Expr::select_field(Expr::identifier("worker"), "response"),
                Expr::untyped_number(BigDecimal::from(1)),
            ),
            Expr::flags(vec!["flag1".to_string(), "flag2".to_string()]),
            Expr::literal("failed"),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"if worker.response == 1 then {flag1, flag2} else "failed""#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

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

mod writer;

use crate::text::writer::WriterError;

pub fn from_string(input: impl AsRef<str>) -> Result<Expr, String> {
    Expr::from_interpolated_str(input.as_ref())
}

pub fn to_string(expr: &Expr) -> Result<String, WriterError> {
    writer::write_expr(expr)
}

#[cfg(test)]
mod record_tests {
    use crate::expr::*;
    use crate::text::{from_string, to_string, Expr};

    #[test]
    fn test_round_trip_simple_record_single() {
        let input_expr = Expr::Record(vec![(
            "field".to_string(),
            Box::new(Expr::Identifier("request".to_string())),
        )]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{field: request}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_multiple() {
        let input_expr = Expr::Record(vec![
            (
                "field".to_string(),
                Box::new(Expr::Identifier("request".to_string())),
            ),
            (
                "field".to_string(),
                Box::new(Expr::Identifier("request".to_string())),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{field: request, field: request}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_literal() {
        let input_expr = Expr::Record(vec![
            (
                "field".to_string(),
                Box::new(Expr::Literal("hello".to_string())),
            ),
            (
                "field".to_string(),
                Box::new(Expr::Literal("world".to_string())),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"${{field: "hello", field: "world"}}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_number() {
        let input_expr = Expr::Record(vec![
            ("field".to_string(), Box::new(Expr::unsigned_integer(1))),
            ("field".to_string(), Box::new(Expr::unsigned_integer(2))),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{field: 1, field: 2}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_select_field() {
        let input_expr = Expr::Record(vec![
            (
                "field".to_string(),
                Box::new(Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
                    "foo".to_string(),
                )),
            ),
            (
                "field".to_string(),
                Box::new(Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
                    "bar".to_string(),
                )),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{field: request.foo, field: request.bar}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_select_index() {
        let input_expr = Expr::Record(vec![
            (
                "field".to_string(),
                Box::new(Expr::SelectIndex(
                    Box::new(Expr::Identifier("request".to_string())),
                    1,
                )),
            ),
            (
                "field".to_string(),
                Box::new(Expr::SelectIndex(
                    Box::new(Expr::Identifier("request".to_string())),
                    2,
                )),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{field: request[1], field: request[2]}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_sequence() {
        let input_expr = Expr::Record(vec![
            (
                "field".to_string(),
                Box::new(Expr::Sequence(vec![
                    Expr::Identifier("request".to_string()),
                    Expr::Identifier("request".to_string()),
                ])),
            ),
            (
                "field".to_string(),
                Box::new(Expr::Sequence(vec![
                    Expr::Identifier("request".to_string()),
                    Expr::Identifier("request".to_string()),
                ])),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{field: [request, request], field: [request, request]}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_record() {
        let input_expr = Expr::Record(vec![
            (
                "a".to_string(),
                Box::new(Expr::Record(vec![
                    (
                        "ab".to_string(),
                        Box::new(Expr::Identifier("request".to_string())),
                    ),
                    (
                        "ac".to_string(),
                        Box::new(Expr::Identifier("request".to_string())),
                    ),
                ])),
            ),
            (
                "b".to_string(),
                Box::new(Expr::Sequence(vec![Expr::Record(vec![
                    (
                        "bc".to_string(),
                        Box::new(Expr::Identifier("request".to_string())),
                    ),
                    (
                        "bd".to_string(),
                        Box::new(Expr::Identifier("request".to_string())),
                    ),
                ])])),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let record_string =
            "{a: {ab: request, ac: request}, b: [{bc: request, bd: request}]}".to_string();
        let expected_record_str = format!("${{{}}}", record_string); // Just wrapping it with interpolation
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_tuple() {
        let input_expr = Expr::Record(vec![
            (
                "a".to_string(),
                Box::new(Expr::Tuple(vec![
                    Expr::Identifier("request".to_string()),
                    Expr::Identifier("worker".to_string()),
                ])),
            ),
            (
                "b".to_string(),
                Box::new(Expr::Tuple(vec![
                    Expr::Identifier("request".to_string()),
                    Expr::Identifier("worker".to_string()),
                ])),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let record_string = "{a: (request, worker), b: (request, worker)}".to_string();
        let expected_record_str = format!("${{{}}}", record_string); // Just wrapping it with interpolation
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_flags() {
        let input_expr = Expr::Record(vec![
            (
                "a".to_string(),
                Box::new(Expr::Flags(vec!["flag1".to_string(), "flag2".to_string()])),
            ),
            (
                "b".to_string(),
                Box::new(Expr::Flags(vec!["flag3".to_string(), "flag4".to_string()])),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let record_string = "{a: {flag1, flag2}, b: {flag3, flag4}}".to_string();
        let expected_record_str = format!("${{{}}}", record_string); // Just wrapping it with interpolation
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_boolean() {
        let input_expr = Expr::Record(vec![
            ("a".to_string(), Box::new(Expr::Boolean(true))),
            ("b".to_string(), Box::new(Expr::Boolean(false))),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let record_string = "{a: true, b: false}".to_string();
        let expected_record_str = format!("${{{}}}", record_string); // Just wrapping it with interpolation
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_concatenation() {
        let input_expr = Expr::Record(vec![
            (
                "a".to_string(),
                Box::new(Expr::Concat(vec![
                    Expr::Literal("user-id-1-".to_string()),
                    Expr::SelectField(
                        Box::new(Expr::Identifier("request".to_string())),
                        "user-id-1".to_string(),
                    ),
                ])),
            ),
            (
                "b".to_string(),
                Box::new(Expr::Concat(vec![
                    Expr::Literal("user-id-2-".to_string()),
                    Expr::SelectField(
                        Box::new(Expr::Identifier("request".to_string())),
                        "user-id-2".to_string(),
                    ),
                ])),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str =
            r#"${{a: "user-id-1-${request.user-id-1}", b: "user-id-2-${request.user-id-2}"}}"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_math_op() {
        let input_expr = Expr::Record(vec![
            (
                "a".to_string(),
                Box::new(Expr::GreaterThan(
                    Box::new(Expr::unsigned_integer(1)),
                    Box::new(Expr::unsigned_integer(2)),
                )),
            ),
            (
                "b".to_string(),
                Box::new(Expr::LessThan(
                    Box::new(Expr::unsigned_integer(1)),
                    Box::new(Expr::unsigned_integer(2)),
                )),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = "${{a: 1 > 2, b: 1 < 2}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_if_condition() {
        let input_expr = Expr::Record(vec![
            (
                "a".to_string(),
                Box::new(Expr::Cond(
                    Box::new(Expr::EqualTo(
                        Box::new(Expr::SelectField(
                            Box::new(Expr::Identifier("request".to_string())),
                            "foo".to_string(),
                        )),
                        Box::new(Expr::Literal("bar".to_string())),
                    )),
                    Box::new(Expr::Literal("success".to_string())),
                    Box::new(Expr::Literal("failed".to_string())),
                )),
            ),
            (
                "b".to_string(),
                Box::new(Expr::Cond(
                    Box::new(Expr::EqualTo(
                        Box::new(Expr::SelectField(
                            Box::new(Expr::Identifier("request".to_string())),
                            "foo".to_string(),
                        )),
                        Box::new(Expr::Literal("bar".to_string())),
                    )),
                    Box::new(Expr::Literal("success".to_string())),
                    Box::new(Expr::Literal("failed".to_string())),
                )),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = r#"${{a: if request.foo == "bar" then "success" else "failed", b: if request.foo == "bar" then "success" else "failed"}}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_pattern_match() {
        let input_expr = Expr::Record(vec![
            (
                "a".to_string(),
                Box::new(Expr::PatternMatch(
                    Box::new(Expr::Identifier("request".to_string())),
                    vec![
                        MatchArm((
                            ArmPattern::ok("foo"),
                            Box::new(Expr::Literal("success".to_string())),
                        )),
                        MatchArm((
                            ArmPattern::err("msg"),
                            Box::new(Expr::Literal("failure".to_string())),
                        )),
                    ],
                )),
            ),
            (
                "b".to_string(),
                Box::new(Expr::PatternMatch(
                    Box::new(Expr::Identifier("request".to_string())),
                    vec![
                        MatchArm((
                            ArmPattern::ok("foo"),
                            Box::new(Expr::Literal("success".to_string())),
                        )),
                        MatchArm((
                            ArmPattern::err("msg"),
                            Box::new(Expr::PatternMatch(
                                Box::new(Expr::Identifier("request".to_string())),
                                vec![
                                    MatchArm((
                                        ArmPattern::ok("foo"),
                                        Box::new(Expr::Literal("success".to_string())),
                                    )),
                                    MatchArm((
                                        ArmPattern::err("msg"),
                                        Box::new(Expr::Literal("failure".to_string())),
                                    )),
                                ],
                            )),
                        )),
                    ],
                )),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = r#"${{a: match request {  ok(foo) => "success", err(msg) => "failure" } , b: match request {  ok(foo) => "success", err(msg) => match request {  ok(foo) => "success", err(msg) => "failure" }  } }}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_record_of_constructor() {
        let input_expr = Expr::Record(vec![
            (
                "a".to_string(),
                Box::new(Expr::Result(Ok(Box::new(Expr::Literal("foo".to_string()))))),
            ),
            (
                "b".to_string(),
                Box::new(Expr::Result(Err(Box::new(Expr::Literal(
                    "msg".to_string(),
                ))))),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_record_str = r#"${{a: ok("foo"), b: err("msg")}}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }
}

#[cfg(test)]
mod sequence_tests {
    use crate::expr::Expr;
    use crate::text::{from_string, to_string};
    use crate::{ArmPattern, MatchArm};

    #[test]
    fn test_round_trip_read_write_sequence_empty() {
        let input_expr = Expr::Sequence(vec![]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    // A few non-round-trip text based tests
    #[test]
    fn test_sequence_of_records_singleton() {
        let expr_string = "${[{bc: request}]}";
        let output_expr = from_string(expr_string).unwrap();
        let expected_expr = Expr::Sequence(vec![Expr::Record(vec![(
            "bc".to_string(),
            Box::new(Expr::Identifier("request".to_string())),
        )])]);
        assert_eq!(output_expr, expected_expr);
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_request() {
        let input_expr = Expr::Sequence(vec![
            Expr::Identifier("request".to_string()),
            Expr::Identifier("request".to_string()),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[request, request]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_literal() {
        let input_expr = Expr::Sequence(vec![
            Expr::Literal("hello".to_string()),
            Expr::Literal("world".to_string()),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"${["hello", "world"]}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_select_field() {
        let input_expr = Expr::Sequence(vec![
            Expr::SelectField(
                Box::new(Expr::Identifier("request".to_string())),
                "field".to_string(),
            ),
            Expr::SelectField(
                Box::new(Expr::Identifier("request".to_string())),
                "field".to_string(),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[request.field, request.field]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_select_index() {
        let input_expr = Expr::Sequence(vec![
            Expr::SelectIndex(Box::new(Expr::Identifier("request".to_string())), 1),
            Expr::SelectIndex(Box::new(Expr::Identifier("request".to_string())), 2),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[request[1], request[2]]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_sequence() {
        let input_expr = Expr::Sequence(vec![
            Expr::Sequence(vec![
                Expr::Identifier("request".to_string()),
                Expr::Identifier("request".to_string()),
            ]),
            Expr::Sequence(vec![
                Expr::Identifier("request".to_string()),
                Expr::Identifier("request".to_string()),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[[request, request], [request, request]]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_tuple() {
        let input_expr = Expr::Sequence(vec![
            Expr::Tuple(vec![
                Expr::Identifier("request".to_string()),
                Expr::Identifier("request".to_string()),
            ]),
            Expr::Tuple(vec![
                Expr::Identifier("request".to_string()),
                Expr::Identifier("request".to_string()),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[(request, request), (request, request)]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_record() {
        let input_expr = Expr::Sequence(vec![
            Expr::Record(vec![(
                "field".to_string(),
                Box::new(Expr::Identifier("request".to_string())),
            )]),
            Expr::Record(vec![(
                "field".to_string(),
                Box::new(Expr::Identifier("request".to_string())),
            )]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[{field: request}, {field: request}]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_flags() {
        let input_expr = Expr::Sequence(vec![
            Expr::Flags(vec!["flag1".to_string(), "flag2".to_string()]),
            Expr::Flags(vec!["flag3".to_string(), "flag4".to_string()]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[{flag1, flag2}, {flag3, flag4}]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_concat() {
        let input_expr = Expr::Sequence(vec![
            Expr::Concat(vec![
                Expr::Literal("user-id-1-".to_string()),
                Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
                    "user-id-1".to_string(),
                ),
            ]),
            Expr::Concat(vec![
                Expr::Literal("user-id-2-".to_string()),
                Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
                    "user-id-2".to_string(),
                ),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${["user-id-1-${request.user-id-1}", "user-id-2-${request.user-id-2}"]}"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_math_op() {
        let input_expr = Expr::Sequence(vec![
            Expr::GreaterThan(
                Box::new(Expr::unsigned_integer(1)),
                Box::new(Expr::unsigned_integer(2)),
            ),
            Expr::LessThan(
                Box::new(Expr::unsigned_integer(1)),
                Box::new(Expr::unsigned_integer(2)),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[1 > 2, 1 < 2]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_if_condition() {
        let input_expr = Expr::Sequence(vec![
            Expr::Cond(
                Box::new(Expr::EqualTo(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("request".to_string())),
                        "foo".to_string(),
                    )),
                    Box::new(Expr::Literal("bar".to_string())),
                )),
                Box::new(Expr::Literal("success".to_string())),
                Box::new(Expr::Literal("failed".to_string())),
            ),
            Expr::Cond(
                Box::new(Expr::EqualTo(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("request".to_string())),
                        "foo".to_string(),
                    )),
                    Box::new(Expr::Literal("bar".to_string())),
                )),
                Box::new(Expr::Literal("success".to_string())),
                Box::new(Expr::Literal("failed".to_string())),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"${[if request.foo == "bar" then "success" else "failed", if request.foo == "bar" then "success" else "failed"]}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_pattern_match() {
        let input_expr = Expr::Sequence(vec![
            Expr::PatternMatch(
                Box::new(Expr::Identifier("request".to_string())),
                vec![
                    MatchArm((
                        ArmPattern::ok("foo"),
                        Box::new(Expr::Literal("success".to_string())),
                    )),
                    MatchArm((
                        ArmPattern::err("msg"),
                        Box::new(Expr::Literal("failure".to_string())),
                    )),
                ],
            ),
            Expr::PatternMatch(
                Box::new(Expr::Identifier("request".to_string())),
                vec![
                    MatchArm((
                        ArmPattern::ok("foo"),
                        Box::new(Expr::Literal("success".to_string())),
                    )),
                    MatchArm((
                        ArmPattern::err("msg"),
                        Box::new(Expr::PatternMatch(
                            Box::new(Expr::Identifier("request".to_string())),
                            vec![
                                MatchArm((
                                    ArmPattern::ok("foo"),
                                    Box::new(Expr::Literal("success".to_string())),
                                )),
                                MatchArm((
                                    ArmPattern::err("msg"),
                                    Box::new(Expr::Literal("failure".to_string())),
                                )),
                            ],
                        )),
                    )),
                ],
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"${[match request {  ok(foo) => "success", err(msg) => "failure" } , match request {  ok(foo) => "success", err(msg) => match request {  ok(foo) => "success", err(msg) => "failure" }  } ]}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence_of_constructor() {
        let input_expr = Expr::Sequence(vec![
            Expr::Result(Ok(Box::new(Expr::Literal("foo".to_string())))),
            Expr::Result(Err(Box::new(Expr::Literal("msg".to_string())))),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[ok(\"foo\"), err(\"msg\")]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod tuple_tests {
    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_read_write_tuple_empty() {
        let input_expr = Expr::Tuple(vec![]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${()}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_request() {
        let input_expr = Expr::Tuple(vec![
            Expr::Identifier("request".to_string()),
            Expr::Identifier("request".to_string()),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${(request, request)}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_literal() {
        let input_expr = Expr::Tuple(vec![
            Expr::Literal("hello".to_string()),
            Expr::Literal("world".to_string()),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"${("hello", "world")}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_select_field() {
        let input_expr = Expr::Tuple(vec![
            Expr::SelectField(
                Box::new(Expr::Identifier("request".to_string())),
                "field".to_string(),
            ),
            Expr::SelectField(
                Box::new(Expr::Identifier("request".to_string())),
                "field".to_string(),
            ),
        ]);
        let _expr_str = to_string(&input_expr).unwrap();
        let _expected_str = "${(request.field, request.field)}".to_string();
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_select_index() {
        let input_expr = Expr::Tuple(vec![
            Expr::SelectIndex(Box::new(Expr::Identifier("request".to_string())), 1),
            Expr::SelectIndex(Box::new(Expr::Identifier("request".to_string())), 2),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${(request[1], request[2])}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_tuple() {
        let input_expr = Expr::Tuple(vec![
            Expr::Tuple(vec![
                Expr::Identifier("request".to_string()),
                Expr::Identifier("request".to_string()),
            ]),
            Expr::Tuple(vec![
                Expr::Identifier("request".to_string()),
                Expr::Identifier("request".to_string()),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${((request, request), (request, request))}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_sequence() {
        let input_expr = Expr::Tuple(vec![
            Expr::Sequence(vec![
                Expr::Identifier("request".to_string()),
                Expr::Identifier("request".to_string()),
            ]),
            Expr::Sequence(vec![
                Expr::Identifier("request".to_string()),
                Expr::Identifier("request".to_string()),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${([request, request], [request, request])}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_record() {
        let input_expr = Expr::Tuple(vec![
            Expr::Record(vec![(
                "field".to_string(),
                Box::new(Expr::Identifier("request".to_string())),
            )]),
            Expr::Record(vec![(
                "field".to_string(),
                Box::new(Expr::Identifier("request".to_string())),
            )]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${({field: request}, {field: request})}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_flags() {
        let input_expr = Expr::Tuple(vec![
            Expr::Flags(vec!["flag1".to_string(), "flag2".to_string()]),
            Expr::Flags(vec!["flag3".to_string(), "flag4".to_string()]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        dbg!(expr_str.clone());
        let expected_str = "${({flag1, flag2}, {flag3, flag4})}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_concat() {
        let input_expr = Expr::Tuple(vec![
            Expr::Concat(vec![
                Expr::Literal("user-id-1-".to_string()),
                Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
                    "user-id-1".to_string(),
                ),
            ]),
            Expr::Concat(vec![
                Expr::Literal("user-id-2-".to_string()),
                Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
                    "user-id-2".to_string(),
                ),
            ]),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${("user-id-1-${request.user-id-1}", "user-id-2-${request.user-id-2}")}"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_math_op() {
        let input_expr = Expr::Tuple(vec![
            Expr::GreaterThan(
                Box::new(Expr::unsigned_integer(1)),
                Box::new(Expr::unsigned_integer(2)),
            ),
            Expr::LessThan(
                Box::new(Expr::unsigned_integer(1)),
                Box::new(Expr::unsigned_integer(2)),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${(1 > 2, 1 < 2)}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple_of_constructor() {
        let input_expr = Expr::Tuple(vec![
            Expr::Result(Ok(Box::new(Expr::Literal("foo".to_string())))),
            Expr::Result(Err(Box::new(Expr::Literal("msg".to_string())))),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"${(ok("foo"), err("msg"))}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod simple_values_test {
    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_read_write_literal() {
        let input_expr = Expr::Literal("hello".to_string());
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "hello".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_request() {
        let input_expr = Expr::Identifier("request".to_string());
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${request}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_number_float() {
        let input_expr = Expr::float(1.1);
        let expr_str = to_string(&input_expr).unwrap();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!(input_expr, output_expr);
    }

    #[test]
    fn test_round_trip_read_write_number_u64() {
        let input_expr = Expr::unsigned_integer(1);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${1}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_number_i64() {
        let input_expr = Expr::signed_integer(-1);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${-1}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_worker() {
        let input_expr = Expr::Identifier("worker".to_string());
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${worker}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_variable() {
        let input_expr = Expr::Identifier("variable".to_string());
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${variable}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_boolean() {
        let input_expr = Expr::Boolean(true);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${true}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod let_tests {
    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_read_write_let() {
        let input_expr = Expr::Multiple(vec![
            Expr::Let(
                "x".to_string(),
                Box::new(Expr::Literal("hello".to_string())),
            ),
            Expr::Let("y".to_string(), Box::new(Expr::Literal("bar".to_string()))),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${let x = \"hello\";\nlet y = \"bar\"}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod selection_tests {
    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_read_write_select_field_from_request() {
        let input_expr = Expr::SelectField(
            Box::new(Expr::Identifier("request".to_string())),
            "field".to_string(),
        );
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${request.field}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_select_index_from_request() {
        let input_expr = Expr::SelectIndex(Box::new(Expr::Identifier("request".to_string())), 1);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${request[1]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_select_field_from_record() {
        let input_expr = Expr::SelectField(
            Box::new(Expr::Record(vec![(
                "field".to_string(),
                Box::new(Expr::Identifier("request".to_string())),
            )])),
            "field".to_string(),
        );
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{field: request}.field}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_select_index_from_sequence() {
        let input_expr = Expr::SelectIndex(
            Box::new(Expr::Sequence(vec![
                Expr::Identifier("request".to_string()),
                Expr::Identifier("request".to_string()),
            ])),
            1,
        );
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[request, request][1]}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod flag_tests {
    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_read_write_flags_single() {
        let input_expr = Expr::Flags(vec!["flag1".to_string()]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{flag1}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_flags() {
        let input_expr = Expr::Flags(vec![
            "flag1".to_string(),
            "flag2".to_string(),
            "flag3".to_string(),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{flag1, flag2, flag3}}".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod match_tests {
    use crate::expr::ArmPattern;
    use crate::expr::Expr;
    use crate::expr::MatchArm;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_match_expr() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::ok("foo"),
                    Box::new(Expr::Literal("success".to_string())),
                )),
                MatchArm((
                    ArmPattern::err("msg"),
                    Box::new(Expr::Literal("failure".to_string())),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  ok(foo) => "success", err(msg) => "failure" } }"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_flags() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::ok("foo"),
                    Box::new(Expr::Flags(vec!["flag1".to_string(), "flag2".to_string()])),
                )),
                MatchArm((
                    ArmPattern::err("msg"),
                    Box::new(Expr::Literal("failure".to_string())),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  ok(foo) => {flag1, flag2}, err(msg) => "failure" } }"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_tuple() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::ok("foo"),
                    Box::new(Expr::Tuple(vec![
                        Expr::Identifier("request".to_string()),
                        Expr::Identifier("request".to_string()),
                    ])),
                )),
                MatchArm((
                    ArmPattern::err("msg"),
                    Box::new(Expr::Literal("failure".to_string())),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  ok(foo) => (request, request), err(msg) => "failure" } }"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_sequence() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::ok("foo"),
                    Box::new(Expr::Sequence(vec![
                        Expr::Identifier("request".to_string()),
                        Expr::Identifier("request".to_string()),
                    ])),
                )),
                MatchArm((
                    ArmPattern::err("msg"),
                    Box::new(Expr::Literal("failure".to_string())),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  ok(foo) => [request, request], err(msg) => "failure" } }"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_record() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::ok("foo"),
                    Box::new(Expr::Record(vec![(
                        "field".to_string(),
                        Box::new(Expr::Identifier("request".to_string())),
                    )])),
                )),
                MatchArm((
                    ArmPattern::err("msg"),
                    Box::new(Expr::Literal("failure".to_string())),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  ok(foo) => {field: request}, err(msg) => "failure" } }"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_math_op() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::ok("foo"),
                    Box::new(Expr::GreaterThan(
                        Box::new(Expr::unsigned_integer(1)),
                        Box::new(Expr::unsigned_integer(2)),
                    )),
                )),
                MatchArm((
                    ArmPattern::err("msg"),
                    Box::new(Expr::LessThan(
                        Box::new(Expr::unsigned_integer(1)),
                        Box::new(Expr::unsigned_integer(2)),
                    )),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${match request {  ok(foo) => 1 > 2, err(msg) => 1 < 2 } }".to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_match_expr_of_if_condition() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::ok("foo"),
                    Box::new(Expr::Cond(
                        Box::new(Expr::EqualTo(
                            Box::new(Expr::SelectField(
                                Box::new(Expr::Identifier("request".to_string())),
                                "foo".to_string(),
                            )),
                            Box::new(Expr::Literal("bar".to_string())),
                        )),
                        Box::new(Expr::Literal("success".to_string())),
                        Box::new(Expr::Literal("failed".to_string())),
                    )),
                )),
                MatchArm((
                    ArmPattern::err("msg"),
                    Box::new(Expr::Literal("failure".to_string())),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  ok(foo) => if request.foo == "bar" then "success" else "failed", err(msg) => "failure" } }"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_multiple_constructor_variables() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::custom_constructor(
                        "foo",
                        vec![ArmPattern::identifier("a"), ArmPattern::identifier("b")],
                    ),
                    Box::new(Expr::Literal("success".to_string())),
                )),
                MatchArm((
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Box::new(Expr::Literal("failure".to_string())),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  foo(a,b) => "success", bar(c) => "failure" } }"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_empty_constructor_variables() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::identifier("foo"),
                    Box::new(Expr::Literal("success".to_string())),
                )),
                MatchArm((
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Box::new(Expr::Literal("failure".to_string())),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  foo => "success", bar(c) => "failure" } }"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_empty_with_nested_constructor_patterns() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::custom_constructor(
                        "foo",
                        vec![ArmPattern::custom_constructor(
                            "bar",
                            vec![ArmPattern::identifier("v1")],
                        )],
                    ),
                    Box::new(Expr::Literal("success".to_string())),
                )),
                MatchArm((
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Box::new(Expr::Literal("failure".to_string())),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        dbg!(expr_str.clone());
        let expected_str =
            r#"${match request {  foo(bar(v1)) => "success", bar(c) => "failure" } }"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_variants_in_arm_rhs() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::identifier("foo1"),
                    Box::new(Expr::Result(Ok(Box::new(Expr::Literal("foo".to_string()))))),
                )),
                MatchArm((
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Box::new(Expr::Result(Err(Box::new(Expr::Literal(
                        "bar".to_string(),
                    ))))),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  foo1 => ok("foo"), bar(c) => err("bar") } }"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_variants_in_wild_pattern() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::custom_constructor("foo1", vec![ArmPattern::WildCard]),
                    Box::new(Expr::Result(Ok(Box::new(Expr::Literal("foo".to_string()))))),
                )),
                MatchArm((
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Box::new(Expr::Result(Err(Box::new(Expr::Literal(
                        "bar".to_string(),
                    ))))),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  foo1(_) => ok("foo"), bar(c) => err("bar") } }"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_variants_with_alias() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
                    ArmPattern::As(
                        "name".to_string(),
                        Box::new(ArmPattern::custom_constructor(
                            "foo1",
                            vec![ArmPattern::WildCard],
                        )),
                    ),
                    Box::new(Expr::Result(Ok(Box::new(Expr::Literal("foo".to_string()))))),
                )),
                MatchArm((
                    ArmPattern::custom_constructor("bar", vec![ArmPattern::identifier("c")]),
                    Box::new(Expr::Result(Err(Box::new(Expr::Literal(
                        "bar".to_string(),
                    ))))),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  name @ foo1(_) => ok("foo"), bar(c) => err("bar") } }"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_pattern_match_variants_with_nested_alias() {
        let input_expr = Expr::PatternMatch(
            Box::new(Expr::Identifier("request".to_string())),
            vec![
                MatchArm((
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
                    Box::new(Expr::Result(Ok(Box::new(Expr::Literal("foo".to_string()))))),
                )),
                MatchArm((
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
                    Box::new(Expr::Result(Err(Box::new(Expr::Literal(
                        "bar".to_string(),
                    ))))),
                )),
            ],
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${match request {  a @ foo(b @ _) => ok("foo"), c @ bar(d @ baz(x)) => err("bar") } }"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

#[cfg(test)]
mod if_cond_tests {
    use crate::expr::Expr;
    use crate::text::{from_string, to_string};

    #[test]
    fn test_round_trip_if_condition_literals() {
        let input_expr = Expr::Cond(
            Box::new(Expr::EqualTo(
                Box::new(Expr::Literal("foo".to_string())),
                Box::new(Expr::Literal("bar".to_string())),
            )),
            Box::new(Expr::Literal("success".to_string())),
            Box::new(Expr::Literal("failed".to_string())),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"${if "foo" == "bar" then "success" else "failed"}"#.to_string();
        let output_expr = from_string(expected_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_select_field() {
        let input_expr = Expr::Cond(
            Box::new(Expr::EqualTo(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
                    "foo".to_string(),
                )),
                Box::new(Expr::Literal("bar".to_string())),
            )),
            Box::new(Expr::Literal("success".to_string())),
            Box::new(Expr::Literal("failed".to_string())),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"${if request.foo == "bar" then "success" else "failed"}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_nested_if_condition() {
        let input_expr = Expr::Cond(
            Box::new(Expr::EqualTo(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
                    "foo".to_string(),
                )),
                Box::new(Expr::Literal("bar".to_string())),
            )),
            Box::new(Expr::Literal("success".to_string())),
            Box::new(Expr::Cond(
                Box::new(Expr::EqualTo(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("request".to_string())),
                        "foo".to_string(),
                    )),
                    Box::new(Expr::Literal("baz".to_string())),
                )),
                Box::new(Expr::Literal("success".to_string())),
                Box::new(Expr::Literal("failed".to_string())),
            )),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = r#"${if request.foo == "bar" then "success" else if request.foo == "baz" then "success" else "failed"}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_tuple() {
        let input_expr = Expr::Cond(
            Box::new(Expr::EqualTo(
                Box::new(Expr::Tuple(vec![
                    Expr::Identifier("foo".to_string()),
                    Expr::Identifier("bar".to_string()),
                ])),
                Box::new(Expr::Tuple(vec![
                    Expr::Identifier("request".to_string()),
                    Expr::Identifier("request".to_string()),
                ])),
            )),
            Box::new(Expr::Literal("success".to_string())),
            Box::new(Expr::Literal("failed".to_string())),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${if (foo, bar) == (request, request) then "success" else "failed"}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_sequence() {
        let input_expr = Expr::Cond(
            Box::new(Expr::EqualTo(
                Box::new(Expr::Sequence(vec![
                    Expr::Identifier("foo".to_string()),
                    Expr::Identifier("bar".to_string()),
                ])),
                Box::new(Expr::Sequence(vec![
                    Expr::Identifier("request".to_string()),
                    Expr::Identifier("request".to_string()),
                ])),
            )),
            Box::new(Expr::Literal("success".to_string())),
            Box::new(Expr::Literal("failed".to_string())),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${if [foo, bar] == [request, request] then "success" else "failed"}"#.to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_record() {
        let input_expr = Expr::Cond(
            Box::new(Expr::EqualTo(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Record(vec![(
                        "field".to_string(),
                        Box::new(Expr::Identifier("request".to_string())),
                    )])),
                    "field".to_string(),
                )),
                Box::new(Expr::Record(vec![(
                    "field".to_string(),
                    Box::new(Expr::Identifier("request".to_string())),
                )])),
            )),
            Box::new(Expr::Literal("success".to_string())),
            Box::new(Expr::Literal("failed".to_string())),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${if {field: request}.field == {field: request} then "success" else "failed"}"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_if_condition_of_flags() {
        let input_expr = Expr::Cond(
            Box::new(Expr::EqualTo(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Identifier("worker".to_string())),
                    "response".to_string(),
                )),
                Box::new(Expr::Flags(vec!["flag1".to_string(), "flag2".to_string()])),
            )),
            Box::new(Expr::Flags(vec!["flag1".to_string(), "flag2".to_string()])),
            Box::new(Expr::Literal("failed".to_string())),
        );

        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            r#"${if worker.response == {flag1, flag2} then {flag1, flag2} else "failed"}"#
                .to_string();
        let output_expr = from_string(expr_str.as_str()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }
}

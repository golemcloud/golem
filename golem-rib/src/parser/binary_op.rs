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

use crate::parser::errors::RibParseError;
use crate::rib_source_span::GetSourcePosition;
use combine::parser::char::string;
use combine::{attempt, choice, ParseError, Parser};

pub fn binary_op<Input>() -> impl Parser<Input, Output = BinaryOp>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    choice((
        attempt(string(">=")).map(|_| BinaryOp::GreaterThanOrEqualTo),
        attempt(string("<=")).map(|_| BinaryOp::LessThanOrEqualTo),
        attempt(string("==")).map(|_| BinaryOp::EqualTo),
        string("<").map(|_| BinaryOp::LessThan),
        string(">").map(|_| BinaryOp::GreaterThan),
        string("&&").map(|_| BinaryOp::And),
        string("||").map(|_| BinaryOp::Or),
        string("+").map(|_| BinaryOp::Add),
        string("-").map(|_| BinaryOp::Subtract),
        string("*").map(|_| BinaryOp::Multiply),
        string("/").map(|_| BinaryOp::Divide),
    ))
}

#[derive(Clone, Debug)]
pub enum BinaryOp {
    GreaterThan,
    LessThan,
    LessThanOrEqualTo,
    GreaterThanOrEqualTo,
    EqualTo,
    And,
    Or,
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[cfg(test)]
mod test {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::{
        DynamicParsedFunctionName, DynamicParsedFunctionReference, Expr, ParsedFunctionSite,
    };

    #[test]
    fn test_greater_than() {
        let input = "foo > bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::greater_than(
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None)
            ))
        );
    }

    #[test]
    fn test_greater_than_or_equal_to() {
        let input = "foo >= bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::greater_than_or_equal_to(
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None)
            ))
        );
    }

    #[test]
    fn test_less_than() {
        let input = "foo < bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::less_than(
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None)
            ))
        );
    }

    #[test]
    fn test_less_than_or_equal_to() {
        let input = "foo <= bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::less_than_or_equal_to(
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None)
            ))
        );
    }

    #[test]
    fn test_equal_to() {
        let input = "foo == bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::equal_to(
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None)
            ))
        );
    }

    #[test]
    fn test_binary_op_in_if_condition() {
        let input = "if true then foo > bar  else  bar == foo";
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::cond(
                Expr::boolean(true),
                Expr::greater_than(
                    Expr::identifier_global("foo", None),
                    Expr::identifier_global("bar", None)
                ),
                Expr::equal_to(
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("foo", None)
                ),
            ),
        );
    }

    #[test]
    fn test_binary_op_in_sequence() {
        let input = "[foo >= bar, foo < bar]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::sequence(
                vec![
                    Expr::greater_than_or_equal_to(
                        Expr::identifier_global("foo", None),
                        Expr::identifier_global("bar", None)
                    ),
                    Expr::less_than(
                        Expr::identifier_global("foo", None),
                        Expr::identifier_global("bar", None)
                    )
                ],
                None
            ))
        );
    }

    #[test]
    fn test_binary_op_of_record() {
        let input = "{foo : 1} == {foo: 2}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::equal_to(
                Expr::record(vec![("foo".to_string(), Expr::number(BigDecimal::from(1)))]),
                Expr::record(vec![("foo".to_string(), Expr::number(BigDecimal::from(2)))]),
            ))
        );
    }

    #[test]
    fn test_binary_op_of_sequence() {
        let input = "[1, 2] == [3, 4]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::equal_to(
                Expr::sequence(
                    vec![
                        Expr::number(BigDecimal::from(1)),
                        Expr::number(BigDecimal::from(2))
                    ],
                    None
                ),
                Expr::sequence(
                    vec![
                        Expr::number(BigDecimal::from(3)),
                        Expr::number(BigDecimal::from(4))
                    ],
                    None
                ),
            ))
        );
    }

    #[test]
    fn test_binary_op_of_tuple() {
        let input = "(1, 2) == (3, 4)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::equal_to(
                Expr::tuple(vec![
                    Expr::number(BigDecimal::from(1)),
                    Expr::number(BigDecimal::from(2))
                ]),
                Expr::tuple(vec![
                    Expr::number(BigDecimal::from(3)),
                    Expr::number(BigDecimal::from(4))
                ]),
            ))
        );
    }

    #[test]
    fn test_binary_op_of_select_field() {
        let input = "foo.bar == baz.qux";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::equal_to(
                Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
                Expr::select_field(Expr::identifier_global("baz", None), "qux", None),
            ))
        );
    }

    #[test]
    fn test_binary_op_of_select_index() {
        let input = "foo[1] == bar[2]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::equal_to(
                Expr::select_index(
                    Expr::identifier_global("foo", None),
                    Expr::number(BigDecimal::from(1)),
                ),
                Expr::select_index(
                    Expr::identifier_global("bar", None),
                    Expr::number(BigDecimal::from(2)),
                ),
            ))
        );
    }

    #[test]
    fn test_binary_op_of_result() {
        let input = "ok(foo) == ok(bar)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::equal_to(
                Expr::ok(Expr::identifier_global("foo", None), None),
                Expr::ok(Expr::identifier_global("bar", None), None),
            ))
        );
    }

    #[test]
    fn test_binary_op_of_option() {
        let input = "some(foo) == some(bar)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::equal_to(
                Expr::option(Some(Expr::identifier_global("foo", None))),
                Expr::option(Some(Expr::identifier_global("bar", None))),
            ))
        );
    }

    #[test]
    fn test_binary_op_of_call() {
        let input = "foo() == bar()";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::equal_to(
                Expr::call_worker_function(
                    DynamicParsedFunctionName {
                        site: ParsedFunctionSite::Global,
                        function: DynamicParsedFunctionReference::Function {
                            function: "foo".to_string(),
                        }
                    },
                    None,
                    None,
                    vec![]
                ),
                Expr::call_worker_function(
                    DynamicParsedFunctionName {
                        site: ParsedFunctionSite::Global,
                        function: DynamicParsedFunctionReference::Function {
                            function: "bar".to_string(),
                        }
                    },
                    None,
                    None,
                    vec![]
                ),
            ))
        );
    }

    #[test]
    fn test_binary_op_in_record() {
        let input = "{foo: bar > baz, baz: bar == foo}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![
                (
                    "foo".to_string(),
                    Expr::greater_than(
                        Expr::identifier_global("bar", None),
                        Expr::identifier_global("baz", None)
                    )
                ),
                (
                    "baz".to_string(),
                    Expr::equal_to(
                        Expr::identifier_global("bar", None),
                        Expr::identifier_global("foo", None)
                    )
                ),
            ]))
        );
    }
}
